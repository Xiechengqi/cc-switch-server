//! Cursor AgentService session registry for the runtime-configured RPC method.
//!
//! Keeps an open h2 stream alive across the OpenAI/Claude tool round-trip so
//! a tool-using turn can complete inline. When the proxy emits a tool_calls
//! response, the matching cursor h2 stream is parked in
//! `awaiting_tool_result`. The follow-up request (carrying the tool result
//! bytes) reacquires the parked session and writes the
//! `ExecClientMessage.McpResult` on the **same** h2 stream, preserving the
//! exec_id mapping cursor needs to resume.
//!
//! See `OmniRoute/open-sse/services/cursorSessionManager.ts` for the
//! reference behaviour.

use super::agent_proto::McpToolDef;
use super::h2_client::CursorH2Stream;
use super::profile::CursorProtocolRail;
use super::request_builder::ResponseToolNamespace;
use super::response_state::CursorLocalTaskState;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Weak;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::sync::RwLock;

const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_SESSIONS: usize = 100;

/// Opaque namespace for every Cursor session and secondary index lookup.
///
/// The digest deliberately hides Account, Share, user, and API-key identity
/// material while retaining exact generation/runtime fencing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorSessionScope(String);

pub struct CursorSessionScopeInput<'a> {
    pub app: &'a str,
    pub provider_id: &'a str,
    pub provider_revision: u64,
    pub runtime_fingerprint: &'a str,
    pub rail: CursorProtocolRail,
    pub protocol_revision: &'a str,
    pub principal: &'a str,
    pub share_id: Option<&'a str>,
    pub user_email: Option<&'a str>,
}

impl CursorSessionScope {
    pub fn derive(input: CursorSessionScopeInput<'_>) -> Self {
        let share_id = normalized_scope_value(input.share_id, "<direct-share>", false);
        let user_email = normalized_scope_value(input.user_email, "<direct-user>", true);
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-server:cursor-session-scope:v1\0");
        update_scope_component(&mut hasher, "app", input.app);
        update_scope_component(&mut hasher, "provider", input.provider_id);
        update_scope_component(
            &mut hasher,
            "provider_revision",
            &input.provider_revision.to_string(),
        );
        update_scope_component(
            &mut hasher,
            "runtime_fingerprint",
            input.runtime_fingerprint,
        );
        update_scope_component(&mut hasher, "rail", input.rail.label());
        update_scope_component(&mut hasher, "protocol_revision", input.protocol_revision);
        update_scope_component(&mut hasher, "principal", input.principal);
        update_scope_component(&mut hasher, "share", &share_id);
        update_scope_component(&mut hasher, "user", &user_email);
        Self(format!("cursor-scope-v1:{:x}", hasher.finalize()))
    }

    #[cfg(test)]
    pub(crate) fn fixture(name: &str) -> Self {
        Self::derive(CursorSessionScopeInput {
            app: "codex",
            provider_id: "provider-fixture",
            provider_revision: 1,
            runtime_fingerprint: "runtime-fixture",
            rail: CursorProtocolRail::OAuthCli,
            protocol_revision: CursorProtocolRail::OAuthCli.protocol_revision(),
            principal: "account-fixture:1:1",
            share_id: Some(name),
            user_email: Some("USER@example.com"),
        })
    }
}

fn normalized_scope_value(value: Option<&str>, fallback: &str, lowercase: bool) -> String {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match (value, lowercase) {
        (Some(value), true) => value.to_ascii_lowercase(),
        (Some(value), false) => value.to_string(),
        (None, _) => fallback.to_string(),
    }
}

