//! Retained discovery payloads and single-flight registrations. Charges use
//! sparse B-tree estimates; they do not measure allocator overhead or RSS.
use super::*;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

#[derive(Debug)]
pub(super) struct ResidentCatalog {
    pub(super) value: FetchedCodeBuddyModelCatalog,
    pub(super) _memory: Option<ResidentMemoryReservation>,
}

pub(super) type CatalogFlights =
    BTreeMap<CodeBuddyRuntimeScope, Arc<Mutex<ResidentMemoryReservation>>>;

pub(super) fn node_bytes<K, V>(dynamic: usize) -> usize {
    std::mem::size_of::<(K, V)>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(dynamic)
}

impl Default for CodeBuddyRuntimeCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl CodeBuddyRuntimeCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            catalogs: RwLock::new(BTreeMap::new()),
            catalog_flights: Mutex::new(BTreeMap::new()),
            parent,
        }
    }

    pub async fn catalog(
        &self,
        scope: &CodeBuddyRuntimeScope,
        now_ms: i64,
    ) -> Option<CodeBuddyModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| catalog.is_usable_stale(now_ms));
        catalogs
            .get(scope)
            .filter(|catalog| catalog.is_fresh(now_ms))
            .cloned()
    }

    pub async fn stale_catalog(
        &self,
        scope: &CodeBuddyRuntimeScope,
        now_ms: i64,
    ) -> Option<CodeBuddyModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| catalog.is_usable_stale(now_ms));
        catalogs.get(scope).map(CodeBuddyModelCatalog::as_stale)
    }

    pub async fn insert_catalog(
        &self,
        scope: CodeBuddyRuntimeScope,
        fetched: FetchedCodeBuddyModelCatalog,
        now_ms: i64,
    ) -> CodeBuddyModelCatalog {
        let mut catalog = CodeBuddyModelCatalog {
            data: Arc::new(ResidentCatalog {
                value: fetched,
                _memory: None,
            }),
            fetched_at_ms: now_ms,
            stale: false,
            expires_at_ms: now_ms.saturating_add(CODEBUDDY_MODEL_CATALOG_TTL_MS),
        };
        let mut catalogs = self.catalogs.write().await;
        // An authoritative replacement, including empty, must never revive
        // an older model authorization when optional caching is refused.
        catalogs.remove(&scope);
        catalogs.retain(|_, value| value.is_usable_stale(now_ms));
        let dynamic = catalog
            .retained_bytes()
            .saturating_add(scope.0.capacity())
            .saturating_add(std::mem::size_of::<ResidentCatalog>())
            .saturating_add(std::mem::size_of::<usize>() * 6)
            .saturating_add(
                catalog
                    .raw_configs
                    .len()
                    .saturating_mul(node_bytes::<String, Value>(0)),
            )
            .saturating_add(
                catalog
                    .capabilities
                    .len()
                    .saturating_mul(node_bytes::<String, CodeBuddyModelCapability>(0)),
            );
        if let Some(memory) = self
            .parent
            .reserve(node_bytes::<CodeBuddyRuntimeScope, CodeBuddyModelCatalog>(
                dynamic,
            ))
        {
            Arc::get_mut(&mut catalog.data)
                .expect("unpublished catalog has one owner")
                ._memory = Some(memory);
            catalogs.insert(scope, catalog.clone());
            while catalogs.len() > MAX_RUNTIME_ENTRIES {
                let oldest = catalogs
                    .iter()
                    .min_by_key(|(key, catalog)| (catalog.fetched_at_ms, *key))
                    .map(|(key, _)| key.clone())
                    .expect("nonempty catalog cache");
                catalogs.remove(&oldest);
            }
        }
        // The current result remains usable without a retained cache entry.
        // Request projections continue through the existing request budget.
        catalog
    }

    pub async fn invalidate_scope(&self, scope: &CodeBuddyRuntimeScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub(crate) async fn catalog_lock(
        &self,
        scope: &CodeBuddyRuntimeScope,
    ) -> Option<OwnedMutexGuard<ResidentMemoryReservation>> {
        let flight = {
            let mut flights: tokio::sync::MutexGuard<'_, CatalogFlights> =
                self.catalog_flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            if let Some(flight) = flights.get(scope) {
                flight.clone()
            } else {
                if flights.len() >= MAX_RUNTIME_ENTRIES {
                    return None;
                }
                let bytes =
                    node_bytes::<CodeBuddyRuntimeScope, Arc<Mutex<ResidentMemoryReservation>>>(
                        scope
                            .0
                            .capacity()
                            .saturating_add(std::mem::size_of::<Mutex<ResidentMemoryReservation>>())
                            .saturating_add(std::mem::size_of::<usize>() * 2),
                    );
                let memory = self.parent.reserve(bytes)?;
                let flight = Arc::new(Mutex::new(memory));
                flights.insert(scope.clone(), flight.clone());
                flight
            }
        };
        Some(flight.lock_owned().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(index: usize) -> CodeBuddyRuntimeScope {
        CodeBuddyRuntimeScope(format!("fixture-scope-{index}"))
    }

    fn fetched(model: &str) -> FetchedCodeBuddyModelCatalog {
        FetchedCodeBuddyModelCatalog {
            enabled_models: vec![model.to_string()],
            raw_configs: BTreeMap::from([(model.into(), json!({"id":model}))]),
            capabilities: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn codebuddy_r4_catalog_owner_follows_last_shared_snapshot() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CodeBuddyRuntimeCache::with_parent(parent.clone());
        let first = cache.insert_catalog(scope(0), fetched("old"), 1).await;
        let second = cache.catalog(&scope(0), 2).await.unwrap();
        assert!(Arc::ptr_eq(&first.data, &second.data));
        let used = parent.snapshot().used_bytes;
        assert!(used > 0);
        cache.invalidate_scope(&scope(0)).await;
        drop(first);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(second);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codebuddy_r4_rejected_catalog_replacement_returns_current_without_old_authorization() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CodeBuddyRuntimeCache::with_parent(parent.clone());
        drop(cache.insert_catalog(scope(0), fetched("old"), 1).await);
        parent.configure_limit(Some(0));
        let current = cache.insert_catalog(scope(0), fetched("new"), 2).await;
        assert_eq!(current.enabled_models, ["new"]);
        assert!(current.data._memory.is_none());
        assert!(cache.catalog(&scope(0), 3).await.is_none());
        assert!(cache.stale_catalog(&scope(0), 3).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codebuddy_r4_authoritative_empty_under_pressure_cannot_revive_previous_models() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CodeBuddyRuntimeCache::with_parent(parent.clone());
        drop(cache.insert_catalog(scope(0), fetched("old"), 1).await);
        parent.configure_limit(Some(0));
        let empty = cache
            .insert_catalog(
                scope(0),
                FetchedCodeBuddyModelCatalog {
                    enabled_models: Vec::new(),
                    raw_configs: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                },
                2,
            )
            .await;
        assert!(empty.enabled_models.is_empty());
        assert!(cache.stale_catalog(&scope(0), 3).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codebuddy_r4_catalog_count_and_lazy_expiry_release_index_owners() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CodeBuddyRuntimeCache::with_parent(parent.clone());
        for index in 0..=MAX_RUNTIME_ENTRIES {
            drop(
                cache
                    .insert_catalog(scope(index), fetched("model"), index as i64)
                    .await,
            );
        }
        assert_eq!(cache.catalogs.read().await.len(), MAX_RUNTIME_ENTRIES);
        assert!(cache.catalog(&scope(0), 200).await.is_none());
        assert!(cache
            .catalog(
                &scope(MAX_RUNTIME_ENTRIES),
                CODEBUDDY_MODEL_CATALOG_TTL_MS + 200
            )
            .await
            .is_none());
        let stale = cache
            .stale_catalog(
                &scope(MAX_RUNTIME_ENTRIES),
                CODEBUDDY_MODEL_CATALOG_TTL_MS + 200,
            )
            .await
            .unwrap();
        assert!(stale.stale);
        drop(stale);
        assert!(cache
            .stale_catalog(&scope(MAX_RUNTIME_ENTRIES), MAX_STALE_CATALOG_AGE_MS + 200)
            .await
            .is_none());
        assert!(cache.catalogs.read().await.is_empty());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codebuddy_r4_catalog_flights_share_waiters_and_keep_last_guard_owner() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = Arc::new(CodeBuddyRuntimeCache::with_parent(parent.clone()));
        let first = cache.catalog_lock(&scope(0)).await.unwrap();
        let used = parent.snapshot().used_bytes;
        parent.configure_limit(Some(used));
        assert!(cache.catalog_lock(&scope(1)).await.is_none());
        let waiting_cache = cache.clone();
        let waiting = tokio::spawn(async move { waiting_cache.catalog_lock(&scope(0)).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(first);
        let last = waiting.await.unwrap().unwrap();
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(last);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn codebuddy_r4_active_catalog_flights_are_count_bounded_and_cancelled_waiters_release() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = Arc::new(CodeBuddyRuntimeCache::with_parent(parent.clone()));
        let mut guards = Vec::new();
        for index in 0..MAX_RUNTIME_ENTRIES {
            guards.push(cache.catalog_lock(&scope(index)).await.unwrap());
        }
        assert!(cache
            .catalog_lock(&scope(MAX_RUNTIME_ENTRIES))
            .await
            .is_none());
        let waiting_cache = cache.clone();
        let waiting = tokio::spawn(async move { waiting_cache.catalog_lock(&scope(0)).await });
        tokio::task::yield_now().await;
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        drop(guards);
        let current = cache
            .catalog_lock(&scope(MAX_RUNTIME_ENTRIES))
            .await
            .unwrap();
        assert_eq!(cache.catalog_flights.lock().await.len(), 1);
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(current);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
