//! Optional retained discovery state, separate from control-plane result
//! projections. Catalog snapshots share payload and one resident charge.
use super::*;
use crate::infra::resident_memory::process_resident_memory;

pub(super) const MAX_CACHED_CATALOGS: usize = 128;

impl Default for RuntimeCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl RuntimeCache {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            ide_version: None,
            models: BTreeMap::new(),
            parent,
        }
    }

    pub(super) fn models_snapshot(
        &mut self,
        key: &ModelCacheKey,
        now: i64,
        ttl: i64,
    ) -> Option<CachedModels> {
        self.models
            .retain(|_, value| now.saturating_sub(value.fetched_at_ms) < STALE_MODEL_CACHE_TTL_MS);
        self.models
            .get(key)
            .filter(|value| now.saturating_sub(value.fetched_at_ms) < ttl)
            .cloned()
    }

    pub(super) fn publish_models(
        &mut self,
        key: ModelCacheKey,
        descriptors: &[KiroModelDescriptor],
        now: i64,
    ) -> bool {
        self.models.retain(|existing, value| {
            existing != &key && now.saturating_sub(value.fetched_at_ms) < STALE_MODEL_CACHE_TTL_MS
        });
        let Some(memory) = self.parent.reserve(catalog_bytes(&key, descriptors)) else {
            return false;
        };
        let snapshot = CachedModels {
            descriptors: Arc::new(descriptors.to_vec()),
            fetched_at_ms: now,
            _memory: memory,
        };
        self.models.insert(key, snapshot);
        while self.models.len() > MAX_CACHED_CATALOGS {
            let oldest = self
                .models
                .iter()
                .min_by_key(|(key, value)| (value.fetched_at_ms, *key))
                .map(|(key, _)| key.clone())
                .expect("nonempty catalog cache");
            self.models.remove(&oldest);
        }
        true
    }

    pub(super) fn publish_ide(&mut self, version: &str, now: i64) -> bool {
        self.ide_version = None;
        let Some(memory) = self.parent.reserve(
            std::mem::size_of::<(String, i64, ResidentMemoryReservation)>()
                + version.len()
                + std::mem::size_of::<usize>() * 4,
        ) else {
            return false;
        };
        self.ide_version = Some((version.to_string(), now, memory));
        true
    }
}

fn catalog_bytes(key: &ModelCacheKey, descriptors: &[KiroModelDescriptor]) -> usize {
    let mut bytes = std::mem::size_of::<(ModelCacheKey, CachedModels)>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
        .saturating_mul(12);
    for value in [
        &key.app,
        &key.provider_id,
        &key.runtime_fingerprint,
        &key.account_id,
        &key.profile_scope,
        &key.runtime_region,
    ] {
        bytes = bytes.saturating_add(value.capacity());
    }
    bytes = bytes
        .saturating_add(std::mem::size_of::<usize>() * 8)
        .saturating_add(
            descriptors
                .len()
                .saturating_mul(std::mem::size_of::<KiroModelDescriptor>()),
        );
    for value in descriptors {
        bytes = bytes
            .saturating_add(value.model_id.capacity())
            .saturating_add(value.display_name.as_ref().map_or(0, String::capacity))
            .saturating_add(value.description.as_ref().map_or(0, String::capacity));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(index: usize) -> ModelCacheKey {
        ModelCacheKey {
            app: "claude".into(),
            provider_id: format!("provider-{index}"),
            provider_revision: 1,
            runtime_fingerprint: "runtime".into(),
            account_id: "account".into(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            request_identity_fingerprint: [1; 32],
            profile_scope: "profile".into(),
            runtime_region: "us-east-1".into(),
        }
    }

    fn models() -> Vec<KiroModelDescriptor> {
        vec![KiroModelDescriptor {
            model_id: "fixture-model".into(),
            display_name: Some("fixture".into()),
            description: Some("reviewed description".into()),
            max_input_tokens: Some(200000),
            max_output_tokens: Some(1000),
        }]
    }

    #[test]
    fn kiro_r4_catalog_snapshot_shares_owner_until_last_payload_drop() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = RuntimeCache::with_parent(parent.clone());
        assert!(cache.publish_models(key(0), &models(), 1));
        let one = cache
            .models_snapshot(&key(0), 2, MODEL_CACHE_TTL_MS)
            .unwrap();
        let two = one.clone();
        assert!(Arc::ptr_eq(&one.descriptors, &two.descriptors));
        let charged = parent.snapshot().used_bytes;
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, charged);
        drop(one);
        assert_eq!(parent.snapshot().used_bytes, charged);
        drop(two);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn kiro_r4_catalog_pressure_invalidates_replacement_without_reusing_old_authorization() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = RuntimeCache::with_parent(parent.clone());
        assert!(cache.publish_models(key(0), &models(), 1));
        parent.configure_limit(Some(0));
        assert!(!cache.publish_models(key(0), &models(), 2));
        assert!(cache
            .models_snapshot(&key(0), 3, MODEL_CACHE_TTL_MS)
            .is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn kiro_r4_catalog_count_eviction_and_lazy_stale_expiry_release_owners() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = RuntimeCache::with_parent(parent.clone());
        for index in 0..=MAX_CACHED_CATALOGS {
            assert!(cache.publish_models(key(index), &models(), 1 + index as i64));
        }
        assert_eq!(cache.models.len(), MAX_CACHED_CATALOGS);
        assert!(cache
            .models_snapshot(&key(0), 200, STALE_MODEL_CACHE_TTL_MS)
            .is_none());
        assert!(parent.snapshot().used_bytes > 0);
        assert!(cache
            .models_snapshot(
                &key(MAX_CACHED_CATALOGS),
                STALE_MODEL_CACHE_TTL_MS + 200,
                STALE_MODEL_CACHE_TTL_MS
            )
            .is_none());
        assert!(cache.models.is_empty());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn kiro_r4_ide_version_is_optional_and_replacement_releases_old_charge() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = RuntimeCache::with_parent(parent.clone());
        assert!(cache.publish_ide("0.9.2", 1));
        assert!(parent.snapshot().used_bytes > 0);
        parent.configure_limit(Some(0));
        assert!(!cache.publish_ide("0.9.3", 2));
        assert!(cache.ide_version.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
