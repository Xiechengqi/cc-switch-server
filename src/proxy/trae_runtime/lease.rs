//! Revocation lives independently of optional catalog payload retention.
//! The index holds weak payload references; live catalogs own the epoch charge.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;

const MAX_EPOCH_SCOPES: usize = 256;

#[derive(Debug)]
pub(super) struct CatalogEpoch {
    current: AtomicBool,
    payload: Weak<TraeModelCatalogData>,
    _memory: ResidentMemoryReservation,
}

impl CatalogEpoch {
    pub(super) fn is_current(&self) -> bool {
        self.current.load(Ordering::Acquire)
    }

    pub(super) fn revoke(&self) {
        self.current.store(false, Ordering::Release);
    }
}

pub(super) fn prune(epochs: &mut BTreeMap<TraeRuntimeScope, Arc<CatalogEpoch>>) {
    epochs.retain(|_, epoch| Arc::strong_count(epoch) > 1);
}

fn same_data(data: &TraeModelCatalogData, fetched: &FetchedTraeModelCatalog) -> bool {
    data.enabled_models == fetched.enabled_models
        && data.raw_configs == fetched.raw_configs
        && data.capabilities == fetched.capabilities
}

// Caller owns the catalog write lock. No await, network or state callback is
// allowed while the short epoch-index mutex is held.
pub(super) fn publish(
    cache: &TraeRuntimeCache,
    catalogs: &mut BTreeMap<TraeRuntimeScope, TraeModelCatalog>,
    scope: TraeRuntimeScope,
    fetched: FetchedTraeModelCatalog,
    now: i64,
) -> Result<TraeModelCatalog, TraeCatalogCapacity> {
    catalogs.retain(|_, catalog| catalog.is_usable_stale(now));
    let mut epochs = cache.epochs.lock().unwrap_or_else(|e| e.into_inner());
    prune(&mut epochs);
    let equal = epochs
        .get(&scope)
        .filter(|epoch| epoch.is_current())
        .and_then(|epoch| {
            epoch
                .payload
                .upgrade()
                .filter(|data| same_data(data, &fetched))
                .map(|data| (data, epoch.clone()))
        });
    // A failed replacement must not resurrect the previous authorization.
    catalogs.remove(&scope);
    let (data, epoch) = if let Some(equal) = equal {
        equal
    } else {
        if let Some(old) = epochs.remove(&scope) {
            old.revoke();
        }
        prune(&mut epochs);
        while epochs.len() >= MAX_EPOCH_SCOPES {
            // Cache-only owners can yield space; a prepared owner must retain
            // its revocation channel even when its payload index is evicted.
            let victim = catalogs
                .iter()
                .filter(|(_, catalog)| {
                    catalog
                        .epoch
                        .as_ref()
                        .is_some_and(|epoch| Arc::strong_count(epoch) == 2)
                })
                .min_by_key(|(scope, catalog)| (catalog.fetched_at_ms, *scope))
                .map(|(scope, _)| scope.clone());
            let Some(victim) = victim else {
                return Err(TraeCatalogCapacity);
            };
            catalogs.remove(&victim);
            prune(&mut epochs);
        }
        let epoch_bytes = node_bytes::<TraeRuntimeScope, Arc<CatalogEpoch>>(
            scope
                .0
                .capacity()
                .saturating_add(std::mem::size_of::<CatalogEpoch>())
                .saturating_add(32),
        );
        let epoch_memory = cache
            .parent
            .reserve(epoch_bytes)
            .ok_or(TraeCatalogCapacity)?;
        let bytes = catalog_bytes(&scope, &fetched);
        let memory = cache.parent.reserve(bytes).ok_or(TraeCatalogCapacity)?;
        let data = Arc::new(TraeModelCatalogData {
            enabled_models: fetched.enabled_models,
            raw_configs: fetched.raw_configs,
            capabilities: fetched.capabilities,
            retained_bytes: bytes,
            _memory: Some(memory),
        });
        let epoch = Arc::new(CatalogEpoch {
            current: AtomicBool::new(true),
            payload: Arc::downgrade(&data),
            _memory: epoch_memory,
        });
        epochs.insert(scope.clone(), epoch.clone());
        (data, epoch)
    };
    let bytes = data.retained_bytes;
    let catalog = TraeModelCatalog {
        data,
        epoch: Some(epoch),
        fetched_at_ms: now,
        stale: false,
        expires_at_ms: now.saturating_add(TRAE_MODEL_CATALOG_TTL_MS),
    };
    if bytes > MAX_RUNTIME_BYTES {
        crate::metrics::record_resident_retention_refused("trae_catalog");
        return Ok(catalog);
    }
    while catalogs.len() >= MAX_RUNTIME_ENTRIES
        || catalogs
            .values()
            .fold(0_usize, |n, c| n.saturating_add(c.retained_bytes))
            .saturating_add(bytes)
            > MAX_RUNTIME_BYTES
    {
        let Some(oldest) = catalogs
            .iter()
            .min_by_key(|(scope, c)| (c.fetched_at_ms, *scope))
            .map(|(scope, _)| scope.clone())
        else {
            break;
        };
        catalogs.remove(&oldest);
    }
    catalogs.insert(scope, catalog.clone());
    prune(&mut epochs);
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> TraeRuntimeScope {
        TraeRuntimeScope(format!("lease-fixture-{n}"))
    }

    fn fetched() -> FetchedTraeModelCatalog {
        let catalog = parse_trae_model_catalog(&serde_json::json!({
            "config_info_list":[{"config_name":"glm-5.2","display_config":{"display_name":"Lease fixture"}}]
        }))
        .unwrap();
        assert!(catalog.capabilities.contains_key("glm-5.2"));
        catalog
    }

    #[tokio::test]
    async fn r7_trae_catalog_epoch_survives_payload_index_eviction() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = TraeRuntimeCache::with_parent(parent.clone());
        let held = cache.insert_catalog(key(0), fetched(), 1000).await.unwrap();
        for n in 1..=MAX_RUNTIME_ENTRIES {
            cache
                .insert_catalog(key(n), fetched(), 1000 + n as i64)
                .await
                .unwrap();
        }
        assert!(cache.catalog(&key(0), 2000).await.is_none());
        assert!(
            held.admits_generation(2000),
            "cache pressure is not revocation"
        );
        let mut changed = fetched();
        changed
            .capabilities
            .get_mut("glm-5.2")
            .unwrap()
            .max_output_tokens = Some(17);
        let current = cache.insert_catalog(key(0), changed, 2000).await.unwrap();
        assert!(!held.admits_generation(2001));
        assert!(current.admits_generation(2001));
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(held);
        drop(current);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_trae_equal_rediscovery_preserves_each_prepared_snapshot_ttl() {
        let cache = TraeRuntimeCache::with_parent(ResidentMemoryBudget::new(None));
        let first = cache.insert_catalog(key(0), fetched(), 1000).await.unwrap();
        let next_time = 1001 + TRAE_MODEL_CATALOG_TTL_MS;
        let next = cache
            .insert_catalog(key(0), fetched(), next_time)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            first.epoch.as_ref().unwrap(),
            next.epoch.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(&first.data, &next.data));
        assert!(!first.admits_generation(next_time));
        assert!(next.admits_generation(next_time));
        let stale = first.as_stale();
        assert!(stale.admits_generation(next_time));
        assert!(!stale.admits_generation(1001 + MAX_STALE_CATALOG_AGE_MS));
        cache.invalidate_scope(&key(0)).await;
        assert!(!next.admits_generation(next_time));
        assert!(!stale.admits_generation(next_time));
    }

    #[tokio::test]
    async fn r7_trae_uncached_payload_retains_its_revocation_channel() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = TraeRuntimeCache::with_parent(parent.clone());
        let mut large = fetched();
        large
            .raw_configs
            .insert("large".into(), Value::String("x".repeat(MAX_RUNTIME_BYTES)));
        let held = cache.insert_catalog(key(0), large, 1000).await.unwrap();
        assert!(cache.catalog(&key(0), 1001).await.is_none());
        assert!(held.admits_generation(1001));
        let replacement = cache.insert_catalog(key(0), fetched(), 1001).await.unwrap();
        assert!(!held.admits_generation(1001));
        drop(cache);
        drop(replacement);
        assert!(parent.snapshot().used_bytes > MAX_RUNTIME_BYTES);
        drop(held);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_trae_epoch_scope_capacity_is_bounded_and_owners_release() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = TraeRuntimeCache::with_parent(parent.clone());
        let mut held = Vec::new();
        for n in 0..MAX_EPOCH_SCOPES {
            held.push(
                cache
                    .insert_catalog(key(n), fetched(), 1000 + n as i64)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(cache.epochs.lock().unwrap().len(), MAX_EPOCH_SCOPES);
        assert!(cache
            .insert_catalog(key(MAX_EPOCH_SCOPES), fetched(), 2000)
            .await
            .is_err());
        drop(held.remove(0));
        let next = cache
            .insert_catalog(key(MAX_EPOCH_SCOPES), fetched(), 2000)
            .await
            .unwrap();
        assert!(next.admits_generation(2000));
        assert_eq!(cache.epochs.lock().unwrap().len(), MAX_EPOCH_SCOPES);
        drop(held);
        drop(next);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_trae_equal_catalog_reuses_charged_data_and_failed_change_revokes() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = TraeRuntimeCache::with_parent(parent.clone());
        let held = cache.insert_catalog(key(0), fetched(), 1000).await.unwrap();
        parent.configure_limit(Some(parent.snapshot().used_bytes));
        let equal = cache.insert_catalog(key(0), fetched(), 1001).await.unwrap();
        assert!(Arc::ptr_eq(&held.data, &equal.data));
        let mut changed = fetched();
        changed
            .capabilities
            .get_mut("glm-5.2")
            .unwrap()
            .max_output_tokens = Some(17);
        assert!(cache.insert_catalog(key(0), changed, 1002).await.is_err());
        assert!(!held.admits_generation(1002));
        assert!(!equal.admits_generation(1002));
        assert!(cache.stale_catalog(&key(0), 1003).await.is_none());
        drop(cache);
        drop(held);
        drop(equal);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_trae_cancelled_uncached_owner_releases_payload_and_lazy_index() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = Arc::new(TraeRuntimeCache::with_parent(parent.clone()));
        let ready = Arc::new(tokio::sync::Notify::new());
        let (task_cache, task_ready) = (cache.clone(), ready.clone());
        let task = tokio::spawn(async move {
            let mut large = fetched();
            large
                .raw_configs
                .insert("large".into(), Value::String("x".repeat(MAX_RUNTIME_BYTES)));
            let _held = task_cache
                .insert_catalog(key(0), large, 1000)
                .await
                .unwrap();
            task_ready.notify_one();
            std::future::pending::<()>().await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), ready.notified())
            .await
            .unwrap();
        assert!(parent.snapshot().used_bytes > MAX_RUNTIME_BYTES);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(cache.catalog(&key(0), 1001).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
