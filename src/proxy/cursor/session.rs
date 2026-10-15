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
use super::memory::session_retained_bytes;
use super::profile::CursorProtocolRail;
use super::request_builder::ResponseToolNamespace;
use super::response_state::CursorLocalTaskState;
use crate::proxy::request_memory::{
    RequestMemoryBudget, RequestMemoryComponent, RequestMemoryError, RequestMemoryReservation,
};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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

    /// Allocations held by both the registry key and the session's own key.
    pub(crate) fn retained_key_bytes(&self) -> usize {
        self.scope
            .0
            .capacity()
            .saturating_add(self.conversation_id.capacity())
            .saturating_mul(2)
            .saturating_add(std::mem::size_of::<Self>())
            .saturating_add(std::mem::size_of::<usize>())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CursorScopedIndexKey {
    scope: CursorSessionScope,
    raw_id: String,
}

impl CursorScopedIndexKey {
    fn new(scope: &CursorSessionScope, raw_id: &str) -> Option<Self> {
        (!raw_id.is_empty() && raw_id.trim() == raw_id).then(|| Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorSessionReserveError {
    ConversationBusy(CursorSessionOpenConflict),
    CapacityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorContinuationClaimError {
    AlreadyClaimed,
    CapacityExhausted,
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
    pub(super) idle_expiry_task: Option<tokio::task::AbortHandle>,
    /// Reservation for request-owned state retained while this session is
    /// running or parked. A continuation reserves against its new request
    /// before releasing the previous request's reservation.
    pub(crate) request_memory: Option<RequestMemoryReservation>,
    /// Conservative charge for secondary lookup keys and candidate copies
    /// owned by the registry while this session remains live.
    pub(crate) index_retained_bytes: usize,
}

impl CursorSession {
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    pub(crate) fn attach_request_memory(
        &mut self,
        budget: &RequestMemoryBudget,
    ) -> Result<(), RequestMemoryError> {
        let bytes = session_retained_bytes(self);
        let next = budget.reserve(RequestMemoryComponent::StreamRetainedState, bytes)?;
        if let Some(stream) = self.stream.as_mut() {
            stream.transfer_request_memory(budget)?;
        }
        self.request_memory = Some(next);
        Ok(())
    }

    pub(crate) fn refresh_request_memory(&self) -> Result<(), RequestMemoryError> {
        let bytes = session_retained_bytes(self);
        if let Some(reservation) = self.request_memory.as_ref() {
            reservation.resize(bytes)?;
        }
        Ok(())
    }

    fn reserve_index_binding(&mut self, raw_id: &str) -> Result<(), RequestMemoryError> {
        // Charge a whole new hash slot and the first Vec allocation even if
        // this ID later shares a bucket with another live session. Duplicate
        // binds can overcharge, but must never make the estimate too small.
        let candidate = std::mem::size_of::<CursorSessionIndexCandidate>()
            .saturating_add(self.key.scope.0.capacity())
            .saturating_add(self.key.conversation_id.capacity());
        let bytes = std::mem::size_of::<CursorScopedIndexKey>()
            .saturating_add(self.key.scope.0.capacity())
            .saturating_add(raw_id.len())
            .saturating_add(std::mem::size_of::<CursorSessionIndexBinding>())
            .saturating_add(candidate.saturating_mul(4))
            .saturating_add(std::mem::size_of::<usize>().saturating_mul(4));
        let previous = self.index_retained_bytes;
        self.index_retained_bytes = previous.saturating_add(bytes);
        if let Err(error) = self.refresh_request_memory() {
            self.index_retained_bytes = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(in crate::proxy) fn reserve_transient_memory(
        &self,
        component: RequestMemoryComponent,
        bytes: usize,
    ) -> Result<Option<RequestMemoryReservation>, RequestMemoryError> {
        self.request_memory
            .as_ref()
            .map(|reservation| reservation.budget().reserve(component, bytes))
            .transpose()
    }

    fn clear_retained_state(&mut self) {
        if let Some(task) = self.idle_expiry_task.take() {
            task.abort();
        }
        self.stream = None;
        self.declared_tool_names.clear();
        self.declared_tools.clear();
        self.custom_tool_names.clear();
        self.response_tool_namespaces.clear();
        self.semantic_items.clear();
        self.working_directory.clear();
        self.pending_tool_calls.clear();
        self.cold_resume_completed_calls.clear();
        self.blob_store.clear();
        self.request_memory = None;
        self.index_retained_bytes = 0;
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

#[derive(Default)]
struct CursorSessionIndexBinding {
    candidates: Vec<CursorSessionIndexCandidate>,
}

struct CursorSessionIndexCandidate {
    key: CursorSessionKey,
    entry: Weak<Mutex<CursorSession>>,
}

impl CursorSessionIndexBinding {
    fn bind(&mut self, key: &CursorSessionKey, entry: &Arc<Mutex<CursorSession>>) {
        let weak = Arc::downgrade(entry);
        if !self
            .candidates
            .iter()
            .any(|candidate| Weak::ptr_eq(&candidate.entry, &weak))
        {
            self.candidates.push(CursorSessionIndexCandidate {
                key: key.clone(),
                entry: weak,
            });
        }
    }
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
    pub async fn claim_continuation_ids(
        &self,
        scope: &CursorSessionScope,
        raw_ids: &[String],
    ) -> Result<(), CursorContinuationClaimError> {
        let now = Instant::now();
        let mut claims = self.inner.continuation_claims.write().await;
        claims.retain(|_, claimed_at| now.duration_since(*claimed_at) <= self.inner.idle_ttl);
        let keys = raw_ids
            .iter()
            .filter_map(|raw_id| CursorScopedIndexKey::new(scope, raw_id))
            .collect::<HashSet<_>>();
        if keys.iter().any(|key| claims.contains_key(key)) {
            return Err(CursorContinuationClaimError::AlreadyClaimed);
        }
        let max_claims = self.inner.max_sessions.saturating_mul(16).max(64);
        if claims.len().saturating_add(keys.len()) > max_claims {
            return Err(CursorContinuationClaimError::CapacityExhausted);
        }
        for key in keys {
            claims.insert(key, now);
        }
        Ok(())
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
        if let Some(task) = session.idle_expiry_task.take() {
            task.abort();
        }
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
    ) -> Result<Arc<Mutex<CursorSession>>, CursorSessionReserveError> {
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
            idle_expiry_task: None,
            request_memory: None,
            index_retained_bytes: 0,
        };
        let entry = Arc::new(Mutex::new(session));
        self.insert_new_entry(key, entry.clone())
            .await
            .map_err(CursorSessionReserveError::ConversationBusy)?;
        self.enforce_max_sessions().await;
        // Never let a full registry of active h2 streams turn the parked
        // session ceiling into an unbounded soft limit. This entry has not
        // escaped to a caller yet, so reject it without interrupting older
        // running sessions or opening an upstream AgentService stream.
        if self.size().await > self.inner.max_sessions {
            self.release(entry, SessionState::Closed).await;
            return Err(CursorSessionReserveError::CapacityExhausted);
        }
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
                    if let Some(task) = session.idle_expiry_task.take() {
                        task.abort();
                    }
                    let inner = Arc::downgrade(&self.inner);
                    let parked_entry = Arc::downgrade(&entry);
                    let key = session.key.clone();
                    let idle_ttl = self.inner.idle_ttl;
                    let task = tokio::spawn(async move {
                        tokio::time::sleep(idle_ttl.saturating_add(Duration::from_millis(1))).await;
                        let (Some(inner), Some(entry)) = (inner.upgrade(), parked_entry.upgrade())
                        else {
                            return;
                        };
                        let manager = CursorSessionManager { inner };
                        manager
                            .evict_expired_candidate(&key, &entry, Instant::now())
                            .await;
                    });
                    session.idle_expiry_task = Some(task.abort_handle());
                    return;
                }
                SessionState::Closed | SessionState::Running => {
                    session.state = SessionState::Closed;
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
        entry.lock().await.clear_retained_state();
    }

    pub(crate) async fn try_bind_response_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        response_id: &str,
    ) -> Result<(), RequestMemoryError> {
        let Some(index_key) = CursorScopedIndexKey::new(key.scope(), response_id) else {
            return Ok(());
        };
        entry.lock().await.reserve_index_binding(response_id)?;
        let sessions = self.inner.sessions.read().await;
        if !sessions
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return Ok(());
        }
        self.inner
            .response_sessions
            .write()
            .await
            .entry(index_key)
            .or_default()
            .bind(key, entry);
        Ok(())
    }

    #[cfg(test)]
    pub async fn bind_response_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        response_id: &str,
    ) {
        self.try_bind_response_id(key, entry, response_id)
            .await
            .unwrap();
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

    pub(crate) async fn try_bind_tool_call_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        tool_call_id: &str,
    ) -> Result<(), RequestMemoryError> {
        let Some(index_key) = CursorScopedIndexKey::new(key.scope(), tool_call_id) else {
            return Ok(());
        };
        entry.lock().await.reserve_index_binding(tool_call_id)?;
        let sessions = self.inner.sessions.read().await;
        if !sessions
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return Ok(());
        }
        self.inner
            .tool_call_sessions
            .write()
            .await
            .entry(index_key)
            .or_default()
            .bind(key, entry);
        Ok(())
    }

    #[cfg(test)]
    pub async fn bind_tool_call_id(
        &self,
        key: &CursorSessionKey,
        entry: &Arc<Mutex<CursorSession>>,
        tool_call_id: &str,
    ) {
        self.try_bind_tool_call_id(key, entry, tool_call_id)
            .await
            .unwrap();
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
        // Expired parked owners must not make a still-live ID ambiguous.
        // The final candidate check also covers expiry racing this sweep.
        self.evict_expired().await;
        let now = Instant::now();
        // Keep the primary registry read-locked until the index binding is
        // validated so a same-key replacement cannot inherit stale indexes.
        let sessions = self.inner.sessions.read().await;
        let indexes = index.read().await;
        let binding = indexes.get(index_key)?;
        let mut live = binding.candidates.iter().filter_map(|candidate| {
            let entry = candidate.entry.upgrade()?;
            if entry.try_lock().is_ok_and(|session| {
                session.state == SessionState::Closed
                    || (session.state == SessionState::AwaitingToolResult
                        && now.saturating_duration_since(session.last_activity)
                            > self.inner.idle_ttl)
            }) {
                return None;
            }
            sessions
                .get(&candidate.key)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
                .then(|| CursorSessionReference {
                    key: candidate.key.clone(),
                    entry,
                })
        });
        let unique = live.next()?;
        live.next().is_none().then_some(unique)
    }

    async fn remove_indexes_for_session(&self, entry: &Arc<Mutex<CursorSession>>) {
        let weak = Arc::downgrade(entry);
        {
            let mut map = self.inner.response_sessions.write().await;
            map.retain(|_, value| {
                value
                    .candidates
                    .retain(|candidate| !Weak::ptr_eq(&candidate.entry, &weak));
                !value.candidates.is_empty()
            });
        }
        {
            let mut map = self.inner.tool_call_sessions.write().await;
            map.retain(|_, value| {
                value
                    .candidates
                    .retain(|candidate| !Weak::ptr_eq(&candidate.entry, &weak));
                !value.candidates.is_empty()
            });
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
        // A timer may be this very task. Drop its handle before clearing
        // retained state so it cannot abort itself before index cleanup.
        session.idle_expiry_task = None;
        session.clear_retained_state();
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
                session.clear_retained_state();
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
            idle_expiry_task: None,
            request_memory: None,
            index_retained_bytes: 0,
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
    async fn parked_session_reservation_transfers_and_releases_on_close() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("memory-transfer"));
        let original_budget = RequestMemoryBudget::new(128 * 1024);
        let next_budget = RequestMemoryBudget::new(128 * 1024);
        let entry = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                vec![serde_json::json!({"type":"message","content":"x".repeat(4096)})],
                "/workspace".to_string(),
            )
            .await
            .unwrap();
        {
            let mut session = entry.lock().await;
            session.attach_request_memory(&original_budget).unwrap();
        }
        let original_used = original_budget.snapshot().used_bytes;
        assert!(original_used > 4_096);

        mgr.release(entry.clone(), SessionState::AwaitingToolResult)
            .await;
        assert_eq!(original_budget.snapshot().used_bytes, original_used);

        let acquired = mgr.acquire(&key).await.unwrap();
        {
            let mut session = acquired.lock().await;
            session.attach_request_memory(&next_budget).unwrap();
        }
        assert_eq!(original_budget.snapshot().used_bytes, 0);
        assert!(next_budget.snapshot().used_bytes > 4_096);

        mgr.release(acquired, SessionState::Closed).await;
        assert_eq!(next_budget.snapshot().used_bytes, 0);
        assert_eq!(next_budget.snapshot().active_reservations, 0);
    }

    #[tokio::test]
    async fn parked_session_memory_counts_both_conversation_key_copies() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("conversation-key-memory");
        let short_budget = RequestMemoryBudget::new(128 * 1024);
        let long_budget = RequestMemoryBudget::new(128 * 1024);
        let short = mgr
            .reserve(
                CursorSessionKey::new(scope.clone(), "short"),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        short
            .lock()
            .await
            .attach_request_memory(&short_budget)
            .unwrap();
        let long = mgr
            .reserve(
                CursorSessionKey::new(scope, "x".repeat(4096)),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        long.lock()
            .await
            .attach_request_memory(&long_budget)
            .unwrap();
        assert!(
            long_budget.snapshot().used_bytes >= short_budget.snapshot().used_bytes + 8_000,
            "session and registry each retain a conversation key copy"
        );
        mgr.release(short, SessionState::Closed).await;
        mgr.release(long, SessionState::Closed).await;
    }

    #[tokio::test]
    async fn secondary_index_binding_charges_request_memory() {
        let mgr = CursorSessionManager::default();
        let key = session_key(&CursorSessionScope::fixture("index-memory"));
        let entry = mgr
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
        let budget = RequestMemoryBudget::new(128 * 1024);
        entry.lock().await.attach_request_memory(&budget).unwrap();
        let before = budget.snapshot().used_bytes;
        mgr.bind_response_id(&key, &entry, &"r".repeat(4096)).await;
        assert!(budget.snapshot().used_bytes >= before + 4096);
        let after_response = budget.snapshot().used_bytes;
        mgr.bind_tool_call_id(&key, &entry, &"t".repeat(512)).await;
        assert!(budget.snapshot().used_bytes >= after_response + 512);
        mgr.release(entry, SessionState::Closed).await;
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn secondary_index_binding_rejects_before_registry_insert() {
        for (label, response) in [("response", true), ("tool", false)] {
            let mgr = CursorSessionManager::default();
            let key = session_key(&CursorSessionScope::fixture(label));
            let entry = mgr
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
            let base = session_retained_bytes(&*entry.lock().await);
            let budget = RequestMemoryBudget::new(base + 512);
            entry.lock().await.attach_request_memory(&budget).unwrap();
            let id = "i".repeat(512);
            if response {
                assert!(mgr.try_bind_response_id(&key, &entry, &id).await.is_err());
                assert!(mgr.resolve_response_id(key.scope(), &id).await.is_none());
            } else {
                assert!(mgr.try_bind_tool_call_id(&key, &entry, &id).await.is_err());
                assert!(mgr.resolve_tool_call_id(key.scope(), &id).await.is_none());
            }
            assert_eq!(budget.snapshot().used_bytes, base);
            mgr.release(entry, SessionState::Closed).await;
            assert_eq!(budget.snapshot().used_bytes, 0);
        }
    }

    #[tokio::test]
    async fn expired_parked_session_releases_request_memory() {
        let mgr = CursorSessionManager::new(Duration::from_millis(1), 4);
        let key = session_key(&CursorSessionScope::fixture("memory-expiry"));
        let budget = RequestMemoryBudget::new(64 * 1024);
        let entry = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                vec![serde_json::json!({"content":"parked".repeat(128)})],
                String::new(),
            )
            .await
            .unwrap();
        entry.lock().await.attach_request_memory(&budget).unwrap();
        mgr.release(entry, SessionState::AwaitingToolResult).await;
        assert!(budget.snapshot().used_bytes > 0);

        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(mgr.acquire(&key).await.is_none());
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(budget.snapshot().active_reservations, 0);
    }

    #[tokio::test]
    async fn parked_session_expires_without_another_registry_request() {
        let mgr = CursorSessionManager::new(Duration::from_millis(5), 4);
        let key = session_key(&CursorSessionScope::fixture("idle-expiry"));
        let budget = RequestMemoryBudget::new(64 * 1024);
        let entry = mgr
            .reserve(
                key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                vec![serde_json::json!({"content":"parked".repeat(128)})],
                String::new(),
            )
            .await
            .unwrap();
        entry.lock().await.attach_request_memory(&budget).unwrap();
        mgr.release(entry, SessionState::AwaitingToolResult).await;
        assert!(budget.snapshot().used_bytes > 0);

        tokio::time::timeout(Duration::from_secs(1), async {
            while mgr.has(&key).await {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("idle parked session should expire without a new request");
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(budget.snapshot().active_reservations, 0);
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
    async fn duplicate_ids_in_one_scope_never_select_the_last_parked_session() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("duplicate-ids");
        let key_a = CursorSessionKey::new(scope.clone(), "conversation-a");
        let key_b = CursorSessionKey::new(scope.clone(), "conversation-b");
        let entry_a = session_entry(key_a.clone(), SessionState::AwaitingToolResult);
        let entry_b = session_entry(key_b.clone(), SessionState::AwaitingToolResult);
        {
            let mut sessions = mgr.inner.sessions.write().await;
            sessions.insert(key_a.clone(), entry_a.clone());
            sessions.insert(key_b.clone(), entry_b.clone());
        }
        mgr.bind_response_id(&key_a, &entry_a, "resp-collision")
            .await;
        mgr.bind_tool_call_id(&key_a, &entry_a, "call-collision")
            .await;
        mgr.bind_response_id(&key_a, &entry_a, "resp-collision")
            .await;
        assert_eq!(
            mgr.resolve_response_id(&scope, "resp-collision")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a.clone())
        );
        for (key, entry) in [(&key_a, &entry_a), (&key_b, &entry_b)] {
            mgr.bind_response_id(key, entry, "resp-collision").await;
            mgr.bind_tool_call_id(key, entry, "call-collision").await;
        }
        assert!(mgr
            .resolve_response_id(&scope, "resp-collision")
            .await
            .is_none());
        assert!(mgr
            .resolve_tool_call_id(&scope, "call-collision")
            .await
            .is_none());

        mgr.release(entry_b, SessionState::Closed).await;
        assert_eq!(
            mgr.resolve_response_id(&scope, "resp-collision")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope, "call-collision")
                .await
                .map(|reference| reference.key().clone()),
            Some(key_a)
        );
    }

    #[tokio::test]
    async fn running_owner_keeps_a_colliding_parked_id_ambiguous() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("running-parked-collision");
        let running_key = CursorSessionKey::new(scope.clone(), "conversation-running");
        let parked_key = CursorSessionKey::new(scope.clone(), "conversation-parked");
        let running = session_entry(running_key.clone(), SessionState::Running);
        let parked = session_entry(parked_key.clone(), SessionState::AwaitingToolResult);
        {
            let mut sessions = mgr.inner.sessions.write().await;
            sessions.insert(running_key.clone(), running.clone());
            sessions.insert(parked_key.clone(), parked.clone());
        }
        for (key, entry) in [(&running_key, &running), (&parked_key, &parked)] {
            mgr.bind_response_id(key, entry, "response-collision").await;
            mgr.bind_tool_call_id(key, entry, "call-collision").await;
        }

        // A duplicate result may belong to the in-flight owner. Selecting the
        // parked owner just because it is currently acquirable risks resuming
        // another conversation's h2 stream.
        assert!(mgr
            .resolve_response_id(&scope, "response-collision")
            .await
            .is_none());
        assert!(mgr
            .resolve_tool_call_id(&scope, "call-collision")
            .await
            .is_none());
        assert_eq!(parked.lock().await.state, SessionState::AwaitingToolResult);

        mgr.release(running, SessionState::Closed).await;
        assert_eq!(
            mgr.resolve_response_id(&scope, "response-collision")
                .await
                .map(|reference| reference.key().clone()),
            Some(parked_key.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope, "call-collision")
                .await
                .map(|reference| reference.key().clone()),
            Some(parked_key)
        );
    }

    #[tokio::test]
    async fn expired_index_candidate_does_not_mask_a_live_parked_owner() {
        let ttl = Duration::from_secs(300);
        let mgr = CursorSessionManager::new(ttl, 10);
        let scope = CursorSessionScope::fixture("expired-collision");
        let stale_key = CursorSessionKey::new(scope.clone(), "conversation-stale");
        let live_key = CursorSessionKey::new(scope.clone(), "conversation-live");
        let stale = session_entry(stale_key.clone(), SessionState::AwaitingToolResult);
        let live = session_entry(live_key.clone(), SessionState::AwaitingToolResult);
        {
            let mut sessions = mgr.inner.sessions.write().await;
            sessions.insert(stale_key.clone(), stale.clone());
            sessions.insert(live_key.clone(), live.clone());
        }
        for (key, entry) in [(&stale_key, &stale), (&live_key, &live)] {
            mgr.bind_response_id(key, entry, "response-shared").await;
            mgr.bind_tool_call_id(key, entry, "call-shared").await;
        }
        stale.lock().await.last_activity = Instant::now() - ttl - Duration::from_secs(1);

        assert_eq!(
            mgr.resolve_response_id(&scope, "response-shared")
                .await
                .map(|reference| reference.key().clone()),
            Some(live_key.clone())
        );
        assert_eq!(
            mgr.resolve_tool_call_id(&scope, "call-shared")
                .await
                .map(|reference| reference.key().clone()),
            Some(live_key.clone())
        );
        assert!(!mgr.has(&stale_key).await);
        assert_eq!(stale.lock().await.state, SessionState::Closed);
        assert_eq!(live.lock().await.state, SessionState::AwaitingToolResult);
    }

    #[tokio::test]
    async fn parked_response_and_tool_ids_remain_exact() {
        let mgr = CursorSessionManager::default();
        let scope = CursorSessionScope::fixture("exact-id-share");
        let key = session_key(&scope);
        let entry = session_entry(key.clone(), SessionState::AwaitingToolResult);
        mgr.inner
            .sessions
            .write()
            .await
            .insert(key.clone(), entry.clone());
        mgr.bind_response_id(&key, &entry, "resp-a").await;
        mgr.bind_tool_call_id(&key, &entry, "call-a").await;
        assert!(mgr.resolve_response_id(&scope, "resp-a").await.is_some());
        assert!(mgr.resolve_tool_call_id(&scope, "call-a").await.is_some());
        for padded in [" resp-a", "resp-a "] {
            assert!(mgr.resolve_response_id(&scope, padded).await.is_none());
        }
        for padded in [" call-a", "call-a "] {
            assert!(mgr.resolve_tool_call_id(&scope, padded).await.is_none());
        }
        mgr.claim_continuation_ids(&scope, &["call-a".to_string()])
            .await
            .unwrap();
        assert!(
            !mgr.continuation_was_claimed(&scope, &[" call-a".to_string()])
                .await
        );
    }

    #[tokio::test]
    async fn consumed_continuation_claims_are_scope_isolated() {
        let mgr = CursorSessionManager::default();
        let scope_a = CursorSessionScope::fixture("claim-share-a");
        let scope_b = CursorSessionScope::fixture("claim-share-b");
        let ids = vec!["call_same".to_string(), "resp_same".to_string()];

        mgr.claim_continuation_ids(&scope_a, &ids).await.unwrap();

        assert!(mgr.continuation_was_claimed(&scope_a, &ids).await);
        assert!(!mgr.continuation_was_claimed(&scope_b, &ids).await);
    }

    #[tokio::test]
    async fn consumed_continuation_claim_cannot_be_reused_by_a_later_batch() {
        let mgr = CursorSessionManager::default();
        let scope_a = CursorSessionScope::fixture("claim-reuse-a");
        let scope_b = CursorSessionScope::fixture("claim-reuse-b");
        let consumed = "call-consumed".to_string();
        let fresh = "call-fresh".to_string();
        mgr.claim_continuation_ids(&scope_a, std::slice::from_ref(&consumed))
            .await
            .unwrap();

        assert_eq!(
            mgr.claim_continuation_ids(&scope_a, &[consumed.clone(), fresh.clone()])
                .await,
            Err(CursorContinuationClaimError::AlreadyClaimed)
        );
        assert!(
            mgr.continuation_was_claimed(&scope_a, std::slice::from_ref(&consumed))
                .await
        );
        assert!(!mgr.continuation_was_claimed(&scope_a, &[fresh]).await);
        assert!(mgr
            .claim_continuation_ids(&scope_b, &[consumed])
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn unexpired_continuation_claims_survive_capacity_pressure() {
        let mgr = CursorSessionManager::new(Duration::from_secs(300), 1);
        let scope = CursorSessionScope::fixture("claim-capacity");
        let original = "call_original".to_string();
        mgr.claim_continuation_ids(&scope, std::slice::from_ref(&original))
            .await
            .unwrap();

        // The minimum claim ceiling is 64. A later batch must not displace
        // an unexpired replay fence merely because its own IDs fill the map.
        let competing = (0..64)
            .map(|index| format!("call_{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            mgr.claim_continuation_ids(&scope, &competing).await,
            Err(CursorContinuationClaimError::CapacityExhausted)
        );

        assert!(mgr.continuation_was_claimed(&scope, &[original]).await);
        assert!(!mgr.continuation_was_claimed(&scope, &competing).await);
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
        assert_eq!(
            conflict,
            CursorSessionReserveError::ConversationBusy(CursorSessionOpenConflict {
                state: SessionState::Running,
            })
        );
        let current = mgr.inner.sessions.read().await.get(&key).cloned().unwrap();
        assert!(Arc::ptr_eq(&current, &first));
    }

    #[tokio::test]
    async fn running_sessions_do_not_overrun_global_session_capacity() {
        let mgr = CursorSessionManager::new(Duration::from_secs(300), 1);
        let first_key = session_key(&CursorSessionScope::fixture("capacity-first"));
        let second_key = session_key(&CursorSessionScope::fixture("capacity-second"));
        let first = mgr
            .reserve(
                first_key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        let second = mgr
            .reserve(
                second_key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await;
        assert!(second.is_err());
        assert!(matches!(
            second,
            Err(CursorSessionReserveError::CapacityExhausted)
        ));
        assert_eq!(mgr.size().await, 1);
        assert!(mgr.has(&first_key).await);
        assert!(!mgr.has(&second_key).await);
        assert_eq!(first.lock().await.state, SessionState::Running);
    }

    #[tokio::test]
    async fn parked_session_can_be_evicted_to_admit_new_running_session() {
        let mgr = CursorSessionManager::new(Duration::from_secs(300), 1);
        let first_key = session_key(&CursorSessionScope::fixture("capacity-parked"));
        let second_key = session_key(&CursorSessionScope::fixture("capacity-new"));
        let first = mgr
            .reserve(
                first_key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        mgr.release(first.clone(), SessionState::AwaitingToolResult)
            .await;
        let second = mgr
            .reserve(
                second_key.clone(),
                CursorProtocolRail::OAuthCli,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        assert_eq!(mgr.size().await, 1);
        assert!(!mgr.has(&first_key).await);
        assert!(mgr.has(&second_key).await);
        assert_eq!(first.lock().await.state, SessionState::Closed);
        assert_eq!(second.lock().await.state, SessionState::Running);
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
