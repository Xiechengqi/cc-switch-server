//! Retained discovery payloads and single-flight registrations. Charges use
//! sparse B-tree estimates; they do not measure allocator overhead or RSS.
use super::*;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, Weak};

#[derive(Debug)]
pub(super) struct ResidentCatalog {
    pub(super) value: FetchedCodeBuddyModelCatalog,
    pub(super) _memory: Option<ResidentMemoryReservation>,
}

#[derive(Debug)]
pub(super) struct CatalogFlight {
    scope: CodeBuddyRuntimeScope,
    lock: Arc<Mutex<ResidentMemoryReservation>>,
    generation: AtomicU64,
    // Weak comparison preserves identical-refresh leases without retaining a
    // second catalog body. Its possible Arc allocation is in the small owner.
    latest: StdMutex<Weak<ResidentCatalog>>,
}

#[derive(Debug, Clone)]
pub(super) struct CatalogEpoch {
    flight: Arc<CatalogFlight>,
    generation: u64,
}

impl CatalogEpoch {
    pub(super) fn is_current(&self, scope: &CodeBuddyRuntimeScope) -> bool {
        &self.flight.scope == scope
            && self.flight.generation.load(Ordering::Acquire) == self.generation
    }
}

pub(super) type CatalogFlights = BTreeMap<CodeBuddyRuntimeScope, Arc<CatalogFlight>>;

