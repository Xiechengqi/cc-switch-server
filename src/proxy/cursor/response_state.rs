//! Bounded, scope-isolated semantic state for completed OpenAI Responses turns.
//!
//! Unlike `CursorSessionManager`, entries here never contain an h2 stream or a
//! credential. They are safe-to-replay normalized input/output items and are
//! intentionally process-local: restart/expiry is reported as state loss.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;

use super::resident::entry_bytes;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};
use crate::proxy::request_memory::{
    retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent, RequestMemoryError,
    RequestMemoryReservation,
};

const TTL_MS: i64 = 10 * 60 * 1_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_ITEMS: usize = 200;
const MAX_RESPONSE_ID_LEN: usize = 512;
const MAX_CONVERSATION_ID_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CursorResponseScope(String);

pub struct CursorResponseScopeInput<'a> {
    pub app: &'a str,
    pub provider_id: &'a str,
    pub provider_revision: u64,
    pub runtime_fingerprint: &'a str,
    pub rail: &'a str,
    pub protocol_revision: &'a str,
    pub credential_identity: &'a str,
    pub share_id: Option<&'a str>,
    pub user_email: Option<&'a str>,
    pub workspace_id: &'a str,
}

impl CursorResponseScope {
    pub fn derive(input: CursorResponseScopeInput<'_>) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-server:cursor-response-scope:v1\0");
        for (label, value) in [
            ("app", input.app.to_string()),
            ("provider", input.provider_id.to_string()),
            ("provider_revision", input.provider_revision.to_string()),
            ("runtime", input.runtime_fingerprint.to_string()),
            ("rail", input.rail.to_string()),
            ("protocol", input.protocol_revision.to_string()),
            ("credential_identity", input.credential_identity.to_string()),
            ("share", normalized(input.share_id, "<direct-share>", false)),
            ("user", normalized(input.user_email, "<direct-user>", true)),
            ("workspace", input.workspace_id.trim().to_string()),
        ] {
            digest.update((label.len() as u64).to_be_bytes());
            digest.update(label.as_bytes());
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Self(format!("cursor-response-v1:{:x}", digest.finalize()))
    }
}

fn normalized(value: Option<&str>, fallback: &str, lowercase: bool) -> String {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match (value, lowercase) {
        (Some(value), true) => value.to_ascii_lowercase(),
        (Some(value), false) => value.to_string(),
        (None, _) => fallback.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    scope: CursorResponseScope,
    response_id: String,
}

#[derive(Debug)]
struct Entry {
    items: Vec<Value>,
    conversation_id: String,
    local_task_state: CursorLocalTaskState,
    bytes: usize,
    expires_at_ms: i64,
    access_sequence: u64,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug, Clone)]
pub struct CursorCompletedResponse {
    pub items: Vec<Value>,
    pub conversation_id: String,
    pub local_task_state: CursorLocalTaskState,
}

/// Credential-free semantic state carried across OpenAI Responses turns.
///
/// This deliberately stores no prompt or tool output. `origin_turn_digest`
/// is an opaque correlation value that lets us distinguish a continued local
/// task from a newly activated one without retaining user text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorLocalTaskState {
    pub active: bool,
    pub tool_commit_required: bool,
    pub origin_turn_digest: String,
    pub generation: u64,
}

#[derive(Debug, Default)]
struct Inner {
    entries: BTreeMap<Key, Entry>,
    total_bytes: usize,
    access_sequence: u64,
}

#[derive(Debug)]
pub struct CursorCompletedResponseStore {
    inner: Mutex<Inner>,
    parent: ResidentMemoryBudget,
}

