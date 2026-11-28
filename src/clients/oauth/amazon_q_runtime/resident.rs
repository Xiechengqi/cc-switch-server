//! Amazon Q discovery snapshots and result-sharing flights. One payload owner
//! serves callers, the optional index and concurrent waiters. Index eviction
//! never drops the charge of a caller still using that snapshot.
use super::*;
use crate::infra::resident_flights::node_bytes;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;

pub(super) const MAX_CACHED_CATALOGS: usize = 128;
const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ACTIVE_FLIGHTS: usize = 64;

#[derive(Debug, Clone)]
pub(super) struct CatalogSnapshot {
    pub(super) data: Arc<AmazonQModelCatalogData>,
    pub(super) fetched_at_ms: i64,
}

struct CachedCatalog {
    snapshot: CatalogSnapshot,
    index_bytes: usize,
    _memory: ResidentMemoryReservation,
}

type Outcome = Result<CatalogSnapshot, DiscoveryFailure>;

struct CatalogFlight {
    gate: Mutex<()>,
    state: StdMutex<FlightState>,
    current: AtomicBool,
    _memory: ResidentMemoryReservation,
}

#[derive(Default)]
struct FlightState {
    users: usize,
    outcome: Option<Outcome>,
}

pub(super) struct FlightLease {
    flight: Arc<CatalogFlight>,
    _memory: ResidentMemoryReservation,
}

impl FlightLease {
    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.flight.gate.lock().await
    }

    pub(super) fn is_current(&self) -> bool {
        self.flight.current.load(Ordering::Acquire)
    }

    pub(super) fn outcome(&self) -> Option<Outcome> {
        self.flight
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outcome
            .clone()
    }

    pub(super) fn complete(&self, outcome: Outcome) {
        self.flight
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outcome = Some(outcome);
    }
}

impl Drop for FlightLease {
    fn drop(&mut self) {
        let mut state = self.flight.state.lock().unwrap_or_else(|e| e.into_inner());
        state.users -= 1;
        if state.users == 0 {
            // The sparse index can remain until lazy cleanup, but must not pin
            // a second payload/error result once its last caller has left.
            state.outcome.take();
        }
    }
}

pub(super) struct RuntimeCache {
    catalogs: Mutex<BTreeMap<CacheKey, CachedCatalog>>,
    flights: Mutex<BTreeMap<CacheKey, Arc<CatalogFlight>>>,
    pub(super) parent: ResidentMemoryBudget,
}

