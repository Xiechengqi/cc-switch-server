//! Ownership for cache indexes and persistence batches. Mutation owners move
//! with a detached writer batch; a failed batch never needs a second admission
//! merely to merge back into the cache.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::infra::resident_memory::{ResidentMemoryBudget, ResidentMemoryReservation};

use super::{CacheEntry, PendingMutation, MAX_BREAKPOINTS};

pub(super) const MAX_PENDING_MUTATIONS: usize = 8_192;

pub(super) struct CacheResident {
    pub(super) parent: ResidentMemoryBudget,
    pub(super) entries: BTreeMap<u64, ResidentMemoryReservation>,
    slots: Arc<PendingSlots>,
}

#[derive(Debug)]
struct PendingSlots {
    used: AtomicUsize,
    limit: usize,
}

#[derive(Debug)]
pub(super) struct PendingOwner {
    _memory: ResidentMemoryReservation,
    slots: Arc<PendingSlots>,
}

impl Drop for PendingOwner {
    fn drop(&mut self) {
        self.slots.used.fetch_sub(1, Ordering::AcqRel);
    }
}

fn node_bytes<T>() -> usize {
    std::mem::size_of::<(u64, T)>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
}

impl CacheResident {
    pub(super) fn new(parent: ResidentMemoryBudget, pending_limit: usize) -> Self {
        Self {
            parent,
            entries: BTreeMap::new(),
            slots: Arc::new(PendingSlots {
                used: AtomicUsize::new(0),
                limit: pending_limit,
            }),
        }
    }

    pub(super) fn scratch(&self, entry_count: usize) -> Option<ResidentMemoryReservation> {
        // Planned expiry/renewal/eviction nodes, victim vectors and admission
        // indexes are temporary and can coexist before the atomic publication.
        self.parent.reserve(
            entry_count
                .saturating_add(MAX_BREAKPOINTS * 2)
                .saturating_mul(
                    node_bytes::<Option<CacheEntry>>()
                        .saturating_add(node_bytes::<PendingOwner>())
                        .saturating_add(std::mem::size_of::<(u64, i64)>() * 2),
                ),
        )
    }

    pub(super) fn entry(&self) -> Option<ResidentMemoryReservation> {
        self.parent.reserve(
            node_bytes::<CacheEntry>()
                .saturating_add(node_bytes::<ResidentMemoryReservation>())
                .saturating_add(std::mem::size_of::<usize>() * 8),
        )
    }

    pub(super) fn pending(&self) -> Option<PendingOwner> {
        let memory = self.parent.reserve(
            node_bytes::<PendingMutation>().saturating_add(std::mem::size_of::<usize>() * 8),
        )?;
        self.slots
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(1).filter(|next| *next <= self.slots.limit)
            })
            .ok()?;
        Some(PendingOwner {
            _memory: memory,
            slots: self.slots.clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn pending_count(&self) -> usize {
        self.slots.used.load(Ordering::Acquire)
    }
}
