use std::collections::BTreeMap;

use crate::infra::resident_memory::{
    process_resident_memory, retained_json_bytes, ResidentMemoryBudget, ResidentMemoryReservation,
};
use serde_json::Value;

const CACHE_TTL_MS: i64 = 10 * 60 * 1_000;
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
const CACHE_MAX_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 2_000;
const CACHE_MAX_ITEMS_PER_ENTRY: usize = 200;
const CACHE_MAX_RESPONSE_ID_LEN: usize = 512;
const TOMBSTONE_TTL_MS: i64 = 2 * 60 * 1_000;
const TOMBSTONE_MAX_ENTRIES: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreviousResponseCacheScope {
    pub share_id: String,
    pub principal: String,
    pub provider_id: String,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub account_id: String,
    pub account_incarnation: u64,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
    pub credential_fingerprint: [u8; 32],
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PreviousResponseCacheKey {
    scope: PreviousResponseCacheScope,
    response_id: String,
}

#[derive(Debug)]
struct PreviousResponseCacheEntry {
    items: Vec<Value>,
    bytes: usize,
    expires_at_ms: i64,
    access_sequence: u64,
    retained_bytes: usize,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousResponseUnavailableReason {
    Expired,
    CountEvicted,
    ByteEvicted,
    EntryTooLarge,
    TooManyItems,
    MemoryCapacity,
}

#[derive(Debug)]
struct PreviousResponseTombstone {
    reason: PreviousResponseUnavailableReason,
    expires_at_ms: i64,
    access_sequence: u64,
    retained_bytes: usize,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviousResponseCacheStats {
    pub current_entries: usize,
    pub current_bytes: usize,
    pub current_tombstones: usize,
    pub current_retained_bytes: usize,
    pub high_water_retained_bytes: usize,
    pub process_capacity_rejections: u64,
    pub high_water_entries: usize,
    pub high_water_bytes: usize,
    pub max_observed_entry_bytes: usize,
    pub max_observed_entry_items: usize,
    pub hits: u64,
    pub misses: u64,
    pub expired: u64,
    pub count_evictions: u64,
    pub byte_evictions: u64,
    pub oversize_entry_rejections: u64,
    pub too_many_items_rejections: u64,
    pub invalid_response_id_rejections: u64,
    pub required_context_unavailable: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviousResponseCacheLookup<T = Vec<Value>> {
    pub items: Option<T>,
    pub unavailable_reason: Option<PreviousResponseUnavailableReason>,
}

#[derive(Debug)]
pub struct PreviousResponseCache {
    entries: BTreeMap<PreviousResponseCacheKey, PreviousResponseCacheEntry>,
    tombstones: BTreeMap<PreviousResponseCacheKey, PreviousResponseTombstone>,
    total_bytes: usize,
    access_sequence: u64,
    stats: PreviousResponseCacheStats,
    parent: ResidentMemoryBudget,
}

impl Default for PreviousResponseCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl PreviousResponseCache {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            entries: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            total_bytes: 0,
            access_sequence: 0,
            stats: PreviousResponseCacheStats::default(),
            parent,
        }
    }

    pub fn get(
        &mut self,
        scope: &PreviousResponseCacheScope,
        response_id: &str,
        now_ms: i64,
    ) -> Option<Vec<Value>> {
        self.lookup(scope, response_id, now_ms).items
    }

    pub fn lookup(
        &mut self,
        scope: &PreviousResponseCacheScope,
        response_id: &str,
        now_ms: i64,
    ) -> PreviousResponseCacheLookup {
        self.lookup_with(scope, response_id, now_ms, <[Value]>::to_vec)
    }

    /// Borrow under the cache lock so callers can admit their working set
    /// before cloning selected items. The callback must not block or reenter.
    pub fn lookup_with<T>(
        &mut self,
        scope: &PreviousResponseCacheScope,
        response_id: &str,
        now_ms: i64,
        prepare: impl FnOnce(&[Value]) -> T,
    ) -> PreviousResponseCacheLookup<T> {
        self.cleanup(now_ms);
        let Some(key) = cache_key(scope, response_id) else {
            self.stats.invalid_response_id_rejections =
                self.stats.invalid_response_id_rejections.saturating_add(1);
            self.stats.misses = self.stats.misses.saturating_add(1);
            return PreviousResponseCacheLookup {
                items: None,
                unavailable_reason: None,
            };
        };
        self.access_sequence = self.access_sequence.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.access_sequence = self.access_sequence;
            self.stats.hits = self.stats.hits.saturating_add(1);
            return PreviousResponseCacheLookup {
                items: Some(prepare(&entry.items)),
                unavailable_reason: None,
            };
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        let unavailable_reason = self.tombstones.get_mut(&key).map(|tombstone| {
            tombstone.access_sequence = self.access_sequence;
            tombstone.reason
        });
        PreviousResponseCacheLookup {
            items: None,
            unavailable_reason,
        }
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
            self.stats.invalid_response_id_rejections =
                self.stats.invalid_response_id_rejections.saturating_add(1);
            return false;
        };
        if items.is_empty() {
            return false;
        }
        self.stats.max_observed_entry_items = self.stats.max_observed_entry_items.max(items.len());
        if items.len() > CACHE_MAX_ITEMS_PER_ENTRY {
            self.stats.too_many_items_rejections =
                self.stats.too_many_items_rejections.saturating_add(1);
            self.mark_tombstone(key, PreviousResponseUnavailableReason::TooManyItems, now_ms);
            return false;
        }
        let Some(bytes) = encoded_items_size(&items) else {
            return false;
        };
        self.stats.max_observed_entry_bytes = self.stats.max_observed_entry_bytes.max(bytes);
        if bytes > CACHE_MAX_ENTRY_BYTES || bytes > CACHE_MAX_BYTES {
            self.stats.oversize_entry_rejections =
                self.stats.oversize_entry_rejections.saturating_add(1);
            self.mark_tombstone(
                key,
                PreviousResponseUnavailableReason::EntryTooLarge,
                now_ms,
            );
            return false;
        }
        let retained_bytes = items.iter().fold(
            key.retained_bytes::<PreviousResponseCacheEntry>()
                .saturating_add(
                    items
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Value>()),
                ),
            |bytes, item| {
                bytes.saturating_add(
                    retained_json_bytes(item).saturating_sub(std::mem::size_of::<Value>()),
                )
            },
        );
        let Some(memory) = self.parent.reserve(retained_bytes) else {
            self.stats.process_capacity_rejections =
                self.stats.process_capacity_rejections.saturating_add(1);
            self.mark_tombstone(
                key,
                PreviousResponseUnavailableReason::MemoryCapacity,
                now_ms,
            );
            return false;
        };
        self.tombstones.remove(&key);
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        while self.entries.len() >= CACHE_MAX_ENTRIES
            || self.total_bytes.saturating_add(bytes) > CACHE_MAX_BYTES
        {
            let reason = if self.entries.len() >= CACHE_MAX_ENTRIES {
                PreviousResponseUnavailableReason::CountEvicted
            } else {
                PreviousResponseUnavailableReason::ByteEvicted
            };
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
                match reason {
                    PreviousResponseUnavailableReason::CountEvicted => {
                        self.stats.count_evictions = self.stats.count_evictions.saturating_add(1)
                    }
                    PreviousResponseUnavailableReason::ByteEvicted => {
                        self.stats.byte_evictions = self.stats.byte_evictions.saturating_add(1)
                    }
                    _ => {}
                }
                self.mark_tombstone(oldest, reason, now_ms);
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
                retained_bytes,
                _memory: memory,
            },
        );
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.refresh_current_stats();
        true
    }

    pub fn stats(&self) -> PreviousResponseCacheStats {
        self.stats.clone()
    }

    pub fn record_required_context_unavailable(&mut self) {
        self.stats.required_context_unavailable =
            self.stats.required_context_unavailable.saturating_add(1);
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
                self.stats.expired = self.stats.expired.saturating_add(1);
                self.mark_tombstone(key, PreviousResponseUnavailableReason::Expired, now_ms);
            }
        }
        self.tombstones
            .retain(|_, tombstone| tombstone.expires_at_ms > now_ms);
        self.refresh_current_stats();
    }

    fn mark_tombstone(
        &mut self,
        key: PreviousResponseCacheKey,
        reason: PreviousResponseUnavailableReason,
        now_ms: i64,
    ) {
        let retained_bytes = key.retained_bytes::<PreviousResponseTombstone>();
        let Some(memory) = self.parent.reserve(retained_bytes) else {
            // Diagnostics are optional. A missing required context remains a
            // strict miss even when no tombstone can be retained.
            self.refresh_current_stats();
            return;
        };
        self.access_sequence = self.access_sequence.wrapping_add(1);
        self.tombstones.insert(
            key,
            PreviousResponseTombstone {
                reason,
                expires_at_ms: now_ms.saturating_add(TOMBSTONE_TTL_MS),
                access_sequence: self.access_sequence,
                retained_bytes,
                _memory: memory,
            },
        );
        while self.tombstones.len() > TOMBSTONE_MAX_ENTRIES {
            let Some(oldest) = self
                .tombstones
                .iter()
                .min_by_key(|(_, tombstone)| tombstone.access_sequence)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.tombstones.remove(&oldest);
        }
        self.refresh_current_stats();
    }

    fn refresh_current_stats(&mut self) {
        self.stats.current_entries = self.entries.len();
        self.stats.current_bytes = self.total_bytes;
        self.stats.current_tombstones = self.tombstones.len();
        self.stats.current_retained_bytes = self.entries.values().fold(0_usize, |bytes, entry| {
            bytes.saturating_add(entry.retained_bytes)
        });
        self.stats.current_retained_bytes = self
            .tombstones
            .values()
            .fold(self.stats.current_retained_bytes, |bytes, entry| {
                bytes.saturating_add(entry.retained_bytes)
            });
        self.stats.high_water_retained_bytes = self
            .stats
            .high_water_retained_bytes
            .max(self.stats.current_retained_bytes);
        self.stats.high_water_entries = self.stats.high_water_entries.max(self.entries.len());
        self.stats.high_water_bytes = self.stats.high_water_bytes.max(self.total_bytes);
    }
}