impl Default for RuntimeCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl RuntimeCache {
    pub(super) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            catalogs: Mutex::new(BTreeMap::new()),
            flights: Mutex::new(BTreeMap::new()),
            parent,
        }
    }

    pub(super) async fn snapshot(
        &self,
        key: &CacheKey,
        now: i64,
        ttl: i64,
    ) -> Option<CatalogSnapshot> {
        let mut catalogs = self.catalogs.lock().await;
        catalogs.retain(|_, c| now.saturating_sub(c.snapshot.fetched_at_ms) < STALE_CACHE_TTL_MS);
        catalogs
            .get(key)
            .filter(|c| now.saturating_sub(c.snapshot.fetched_at_ms) < ttl)
            .map(|c| c.snapshot.clone())
    }

    pub(super) async fn flight(&self, key: &CacheKey) -> Option<FlightLease> {
        let dynamic = key_bytes(key);
        let waiter = self.parent.reserve(dynamic.saturating_add(2048))?;
        let mut flights = self.flights.lock().await;
        flights
            .retain(|_, flight| flight.state.lock().unwrap_or_else(|e| e.into_inner()).users > 0);
        let flight = if let Some(flight) = flights.get(key) {
            flight.clone()
        } else {
            if flights.len() >= MAX_ACTIVE_FLIGHTS {
                return None;
            }
            let memory = self
                .parent
                .reserve(node_bytes::<CacheKey, Arc<CatalogFlight>>(
                    dynamic
                        .saturating_add(std::mem::size_of::<CatalogFlight>())
                        .saturating_add(32),
                ))?;
            let flight = Arc::new(CatalogFlight {
                gate: Mutex::new(()),
                state: StdMutex::new(FlightState::default()),
                current: AtomicBool::new(true),
                _memory: memory,
            });
            flights.insert(key.clone(), flight.clone());
            flight
        };
        flight.state.lock().unwrap_or_else(|e| e.into_inner()).users += 1;
        Some(FlightLease {
            flight,
            _memory: waiter,
        })
    }

    pub(super) async fn publish(
        &self,
        key: CacheKey,
        snapshot: &CatalogSnapshot,
        lease: &FlightLease,
    ) {
        let mut catalogs = self.catalogs.lock().await;
        if !lease.is_current() {
            return;
        }
        let now = snapshot.fetched_at_ms;
        catalogs.remove(&key);
        catalogs.retain(|_, c| now.saturating_sub(c.snapshot.fetched_at_ms) < STALE_CACHE_TTL_MS);
        let index_bytes = node_bytes::<CacheKey, CachedCatalog>(key_bytes(&key));
        let bytes = index_bytes.saturating_add(snapshot.data.retained_bytes);
        if bytes > MAX_CACHED_BYTES {
            return;
        }
        while catalogs.len() >= MAX_CACHED_CATALOGS
            || catalogs
                .values()
                .fold(0_usize, |n, c| {
                    n.saturating_add(c.index_bytes)
                        .saturating_add(c.snapshot.data.retained_bytes)
                })
                .saturating_add(bytes)
                > MAX_CACHED_BYTES
        {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(k, c)| (c.snapshot.fetched_at_ms, *k))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            catalogs.remove(&oldest);
        }
        let Some(memory) = self.parent.reserve(index_bytes) else {
            crate::metrics::record_resident_retention_refused("amazon_q_catalog");
            return;
        };
        catalogs.insert(
            key,
            CachedCatalog {
                snapshot: snapshot.clone(),
                index_bytes,
                _memory: memory,
            },
        );
    }

    pub(super) async fn discard(&self, key: &CacheKey) {
        self.catalogs.lock().await.remove(key);
    }

    #[cfg(test)]
    pub(super) async fn age_for_test(&self, age_ms: i64) {
        for entry in self.catalogs.lock().await.values_mut() {
            entry.snapshot.fetched_at_ms -= age_ms;
        }
    }

    pub(super) async fn clear_account(&self, account_id: &str) {
        // Revoke before removing the catalog. A completing leader checks this
        // flag while holding the catalog lock, so it cannot resurrect it.
        let mut flights = self.flights.lock().await;
        flights.retain(|key, flight| {
            if key.account_id != account_id {
                return true;
            }
            flight.current.store(false, Ordering::Release);
            flight.state.lock().unwrap_or_else(|e| e.into_inner()).users > 0
        });
        drop(flights);
        self.catalogs
            .lock()
            .await
            .retain(|key, _| key.account_id != account_id);
    }
}

fn key_bytes(key: &CacheKey) -> usize {
    [
        &key.app,
        &key.provider_id,
        &key.runtime_fingerprint,
        &key.account_id,
        &key.profile_scope,
        &key.runtime_region,
    ]
    .into_iter()
    .fold(0_usize, |n, s| n.saturating_add(s.capacity()))
}

