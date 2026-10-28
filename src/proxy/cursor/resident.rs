//! Process ownership for Cursor state that outlives an individual request.
//! B-tree slot estimates deliberately allow a sparse node per entry. These
//! logical charges include capacities, but are not allocator or RSS readings.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::infra::resident_memory::{ResidentMemoryBudget, ResidentMemoryReservation};
use crate::proxy::ProxyError;

pub(super) fn entry_bytes<K, V>(dynamic_bytes: usize) -> usize {
    size_of::<(K, V)>()
        .saturating_add(size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(dynamic_bytes)
}

#[derive(Debug)]
pub(super) struct ResidentEntry<T> {
    pub(super) value: T,
    _memory: ResidentMemoryReservation,
}

impl<T> ResidentEntry<T> {
    pub(super) fn new<K>(
        parent: &ResidentMemoryBudget,
        value: T,
        dynamic_bytes: usize,
    ) -> Option<Self> {
        Self::with::<K>(parent, dynamic_bytes, || value)
    }

    pub(super) fn with<K>(
        parent: &ResidentMemoryBudget,
        dynamic_bytes: usize,
        value: impl FnOnce() -> T,
    ) -> Option<Self> {
        let memory = parent.reserve(entry_bytes::<K, Self>(dynamic_bytes))?;
        Some(Self {
            _memory: memory,
            value: value(),
        })
    }
}

type FlightMap<K> = BTreeMap<K, Arc<Mutex<ResidentMemoryReservation>>>;

#[derive(Debug)]
pub(super) struct CursorFlights<K> {
    entries: Mutex<FlightMap<K>>,
    parent: ResidentMemoryBudget,
}

impl<K: Ord + Clone> CursorFlights<K> {
    pub(super) fn new(parent: ResidentMemoryBudget) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            parent,
        }
    }

    pub(super) async fn lock(
        &self,
        key: &K,
        key_bytes: usize,
    ) -> Result<OwnedMutexGuard<ResidentMemoryReservation>, ProxyError> {
        let flight = {
            let mut entries = self.entries.lock().await;
            entries.retain(|candidate, flight| candidate == key || Arc::strong_count(flight) > 1);
            if let Some(flight) = entries.get(key) {
                flight.clone()
            } else {
                let memory = self
                    .parent
                    .reserve(entry_bytes::<K, Arc<Mutex<ResidentMemoryReservation>>>(
                        key_bytes
                            .saturating_add(size_of::<Mutex<ResidentMemoryReservation>>())
                            .saturating_add(size_of::<usize>() * 2),
                    ))
                    .ok_or_else(ProxyError::request_memory_exhausted)?;
                let flight = Arc::new(Mutex::new(memory));
                entries.insert(key.clone(), flight.clone());
                flight
            }
        };
        Ok(flight.lock_owned().await)
    }

    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cursor_r4_flights_count_waiters_reject_new_scopes_and_release_last_owner() {
        let parent = ResidentMemoryBudget::new(None);
        let flights = Arc::new(CursorFlights::new(parent.clone()));
        let key = "scope-a".to_string();
        let first = flights.lock(&key, key.capacity()).await.unwrap();
        let used = parent.snapshot().used_bytes;
        assert!(used > 0);
        parent.configure_limit(Some(used));
        let other = "scope-b".to_string();
        assert!(flights
            .lock(&other, other.capacity())
            .await
            .unwrap_err()
            .is_request_memory_exhausted());
        let waiting_flights = flights.clone();
        let waiting = tokio::spawn(async move { waiting_flights.lock(&key, key.capacity()).await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(first);
        let last = waiting.await.unwrap().unwrap();
        drop(flights);
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(last);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