impl PreviousResponseCacheKey {
    fn retained_bytes<T>(&self) -> usize {
        // Include value/key storage and an index allowance. This logical
        // estimate does not claim allocator or exact B-tree node accounting.
        [
            &self.scope.share_id,
            &self.scope.principal,
            &self.scope.provider_id,
            &self.scope.runtime_fingerprint,
            &self.scope.account_id,
            &self.scope.workspace_id,
            &self.response_id,
        ]
        .into_iter()
        .fold(
            std::mem::size_of::<(Self, T)>().saturating_add(std::mem::size_of::<usize>() * 3),
            |bytes, value| bytes.saturating_add(value.capacity()),
        )
    }
}

fn cache_key(
    scope: &PreviousResponseCacheScope,
    response_id: &str,
) -> Option<PreviousResponseCacheKey> {
    if response_id.is_empty()
        || response_id.len() > CACHE_MAX_RESPONSE_ID_LEN
        || response_id.trim() != response_id
    {
        return None;
    }
    Some(PreviousResponseCacheKey {
        scope: scope.clone(),
        response_id: response_id.to_string(),
    })
}

fn encoded_items_size(items: &[Value]) -> Option<usize> {
    struct CountingWriter(usize);
    impl std::io::Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("previous response size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    items.iter().try_fold(0_usize, |total, item| {
        let mut writer = CountingWriter(0);
        serde_json::to_writer(&mut writer, item).ok()?;
        total.checked_add(writer.0)
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
            provider_id: "provider-a".to_string(),
            provider_revision: 1,
            runtime_fingerprint: "runtime-a".to_string(),
            account_id: "account-a".to_string(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            credential_fingerprint: [1; 32],
            workspace_id: "workspace-a".to_string(),
        }
    }

    #[test]
    fn codex_r4_previous_response_charges_allocations_and_releases_expiry_and_tombstones() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = PreviousResponseCache::with_parent(parent.clone());
        let base = scope("share-r4", "user-r4");
        let mut text = String::with_capacity(65_536);
        text.push_str("short");
        assert!(cache.insert(base.clone(), "resp-r4", vec![Value::String(text)], 1_000));
        let retained = parent.snapshot().used_bytes;
        assert!(retained >= 65_536);
        assert!(cache.stats().current_bytes < retained);
        assert_eq!(cache.stats().current_retained_bytes, retained);
        assert!(cache.get(&base, "resp-r4", 1_000 + CACHE_TTL_MS).is_none());
        assert!(parent.snapshot().used_bytes < retained);
        assert!(
            parent.snapshot().used_bytes > 0,
            "expiry diagnostic has an owner"
        );
        cache.lookup(&base, "missing", 1_000 + CACHE_TTL_MS + TOMBSTONE_TTL_MS);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert!(cache.insert(
            base,
            "resp-next",
            vec![json!({"call_id":"next"})],
            2_000_000
        ));
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
    }

    #[test]
    fn codex_r4_previous_response_capacity_denial_preserves_strict_scoped_miss() {
        let parent = ResidentMemoryBudget::new(Some(0));
        let mut cache = PreviousResponseCache::with_parent(parent.clone());
        let base = scope("share-r4", "user-r4");
        assert!(!cache.insert(
            base.clone(),
            "resp-r4",
            vec![json!({"call_id":"private"})],
            1_000
        ));
        assert!(cache.get(&base, "resp-r4", 1_001).is_none());
        assert!(cache
            .get(&scope("share-decoy", "user-r4"), "resp-r4", 1_001)
            .is_none());
        assert_eq!(cache.stats().process_capacity_rejections, 1);
        assert_eq!(cache.stats().current_entries, 0);
        assert_eq!(cache.stats().current_tombstones, 0);
        assert_eq!(parent.snapshot().used_bytes, 0);
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
        let mut different_account = base.clone();
        different_account.account_id = "account-b".to_string();
        assert!(cache.get(&different_account, "resp-a", 1_001).is_none());
        let mut recreated_account = base.clone();
        recreated_account.account_incarnation += 1;
        assert!(cache.get(&recreated_account, "resp-a", 1_001).is_none());
        let mut changed_credential = base.clone();
        changed_credential.credential_fingerprint[0] ^= 1;
        assert!(cache.get(&changed_credential, "resp-a", 1_001).is_none());
        assert!(cache.get(&base, "resp-a", 1_000 + CACHE_TTL_MS).is_none());
    }

    #[test]
    fn response_ids_are_opaque_and_padding_cannot_reuse_tool_context() {
        let mut cache = PreviousResponseCache::default();
        let scope = scope("share-a", "user@example.com");
        let items = vec![json!({"type":"function_call","call_id":"call-a"})];
        assert!(cache.insert(scope.clone(), "resp-a", items.clone(), 1_000));
        for padded in [" resp-a", "resp-a ", "\tresp-a", "resp-a\n"] {
            assert!(cache.get(&scope, padded, 1_001).is_none());
            assert!(!cache.insert(scope.clone(), padded, items.clone(), 1_001));
        }
        assert_eq!(cache.get(&scope, "resp-a", 1_002), Some(items));
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
        assert_eq!(cache.stats().count_evictions, 1);
        assert_eq!(cache.stats().current_entries, CACHE_MAX_ENTRIES);
        assert_eq!(
            cache.lookup(&scope, "resp-1", 1_003).unavailable_reason,
            Some(PreviousResponseUnavailableReason::CountEvicted)
        );
    }

    #[test]
    fn tombstones_are_scoped_bounded_and_expire() {
        let mut cache = PreviousResponseCache::default();
        let base = scope("share-a", "user@example.com");
        assert!(!cache.insert(
            base.clone(),
            "resp-too-many",
            (0..=CACHE_MAX_ITEMS_PER_ENTRY)
                .map(|index| json!(index))
                .collect(),
            1_000,
        ));
        assert_eq!(
            cache
                .lookup(&base, "resp-too-many", 1_001)
                .unavailable_reason,
            Some(PreviousResponseUnavailableReason::TooManyItems)
        );
        assert!(cache
            .lookup(
                &scope("share-b", "user@example.com"),
                "resp-too-many",
                1_001,
            )
            .unavailable_reason
            .is_none());
        assert!(cache
            .lookup(&base, "resp-too-many", 1_000 + TOMBSTONE_TTL_MS)
            .unavailable_reason
            .is_none());
    }

    #[test]
    fn stats_cover_hits_misses_expiry_rejections_and_required_context() {
        let mut cache = PreviousResponseCache::default();
        let base = scope("share-a", "user@example.com");
        assert!(cache.insert(
            base.clone(),
            "resp-a",
            vec![json!({"type":"function_call","call_id":"call-a"})],
            1_000,
        ));
        assert!(cache.get(&base, "resp-a", 1_001).is_some());
        assert!(cache.get(&base, "missing", 1_001).is_none());
        assert!(cache.get(&base, "", 1_001).is_none());
        assert!(cache.get(&base, "resp-a", 1_000 + CACHE_TTL_MS).is_none());
        cache.record_required_context_unavailable();
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert!(stats.misses >= 3);
        assert_eq!(stats.invalid_response_id_rejections, 1);
        assert_eq!(stats.expired, 1);
        assert_eq!(stats.required_context_unavailable, 1);
        assert!(stats.high_water_entries >= 1);
        assert!(stats.max_observed_entry_bytes > 0);
    }
}
