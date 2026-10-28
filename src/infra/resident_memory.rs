//! Logical retained bytes shared by request budgets and long-lived owners.
//! This does not measure allocator overhead, transport internals, or process RSS.
use std::sync::{Arc, Mutex, OnceLock};

/// Count encoded bytes without allocating another serialized copy. The
/// writer stops at the existing wire limit, including JSON escape expansion.
pub(crate) fn serialized_json_bytes<T: serde::Serialize>(value: &T, limit: usize) -> Option<usize> {
    struct Counter {
        used: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .used
                .checked_add(bytes.len())
                .filter(|next| *next <= self.limit)
                .ok_or_else(|| std::io::Error::other("serialized JSON limit exceeded"))?;
            self.used = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Counter { used: 0, limit };
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.used)
}

/// An allocation-oriented estimate for retained JSON. Collection indexes are
/// estimated; allocator slack and library internals still require RSS sampling.
pub(crate) fn retained_json_bytes(value: &serde_json::Value) -> usize {
    fn dynamic_bytes(value: &serde_json::Value) -> usize {
        use serde_json::Value;
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            Value::String(value) => value.capacity(),
            Value::Array(values) => values.iter().fold(
                values
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Value>()),
                |bytes, value| bytes.saturating_add(dynamic_bytes(value)),
            ),
            Value::Object(values) => values.iter().fold(
                values.len().saturating_mul(
                    std::mem::size_of::<String>()
                        .saturating_add(std::mem::size_of::<Value>())
                        .saturating_add(std::mem::size_of::<usize>() * 3),
                ),
                |bytes, (key, value)| {
                    bytes
                        .saturating_add(key.capacity())
                        .saturating_add(dynamic_bytes(value))
                },
            ),
        }
    }
    std::mem::size_of::<serde_json::Value>().saturating_add(dynamic_bytes(value))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResidentMemorySnapshot {
    pub(crate) limit_bytes: Option<usize>,
    pub(crate) used_bytes: usize,
    pub(crate) high_water_bytes: usize,
    pub(crate) active_reservations: usize,
    pub(crate) rejections: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ResidentMemoryBudget(Arc<Mutex<ResidentMemorySnapshot>>);

pub(crate) fn process_resident_memory() -> ResidentMemoryBudget {
    static BUDGET: OnceLock<ResidentMemoryBudget> = OnceLock::new();
    BUDGET
        .get_or_init(|| {
            let budget = ResidentMemoryBudget::new(None);
            let limit = std::env::var("CC_SWITCH_PROCESS_MEMORY_BUDGET_MB")
                .ok()
                .and_then(|value| match value.parse::<usize>() {
                    Ok(0) => None,
                    Ok(megabytes) => megabytes.checked_mul(1024 * 1024),
                    Err(_) => {
                        tracing::warn!("invalid CC_SWITCH_PROCESS_MEMORY_BUDGET_MB; observing logical memory without a hard process limit");
                        None
                    }
                });
            budget.configure_limit(limit);
            budget
        })
        .clone()
}

impl ResidentMemoryBudget {
    pub(crate) fn new(limit_bytes: Option<usize>) -> Self {
        Self(Arc::new(Mutex::new(ResidentMemorySnapshot {
            limit_bytes,
            ..Default::default()
        })))
    }

    /// A lower limit never erases charges for existing owners. Shrinking
    /// remains possible while the aggregate is above its new limit.
    pub(crate) fn configure_limit(&self, limit_bytes: Option<usize>) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.limit_bytes = limit_bytes;
        crate::metrics::set_process_resident_memory(*state);
    }

    pub(crate) fn snapshot(&self) -> ResidentMemorySnapshot {
        *self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn remaining_bytes(&self) -> usize {
        let state = self.snapshot();
        state
            .limit_bytes
            .unwrap_or(usize::MAX)
            .saturating_sub(state.used_bytes)
    }

    /// Callers hold their owner's lock until committing its matching update.
    /// Parent code never acquires a child lock or performs I/O.
    pub(crate) fn try_resize(&self, current: usize, next: usize) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if next > current {
            let Some(total) = state
                .used_bytes
                .checked_add(next - current)
                .filter(|total| state.limit_bytes.is_none_or(|limit| *total <= limit))
            else {
                state.rejections = state.rejections.saturating_add(1);
                crate::metrics::set_process_resident_memory(*state);
                return false;
            };
            state.used_bytes = total;
            if current == 0 {
                state.active_reservations = state.active_reservations.saturating_add(1);
            }
            state.high_water_bytes = state.high_water_bytes.max(total);
        } else {
            state.used_bytes = state.used_bytes.saturating_sub(current - next);
            if current > 0 && next == 0 {
                state.active_reservations = state.active_reservations.saturating_sub(1);
            }
        }
        crate::metrics::set_process_resident_memory(*state);
        true
    }

    pub(crate) fn reserve(&self, bytes: usize) -> Option<ResidentMemoryReservation> {
        self.try_resize(0, bytes)
            .then(|| ResidentMemoryReservation {
                _owner: Arc::new(ResidentMemoryOwner {
                    budget: self.clone(),
                    bytes,
                }),
            })
    }
}

/// Clones represent shared ownership, with one charge until the last drop.
#[derive(Clone, Debug)]
pub(crate) struct ResidentMemoryReservation {
    _owner: Arc<ResidentMemoryOwner>,
}

#[derive(Debug)]
struct ResidentMemoryOwner {
    budget: ResidentMemoryBudget,
    bytes: usize,
}

impl Drop for ResidentMemoryOwner {
    fn drop(&mut self) {
        self.budget.try_resize(self.bytes, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_r4_json_size_count_obeys_escaped_wire_bytes_without_a_copy() {
        let value = serde_json::json!({"text":"\u{0000}\"\\中"});
        let encoded = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            serialized_json_bytes(&value, encoded.len()),
            Some(encoded.len())
        );
        assert_eq!(serialized_json_bytes(&value, encoded.len() - 1), None);
        assert_eq!(serialized_json_bytes(&serde_json::json!(null), 0), None);
    }

    #[test]
    fn codex_r4_process_budget_tracks_observation_limits_and_shared_ownership() {
        let budget = ResidentMemoryBudget::new(None);
        let reservation = budget.reserve(80).unwrap();
        let shared = reservation.clone();
        budget.configure_limit(Some(50));
        assert!(budget.reserve(1).is_none());
        assert_eq!(budget.snapshot().used_bytes, 80);
        assert_eq!(budget.snapshot().high_water_bytes, 80);
        assert_eq!(budget.snapshot().rejections, 1);
        drop(reservation);
        assert_eq!(budget.snapshot().used_bytes, 80);
        drop(shared);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(budget.snapshot().active_reservations, 0);
        assert!(budget.reserve(50).is_some());
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[test]
    fn codex_r4_concurrent_reservations_cannot_overcommit_process_capacity() {
        let budget = ResidentMemoryBudget::new(Some(64));
        let barrier = Arc::new(std::sync::Barrier::new(9));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = budget.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let reservation = budget.reserve(16);
                    barrier.wait();
                    barrier.wait();
                    drop(reservation);
                });
            }
            barrier.wait();
            assert_eq!(budget.snapshot().used_bytes, 64);
            assert_eq!(budget.snapshot().active_reservations, 4);
            assert_eq!(budget.snapshot().rejections, 4);
            barrier.wait();
        });
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(budget.snapshot().active_reservations, 0);
    }
}
