//! Catalog, replay and singleflight owners share the existing process budget.
use super::*;
use crate::infra::resident_flights::node_bytes;
use crate::infra::resident_memory::{
    process_resident_memory, retained_json_bytes, ResidentMemoryBudget, ResidentMemoryReservation,
};

pub(super) fn models_bytes(models: &Vec<String>) -> usize {
    models
        .capacity()
        .saturating_mul(std::mem::size_of::<String>())
        .saturating_add(models.iter().map(String::capacity).sum::<usize>())
}

impl Default for KimiModelCatalogCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl KimiModelCatalogCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            catalogs: RwLock::new(BTreeMap::new()),
            flights: crate::infra::resident_flights::ResidentFlights::new(
                parent.clone(),
                MAX_MODEL_CATALOG_ENTRIES,
            ),
            parent,
        }
    }

    pub async fn fresh(
        &self,
        scope: &KimiModelCatalogScope,
        now_ms: i64,
    ) -> Option<KimiModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| {
            now_ms.saturating_sub(catalog.fetched_at_ms) < KIMI_STALE_MODEL_CATALOG_TTL_MS
        });
        catalogs
            .get(scope)
            .filter(|catalog| catalog.expires_at_ms > now_ms)
            .cloned()
    }

    pub async fn stale(
        &self,
        scope: &KimiModelCatalogScope,
        now_ms: i64,
    ) -> Option<KimiModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| {
            now_ms.saturating_sub(catalog.fetched_at_ms) < KIMI_STALE_MODEL_CATALOG_TTL_MS
        });
        catalogs.get(scope).cloned().map(|mut catalog| {
            catalog.source = "kimi_models_cache";
            catalog.stale = true;
            catalog
        })
    }

    pub async fn insert(
        &self,
        scope: KimiModelCatalogScope,
        models: Vec<String>,
        fetched_at_ms: i64,
    ) -> KimiModelCatalog {
        self.insert_fetched(
            scope,
            FetchedKimiModelCatalog {
                models,
                upstream_non_empty: true,
                _memory: None,
            },
            fetched_at_ms,
        )
        .await
    }

    pub(crate) async fn insert_fetched(
        &self,
        scope: KimiModelCatalogScope,
        fetched: FetchedKimiModelCatalog,
        fetched_at_ms: i64,
    ) -> KimiModelCatalog {
        let FetchedKimiModelCatalog {
            mut models,
            _memory: payload_memory,
            ..
        } = fetched;
        models.retain(|model| !model.trim().is_empty());
        models.sort();
        models.dedup();
        let mut catalogs = self.catalogs.write().await;
        // Authoritative replacement must revoke the old entry even if the new
        // owner cannot be admitted. External snapshots keep their own charge.
        catalogs.remove(&scope);
        catalogs.retain(|_, catalog| {
            fetched_at_ms.saturating_sub(catalog.fetched_at_ms) < KIMI_STALE_MODEL_CATALOG_TTL_MS
        });
        while catalogs.len() >= MAX_MODEL_CATALOG_ENTRIES {
            let oldest = catalogs
                .iter()
                .min_by_key(|(key, value)| (value.fetched_at_ms, *key))
                .map(|(key, _)| key.clone())
                .expect("nonempty catalog");
            catalogs.remove(&oldest);
        }
        let bytes = node_bytes::<KimiModelCatalogScope, KimiModelCatalog>(
            scope
                .0
                .capacity()
                .saturating_add(std::mem::size_of::<KimiModelCatalogData>())
                .saturating_add(std::mem::size_of::<usize>() * 2)
                .saturating_add(if payload_memory.is_none() {
                    models_bytes(&models)
                } else {
                    0
                }),
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            crate::metrics::record_resident_retention_refused("kimi_catalog");
            let mut catalog = unavailable_catalog();
            catalog.source = "kimi_models_capacity_rejected";
            return catalog;
        };
        let catalog = KimiModelCatalog {
            data: Arc::new(KimiModelCatalogData {
                models,
                _memory: Some(memory),
                _payload_memory: payload_memory,
            }),
            source: "coding_v1_models",
            fetched_at_ms,
            stale: false,
            expires_at_ms: fetched_at_ms.saturating_add(KIMI_MODEL_CATALOG_TTL_MS),
        };
        catalogs.insert(scope, catalog.clone());
        catalog
    }

    pub async fn invalidate(&self, scope: &KimiModelCatalogScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub(crate) async fn lock(
        &self,
        scope: &KimiModelCatalogScope,
    ) -> Option<crate::infra::resident_flights::ResidentFlightGuard> {
        self.flights.lock(scope, scope.0.capacity()).await
    }
}

impl KimiReplayWriteOutcome {
    pub(crate) fn metric(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Conflict => "write_conflict",
            Self::CapacityRejected => "capacity_rejected",
            Self::InvalidContent => "invalid_content",
        }
    }
}

/// A copied allocation avoids charging only a slice while pinning a much
/// larger caller-owned Bytes backing store. Clones and slices share this owner.
#[derive(Debug)]
pub(super) struct ReplayBytes {
    pub(super) content: Vec<u8>,
    pub(super) _memory: ResidentMemoryReservation,
}

impl AsRef<[u8]> for ReplayBytes {
    fn as_ref(&self) -> &[u8] {
        &self.content
    }
}