fn retain_active_flights(flights: &mut CatalogFlights) {
    flights
        .retain(|_, flight| Arc::strong_count(flight) > 1 || Arc::strong_count(&flight.lock) > 1);
}

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
        let catalog = catalogs
            .get(scope)
            .filter(|catalog| catalog.is_fresh(now_ms))
            .cloned();
        drop(catalogs);
        retain_active_flights(&mut *self.catalog_flights.lock().await);
        catalog
    }

    pub async fn stale_catalog(
        &self,
        scope: &CodeBuddyRuntimeScope,
        now_ms: i64,
    ) -> Option<CodeBuddyModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| catalog.is_usable_stale(now_ms));
        let catalog = catalogs.get(scope).map(CodeBuddyModelCatalog::as_stale);
        drop(catalogs);
        retain_active_flights(&mut *self.catalog_flights.lock().await);
        catalog
    }

    pub async fn insert_catalog(
        &self,
        scope: CodeBuddyRuntimeScope,
        fetched: FetchedCodeBuddyModelCatalog,
        now_ms: i64,
    ) -> CodeBuddyModelCatalog {
        let flight = self.catalog_flight(&scope).await;
        let mut catalogs = self.catalogs.write().await;
        let mut catalog = CodeBuddyModelCatalog {
            data: Arc::new(ResidentCatalog {
                value: fetched,
                _memory: None,
            }),
            fetched_at_ms: now_ms,
            stale: false,
            expires_at_ms: now_ms.saturating_add(CODEBUDDY_MODEL_CATALOG_TTL_MS),
            epoch: None,
        };
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
        let memory =
            self.parent
                .reserve(node_bytes::<CodeBuddyRuntimeScope, CodeBuddyModelCatalog>(
                    dynamic,
                ));
        let cache_admitted = memory.is_some();
        // Install the payload owner before publishing a Weak pointer: get_mut
        // requires no weak references as well as one strong owner.
        Arc::get_mut(&mut catalog.data)
            .expect("unpublished catalog has one owner")
            ._memory = memory;
        catalog.epoch = flight.map(|flight| {
            let mut latest = flight
                .latest
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let unchanged = latest
                .upgrade()
                .is_some_and(|previous| previous.value == catalog.data.value);
            let generation = if unchanged {
                flight.generation.load(Ordering::Acquire)
            } else {
                flight
                    .generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1)
            };
            *latest = Arc::downgrade(&catalog.data);
            drop(latest);
            CatalogEpoch { flight, generation }
        });
        if cache_admitted {
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
        let flight = self.catalog_flights.lock().await.get(scope).cloned();
        let mut catalogs = self.catalogs.write().await;
        if let Some(flight) = flight {
            flight.generation.fetch_add(1, Ordering::AcqRel);
            *flight
                .latest
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Weak::new();
        }
        catalogs.remove(scope);
        drop(catalogs);
        retain_active_flights(&mut *self.catalog_flights.lock().await);
    }

    /// A failed explicit management refresh must not turn its old snapshot
    /// back into a fresh result on the next read. Retain only its stale backup.
    pub(crate) async fn mark_catalog_stale(&self, scope: &CodeBuddyRuntimeScope) {
        if let Some(catalog) = self.catalogs.write().await.get_mut(scope) {
            catalog.expires_at_ms = i64::MIN;
            catalog.stale = true;
        }
    }

    pub(crate) async fn catalog_lock(
        &self,
        scope: &CodeBuddyRuntimeScope,
    ) -> Option<OwnedMutexGuard<ResidentMemoryReservation>> {
        let flight = self.catalog_flight(scope).await?;
        Some(flight.lock.clone().lock_owned().await)
    }

    async fn catalog_flight(&self, scope: &CodeBuddyRuntimeScope) -> Option<Arc<CatalogFlight>> {
        let mut flights = self.catalog_flights.lock().await;
        retain_active_flights(&mut flights);
        if let Some(flight) = flights.get(scope) {
            return Some(flight.clone());
        }
        if flights.len() >= MAX_RUNTIME_ENTRIES {
            // Optional cache-only leases must not crowd out a new scope. An
            // external snapshot or fetch guard remains pinned and never loses
            // its revocation channel merely because an index was evicted.
            let mut catalogs = self.catalogs.write().await;
            let oldest = catalogs
                .iter()
                .filter(|(_, catalog)| {
                    catalog.epoch.as_ref().is_some_and(|epoch| {
                        Arc::strong_count(&epoch.flight) == 2
                            && Arc::strong_count(&epoch.flight.lock) == 1
                    })
                })
                .min_by_key(|(key, catalog)| (catalog.fetched_at_ms, *key))
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                catalogs.remove(&oldest);
            }
            drop(catalogs);
            retain_active_flights(&mut flights);
        }
        if flights.len() >= MAX_RUNTIME_ENTRIES {
            return None;
        }
        let bytes = node_bytes::<CodeBuddyRuntimeScope, Arc<CatalogFlight>>(
            scope.0.capacity().saturating_mul(2).saturating_add(
                std::mem::size_of::<Mutex<ResidentMemoryReservation>>()
                    + std::mem::size_of::<CatalogFlight>()
                    + std::mem::size_of::<ResidentCatalog>()
                    + std::mem::size_of::<usize>() * 6,
            ),
        );
        let memory = self.parent.reserve(bytes)?;
        let flight = Arc::new(CatalogFlight {
            scope: scope.clone(),
            lock: Arc::new(Mutex::new(memory)),
            generation: AtomicU64::new(0),
            latest: StdMutex::new(Weak::new()),
        });
        flights.insert(scope.clone(), flight.clone());
        Some(flight)
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
            preferred_models: vec![model.to_string()],
            exclusions: BTreeMap::new(),
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
        assert!(current.epoch_is_current(&scope(0)));
        drop(current);
        assert!(cache.catalog(&scope(0), 4).await.is_none());
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
                    preferred_models: Vec::new(),
                    exclusions: BTreeMap::new(),
                    raw_configs: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                },
                2,
            )
            .await;
        assert!(empty.enabled_models.is_empty());
        assert!(cache.stale_catalog(&scope(0), 3).await.is_none());
        assert!(empty.epoch_is_current(&scope(0)));
        drop(empty);
        assert!(cache.catalog(&scope(0), 4).await.is_none());
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

    #[tokio::test]
    async fn buddy_r6_catalog_lease_survives_optional_cache_refusal_and_rejects_aba() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CodeBuddyRuntimeCache::with_parent(parent.clone());
        let old = cache.insert_catalog(scope(0), fetched("old"), 1).await;
        assert!(old.admits_generation(&scope(0), 2));
        parent.configure_limit(Some(parent.snapshot().used_bytes));
        let new = cache.insert_catalog(scope(0), fetched("new"), 3).await;
        assert!(new.data._memory.is_none());
        assert!(new.admits_generation(&scope(0), 4));
        assert!(!new.admits_generation(&scope(1), 4));
        assert!(!old.epoch_is_current(&scope(0)));
        assert!(cache.catalog(&scope(0), 4).await.is_none());
        cache.invalidate_scope(&scope(0)).await;
        assert!(!new.epoch_is_current(&scope(0)));
        parent.configure_limit(None);
        let restored = cache.insert_catalog(scope(0), fetched("old"), 5).await;
        assert!(restored.admits_generation(&scope(0), 6));
        assert!(
            !old.epoch_is_current(&scope(0)),
            "restoring a previous body must not revive its old lease"
        );
        drop((old, new, restored, cache));
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn buddy_r6_equal_refresh_preserves_epoch_but_not_the_old_admission_ttl() {
        let cache = CodeBuddyRuntimeCache::with_parent(ResidentMemoryBudget::new(None));
        let old = cache.insert_catalog(scope(0), fetched("same"), 1).await;
        let new = cache.insert_catalog(scope(0), fetched("same"), 10).await;
        assert!(old.epoch_is_current(&scope(0)));
        assert!(new.epoch_is_current(&scope(0)));
        assert!(!old.admits_generation(&scope(0), CODEBUDDY_MODEL_CATALOG_TTL_MS + 2));
        assert!(new.admits_generation(&scope(0), CODEBUDDY_MODEL_CATALOG_TTL_MS + 2));
        assert!(
            old.epoch_is_current(&scope(0)),
            "TTL alone cannot retroactively revoke an admitted response"
        );
        cache.mark_catalog_stale(&scope(0)).await;
        assert!(
            old.admits_generation(&scope(0), 3),
            "failed management refresh does not invent new entitlement facts"
        );
        let stale = cache.stale_catalog(&scope(0), 11).await.unwrap();
        assert!(stale.admits_generation(&scope(0), 12));
        assert!(!stale.admits_generation(&scope(0), MAX_STALE_CATALOG_AGE_MS + 11));
    }

    #[tokio::test]
    async fn buddy_r6_optional_eviction_preserves_the_later_revocation_channel() {
        let cache = CodeBuddyRuntimeCache::with_parent(ResidentMemoryBudget::new(None));
        let prepared = cache.insert_catalog(scope(0), fetched("old"), 1).await;
        // The production optional index evicts a value without authoritatively
        // invalidating its scope. Exercise that exact removal separately.
        cache.catalogs.write().await.remove(&scope(0));
        assert!(prepared.admits_generation(&scope(0), 2));
        let changed = cache.insert_catalog(scope(0), fetched("changed"), 3).await;
        assert!(!prepared.epoch_is_current(&scope(0)));
        assert!(changed.epoch_is_current(&scope(0)));
        assert!(cache
            .insert_catalog(scope(1), fetched("other"), 4)
            .await
            .epoch_is_current(&scope(1)));
        assert!(
            changed.epoch_is_current(&scope(0)),
            "another scope cannot revoke this catalog"
        );
    }
}
