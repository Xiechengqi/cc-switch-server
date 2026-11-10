//! Per-cache, bounded singleflight registrations with a shared process parent.
use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use super::resident_memory::{ResidentMemoryBudget, ResidentMemoryReservation};

/// Conservative sparse B-tree node estimate, including unused node slots.
pub(crate) fn node_bytes<K, V>(dynamic: usize) -> usize {
    std::mem::size_of::<(K, V)>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(dynamic)
}

type Flights<K> = BTreeMap<K, Arc<Mutex<ResidentMemoryReservation>>>;

/// Covers bounded control-plane body buffering and transient JSON expansion.
/// This conservative allocation estimate is not an RSS measurement.
pub(crate) struct CatalogFetchMemory {
    parent: ResidentMemoryBudget,
    _body: ResidentMemoryReservation,
}

impl CatalogFetchMemory {
    pub(crate) fn new(parent: ResidentMemoryBudget, body_limit: usize) -> Option<Self> {
        let body = parent.reserve(body_limit)?;
        Some(Self {
            parent,
            _body: body,
        })
    }

    pub(crate) fn parsing(&self, encoded_bytes: usize) -> Option<ResidentMemoryReservation> {
        self.parent
            .reserve(encoded_bytes.saturating_mul(256).saturating_add(64 * 1024))
    }

    pub(crate) fn payload(&self, bytes: usize) -> Option<ResidentMemoryReservation> {
        self.parent.reserve(bytes)
    }
}

#[derive(Debug)]
pub(crate) struct ResidentFlightGuard {
    _guard: OwnedMutexGuard<ResidentMemoryReservation>,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug)]
pub(crate) struct ResidentFlights<K> {
    entries: Mutex<Flights<K>>,
    parent: ResidentMemoryBudget,
    limit: usize,
}

impl<K: Clone + Ord> ResidentFlights<K> {
    pub(crate) fn new(parent: ResidentMemoryBudget, limit: usize) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            parent,
            limit,
        }
    }

    pub(crate) async fn lock(&self, key: &K, key_bytes: usize) -> Option<ResidentFlightGuard> {
        let flight = {
            let mut entries = self.entries.lock().await;
            // Guards and waiters own the mutex Arc, so removing an index can
            // never release their charge or start another flight for that key.
            entries.retain(|current, flight| current == key || Arc::strong_count(flight) > 1);
            if let Some(flight) = entries.get(key) {
                flight.clone()
            } else {
                if entries.len() >= self.limit {
                    return None;
                }
                let bytes = node_bytes::<K, Arc<Mutex<ResidentMemoryReservation>>>(
                    key_bytes
                        .saturating_add(std::mem::size_of::<Mutex<ResidentMemoryReservation>>())
                        .saturating_add(std::mem::size_of::<usize>() * 2),
                );
                let memory = self.parent.reserve(bytes)?;
                let flight = Arc::new(Mutex::new(memory));
                entries.insert(key.clone(), flight.clone());
                flight
            }
        };
        // Each waiting future also owns a reservation. Cancellation releases
        // it before taking the lock; successful acquisition transfers it.
        let memory = self.parent.reserve(
            key_bytes
                .saturating_add(std::mem::size_of::<ResidentFlightGuard>())
                .saturating_add(256),
        )?;
        Some(ResidentFlightGuard {
            _guard: flight.lock_owned().await,
            _memory: memory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn r5_resident_flights_charge_waiters_and_guards_after_index_drop() {
        let parent = ResidentMemoryBudget::new(None);
        let flights = Arc::new(ResidentFlights::new(parent.clone(), 1));
        let first = flights.lock(&1_u64, 0).await.unwrap();
        let bytes = parent.snapshot().used_bytes;
        assert!(bytes > 0);
        assert!(flights.lock(&2, 0).await.is_none());
        let waiting_flights = flights.clone();
        let waiter = tokio::spawn(async move { waiting_flights.lock(&1, 0).await.unwrap() });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        assert!(parent.snapshot().used_bytes > bytes);
        drop(first);
        let second = waiter.await.unwrap();
        drop(flights);
        assert_eq!(parent.snapshot().used_bytes, bytes);
        drop(second);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r5_resident_flights_refusal_and_cancellation_conserve_capacity() {
        let parent = ResidentMemoryBudget::new(Some(0));
        let flights = Arc::new(ResidentFlights::new(parent.clone(), 2));
        assert!(flights.lock(&1_u64, 0).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
        parent.configure_limit(None);
        let first = flights.lock(&1, 0).await.unwrap();
        let bytes = parent.snapshot().used_bytes;
        let waiting_flights = flights.clone();
        let waiter = tokio::spawn(async move { waiting_flights.lock(&1, 0).await });
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        drop(first);
        drop(flights.lock(&2, 0).await.unwrap());
        assert!(parent.snapshot().used_bytes < bytes);
        assert_eq!(parent.snapshot().active_reservations, 1);
        drop(flights);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