pub(super) fn payload_bytes(
    descriptors: &Vec<AmazonQModelDescriptor>,
    default: Option<&String>,
) -> usize {
    let mut bytes = std::mem::size_of::<AmazonQModelCatalogData>()
        .saturating_add(32)
        .saturating_add(
            descriptors
                .capacity()
                .saturating_mul(std::mem::size_of::<AmazonQModelDescriptor>()),
        )
        .saturating_add(default.map_or(0, String::capacity));
    for d in descriptors {
        bytes = bytes
            .saturating_add(d.model_id.capacity())
            .saturating_add(d.display_name.as_ref().map_or(0, String::capacity))
            .saturating_add(d.description.as_ref().map_or(0, String::capacity))
            .saturating_add(
                d.supported_input_types
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            );
        for input in &d.supported_input_types {
            bytes = bytes.saturating_add(input.capacity());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(revision: u64) -> CacheKey {
        CacheKey {
            account_incarnation: 1,
            app: "claude".into(),
            provider_id: "q-resident".into(),
            provider_revision: revision,
            runtime_fingerprint: format!("runtime-{revision}"),
            account_id: "q-resident-account".into(),
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            credential_fingerprint: [0; 32],
            profile_scope: "builder_id_profileless".into(),
            runtime_region: "us-east-1".into(),
        }
    }

    fn snapshot(parent: &ResidentMemoryBudget, at: i64, models: usize) -> CatalogSnapshot {
        let descriptors = (0..models)
            .map(|n| {
                parse_model(&json!({
                    "modelId": format!("model-{n}"), "description": "d".repeat(8192)
                }))
                .unwrap()
            })
            .collect::<Vec<_>>();
        let bytes = payload_bytes(&descriptors, None);
        CatalogSnapshot {
            data: Arc::new(AmazonQModelCatalogData {
                descriptors,
                default_model_id: None,
                retained_bytes: bytes,
                _memory: Some(parent.reserve(bytes).unwrap()),
            }),
            fetched_at_ms: at,
        }
    }

    #[tokio::test]
    async fn r7_amazon_q_snapshot_last_owner_survives_cache_and_flight_removal() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = RuntimeCache::with_parent(parent.clone());
        let lease = cache.flight(&key(1)).await.unwrap();
        let first = snapshot(&parent, 1, 1);
        let bytes = first.data.retained_bytes;
        lease.complete(Ok(first.clone()));
        cache.publish(key(1), &first, &lease).await;
        let live = cache.snapshot(&key(1), 2, CACHE_TTL_MS).await.unwrap();
        assert!(Arc::ptr_eq(&first.data, &live.data));
        cache.clear_account("q-resident-account").await;
        assert!(!lease.is_current());
        assert!(cache.snapshot(&key(1), 2, CACHE_TTL_MS).await.is_none());
        drop(first);
        drop(lease);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, bytes);
        drop(live);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
    }

    #[tokio::test]
    async fn r7_amazon_q_catalog_count_expiry_and_byte_bounds_are_independent() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = RuntimeCache::with_parent(parent.clone());
        for n in 0..(MAX_CACHED_CATALOGS as u64 + 5) {
            let lease = cache.flight(&key(n)).await.unwrap();
            cache
                .publish(key(n), &snapshot(&parent, n as i64, 0), &lease)
                .await;
        }
        assert_eq!(cache.catalogs.lock().await.len(), MAX_CACHED_CATALOGS);
        assert!(cache.snapshot(&key(0), 200, CACHE_TTL_MS).await.is_none());
        assert!(cache
            .snapshot(&key(132), STALE_CACHE_TTL_MS + 200, STALE_CACHE_TTL_MS)
            .await
            .is_none());
        assert!(cache.catalogs.lock().await.is_empty());
        for n in 0..10 {
            let lease = cache.flight(&key(n)).await.unwrap();
            cache
                .publish(key(n), &snapshot(&parent, n as i64, 500), &lease)
                .await;
        }
        let entries = cache.catalogs.lock().await;
        assert!(entries.len() < 10);
        assert!(
            entries
                .values()
                .map(|c| c.index_bytes + c.snapshot.data.retained_bytes)
                .sum::<usize>()
                <= MAX_CACHED_BYTES
        );
        drop(entries);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_amazon_q_flight_capacity_and_waiter_cancellation_preserve_leader() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = RuntimeCache::with_parent(parent.clone());
        let mut active = Vec::new();
        for n in 0..MAX_ACTIVE_FLIGHTS as u64 {
            active.push(cache.flight(&key(n)).await.unwrap());
        }
        assert!(cache.flight(&key(1000)).await.is_none());
        let before_waiter = parent.snapshot().used_bytes;
        let waiter = cache.flight(&key(0)).await.unwrap();
        let guard = active[0].lock().await;
        let waiting = tokio::spawn(async move {
            let _guard = waiter.lock().await;
        });
        tokio::task::yield_now().await;
        assert!(parent.snapshot().used_bytes > before_waiter);
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert_eq!(parent.snapshot().used_bytes, before_waiter);
        active[0].complete(Ok(snapshot(&parent, 1, 1)));
        let completed = active[0].outcome().unwrap().unwrap();
        let weak = Arc::downgrade(&completed.data);
        drop(completed);
        drop(guard);
        drop(active);
        assert!(
            weak.upgrade().is_none(),
            "idle flight index must not retain a full result"
        );
        assert!(cache.flight(&key(1000)).await.is_some());
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r7_amazon_q_optional_cache_refusal_keeps_live_payload_and_charge() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = RuntimeCache::with_parent(parent.clone());
        let lease = cache.flight(&key(1)).await.unwrap();
        let live = snapshot(&parent, 1, 1);
        let before = parent.snapshot().used_bytes;
        parent.configure_limit(Some(before));
        cache.publish(key(1), &live, &lease).await;
        assert!(cache.catalogs.lock().await.is_empty());
        assert_eq!(live.data.descriptors.len(), 1);
        assert_eq!(parent.snapshot().used_bytes, before);
        assert!(cache.flight(&key(2)).await.is_none());
        drop(lease);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, live.data.retained_bytes);
        drop(live);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