impl Default for CursorCompletedResponseStore {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl CursorCompletedResponseStore {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            parent,
        }
    }

    #[cfg(test)]
    pub fn get(
        &self,
        scope: &CursorResponseScope,
        response_id: &str,
        now_ms: i64,
    ) -> Option<CursorCompletedResponse> {
        let key = cache_key(scope, response_id)?;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cleanup(&mut inner, now_ms);
        inner.access_sequence = inner.access_sequence.wrapping_add(1);
        let sequence = inner.access_sequence;
        let entry = inner.entries.get_mut(&key)?;
        entry.access_sequence = sequence;
        Some(CursorCompletedResponse {
            items: entry.items.clone(),
            conversation_id: entry.conversation_id.clone(),
            local_task_state: entry.local_task_state.clone(),
        })
    }

    pub(crate) fn get_accounted(
        &self,
        scope: &CursorResponseScope,
        response_id: &str,
        now_ms: i64,
        request_memory: Option<&RequestMemoryBudget>,
    ) -> Result<
        Option<(CursorCompletedResponse, Option<RequestMemoryReservation>)>,
        RequestMemoryError,
    > {
        let Some(key) = cache_key(scope, response_id) else {
            return Ok(None);
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cleanup(&mut inner, now_ms);
        inner.access_sequence = inner.access_sequence.wrapping_add(1);
        let sequence = inner.access_sequence;
        let Some(entry) = inner.entries.get_mut(&key) else {
            return Ok(None);
        };
        entry.access_sequence = sequence;
        let retained = entry
            .items
            .iter()
            .fold(
                entry
                    .items
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Value>()),
                |total, item| total.saturating_add(retained_json_bytes(item)),
            )
            .saturating_add(entry.conversation_id.capacity())
            .saturating_add(entry.local_task_state.origin_turn_digest.capacity())
            .saturating_add(std::mem::size_of::<CursorCompletedResponse>());
        let reservation = request_memory
            .map(|budget| budget.reserve(RequestMemoryComponent::StreamRetainedState, retained))
            .transpose()?;
        Ok(Some((
            CursorCompletedResponse {
                items: entry.items.clone(),
                conversation_id: entry.conversation_id.clone(),
                local_task_state: entry.local_task_state.clone(),
            },
            reservation,
        )))
    }

    pub fn insert(
        &self,
        scope: CursorResponseScope,
        response_id: &str,
        conversation_id: &str,
        items: Vec<Value>,
        local_task_state: CursorLocalTaskState,
        now_ms: i64,
    ) -> bool {
        let Some(key) = cache_key(&scope, response_id) else {
            return false;
        };
        if conversation_id.trim().is_empty()
            || conversation_id.len() > MAX_CONVERSATION_ID_LEN
            || items.is_empty()
            || items.len() > MAX_ITEMS
        {
            return false;
        }
        if local_task_state.origin_turn_digest.len() > 128 {
            return false;
        }
        let Some(bytes) = encoded_size(&items).and_then(|bytes| {
            bytes
                .checked_add(key.response_id.len())
                .and_then(|bytes| bytes.checked_add(key.scope.0.len()))
                .and_then(|bytes| bytes.checked_add(conversation_id.len()))
                .and_then(|bytes| bytes.checked_add(local_task_state.origin_turn_digest.len()))
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Key>()))
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Entry>()))
        }) else {
            return false;
        };
        if bytes > MAX_ENTRY_BYTES || bytes > MAX_BYTES {
            return false;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cleanup(&mut inner, now_ms);
        if let Some(previous) = inner.entries.remove(&key) {
            inner.total_bytes = inner.total_bytes.saturating_sub(previous.bytes);
        }
        let retained = items.iter().fold(
            entry_bytes::<Key, Entry>(
                key.scope
                    .0
                    .capacity()
                    .saturating_add(key.response_id.capacity())
                    .saturating_add(conversation_id.len())
                    .saturating_add(local_task_state.origin_turn_digest.capacity())
                    .saturating_add(
                        items
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Value>()),
                    ),
            ),
            |bytes, item| {
                bytes.saturating_add(
                    retained_json_bytes(item).saturating_sub(std::mem::size_of::<Value>()),
                )
            },
        );
        let Some(memory) = self.parent.reserve(retained) else {
            return false;
        };
        while inner.entries.len() >= MAX_ENTRIES
            || inner.total_bytes.saturating_add(bytes) > MAX_BYTES
        {
            let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.access_sequence)
                .map(|(key, _)| key.clone())
            else {
                return false;
            };
            if let Some(removed) = inner.entries.remove(&oldest) {
                inner.total_bytes = inner.total_bytes.saturating_sub(removed.bytes);
            }
        }
        inner.access_sequence = inner.access_sequence.wrapping_add(1);
        let access_sequence = inner.access_sequence;
        inner.entries.insert(
            key,
            Entry {
                items,
                conversation_id: conversation_id.to_string(),
                local_task_state,
                bytes,
                expires_at_ms: now_ms.saturating_add(TTL_MS),
                access_sequence,
                _memory: memory,
            },
        );
        inner.total_bytes = inner.total_bytes.saturating_add(bytes);
        true
    }
}

fn cache_key(scope: &CursorResponseScope, response_id: &str) -> Option<Key> {
    if response_id.is_empty()
        || response_id.len() > MAX_RESPONSE_ID_LEN
        || response_id.trim() != response_id
    {
        return None;
    }
    Some(Key {
        scope: scope.clone(),
        response_id: response_id.to_string(),
    })
}

