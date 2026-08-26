//! Bounded, scope-isolated semantic state for completed OpenAI Responses turns.
//!
//! Unlike `CursorSessionManager`, entries here never contain an h2 stream or a
//! credential. They are safe-to-replay normalized input/output items and are
//! intentionally process-local: restart/expiry is reported as state loss.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;

const TTL_MS: i64 = 10 * 60 * 1_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_ITEMS: usize = 200;
const MAX_RESPONSE_ID_LEN: usize = 512;

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

#[derive(Debug, Clone)]
struct Entry {
    items: Vec<Value>,
    conversation_id: String,
    bytes: usize,
    expires_at_ms: i64,
    access_sequence: u64,
}

#[derive(Debug, Clone)]
pub struct CursorCompletedResponse {
    pub items: Vec<Value>,
    pub conversation_id: String,
}

#[derive(Debug, Default)]
struct Inner {
    entries: BTreeMap<Key, Entry>,
    total_bytes: usize,
    access_sequence: u64,
}

#[derive(Debug, Default)]
pub struct CursorCompletedResponseStore {
    inner: Mutex<Inner>,
}

impl CursorCompletedResponseStore {
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
        })
    }

    pub fn insert(
        &self,
        scope: CursorResponseScope,
        response_id: &str,
        conversation_id: &str,
        items: Vec<Value>,
        now_ms: i64,
    ) -> bool {
        let Some(key) = cache_key(&scope, response_id) else {
            return false;
        };
        if conversation_id.trim().is_empty() || items.is_empty() || items.len() > MAX_ITEMS {
            return false;
        }
        let Some(bytes) = encoded_size(&items) else {
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
                bytes,
                expires_at_ms: now_ms.saturating_add(TTL_MS),
                access_sequence,
            },
        );
        inner.total_bytes = inner.total_bytes.saturating_add(bytes);
        true
    }
}

fn cache_key(scope: &CursorResponseScope, response_id: &str) -> Option<Key> {
    let response_id = response_id.trim();
    if response_id.is_empty() || response_id.len() > MAX_RESPONSE_ID_LEN {
        return None;
    }
    Some(Key {
        scope: scope.clone(),
        response_id: response_id.to_string(),
    })
}

fn encoded_size(items: &[Value]) -> Option<usize> {
    items.iter().try_fold(0usize, |total, item| {
        serde_json::to_vec(item)
            .ok()
            .and_then(|encoded| total.checked_add(encoded.len()))
    })
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
    fn isolates_scope_expires_and_retains_conversation_affinity() {
        let store = CursorCompletedResponseStore::default();
        assert!(store.insert(
            scope("generation-a"),
            "resp-a",
            "conversation-a",
            vec![json!({"type":"message","role":"user","content":"hi"})],
            1_000,
        ));
        let hit = store.get(&scope("generation-a"), "resp-a", 1_001).unwrap();
        assert_eq!(hit.conversation_id, "conversation-a");
        assert!(store.get(&scope("generation-b"), "resp-a", 1_001).is_none());
        assert!(store
            .get(&scope("generation-a"), "resp-a", 1_000 + TTL_MS)
            .is_none());
    }

    #[test]
    fn rejects_oversized_item_sets_and_invalid_identifiers() {
        let store = CursorCompletedResponseStore::default();
        assert!(!store.insert(
            scope("generation-a"),
            "resp-too-many",
            "conversation-a",
            vec![json!(null); MAX_ITEMS + 1],
            1_000,
        ));
        assert!(!store.insert(
            scope("generation-a"),
            &"r".repeat(MAX_RESPONSE_ID_LEN + 1),
            "conversation-a",
            vec![json!({"type":"message"})],
            1_000,
        ));
        assert!(!store.insert(
            scope("generation-a"),
            "resp-empty-conversation",
            " ",
            vec![json!({"type":"message"})],
            1_000,
        ));
    }
}
