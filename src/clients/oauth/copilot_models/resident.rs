use super::*;
use crate::infra::resident_flights::{node_bytes, ResidentFlightGuard, ResidentFlights};
use crate::infra::resident_memory::{process_resident_memory, ResidentMemoryBudget};

impl Default for CopilotModelCatalogCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

fn key_bytes(key: &CopilotModelCatalogKey) -> usize {
    key.account_id
        .capacity()
        .saturating_add(key.github_domain.capacity())
        .saturating_add(key.api_origin.capacity())
}

pub(super) fn descriptors_bytes(descriptors: &Vec<CopilotModelDescriptor>) -> usize {
    descriptors.iter().fold(
        descriptors
            .capacity()
            .saturating_mul(std::mem::size_of::<CopilotModelDescriptor>()),
        |bytes, descriptor| {
            bytes
                .saturating_add(descriptor.model_id.capacity())
                .saturating_add(descriptor.display_name.as_ref().map_or(0, String::capacity))
                .saturating_add(descriptor.vendor.as_ref().map_or(0, String::capacity))
                .saturating_add(descriptor.policy_state.as_ref().map_or(0, String::capacity))
                .saturating_add(descriptor.model_type.as_ref().map_or(0, String::capacity))
                .saturating_add(
                    descriptor
                        .supported_endpoints
                        .capacity()
                        .saturating_mul(std::mem::size_of::<String>()),
                )
                .saturating_add(
                    descriptor
                        .supported_endpoints
                        .iter()
                        .map(String::capacity)
                        .sum::<usize>(),
                )
        },
    )
}

impl CopilotModelCatalogCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            catalogs: RwLock::new(BTreeMap::new()),
            flights: ResidentFlights::new(parent.clone(), MAX_MODEL_CACHE_ENTRIES),
            parent,
        }
    }

    pub async fn fresh(
        &self,
        key: &CopilotModelCatalogKey,
        now_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, value| {
            now_ms.saturating_sub(value.fetched_at_ms.unwrap_or(0)) < STALE_MODEL_CACHE_TTL_MS
        });
        catalogs
            .get(key)
            .filter(|value| {
                now_ms.saturating_sub(value.fetched_at_ms.unwrap_or(0)) < MODEL_CACHE_TTL_MS
            })
            .cloned()
            .map(|mut catalog| {
                catalog.source = "copilot_account_cache";
                catalog
            })
    }

    pub async fn stale(
        &self,
        key: &CopilotModelCatalogKey,
        now_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, value| {
            now_ms.saturating_sub(value.fetched_at_ms.unwrap_or(0)) < STALE_MODEL_CACHE_TTL_MS
        });
        catalogs.get(key).cloned().map(|mut catalog| {
            catalog.source = "copilot_account_cache";
            catalog.stale = true;
            catalog
        })
    }

    pub async fn insert(
        &self,
        key: CopilotModelCatalogKey,
        descriptors: Vec<CopilotModelDescriptor>,
        fetched_at_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        self.insert_fetched(
            key,
            FetchedCopilotModels {
                descriptors,
                _memory: None,
            },
            fetched_at_ms,
        )
        .await
    }

    pub(crate) async fn insert_fetched(
        &self,
        key: CopilotModelCatalogKey,
        fetched: FetchedCopilotModels,
        fetched_at_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        let FetchedCopilotModels {
            descriptors,
            _memory: payload_memory,
        } = fetched;
        let mut catalogs = self.catalogs.write().await;
        catalogs.remove(&key);
        catalogs.retain(|_, value| {
            fetched_at_ms.saturating_sub(value.fetched_at_ms.unwrap_or(0))
                < STALE_MODEL_CACHE_TTL_MS
        });
        while catalogs.len() >= MAX_MODEL_CACHE_ENTRIES {
            let oldest = catalogs
                .iter()
                .min_by_key(|(key, value)| (value.fetched_at_ms, *key))
                .map(|(key, _)| key.clone())
                .expect("nonempty catalog");
            catalogs.remove(&oldest);
        }
        let bytes = node_bytes::<CopilotModelCatalogKey, CopilotModelCatalog>(
            key_bytes(&key)
                .saturating_add(key.github_domain.len())
                .saturating_add(key.api_origin.len())
                .saturating_add(std::mem::size_of::<CopilotModelCatalogData>())
                .saturating_add(std::mem::size_of::<usize>() * 2)
                .saturating_add(if payload_memory.is_none() {
                    descriptors_bytes(&descriptors)
                } else {
                    0
                })
                .saturating_add(
                    descriptors
                        .len()
                        .saturating_mul(std::mem::size_of::<String>()),
                )
                .saturating_add(
                    descriptors
                        .iter()
                        .map(|descriptor| descriptor.model_id.len())
                        .sum::<usize>(),
                ),
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            crate::metrics::record_resident_retention_refused("copilot_catalog");
            return None;
        };
        let models = descriptors
            .iter()
            .map(|descriptor| descriptor.model_id.clone())
            .collect();
        let catalog = CopilotModelCatalog {
            data: Arc::new(CopilotModelCatalogData {
                models,
                descriptors,
                api_origin: Some(key.api_origin.clone()),
                github_domain: Some(key.github_domain.clone()),
                _memory: Some(memory),
                _payload_memory: payload_memory,
            }),
            source: "copilot_models_api",
            stale: false,
            fetched_at_ms: Some(fetched_at_ms),
        };
        catalogs.insert(key, catalog.clone());
        Some(catalog)
    }

    pub(crate) async fn invalidate(&self, key: &CopilotModelCatalogKey) {
        self.catalogs.write().await.remove(key);
    }

    pub(crate) async fn lock(&self, key: &CopilotModelCatalogKey) -> Option<ResidentFlightGuard> {
        let guard = self.flights.lock(key, key_bytes(key)).await;
        if guard.is_none() {
            crate::metrics::record_resident_retention_refused("copilot_catalog_flight");
        }
        guard
    }
}