fn update_scope_component(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorSessionKey {
    scope: CursorSessionScope,
    conversation_id: String,
}

impl CursorSessionKey {
    pub fn new(scope: CursorSessionScope, conversation_id: impl Into<String>) -> Self {
        Self {
            scope,
            conversation_id: conversation_id.into(),
        }
    }

    pub fn scope(&self) -> &CursorSessionScope {
        &self.scope
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CursorScopedIndexKey {
    scope: CursorSessionScope,
    raw_id: String,
}

impl CursorScopedIndexKey {
    fn new(scope: &CursorSessionScope, raw_id: &str) -> Option<Self> {
        let raw_id = raw_id.trim();
        (!raw_id.is_empty()).then(|| Self {
            scope: scope.clone(),
            raw_id: raw_id.to_string(),
        })
    }
}

/// Per-pending-tool-call bookkeeping. `exec_msg_id` + `exec_id` are the
/// identifiers cursor needs in the `McpResult` reply.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub exec_msg_id: u64,
    pub exec_id: String,
    pub tool_name: String,
    pub custom: bool,
}

/// Lifecycle state of a session held by the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// A handler currently owns the session's h2 stream.
    Running,
    /// The handler returned tool_calls and parked the session. The next
    /// matching client request can reacquire it.
    AwaitingToolResult,
    /// The session is being torn down. Subsequent acquires fail.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSessionOpenConflict {
    pub state: SessionState,
}

/// Live state of a single AgentService run.
pub struct CursorSession {
    pub key: CursorSessionKey,
    pub rail: CursorProtocolRail,
    pub stream: Option<CursorH2Stream>,
    /// MCP tool names declared on the inbound turn (for shell→MCP bridging).
    pub declared_tool_names: Vec<String>,
    /// Full declared MCP tool definitions for response-side schema validation.
    pub declared_tools: Vec<McpToolDef>,
    /// Declared OpenAI custom tools. Cursor receives JSON MCP wrappers for
    /// these names, while the downstream Responses client must receive
    /// `custom_tool_call` / `custom_tool_call_output` items.
    pub custom_tool_names: Vec<String>,
    pub response_tool_namespaces: Vec<ResponseToolNamespace>,
    pub semantic_items: Vec<serde_json::Value>,
    /// Opaque local-task continuation state. This retains no prompt or tool
    /// output and lets a parked Responses run preserve agent intent across
    /// the tool-result round trip.
    pub local_task_state: CursorLocalTaskState,
    /// Working directory for RequestContext ack.
    pub working_directory: String,
    /// Map: client-facing tool call id → cursor exec metadata.
    pub pending_tool_calls: HashMap<String, PendingToolCall>,
    /// Exact tool calls whose results were supplied to a cold-resumed run.
    /// Re-emitting one would ask the client to repeat an already completed
    /// operation, so the driver rejects it before client commit.
    pub cold_resume_completed_calls: Vec<(String, serde_json::Value)>,
    pub cold_resume_replay_rejections: usize,
    /// Request-scoped KV blob store (system blob, future attachments).
    pub blob_store: HashMap<String, Bytes>,
    pub state: SessionState,
    pub last_activity: Instant,
}

impl CursorSession {
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }
}

/// Shared, process-wide cursor session registry. One per cc-switch instance.
#[derive(Clone)]
pub struct CursorSessionManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for CursorSessionManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CursorSessionManager")
            .field("idle_ttl", &self.inner.idle_ttl)
            .field("max_sessions", &self.inner.max_sessions)
            .finish_non_exhaustive()
    }
}

struct Inner {
    sessions: RwLock<HashMap<CursorSessionKey, Arc<Mutex<CursorSession>>>>,
    response_sessions: RwLock<HashMap<CursorScopedIndexKey, CursorSessionIndexBinding>>,
    tool_call_sessions: RwLock<HashMap<CursorScopedIndexKey, CursorSessionIndexBinding>>,
    /// Recently consumed cold-continuation identifiers. These short-lived,
    /// scope-fenced claims close the race after a parked entry and its live
    /// indexes have been removed but a duplicate request is still in flight.
    continuation_claims: RwLock<HashMap<CursorScopedIndexKey, Instant>>,
    idle_ttl: Duration,
    max_sessions: usize,
}

#[derive(Clone)]
struct CursorSessionIndexBinding {
    key: CursorSessionKey,
    entry: Weak<Mutex<CursorSession>>,
}

#[derive(Clone)]
pub struct CursorSessionReference {
    key: CursorSessionKey,
    entry: Arc<Mutex<CursorSession>>,
}

impl CursorSessionReference {
    pub fn key(&self) -> &CursorSessionKey {
        &self.key
    }
}

impl Default for CursorSessionManager {
    fn default() -> Self {
        Self::new(DEFAULT_IDLE_TTL, DEFAULT_MAX_SESSIONS)
    }
}

