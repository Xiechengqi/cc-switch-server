//! Cursor `agent.v1.AgentService/Run` session registry.
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
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::sync::RwLock;

const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_SESSIONS: usize = 100;

/// Per-pending-tool-call bookkeeping. `exec_msg_id` + `exec_id` are the
/// identifiers cursor needs in the `McpResult` reply.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub exec_msg_id: u64,
    pub exec_id: String,
    pub tool_name: String,
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

/// Live state of a single AgentService run.
pub struct CursorSession {
    pub conversation_id: String,
    pub stream: Option<CursorH2Stream>,
    /// MCP tool names declared on the inbound turn (for shell→MCP bridging).
    pub declared_tool_names: Vec<String>,
    /// Full declared MCP tool definitions for response-side schema validation.
    pub declared_tools: Vec<McpToolDef>,
    /// Working directory for RequestContext ack.
    pub working_directory: String,
    /// Map: client-facing tool call id → cursor exec metadata.
    pub pending_tool_calls: HashMap<String, PendingToolCall>,
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
    sessions: RwLock<HashMap<String, Arc<Mutex<CursorSession>>>>,
    response_sessions: RwLock<HashMap<String, String>>,
    tool_call_sessions: RwLock<HashMap<String, String>>,
    idle_ttl: Duration,
    max_sessions: usize,
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
                idle_ttl,
                max_sessions,
            }),
        }
    }

    /// Try to reacquire a parked session. Returns `Some` only when the entry
    /// exists, is `AwaitingToolResult`, and hasn't passed its idle TTL.
    pub async fn acquire(&self, conversation_id: &str) -> Option<Arc<Mutex<CursorSession>>> {
        self.evict_expired().await;
        let map = self.inner.sessions.read().await;
        let entry = map.get(conversation_id)?.clone();
        drop(map);

        let mut session = entry.lock().await;
        if session.state != SessionState::AwaitingToolResult {
            return None;
        }
        session.state = SessionState::Running;
        session.touch();
        drop(session);
        Some(entry)
    }

    /// Register a freshly-opened h2 stream as the session for this
    /// conversation. If a session for the same key already exists, it is
    /// closed (the request body channel is dropped, releasing hyper).
    pub async fn open(
        &self,
        conversation_id: String,
        stream: CursorH2Stream,
        blob_store: HashMap<String, Bytes>,
        declared_tools: Vec<McpToolDef>,
        working_directory: String,
    ) -> Arc<Mutex<CursorSession>> {
        // Close any existing entry first.
        let existing = {
            let mut map = self.inner.sessions.write().await;
            map.remove(&conversation_id)
        };
        if let Some(prev) = existing {
            let mut guard = prev.lock().await;
            guard.state = SessionState::Closed;
            // Dropping the CursorH2Stream's writer drops the body sender; we
            // intentionally leave the response side ungathered — hyper will
            // drop the stream when the response goes out of scope.
            drop(guard);
            self.remove_indexes_for_session(&conversation_id).await;
        }

        let declared_tool_names = declared_tools.iter().map(|t| t.name.clone()).collect();
        let session = CursorSession {
            conversation_id: conversation_id.clone(),
            stream: Some(stream),
            declared_tool_names,
            declared_tools,
            working_directory,
            pending_tool_calls: HashMap::new(),
            blob_store,
            state: SessionState::Running,
            last_activity: Instant::now(),
        };
        let entry = Arc::new(Mutex::new(session));
        {
            let mut map = self.inner.sessions.write().await;
            map.insert(conversation_id, entry.clone());
        }
        self.enforce_max_sessions().await;
        entry
    }

    /// Mark a session as no longer in-flight. `AwaitingToolResult` parks it
    /// for reacquisition; `Closed` evicts it immediately.
    pub async fn release(&self, entry: Arc<Mutex<CursorSession>>, final_state: SessionState) {
        let conversation_id = {
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
                    session.conversation_id.clone()
                }
            }
        };
        let mut map = self.inner.sessions.write().await;
        map.remove(&conversation_id);
        self.remove_indexes_for_session(&conversation_id).await;
    }

    pub async fn bind_response_id(&self, response_id: &str, conversation_id: &str) {
        let response_id = response_id.trim();
        let conversation_id = conversation_id.trim();
        if response_id.is_empty() || conversation_id.is_empty() {
            return;
        }
        self.inner
            .response_sessions
            .write()
            .await
            .insert(response_id.to_string(), conversation_id.to_string());
    }

    pub async fn resolve_response_id(&self, response_id: &str) -> Option<String> {
        let response_id = response_id.trim();
        if response_id.is_empty() {
            return None;
        }
        self.inner
            .response_sessions
            .read()
            .await
            .get(response_id)
            .cloned()
    }

    pub async fn bind_tool_call_id(&self, tool_call_id: &str, conversation_id: &str) {
        let tool_call_id = tool_call_id.trim();
        let conversation_id = conversation_id.trim();
        if tool_call_id.is_empty() || conversation_id.is_empty() {
            return;
        }
        self.inner
            .tool_call_sessions
            .write()
            .await
            .insert(tool_call_id.to_string(), conversation_id.to_string());
    }

    pub async fn resolve_tool_call_id(&self, tool_call_id: &str) -> Option<String> {
        let tool_call_id = tool_call_id.trim();
        if tool_call_id.is_empty() {
            return None;
        }
        self.inner
            .tool_call_sessions
            .read()
            .await
            .get(tool_call_id)
            .cloned()
    }

    pub async fn take_stream(
        &self,
        conversation_id: &str,
    ) -> Option<(Arc<Mutex<CursorSession>>, CursorH2Stream)> {
        self.evict_expired().await;
        let map = self.inner.sessions.read().await;
        let entry = map.get(conversation_id)?.clone();
        drop(map);

        let mut session = entry.lock().await;
        if session.state != SessionState::AwaitingToolResult {
            return None;
        }
        let stream = session.stream.take()?;
        session.state = SessionState::Running;
        session.touch();
        drop(session);
        Some((entry, stream))
    }

    async fn remove_indexes_for_session(&self, conversation_id: &str) {
        {
            let mut map = self.inner.response_sessions.write().await;
            map.retain(|_, v| v != conversation_id);
        }
        {
            let mut map = self.inner.tool_call_sessions.write().await;
            map.retain(|_, v| v != conversation_id);
        }
    }

    async fn evict_expired(&self) {
        let now = Instant::now();
        let mut to_remove: Vec<String> = Vec::new();
        {
            let map = self.inner.sessions.read().await;
            for (k, entry) in map.iter() {
                if let Ok(session) = entry.try_lock() {
                    if session.state != SessionState::Running
                        && now.duration_since(session.last_activity) > self.inner.idle_ttl
                    {
                        to_remove.push(k.clone());
                    }
                }
            }
        }
        if !to_remove.is_empty() {
            let mut map = self.inner.sessions.write().await;
            for k in to_remove {
                if let Some(entry) = map.remove(&k) {
                    if let Ok(mut session) = entry.try_lock() {
                        session.state = SessionState::Closed;
                        session.stream = None;
                    }
                    drop(map);
                    self.remove_indexes_for_session(&k).await;
                    map = self.inner.sessions.write().await;
                }
            }
        }
    }

    async fn enforce_max_sessions(&self) {
        loop {
            let len = self.inner.sessions.read().await.len();
            if len <= self.inner.max_sessions {
                break;
            }
            // Find the least-recently-active entry that isn't running.
            let oldest_key: Option<String> = {
                let map = self.inner.sessions.read().await;
                let mut candidates: Vec<(String, Instant)> = Vec::new();
                for (k, entry) in map.iter() {
                    if let Ok(session) = entry.try_lock() {
                        if session.state != SessionState::Running {
                            candidates.push((k.clone(), session.last_activity));
                        }
                    }
                }
                candidates
                    .into_iter()
                    .min_by_key(|(_, t)| *t)
                    .map(|(k, _)| k)
            };
            let Some(key) = oldest_key else { break };
            let mut map = self.inner.sessions.write().await;
            map.remove(&key);
            drop(map);
            self.remove_indexes_for_session(&key).await;
        }
    }

    pub async fn size(&self) -> usize {
        self.inner.sessions.read().await.len()
    }

    pub async fn has(&self, conversation_id: &str) -> bool {
        self.inner
            .sessions
            .read()
            .await
            .contains_key(conversation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_size_starts_at_zero() {
        let mgr = CursorSessionManager::default();
        assert_eq!(mgr.size().await, 0);
        assert!(!mgr.has("foo").await);
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
            },
        );
        let got = pending.get("tc_1").unwrap();
        assert_eq!(got.exec_msg_id, 7);
        assert_eq!(got.exec_id, "exec-x");
    }

    #[tokio::test]
    async fn response_and_tool_indexes_resolve() {
        let mgr = CursorSessionManager::default();
        mgr.bind_response_id("resp_1", "session_1").await;
        mgr.bind_tool_call_id("call_1", "session_1").await;
        assert_eq!(
            mgr.resolve_response_id("resp_1").await.as_deref(),
            Some("session_1")
        );
        assert_eq!(
            mgr.resolve_tool_call_id("call_1").await.as_deref(),
            Some("session_1")
        );
        mgr.remove_indexes_for_session("session_1").await;
        assert!(mgr.resolve_response_id("resp_1").await.is_none());
        assert!(mgr.resolve_tool_call_id("call_1").await.is_none());
    }
}