impl Default for KimiThinkingReplayCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl KimiThinkingReplayCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            store: Mutex::new(ThinkingReplayStore::default()),
            parent,
        }
    }

    pub async fn get(
        &self,
        scope: &KimiThinkingReplayScope,
        now_ms: i64,
    ) -> (Option<Bytes>, KimiThinkingReplaySnapshot) {
        let mut store = self.store.lock().await;
        let expired = prune_replay_store(&mut store, now_ms);
        if expired > 0 {
            crate::metrics::record_kimi_thinking_replay("expired", expired as u64);
        }
        let entry = store.entries.get(scope);
        (
            entry.map(|entry| entry.content.clone()),
            KimiThinkingReplaySnapshot {
                generation: entry.map(|entry| entry.generation),
            },
        )
    }

    pub async fn replace_if_unchanged(
        &self,
        scope: KimiThinkingReplayScope,
        snapshot: KimiThinkingReplaySnapshot,
        content: Bytes,
        now_ms: i64,
    ) -> KimiReplayWriteOutcome {
        if !valid_replay_content(&content) {
            return KimiReplayWriteOutcome::InvalidContent;
        }
        let mut store = self.store.lock().await;
        let expired = prune_replay_store(&mut store, now_ms);
        if expired > 0 {
            crate::metrics::record_kimi_thinking_replay("expired", expired as u64);
        }
        if store.entries.get(&scope).map(|entry| entry.generation) != snapshot.generation {
            return KimiReplayWriteOutcome::Conflict;
        }
        if let Some(previous) = store.entries.remove(&scope) {
            store.total_bytes = store.total_bytes.saturating_sub(previous.content.len());
        }
        while store.entries.len() >= MAX_REPLAY_ENTRIES
            || store.total_bytes.saturating_add(content.len()) > MAX_REPLAY_TOTAL_BYTES
        {
            let Some(oldest) = store
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.recorded_at_ms, *key))
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            let previous = store.entries.remove(&oldest).expect("selected entry");
            store.total_bytes = store.total_bytes.saturating_sub(previous.content.len());
        }
        let bytes = node_bytes::<KimiThinkingReplayScope, ThinkingReplayEntry>(
            content
                .len()
                .saturating_add(scope.0.capacity())
                .saturating_add(std::mem::size_of::<ReplayBytes>())
                .saturating_add(std::mem::size_of::<usize>() * 4),
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            crate::metrics::record_resident_retention_refused("kimi_replay");
            return KimiReplayWriteOutcome::CapacityRejected;
        };
        let content = Bytes::from_owner(ReplayBytes {
            content: content.to_vec(),
            _memory: memory,
        });
        store.next_generation = store.next_generation.saturating_add(1).max(1);
        let generation = store.next_generation;
        store.total_bytes = store.total_bytes.saturating_add(content.len());
        store.entries.insert(
            scope,
            ThinkingReplayEntry {
                content,
                recorded_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(KIMI_THINKING_REPLAY_TTL_MS),
                generation,
            },
        );
        KimiReplayWriteOutcome::Stored
    }

    pub async fn delete_if_unchanged(
        &self,
        scope: &KimiThinkingReplayScope,
        snapshot: KimiThinkingReplaySnapshot,
    ) -> bool {
        let mut store = self.store.lock().await;
        if store.entries.get(scope).map(|entry| entry.generation) != snapshot.generation {
            return false;
        }
        if let Some(previous) = store.entries.remove(scope) {
            store.total_bytes = store.total_bytes.saturating_sub(previous.content.len());
        }
        true
    }
}

#[derive(Debug)]
pub(super) struct ReplayStreamMemory {
    parent: ResidentMemoryBudget,
    charged: usize,
}

impl ReplayStreamMemory {
    pub(super) fn new(parent: ResidentMemoryBudget) -> Self {
        Self { parent, charged: 0 }
    }
    pub(super) fn resize(&mut self, next: usize) -> bool {
        if !self.parent.try_resize(self.charged, next) {
            return false;
        }
        self.charged = next;
        true
    }
    pub(super) fn grow(&mut self, next: usize) -> bool {
        self.resize(next.max(self.charged))
    }
    pub(super) fn output(&self, bytes: usize) -> Option<ResidentMemoryReservation> {
        self.parent.reserve(
            bytes
                .saturating_add(std::mem::size_of::<ReplayBytes>())
                .saturating_add(std::mem::size_of::<usize>() * 4),
        )
    }
}

impl Drop for ReplayStreamMemory {
    fn drop(&mut self) {
        self.parent.try_resize(self.charged, 0);
    }
}

impl Default for KimiThinkingReplayStreamAccumulator {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl KimiThinkingReplayStreamAccumulator {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            pending: Vec::new(),
            blocks: BTreeMap::new(),
            saw_message_start: false,
            complete: false,
            abandoned: false,
            bytes_used: 0,
            memory: ReplayStreamMemory::new(parent),
        }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.blocks.values().fold(
            std::mem::size_of::<Self>()
                .saturating_add(self.pending.capacity())
                .saturating_add(
                    self.blocks
                        .len()
                        .saturating_mul(node_bytes::<u64, KimiThinkingReplayStreamBlock>(0)),
                ),
            |bytes, block| {
                bytes
                    .saturating_add(retained_json_bytes(&block.value))
                    .saturating_add(block.partial_input.as_ref().map_or(0, String::capacity))
            },
        )
    }

    pub(super) fn abandon_capacity(&mut self) {
        self.abandon();
        crate::metrics::record_resident_retention_refused("kimi_replay_stream");
        crate::metrics::record_kimi_thinking_replay("capacity_rejected", 1);
    }
}