impl CursorSessionManager {
    pub fn new(idle_ttl: Duration, max_sessions: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                sessions: RwLock::new(HashMap::new()),
                response_sessions: RwLock::new(HashMap::new()),
                tool_call_sessions: RwLock::new(HashMap::new()),
                continuation_claims: RwLock::new(HashMap::new()),
                idle_ttl,
                max_sessions,
            }),
        }
    }

    /// Record the identifiers consumed while atomically taking ownership of a
    /// parked cold continuation. Claims contain only opaque response/call IDs,
    /// never prompts, tool arguments, results, or credentials.
    pub async fn claim_continuation_ids(&self, scope: &CursorSessionScope, raw_ids: &[String]) {
        let now = Instant::now();
        let mut claims = self.inner.continuation_claims.write().await;
        claims.retain(|_, claimed_at| now.duration_since(*claimed_at) <= self.inner.idle_ttl);
        for raw_id in raw_ids {
            if let Some(key) = CursorScopedIndexKey::new(scope, raw_id) {
                claims.insert(key, now);
            }
        }
        let max_claims = self.inner.max_sessions.saturating_mul(16).max(64);
        if claims.len() > max_claims {
            let mut oldest = claims
                .iter()
                .map(|(key, claimed_at)| (key.clone(), *claimed_at))
                .collect::<Vec<_>>();
            oldest.sort_unstable_by_key(|(_, claimed_at)| *claimed_at);
            for (key, _) in oldest.into_iter().take(claims.len() - max_claims) {
                claims.remove(&key);
            }
        }
    }

    /// Check whether an exact scope/id was already consumed by another cold
    /// continuation request. Expired claims are removed on lookup.
    pub async fn continuation_was_claimed(
        &self,
        scope: &CursorSessionScope,
        raw_ids: &[String],
    ) -> bool {
        let now = Instant::now();
        let mut claims = self.inner.continuation_claims.write().await;
        claims.retain(|_, claimed_at| now.duration_since(*claimed_at) <= self.inner.idle_ttl);
        raw_ids.iter().any(|raw_id| {
            CursorScopedIndexKey::new(scope, raw_id).is_some_and(|key| claims.contains_key(&key))
        })
    }

    /// Try to reacquire a parked session. Returns `Some` only when the entry
    /// exists, is `AwaitingToolResult`, and hasn't passed its idle TTL.
    pub async fn acquire(&self, key: &CursorSessionKey) -> Option<Arc<Mutex<CursorSession>>> {
        let entry = self.inner.sessions.read().await.get(key)?.clone();
        self.acquire_resolved(&CursorSessionReference {
            key: key.clone(),
            entry,
        })
        .await
    }

    pub async fn acquire_resolved(
        &self,
        reference: &CursorSessionReference,
    ) -> Option<Arc<Mutex<CursorSession>>> {
        self.evict_expired().await;
        let mut session = reference.entry.lock().await;
        if session.state != SessionState::AwaitingToolResult
            || Instant::now().duration_since(session.last_activity) > self.inner.idle_ttl
        {
            return None;
        }
        let sessions = self.inner.sessions.read().await;
        if !sessions
            .get(&reference.key)
            .is_some_and(|current| Arc::ptr_eq(current, &reference.entry))
        {
            return None;
        }
        session.state = SessionState::Running;
        session.touch();
        drop(sessions);
        drop(session);
        Some(reference.entry.clone())
    }

    /// Acquire and close an exact parked entry. A concurrently reacquired
    /// Running session is never interrupted, and a stale reference cannot
    /// affect a same-key replacement.
    pub async fn close_parked_resolved(&self, reference: &CursorSessionReference) -> bool {
        let Some(entry) = self.acquire_resolved(reference).await else {
            return false;
        };
        self.release(entry, SessionState::Closed).await;
        true
    }

    /// Reserve this conversation before any AgentService request is sent.
    ///
    /// A live entry is never replaced. Reserving before the outbound open also
    /// prevents a rejected concurrent request from starting an orphaned
    /// upstream run with possible side effects.
    pub async fn reserve(
        &self,
        key: CursorSessionKey,
        rail: CursorProtocolRail,
        blob_store: HashMap<String, Bytes>,
        declared_tools: Vec<McpToolDef>,
        semantic_items: Vec<serde_json::Value>,
        working_directory: String,
    ) -> Result<Arc<Mutex<CursorSession>>, CursorSessionOpenConflict> {
        self.evict_expired().await;
        let declared_tool_names = declared_tools.iter().map(|t| t.name.clone()).collect();
        let session = CursorSession {
            key: key.clone(),
            rail,
            stream: None,
            declared_tool_names,
            declared_tools,
            custom_tool_names: Vec::new(),
            response_tool_namespaces: Vec::new(),
            semantic_items,
            local_task_state: CursorLocalTaskState::default(),
            working_directory,
            pending_tool_calls: HashMap::new(),
            cold_resume_completed_calls: Vec::new(),
            cold_resume_replay_rejections: 0,
            blob_store,
            state: SessionState::Running,
            last_activity: Instant::now(),
        };
        let entry = Arc::new(Mutex::new(session));
        self.insert_new_entry(key, entry.clone()).await?;
        self.enforce_max_sessions().await;
        Ok(entry)
    }

    pub async fn attach_stream(
        &self,
        entry: &Arc<Mutex<CursorSession>>,
        stream: CursorH2Stream,
    ) -> Result<(), CursorSessionOpenConflict> {
        let mut session = entry.lock().await;
        if session.state != SessionState::Running || session.stream.is_some() {
            return Err(CursorSessionOpenConflict {
                state: session.state,
            });
        }
        session.stream = Some(stream);
        session.touch();
        Ok(())
    }

    async fn insert_new_entry(
        &self,
        key: CursorSessionKey,
        entry: Arc<Mutex<CursorSession>>,
    ) -> Result<(), CursorSessionOpenConflict> {
        {
            let mut map = self.inner.sessions.write().await;
            if let Some(existing) = map.get(&key) {
                let state = existing
                    .try_lock()
                    .map(|session| session.state)
                    .unwrap_or(SessionState::Running);
                return Err(CursorSessionOpenConflict { state });
            }
            map.insert(key, entry.clone());
        }
        Ok(())
    }

    /// Mark a session as no longer in-flight. `AwaitingToolResult` parks it
    /// for reacquisition; `Closed` evicts it immediately.
    pub async fn release(&self, entry: Arc<Mutex<CursorSession>>, final_state: SessionState) {
        let key = {
            let mut session = entry.lock().await;
            session.touch();
            match final_state {
                SessionState::AwaitingToolResult => {
                    session.state = SessionState::AwaitingToolResult;
                    return;
                }
                SessionState::Closed | SessionState::Running => {
                    session.state = SessionState::Closed;
                    session.stream = None;
                    session.key.clone()
                }
            }
        };
        let removed = {
            let mut map = self.inner.sessions.write().await;
            if map
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
            {
                map.remove(&key);
                true
            } else {
                false
            }
        };
        if removed {
            self.remove_indexes_for_session(&entry).await;
        }
    }

    pub async fn bind_response_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        response_id: &str,
    ) {
        let Some(index_key) = CursorScopedIndexKey::new(key.scope(), response_id) else {
            return;
        };
        let sessions = self.inner.sessions.read().await;
        if !sessions
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return;
        }
        self.inner.response_sessions.write().await.insert(
            index_key,
            CursorSessionIndexBinding {
                key: key.clone(),
                entry: Arc::downgrade(entry),
            },
        );
    }

    pub async fn resolve_response_id(
        &self,
        scope: &CursorSessionScope,
        response_id: &str,
    ) -> Option<CursorSessionReference> {
        let index_key = CursorScopedIndexKey::new(scope, response_id)?;
        self.resolve_index_binding(&self.inner.response_sessions, &index_key)
            .await
    }

    pub async fn bind_tool_call_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        tool_call_id: &str,
    ) {
        let Some(index_key) = CursorScopedIndexKey::new(key.scope(), tool_call_id) else {
            return;
        };
        let sessions = self.inner.sessions.read().await;
        if !sessions
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return;
        }
        self.inner.tool_call_sessions.write().await.insert(
            index_key,
            CursorSessionIndexBinding {
                key: key.clone(),
                entry: Arc::downgrade(entry),
            },
        );
    }

    pub async fn resolve_tool_call_id(
        &self,
        scope: &CursorSessionScope,
        tool_call_id: &str,
    ) -> Option<CursorSessionReference> {
        let index_key = CursorScopedIndexKey::new(scope, tool_call_id)?;
        self.resolve_index_binding(&self.inner.tool_call_sessions, &index_key)
            .await
    }

    async fn resolve_index_binding(
        &self,
        index: &RwLock<HashMap<CursorScopedIndexKey, CursorSessionIndexBinding>>,
        index_key: &CursorScopedIndexKey,
    ) -> Option<CursorSessionReference> {
        // Keep the primary registry read-locked until the index binding is
        // validated so a same-key replacement cannot inherit stale indexes.
        let sessions = self.inner.sessions.read().await;
        let indexes = index.read().await;
        let binding = indexes.get(index_key)?;
        let entry = binding.entry.upgrade()?;
        sessions
            .get(&binding.key)
            .is_some_and(|current| Arc::ptr_eq(current, &entry))
            .then(|| CursorSessionReference {
                key: binding.key.clone(),
                entry,
            })
    }

    async fn remove_indexes_for_session(&self, entry: &Arc<Mutex<CursorSession>>) {
        let weak = Arc::downgrade(entry);
        {
            let mut map = self.inner.response_sessions.write().await;
            map.retain(|_, value| !Weak::ptr_eq(&value.entry, &weak));
        }
        {
            let mut map = self.inner.tool_call_sessions.write().await;
            map.retain(|_, value| !Weak::ptr_eq(&value.entry, &weak));
        }
    }

    async fn evict_expired(&self) {
        let now = Instant::now();
        let mut to_remove: Vec<(CursorSessionKey, Arc<Mutex<CursorSession>>)> = Vec::new();
        {
            let map = self.inner.sessions.read().await;
            for (k, entry) in map.iter() {
                if let Ok(session) = entry.try_lock() {
                    if session.state != SessionState::Running
                        && now.duration_since(session.last_activity) > self.inner.idle_ttl
                    {
                        to_remove.push((k.clone(), entry.clone()));
                    }
                }
            }
        }
        for (key, entry) in to_remove {
            self.evict_expired_candidate(&key, &entry, now).await;
        }
    }

    async fn evict_expired_candidate(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        now: Instant,
    ) -> bool {
        let mut session = entry.lock().await;
        if session.state == SessionState::Running
            || now.duration_since(session.last_activity) <= self.inner.idle_ttl
        {
            return false;
        }
        let removed = {
            let mut map = self.inner.sessions.write().await;
            if map
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
            {
                map.remove(key);
                true
            } else {
                false
            }
        };
        if !removed {
            return false;
        }
        session.state = SessionState::Closed;
        session.stream = None;
        drop(session);
        self.remove_indexes_for_session(entry).await;
        true
    }

    async fn enforce_max_sessions(&self) {
        loop {
            let len = self.inner.sessions.read().await.len();
            if len <= self.inner.max_sessions {
                break;
            }
            // Find the least-recently-active entry that isn't running.
            let oldest: Option<(CursorSessionKey, Arc<Mutex<CursorSession>>)> = {
                let map = self.inner.sessions.read().await;
                let mut candidates: Vec<(CursorSessionKey, Arc<Mutex<CursorSession>>, Instant)> =
                    Vec::new();
                for (k, entry) in map.iter() {
                    if let Ok(session) = entry.try_lock() {
                        if session.state != SessionState::Running {
                            candidates.push((k.clone(), entry.clone(), session.last_activity));
                        }
                    }
                }
                candidates
                    .into_iter()
                    .min_by_key(|(_, _, t)| *t)
                    .map(|(key, entry, _)| (key, entry))
            };
            let Some((key, entry)) = oldest else { break };
            let mut session = entry.lock().await;
            if session.state == SessionState::Running {
                continue;
            }
            let removed = {
                let mut map = self.inner.sessions.write().await;
                if map.len() > self.inner.max_sessions
                    && map
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                {
                    map.remove(&key);
                    true
                } else {
                    false
                }
            };
            if removed {
                session.state = SessionState::Closed;
                session.stream = None;
                drop(session);
                self.remove_indexes_for_session(&entry).await;
            }
        }
    }

    pub async fn size(&self) -> usize {
        self.inner.sessions.read().await.len()
    }

    pub async fn has(&self, key: &CursorSessionKey) -> bool {
        self.inner.sessions.read().await.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_key(scope: &CursorSessionScope) -> CursorSessionKey {
        CursorSessionKey::new(scope.clone(), "session-1")
    }

    fn session_entry(key: CursorSessionKey, state: SessionState) -> Arc<Mutex<CursorSession>> {
        Arc::new(Mutex::new(CursorSession {
            key,
            rail: CursorProtocolRail::OAuthCli,
            stream: None,
            declared_tool_names: Vec::new(),
            declared_tools: Vec::new(),
            custom_tool_names: Vec::new(),
            response_tool_namespaces: Vec::new(),
            semantic_items: Vec::new(),
            local_task_state: CursorLocalTaskState::default(),
            working_directory: String::new(),
            pending_tool_calls: HashMap::new(),
            cold_resume_completed_calls: Vec::new(),
            cold_resume_replay_rejections: 0,
            blob_store: HashMap::new(),
            state,
            last_activity: Instant::now(),
        }))
    }

    #[tokio::test]
    async fn manager_size_starts_at_zero() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("share-a"));
        assert_eq!(mgr.size().await, 0);
        assert!(!mgr.has(&key).await);
    }

    #[tokio::test]
    async fn pending_tool_call_lookup() {
        let mut pending: HashMap<String, PendingToolCall> = HashMap::new();
        pending.insert(
            "tc_1".to_string(),
            PendingToolCall {
                exec_msg_id: 7,
                exec_id: "exec-x".to_string(),
                tool_name: "weather".to_string(),
                custom: false,
            },
        );
        let got = pending.get("tc_1").unwrap();
        assert_eq!(got.exec_msg_id, 7);
        assert_eq!(got.exec_id, "exec-x");
        assert!(!got.custom);
    }

    #[tokio::test]
    async fn parked_session_preserves_semantic_schema_and_multiple_custom_calls() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("custom-tools"));
        let schema = serde_json::json!({
            "type":"object",
            "properties":{"input":{"type":"string"}},
            "required":["input"],
            "additionalProperties":false
        });
        let entry = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                vec![McpToolDef::new(
                    "exec".to_string(),
                    "run tool".to_string(),
                    schema.clone(),
                    "cc-switch".to_string(),
                    "exec".to_string(),
                )],
                Vec::new(),
                ".".to_string(),
            )
            .await
            .unwrap();
        {
            let mut session = entry.lock().await;
            session.custom_tool_names = vec!["exec".to_string()];
            for index in 1..=2 {
                session.pending_tool_calls.insert(
                    format!("call_{index}"),
                    PendingToolCall {
                        exec_msg_id: index,
                        exec_id: format!("exec-{index}"),
                        tool_name: "exec".to_string(),
                        custom: true,
                    },
                );
            }
        }
        mgr.bind_tool_call_id(&key, &entry, "call_1").await;
        mgr.bind_tool_call_id(&key, &entry, "call_2").await;
        mgr.release(entry.clone(), SessionState::AwaitingToolResult)
            .await;

        let reference = mgr
            .resolve_tool_call_id(key.scope(), "call_1")
            .await
            .unwrap();
        let acquired = mgr.acquire_resolved(&reference).await.unwrap();
        let session = acquired.lock().await;
        assert_eq!(session.declared_tools[0].input_schema.as_json(), &schema);
        assert_eq!(session.custom_tool_names, vec!["exec"]);
        assert_eq!(session.pending_tool_calls.len(), 2);
        assert!(session.pending_tool_calls.values().all(|call| call.custom));
    }

    #[tokio::test]
    async fn response_and_tool_indexes_are_scoped() {
        let mgr = CursorSessionManager::default();
        let scope_a = CursorSessionScope::fixture("share-a");
        let scope_b = CursorSessionScope::fixture("share-b");
        let key_a = session_key(&scope_a);
        let key_b = session_key(&scope_b);
        let entry_a = session_entry(key_a.clone(), SessionState::AwaitingToolResult);
        let entry_b = session_entry(key_b.clone(), SessionState::AwaitingToolResult);
        {
            let mut sessions = mgr.inner.sessions.write().await;
            sessions.insert(key_a.clone(), entry_a.clone());
            sessions.insert(key_b.clone(), entry_b.clone());
        }
        mgr.bind_response_id(&key_a, &entry_a, "resp_1").await;
        mgr.bind_response_id(&key_b, &entry_b, "resp_1").await;
        mgr.bind_tool_call_id(&key_a, &entry_a, "call_1").await;
        mgr.bind_tool_call_id(&key_b, &entry_b, "call_1").await;
        assert_eq!(
            mgr.resolve_response_id(&scope_a, "resp_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a.clone())
        );
        assert_eq!(
            mgr.resolve_response_id(&scope_b, "resp_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_b.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope_a, "call_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope_b, "call_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_b.clone())
        );

        mgr.remove_indexes_for_session(&entry_a).await;
        assert!(mgr.resolve_response_id(&scope_a, "resp_1").await.is_none());
        assert!(mgr.resolve_tool_call_id(&scope_a, "call_1").await.is_none());
        assert_eq!(
            mgr.resolve_response_id(&scope_b, "resp_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_b.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope_b, "call_1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_b)
        );
    }

    #[tokio::test]
    async fn consumed_continuation_claims_are_scope_isolated() {
        let mgr = CursorSessionManager::default();
        let scope_a = CursorSessionScope::fixture("claim-share-a");
        let scope_b = CursorSessionScope::fixture("claim-share-b");
        let ids = vec!["call_same".to_string(), "resp_same".to_string()];

        mgr.claim_continuation_ids(&scope_a, &ids).await;

        assert!(mgr.continuation_was_claimed(&scope_a, &ids).await);
        assert!(!mgr.continuation_was_claimed(&scope_b, &ids).await);
    }

    #[tokio::test]
    async fn cross_scope_lookup_never_closes_or_reveals_parked_session() {
        let mgr = CursorSessionManager::default();
        let scope_a = CursorSessionScope::fixture("share-a");
        let scope_b = CursorSessionScope::fixture("share-b");
        let key_a = session_key(&scope_a);
        let key_b = session_key(&scope_b);
        let entry = session_entry(key_a.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key_a.clone(), entry.clone());
        mgr.bind_response_id(&key_a, &entry, "response-1").await;
        mgr.bind_tool_call_id(&key_a, &entry, "call-1").await;

        assert!(mgr.acquire(&key_b).await.is_none());
        assert_eq!(entry.lock().await.state, SessionState::AwaitingToolResult);
        assert!(mgr.has(&key_a).await);
        assert_eq!(
            mgr.resolve_response_id(&scope_a, "response-1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope_a, "call-1")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a)
        );
        assert!(mgr
            .resolve_response_id(&scope_b, "response-1")
            .await
            .is_none());
        assert!(mgr.resolve_tool_call_id(&scope_b, "call-1").await.is_none());
    }

    #[tokio::test]
    async fn close_parked_resolved_cannot_remove_a_same_key_replacement() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("share-a");
        let key = session_key(&scope);
        let stale = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), stale.clone());
        mgr.bind_tool_call_id(&key, &stale, "call-stale").await;
        let reference = mgr
            .resolve_tool_call_id(&scope, "call-stale")
            .await
            .unwrap();

        let replacement = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), replacement.clone());
        assert!(!mgr.close_parked_resolved(&reference).await);

        assert!(mgr.has(&key).await);
        assert_eq!(
            replacement.lock().await.state,
            SessionState::AwaitingToolResult
        );
        assert_eq!(stale.lock().await.state, SessionState::AwaitingToolResult);
    }

    #[tokio::test]
    async fn close_parked_resolved_never_interrupts_a_running_owner() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("share-a");
        let key = session_key(&scope);
        let entry = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), entry.clone());
        mgr.bind_tool_call_id(&key, &entry, "call-1").await;
        let reference = mgr.resolve_tool_call_id(&scope, "call-1").await.unwrap();
        assert!(mgr.acquire_resolved(&reference).await.is_some());
        assert!(!mgr.close_parked_resolved(&reference).await);
        assert_eq!(entry.lock().await.state, SessionState::Running);
        assert!(mgr.has(&key).await);
    }

    #[tokio::test]
    async fn close_parked_resolved_owns_and_removes_the_exact_entry() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("share-a");
        let key = session_key(&scope);
        let entry = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), entry.clone());
        mgr.bind_tool_call_id(&key, &entry, "call-1").await;
        let reference = mgr.resolve_tool_call_id(&scope, "call-1").await.unwrap();
        assert!(mgr.close_parked_resolved(&reference).await);
        assert!(!mgr.has(&key).await);
        assert_eq!(entry.lock().await.state, SessionState::Closed);
        assert!(mgr.resolve_tool_call_id(&scope, "call-1").await.is_none());
    }

    #[tokio::test]
    async fn same_raw_conversation_id_coexists_across_scopes() {
        let mgr = CursorSessionManager::default();
        let key_a = session_key(&CursorSessionScope::fixture("share-a"));
        let key_b = session_key(&CursorSessionScope::fixture("share-b"));
        let entry_a = session_entry(key_a.clone(), SessionState::AwaitingToolResult);
        let entry_b = session_entry(key_b.clone(), SessionState::AwaitingToolResult);
        let mut sessions = mgr.inner.sessions.write().await;
        sessions.insert(key_a.clone(), entry_a.clone());
        sessions.insert(key_b.clone(), entry_b.clone());
        drop(sessions);

        assert_eq!(mgr.size().await, 2);
        assert!(Arc::ptr_eq(&mgr.acquire(&key_a).await.unwrap(), &entry_a));
        assert!(Arc::ptr_eq(&mgr.acquire(&key_b).await.unwrap(), &entry_b));
    }

    #[tokio::test]
    async fn same_running_conversation_is_rejected_without_replacement() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("share-a"));
        let first = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();

        let conflict = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(conflict.state, SessionState::Running);
        let current = mgr.inner.sessions.read().await.get(&key).cloned().unwrap();
        assert!(Arc::ptr_eq(&current, &first));
    }

    #[test]
    fn scope_normalizes_email_but_fences_principal_generation_and_share() {
        let derive = |principal: &str, share: &str, email: &str| {
            CursorSessionScope::derive(CursorSessionScopeInput {
                app: "codex",
                provider_id: "provider-a",
                provider_revision: 4,
                runtime_fingerprint: "runtime-a",
                rail: CursorProtocolRail::OAuthCli,
                protocol_revision: CursorProtocolRail::OAuthCli.protocol_revision(),
                principal,
                share_id: Some(share),
                user_email: Some(email),
            })
        };
        assert_eq!(
            derive("account-a:2:7", "share-a", " USER@Example.COM "),
            derive("account-a:2:7", "share-a", "user@example.com")
        );
        assert_ne!(
            derive("account-a:2:7", "share-a", "user@example.com"),
            derive("account-a:2:8", "share-a", "user@example.com")
        );
        assert_ne!(
            derive("account-a:2:7", "share-a", "user@example.com"),
            derive("account-a:2:7", "share-b", "user@example.com")
        );
    }

    #[test]
    fn scope_fences_cursor_rail_and_protocol_revision() {
        let derive = |rail: CursorProtocolRail, revision: &str| {
            CursorSessionScope::derive(CursorSessionScopeInput {
                app: "codex",
                provider_id: "provider-a",
                provider_revision: 4,
                runtime_fingerprint: "runtime-a",
                rail,
                protocol_revision: revision,
                principal: "principal-a",
                share_id: Some("share-a"),
                user_email: Some("user@example.com"),
            })
        };
        assert_ne!(
            derive(
                CursorProtocolRail::OAuthCli,
                CursorProtocolRail::OAuthCli.protocol_revision()
            ),
            derive(
                CursorProtocolRail::ApiKeySdk,
                CursorProtocolRail::ApiKeySdk.protocol_revision()
            )
        );
        assert_ne!(
            derive(CursorProtocolRail::OAuthCli, "revision-a"),
            derive(CursorProtocolRail::OAuthCli, "revision-b")
        );
    }

    #[tokio::test]
    async fn stale_release_cannot_remove_replacement_session() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("share-a"));
        let stale = session_entry(key.clone(), SessionState::Running);
        let replacement = session_entry(key.clone(), SessionState::Running);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), replacement.clone());

        mgr.release(stale, SessionState::Closed).await;

        let current = mgr.inner.sessions.read().await.get(&key).cloned().unwrap();
        assert!(Arc::ptr_eq(&current, &replacement));
    }

    #[tokio::test]
    async fn expired_snapshot_cannot_evict_a_reacquired_session() {
        let mgr = CursorSessionManager::new(Duration::from_millis(1), 10);
        let key = session_key(&CursorSessionScope::fixture("share-a"));
        let entry = session_entry(key.clone(), SessionState::AwaitingToolResult);
        entry.lock().await.last_activity = Instant::now() - Duration::from_secs(1);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), entry.clone());
        let eviction_snapshot = Instant::now();

        {
            let mut session = entry.lock().await;
            session.state = SessionState::Running;
            session.touch();
        }
        assert!(
            !mgr.evict_expired_candidate(&key, &entry, eviction_snapshot)
                .await
        );
        assert!(mgr.has(&key).await);
        assert_eq!(entry.lock().await.state, SessionState::Running);
    }

    #[tokio::test]
    async fn stale_index_cleanup_and_bind_cannot_affect_replacement() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("share-a");
        let key = session_key(&scope);
        let stale = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), stale.clone());
        mgr.bind_response_id(&key, &stale, "response-old").await;

        let replacement = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), replacement.clone());
        mgr.bind_response_id(&key, &replacement, "response-new")
            .await;
        mgr.bind_tool_call_id(&key, &stale, "call-stale").await;
        mgr.remove_indexes_for_session(&stale).await;

        assert!(mgr
            .resolve_response_id(&scope, "response-old")
            .await
            .is_none());
        assert_eq!(
            mgr.resolve_response_id(&scope, "response-new")
                .await
                .map(|reference| reference.key().clone()),
            Some(key)
        );
        assert!(mgr
            .resolve_tool_call_id(&scope, "call-stale")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn resolved_tool_reference_cannot_acquire_a_same_key_replacement() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("share-a");
        let key = session_key(&scope);
        let stale = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), stale.clone());
        mgr.bind_tool_call_id(&key, &stale, "call-old").await;
        let resolved = mgr.resolve_tool_call_id(&scope, "call-old").await.unwrap();

        let replacement = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), replacement.clone());

        assert!(mgr.acquire_resolved(&resolved).await.is_none());
        assert_eq!(
            replacement.lock().await.state,
            SessionState::AwaitingToolResult
        );
        assert!(mgr.has(&key).await);
    }
}