fn encoded_size(items: &[Value]) -> Option<usize> {
    // Counting must not allocate a second, potentially huge serialized copy
    // before the cache's entry/process admission has even run.
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|total| *total <= MAX_ENTRY_BYTES)
                .ok_or_else(|| {
                    std::io::Error::other("Cursor cache entry exceeds its byte limit")
                })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    for item in items {
        serde_json::to_writer(&mut counter, item).ok()?;
    }
    Some(counter.0)
}

fn cleanup(inner: &mut Inner, now_ms: i64) {
    let expired = inner
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at_ms <= now_ms)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in expired {
        if let Some(entry) = inner.entries.remove(&key) {
            inner.total_bytes = inner.total_bytes.saturating_sub(entry.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(name: &str) -> CursorResponseScope {
        CursorResponseScope::derive(CursorResponseScopeInput {
            app: "codex",
            provider_id: "cursor-provider",
            provider_revision: 1,
            runtime_fingerprint: "runtime-a",
            rail: "apikey_sdk",
            protocol_revision: "v2",
            credential_identity: name,
            share_id: Some("share-a"),
            user_email: Some("USER@example.com"),
            workspace_id: "/workspace",
        })
    }

    #[test]
    fn cursor_r4_completed_cache_and_request_copy_have_separate_parent_owners() {
        let parent = ResidentMemoryBudget::new(None);
        let store = CursorCompletedResponseStore::with_parent(parent.clone());
        let scope = scope("retained-owner");
        assert!(store.insert(
            scope.clone(),
            "resp-a",
            "conversation-a",
            vec![json!({"content":"x".repeat(4096)})],
            CursorLocalTaskState::default(),
            1_000
        ));
        let stored_bytes = parent.snapshot().used_bytes;
        assert!(stored_bytes > 4096);
        let request = RequestMemoryBudget::with_parent(64 * 1024, parent.clone());
        let (copy, memory) = store
            .get_accounted(&scope, "resp-a", 1_001, Some(&request))
            .unwrap()
            .unwrap();
        assert_eq!(
            parent.snapshot().used_bytes,
            stored_bytes + request.snapshot().used_bytes
        );
        assert!(request.snapshot().used_bytes > 4096);
        assert!(store.get(&scope, "resp-a", 1_000 + TTL_MS).is_none());
        assert_eq!(parent.snapshot().used_bytes, request.snapshot().used_bytes);
        assert_eq!(copy.conversation_id, "conversation-a");
        drop((copy, memory));
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn cursor_r4_completed_cache_pressure_rejects_without_evicting_another_scope() {
        let parent = ResidentMemoryBudget::new(None);
        let store = CursorCompletedResponseStore::with_parent(parent.clone());
        let scope = scope("pressure");
        let insert = |id, conversation| {
            store.insert(
                scope.clone(),
                id,
                conversation,
                vec![json!({"content":"cached"})],
                CursorLocalTaskState::default(),
                1_000,
            )
        };
        assert!(insert("resp-a", "conversation-a"));
        let used = parent.snapshot().used_bytes;
        parent.configure_limit(Some(used));
        assert!(!insert("resp-b", "conversation-b"));
        assert!(store.get(&scope, "resp-b", 1_001).is_none());
        assert_eq!(
            store.get(&scope, "resp-a", 1_001).unwrap().conversation_id,
            "conversation-a"
        );
        assert_eq!(parent.snapshot().used_bytes, used);
        parent.configure_limit(Some(0));
        assert!(!insert("resp-a", "conversation-new"));
        assert!(store.get(&scope, "resp-a", 1_001).is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn cursor_r4_completed_cache_count_eviction_and_drop_release_parent() {
        let parent = ResidentMemoryBudget::new(None);
        let store = CursorCompletedResponseStore::with_parent(parent.clone());
        let scope = scope("eviction");
        for index in 0..=MAX_ENTRIES {
            assert!(store.insert(
                scope.clone(),
                &format!("resp-{index}"),
                "conversation",
                vec![json!({"content":"cached"})],
                CursorLocalTaskState::default(),
                1_000
            ));
        }
        assert!(store.get(&scope, "resp-0", 1_001).is_none());
        assert_eq!(parent.snapshot().active_reservations, MAX_ENTRIES);
        drop(store);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
    }

    #[test]
    fn cursor_r4_completed_size_counter_bounds_escaped_json_before_admission() {
        let small = vec![json!({"content":"\0\"\\\n🦀"})];
        assert_eq!(
            encoded_size(&small),
            Some(serde_json::to_vec(&small[0]).unwrap().len())
        );
        // Each NUL expands to six JSON bytes. Raw text fits the entry cap,
        // while the serialized form plus object syntax exceeds it.
        let items = vec![json!({"content":"\0".repeat(MAX_ENTRY_BYTES / 6)})];
        assert!(items[0]["content"].as_str().unwrap().len() < MAX_ENTRY_BYTES);
        assert!(encoded_size(&items).is_none());
        let parent = ResidentMemoryBudget::new(Some(0));
        let store = CursorCompletedResponseStore::with_parent(parent.clone());
        assert!(!store.insert(
            scope("escaped"),
            "resp",
            "conversation",
            items,
            CursorLocalTaskState::default(),
            1_000
        ));
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().rejections, 0);
    }

    #[test]
    fn isolates_scope_expires_and_retains_conversation_affinity() {
        let store = CursorCompletedResponseStore::default();
        assert!(store.insert(
            scope("generation-a"),
            "resp-a",
            "conversation-a",
            vec![json!({"type":"message","role":"user","content":"hi"})],
            CursorLocalTaskState {
                active: true,
                tool_commit_required: true,
                origin_turn_digest: "digest-a".to_string(),
                generation: 3,
            },
            1_000,
        ));
        let hit = store.get(&scope("generation-a"), "resp-a", 1_001).unwrap();
        assert_eq!(hit.conversation_id, "conversation-a");
        assert_eq!(hit.local_task_state.generation, 3);
        assert!(hit.local_task_state.tool_commit_required);
        assert!(store.get(&scope("generation-b"), "resp-a", 1_001).is_none());
        assert!(store
            .get(&scope("generation-a"), "resp-a", 1_000 + TTL_MS)
            .is_none());
    }

    #[test]
    fn completed_response_ids_do_not_collapse_padding() {
        let store = CursorCompletedResponseStore::default();
        let scope = scope("generation-a");
        let items = vec![json!({"type":"function_call","call_id":"call-a"})];
        assert!(store.insert(
            scope.clone(),
            "resp-a",
            "conversation-a",
            items.clone(),
            CursorLocalTaskState::default(),
            1_000,
        ));
        for padded in [" resp-a", "resp-a ", "\tresp-a", "resp-a\n"] {
            assert!(store.get(&scope, padded, 1_001).is_none());
            assert!(!store.insert(
                scope.clone(),
                padded,
                "conversation-b",
                items.clone(),
                CursorLocalTaskState::default(),
                1_001,
            ));
        }
        assert_eq!(
            store.get(&scope, "resp-a", 1_002).unwrap().conversation_id,
            "conversation-a"
        );
    }

    #[test]
    fn completed_response_budget_includes_key_and_conversation_identity() {
        let store = CursorCompletedResponseStore::default();
        let response_id = "r".repeat(MAX_RESPONSE_ID_LEN);
        let conversation_id = "c".repeat(MAX_CONVERSATION_ID_LEN);
        let items = vec![json!({"type":"message","content":"hi"})];
        let item_bytes = encoded_size(&items).unwrap();
        assert!(store.insert(
            scope("generation-a"),
            &response_id,
            &conversation_id,
            items,
            CursorLocalTaskState::default(),
            1_000,
        ));
        let inner = store.inner.lock().unwrap();
        assert!(inner.total_bytes >= item_bytes + response_id.len() + conversation_id.len());
    }

    #[test]
    fn rejects_oversized_item_sets_and_invalid_identifiers() {
        let store = CursorCompletedResponseStore::default();
        assert!(!store.insert(
            scope("generation-a"),
            "resp-too-many",
            "conversation-a",
            vec![json!(null); MAX_ITEMS + 1],
            CursorLocalTaskState::default(),
            1_000,
        ));
        assert!(!store.insert(
            scope("generation-a"),
            &"r".repeat(MAX_RESPONSE_ID_LEN + 1),
            "conversation-a",
            vec![json!({"type":"message"})],
            CursorLocalTaskState::default(),
            1_000,
        ));
        assert!(!store.insert(
            scope("generation-a"),
            "resp-empty-conversation",
            " ",
            vec![json!({"type":"message"})],
            CursorLocalTaskState::default(),
            1_000,
        ));
        assert!(!store.insert(
            scope("generation-a"),
            "resp-oversized-conversation",
            &"c".repeat(MAX_RESPONSE_ID_LEN + 1),
            vec![json!({"type":"message"})],
            CursorLocalTaskState::default(),
            1_000,
        ));
    }
}
