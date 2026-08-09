use std::collections::BTreeMap;

use serde_json::Value;

const CACHE_TTL_MS: i64 = 10 * 60 * 1_000;
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
const CACHE_MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 2_000;
const CACHE_MAX_ITEMS_PER_ENTRY: usize = 200;
const CACHE_MAX_RESPONSE_ID_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreviousResponseCacheScope {
    pub share_id: String,
    pub principal: String,
    pub runtime_fingerprint: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PreviousResponseCacheKey {
    scope: PreviousResponseCacheScope,
    response_id: String,
}

#[derive(Debug, Clone)]
struct PreviousResponseCacheEntry {
    items: Vec<Value>,
    bytes: usize,
    expires_at_ms: i64,
    access_sequence: u64,
}

#[derive(Debug, Default)]
pub struct PreviousResponseCache {
    entries: BTreeMap<PreviousResponseCacheKey, PreviousResponseCacheEntry>,
    total_bytes: usize,
    access_sequence: u64,
}

impl PreviousResponseCache {
    pub fn get(
        &mut self,
        scope: &PreviousResponseCacheScope,
        response_id: &str,
        now_ms: i64,
    ) -> Option<Vec<Value>> {
        self.cleanup(now_ms);
        let key = cache_key(scope, response_id)?;
        self.access_sequence = self.access_sequence.wrapping_add(1);
        let entry = self.entries.get_mut(&key)?;
        entry.access_sequence = self.access_sequence;
        Some(entry.items.clone())
    }

    pub fn insert(
        &mut self,
        scope: PreviousResponseCacheScope,
        response_id: &str,
        items: Vec<Value>,
        now_ms: i64,
    ) -> bool {
        self.cleanup(now_ms);
        let Some(key) = cache_key(&scope, response_id) else {
            return false;
        };
        if items.is_empty() || items.len() > CACHE_MAX_ITEMS_PER_ENTRY {
            return false;
        }
        let Some(bytes) = encoded_items_size(&items) else {
            return false;
        };
        if bytes > CACHE_MAX_ENTRY_BYTES || bytes > CACHE_MAX_BYTES {
            return false;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        while self.entries.len() >= CACHE_MAX_ENTRIES
            || self.total_bytes.saturating_add(bytes) > CACHE_MAX_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.access_sequence)
                .map(|(key, _)| key.clone())
            else {
                return false;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(removed.bytes);
            }
        }
        self.access_sequence = self.access_sequence.wrapping_add(1);
        self.entries.insert(
            key,
            PreviousResponseCacheEntry {
                items,
                bytes,
                expires_at_ms: now_ms.saturating_add(CACHE_TTL_MS),
                access_sequence: self.access_sequence,
            },
        );
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        true
    }

    fn cleanup(&mut self, now_ms: i64) {
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expires_at_ms <= now_ms)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            if let Some(entry) = self.entries.remove(&key) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

fn cache_key(
    scope: &PreviousResponseCacheScope,
    response_id: &str,
) -> Option<PreviousResponseCacheKey> {
    let response_id = response_id.trim();
    if response_id.is_empty() || response_id.len() > CACHE_MAX_RESPONSE_ID_LEN {
        return None;
    }
    Some(PreviousResponseCacheKey {
        scope: scope.clone(),
        response_id: response_id.to_string(),
    })
}

fn encoded_items_size(items: &[Value]) -> Option<usize> {
    items.iter().try_fold(0_usize, |total, item| {
        serde_json::to_vec(item)
            .ok()
            .and_then(|encoded| total.checked_add(encoded.len()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(share_id: &str, principal: &str) -> PreviousResponseCacheScope {
        PreviousResponseCacheScope {
            share_id: share_id.to_string(),
            principal: principal.to_string(),
            runtime_fingerprint: "runtime-a".to_string(),
            workspace_id: "workspace-a".to_string(),
        }
    }

    #[test]
    fn cache_isolates_every_namespace_dimension_and_expires_entries() {
        let mut cache = PreviousResponseCache::default();
        let base = scope("share-a", "user-a@example.com");
        assert!(cache.insert(
            base.clone(),
            "resp-a",
            vec![json!({"type":"function_call","call_id":"call-a"})],
            1_000,
        ));
        assert!(cache.get(&base, "resp-a", 1_001).is_some());
        assert!(cache.get(&base, "resp-b", 1_001).is_none());
        assert!(cache
            .get(&scope("share-a", "user-b@example.com"), "resp-a", 1_001,)
            .is_none());
        assert!(cache
            .get(&scope("share-b", "user-a@example.com"), "resp-a", 1_001,)
            .is_none());
        let mut different_runtime = base.clone();
        different_runtime.runtime_fingerprint = "runtime-b".to_string();
        assert!(cache.get(&different_runtime, "resp-a", 1_001).is_none());
        let mut different_workspace = base.clone();
        different_workspace.workspace_id = "workspace-b".to_string();
        assert!(cache.get(&different_workspace, "resp-a", 1_001).is_none());
        assert!(cache.get(&base, "resp-a", 1_000 + CACHE_TTL_MS).is_none());
    }

    #[test]
    fn cache_rejects_oversized_item_sets() {
        let mut cache = PreviousResponseCache::default();
        assert!(!cache.insert(
            scope("share-a", "user@example.com"),
            "resp-many",
            (0..=CACHE_MAX_ITEMS_PER_ENTRY)
                .map(|index| json!({"type":"function_call","call_id":index}))
                .collect(),
            1_000,
        ));
        assert!(!cache.insert(
            scope("share-a", "user@example.com"),
            "resp-large",
            vec![json!({"type":"function_call","arguments":"x".repeat(CACHE_MAX_ENTRY_BYTES)})],
            1_000,
        ));
    }

    #[test]
    fn cache_enforces_entry_count_with_lru_eviction() {
        let mut cache = PreviousResponseCache::default();
        let scope = scope("share-a", "user@example.com");
        for index in 0..CACHE_MAX_ENTRIES {
            assert!(cache.insert(
                scope.clone(),
                &format!("resp-{index}"),
                vec![json!({"type":"function_call","call_id":format!("call-{index}")})],
                1_000,
            ));
        }
        assert!(cache.get(&scope, "resp-0", 1_001).is_some());
        assert!(cache.insert(
            scope.clone(),
            "resp-overflow",
            vec![json!({"type":"function_call","call_id":"call-overflow"})],
            1_002,
        ));

        assert!(cache.get(&scope, "resp-0", 1_003).is_some());
        assert!(cache.get(&scope, "resp-1", 1_003).is_none());
        assert!(cache.get(&scope, "resp-overflow", 1_003).is_some());
    }
}
