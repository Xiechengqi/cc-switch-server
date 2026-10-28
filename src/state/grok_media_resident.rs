//! Resident ownership for immutable media-task snapshots and persistence jobs.
//! The persisted schema stays in the parent module; Arc clones share one
//! physical table, while candidate tables are admitted before cloning entries.

use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

use super::{GrokMediaTaskBinding, GrokMediaTaskStore, PersistedStateSnapshot};

#[derive(Debug, thiserror::Error)]
#[error("Grok media task snapshot exceeded local process memory capacity")]
pub(crate) struct GrokMediaMemoryCapacity;

#[derive(Debug)]
pub(super) struct GrokMediaResidentSnapshot {
    store: GrokMediaTaskStore,
    parent: ResidentMemoryBudget,
    _memory: ResidentMemoryReservation,
}

impl Deref for GrokMediaResidentSnapshot {
    type Target = GrokMediaTaskStore;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl GrokMediaResidentSnapshot {
    pub(super) fn load(config_dir: &Path, now_ms: i64) -> anyhow::Result<Arc<Self>> {
        Self::admit(
            GrokMediaTaskStore::load_or_default(config_dir, now_ms)?,
            process_resident_memory(),
        )
    }

    fn admit(store: GrokMediaTaskStore, parent: ResidentMemoryBudget) -> anyhow::Result<Arc<Self>> {
        let bytes = store
            .bindings
            .iter()
            .fold(snapshot_bytes(), |sum, (key, binding)| {
                sum.saturating_add(binding_bytes(key, binding))
            });
        let memory = parent.reserve(bytes).ok_or(GrokMediaMemoryCapacity)?;
        Ok(Arc::new(Self {
            store,
            parent,
            _memory: memory,
        }))
    }

    pub(super) fn candidate(
        &self,
        now_ms: i64,
        replacement: Option<(String, GrokMediaTaskBinding)>,
    ) -> anyhow::Result<Arc<Self>> {
        let keep = |key: &str, binding: &GrokMediaTaskBinding| {
            binding.expires_at_ms > now_ms
                && replacement
                    .as_ref()
                    .is_none_or(|(new_key, _)| new_key != key)
        };
        let bytes = self
            .store
            .bindings
            .iter()
            .filter(|(key, binding)| keep(key, binding))
            .fold(snapshot_bytes(), |sum, (key, binding)| {
                sum.saturating_add(binding_bytes(key, binding))
            })
            .saturating_add(
                replacement
                    .as_ref()
                    .map_or(0, |(key, binding)| binding_bytes(key, binding)),
            );
        let memory = self.parent.reserve(bytes).ok_or(GrokMediaMemoryCapacity)?;
        let mut store = GrokMediaTaskStore {
            schema_version: self.store.schema_version,
            bindings: self
                .store
                .bindings
                .iter()
                .filter(|(key, binding)| keep(key, binding))
                .map(|(key, binding)| (key.clone(), binding.clone()))
                .collect(),
        };
        if let Some((key, binding)) = replacement {
            store.bindings.insert(key, binding);
        }
        store.enforce_capacity();
        // Eviction can only lower retained bytes. The conservative admission
        // stays with this snapshot until its final persistence/read owner drops.
        Ok(Arc::new(Self {
            store,
            parent: self.parent.clone(),
            _memory: memory,
        }))
    }
}

fn snapshot_bytes() -> usize {
    std::mem::size_of::<GrokMediaResidentSnapshot>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
}

fn binding_bytes(key: &String, binding: &GrokMediaTaskBinding) -> usize {
    std::mem::size_of::<(String, GrokMediaTaskBinding)>()
        .saturating_add(std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(key.capacity())
        .saturating_add(binding.task_id.capacity())
        .saturating_add(
            binding
                .creation_request_id
                .as_ref()
                .map_or(0, String::capacity),
        )
        .saturating_add(binding.provider_id.capacity())
        .saturating_add(binding.account_id.capacity())
        .saturating_add(binding.share_id.capacity())
        .saturating_add(binding.runtime_fingerprint.capacity())
        .saturating_add(binding.user_namespace.capacity())
}

impl PersistedStateSnapshot for Arc<GrokMediaResidentSnapshot> {
    const NAME: &'static str = GrokMediaTaskStore::NAME;

    fn save_snapshot(&self, config_dir: &Path) -> anyhow::Result<()> {
        self.store.save_snapshot(config_dir)
    }

    fn load_snapshot(config_dir: &Path) -> anyhow::Result<Self> {
        GrokMediaResidentSnapshot::load(config_dir, crate::infra::time::now_ms() as i64)
    }

    fn reconciliation_value(&self) -> Option<serde_json::Value> {
        self.store.reconciliation_value()
    }

    fn load_reconciliation_value(config_dir: &Path) -> anyhow::Result<Option<serde_json::Value>> {
        // Reconciliation is temporary control-plane work. Requiring another
        // resident owner here could hide a completed disk commit when the live
        // and candidate tables already occupy the available resident budget.
        GrokMediaTaskStore::load_snapshot(config_dir).map(|store| store.reconciliation_value())
    }
}

#[cfg(test)]
impl super::ServerStateInner {
    pub(crate) fn set_grok_media_parent_for_test(&self, parent: ResidentMemoryBudget) {
        let mut current = self.grok_media_tasks.lock().unwrap();
        *current = GrokMediaResidentSnapshot::admit(current.store.clone(), parent).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(expires_at_ms: i64) -> (String, GrokMediaTaskBinding) {
        super::super::tests::grok_media_task_binding_for_test("resident-task", expires_at_ms - 100)
    }

    #[test]
    fn grok_r4_media_arc_clones_share_one_charge_until_the_last_drop() {
        let parent = ResidentMemoryBudget::new(None);
        let mut store = GrokMediaTaskStore::default();
        let (key, binding) = binding(200);
        store.bindings.insert(key, binding);
        let snapshot = GrokMediaResidentSnapshot::admit(store, parent.clone()).unwrap();
        let persistence_owner = snapshot.clone();
        let charged = parent.snapshot().used_bytes;
        assert!(charged > snapshot_bytes());
        assert_eq!(parent.snapshot().active_reservations, 1);
        drop(snapshot);
        assert_eq!(parent.snapshot().used_bytes, charged);
        drop(persistence_owner);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn grok_r4_media_failed_candidate_admission_preserves_existing_ownership() {
        let parent = ResidentMemoryBudget::new(None);
        let snapshot =
            GrokMediaResidentSnapshot::admit(GrokMediaTaskStore::default(), parent.clone())
                .unwrap();
        let used = parent.snapshot().used_bytes;
        parent.configure_limit(Some(used));
        assert!(snapshot
            .candidate(0, Some(binding(200)))
            .unwrap_err()
            .is::<GrokMediaMemoryCapacity>());
        assert!(snapshot.bindings.is_empty());
        assert_eq!(parent.snapshot().used_bytes, used);
        assert_eq!(parent.snapshot().active_reservations, 1);
        drop(snapshot);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn grok_r4_media_candidates_account_separate_tables_and_release_expired_entries() {
        let parent = ResidentMemoryBudget::new(None);
        let empty = GrokMediaResidentSnapshot::admit(GrokMediaTaskStore::default(), parent.clone())
            .unwrap();
        let full = empty.candidate(0, Some(binding(200))).unwrap();
        drop(empty);
        let full_bytes = parent.snapshot().used_bytes;
        let pruned = full.candidate(i64::MAX, None).unwrap();
        assert!(pruned.bindings.is_empty());
        assert!(parent.snapshot().used_bytes > full_bytes);
        drop(full);
        assert_eq!(parent.snapshot().used_bytes, snapshot_bytes());
        assert_eq!(
            pruned.reconciliation_value().unwrap(),
            serde_json::to_value(GrokMediaTaskStore::default()).unwrap()
        );
        drop(pruned);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn grok_r4_media_persistence_failure_drops_only_the_candidate_owner() {
        let parent = ResidentMemoryBudget::new(None);
        let live = GrokMediaResidentSnapshot::admit(GrokMediaTaskStore::default(), parent.clone())
            .unwrap();
        let charged = parent.snapshot().used_bytes;
        let candidate = live.candidate(0, Some(binding(200))).unwrap();
        let path = std::env::temp_dir().join(format!(
            "cc-switch-grok-r4-store-{}",
            crate::domain::sharing::quota_cycle::opaque_id()
        ));
        std::fs::write(&path, b"not a directory").unwrap();
        assert!(
            super::super::persist_state_snapshot(&path, candidate.clone())
                .await
                .is_err()
        );
        drop(candidate);
        assert_eq!(parent.snapshot().used_bytes, charged);
        assert!(live.bindings.is_empty());
        std::fs::remove_file(path).unwrap();
        drop(live);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[derive(Clone)]
    struct PostCommitGrokSnapshot(Arc<GrokMediaResidentSnapshot>);

    impl PersistedStateSnapshot for PostCommitGrokSnapshot {
        const NAME: &'static str = "post-commit Grok media test";

        fn save_snapshot(&self, config_dir: &Path) -> anyhow::Result<()> {
            self.0.save_snapshot(config_dir)?;
            anyhow::bail!("simulated error after media task commit")
        }

        fn load_snapshot(_config_dir: &Path) -> anyhow::Result<Self> {
            anyhow::bail!("reconciliation must not allocate another resident snapshot")
        }

        fn reconciliation_value(&self) -> Option<serde_json::Value> {
            self.0.reconciliation_value()
        }

        fn load_reconciliation_value(
            config_dir: &Path,
        ) -> anyhow::Result<Option<serde_json::Value>> {
            Arc::<GrokMediaResidentSnapshot>::load_reconciliation_value(config_dir)
        }
    }

    #[tokio::test]
    async fn grok_r4_media_post_commit_reconciliation_needs_no_third_resident_owner() {
        let parent = ResidentMemoryBudget::new(None);
        let live = GrokMediaResidentSnapshot::admit(GrokMediaTaskStore::default(), parent.clone())
            .unwrap();
        let now = crate::infra::time::now_ms() as i64;
        let candidate = live.candidate(now, Some(binding(now + 60_000))).unwrap();
        let used = parent.snapshot().used_bytes;
        parent.configure_limit(Some(used));
        let path = std::env::temp_dir().join(format!(
            "cc-switch-grok-r4-reconcile-{}",
            crate::domain::sharing::quota_cycle::opaque_id()
        ));
        std::fs::create_dir(&path).unwrap();
        super::super::persist_state_snapshot(&path, PostCommitGrokSnapshot(candidate.clone()))
            .await
            .unwrap();
        assert_eq!(parent.snapshot().used_bytes, used);
        assert_eq!(parent.snapshot().active_reservations, 2);
        assert_eq!(
            Arc::<GrokMediaResidentSnapshot>::load_reconciliation_value(&path).unwrap(),
            candidate.reconciliation_value()
        );
        assert_eq!(parent.snapshot().used_bytes, used);
        assert!(live.bindings.is_empty());
        drop(candidate);
        drop(live);
        assert_eq!(parent.snapshot().used_bytes, 0);
        std::fs::remove_dir_all(path).unwrap();
    }
}
