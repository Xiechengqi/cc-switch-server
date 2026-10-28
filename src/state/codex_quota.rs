//! Bounded, off-reader coalescing of passive Codex quota observations.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::domain::accounts::codex_quota::{CodexQuotaSnapshot, CodexQuotaTarget};
use crate::infra::resident_memory::{process_resident_memory, ResidentMemoryReservation};

use super::ServerStateInner;

const MAX_PENDING: usize = 64;
// Covers the fixed snapshot, bounded strings, target, queue node and ownership
// metadata. No raw frame, credentials, headers or generation payload is queued.
const JOB_CHARGE: usize = 8 * 1024;

pub(super) struct CodexQuotaJob {
    pub(super) target: CodexQuotaTarget,
    pub(super) snapshot: CodexQuotaSnapshot,
    _memory: ResidentMemoryReservation,
}

#[derive(Default)]
struct Queue {
    running: bool,
    pending: VecDeque<CodexQuotaJob>,
}

pub(super) struct CodexQuotaObserver {
    enabled: bool,
    queue: Mutex<Queue>,
    #[cfg(test)]
    pub(super) commit_failures: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for CodexQuotaObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        formatter
            .debug_struct("CodexQuotaObserver")
            .field("enabled", &self.enabled)
            .field("pending", &queue.pending.len())
            .field("running", &queue.running)
            .finish()
    }
}

impl Default for CodexQuotaObserver {
    fn default() -> Self {
        Self {
            enabled: !matches!(
                std::env::var("CC_SWITCH_CODEX_QUOTA_OBSERVER").as_deref(),
                Ok("0" | "false" | "off")
            ),
            queue: Mutex::new(Queue::default()),
            #[cfg(test)]
            commit_failures: Default::default(),
        }
    }
}

impl CodexQuotaObserver {
    fn enqueue(&self, target: &CodexQuotaTarget, snapshot: CodexQuotaSnapshot) -> bool {
        if !self.enabled
            || target.account_id.len() > 512
            || target.provider_key.provider_id.len() > 512
            || snapshot.retained_bytes() > 2048
            || snapshot.is_empty()
        {
            return false;
        }
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(job) = queue.pending.iter_mut().find(|job| job.target == *target) {
            job.snapshot.merge(&snapshot);
            crate::metrics::record_codex_quota_observer("coalesced");
        } else {
            if queue.pending.len() >= MAX_PENDING {
                crate::metrics::record_codex_quota_observer("queue_full");
                return false;
            }
            let Some(memory) = process_resident_memory().reserve(JOB_CHARGE) else {
                crate::metrics::record_codex_quota_observer("capacity");
                return false;
            };
            queue.pending.push_back(CodexQuotaJob {
                target: target.clone(),
                snapshot,
                _memory: memory,
            });
            crate::metrics::record_codex_quota_observer("queued");
        }
        if queue.running {
            return false;
        }
        queue.running = true;
        true
    }

    fn take_batch(&self, shutdown: bool) -> Vec<CodexQuotaJob> {
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        if shutdown {
            queue.pending.clear();
        }
        if queue.pending.is_empty() {
            queue.running = false;
        }
        queue.pending.drain(..).collect()
    }
}

impl ServerStateInner {
    pub(crate) fn observe_codex_quota(
        self: &Arc<Self>,
        target: &CodexQuotaTarget,
        snapshot: CodexQuotaSnapshot,
    ) {
        if *self.shutdown.borrow() || !self.codex_quota_observer.enqueue(target, snapshot) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let Some(state) = weak.upgrade() else {
                    return;
                };
                let jobs = state
                    .codex_quota_observer
                    .take_batch(*state.shutdown.borrow());
                if jobs.is_empty() {
                    return;
                }
                for attempt in 0..3 {
                    match state.commit_codex_quota_batch(&jobs).await {
                        Ok(changed) => {
                            crate::metrics::record_codex_quota_observer(if changed {
                                "persisted"
                            } else {
                                "stale"
                            });
                            break;
                        }
                        Err(error) => {
                            crate::metrics::record_codex_quota_observer("persist_failed");
                            tracing::warn!(%error, attempt, "passive Codex quota batch was not committed");
                            if attempt == 2 || *state.shutdown.borrow() {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(
                                100 * (attempt + 1),
                            ))
                            .await;
                        }
                    }
                }
                // Releasing the jobs also releases the parent reservations.
                // A failed batch leaves authoritative queries as the fallback.
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::codex_quota::frame_snapshot;
    use crate::domain::providers::{model::AppKind, registry::ProviderKey};
    use serde_json::json;

    #[test]
    fn codex_r4_quota_queue_is_bounded_merges_partial_fields_and_releases_on_shutdown() {
        let observer = CodexQuotaObserver {
            enabled: true,
            queue: Mutex::new(Queue::default()),
            ..Default::default()
        };
        let target = CodexQuotaTarget {
            provider_key: ProviderKey::new(AppKind::Codex, "p").unwrap(),
            provider_revision: 1,
            account_id: "a".into(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
        };
        let first = frame_snapshot(
            &json!({"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":0}}}),
            10,
        )
        .unwrap();
        let next = frame_snapshot(
            &json!({"type":"codex.rate_limits","rate_limits":{"secondary":{"used_percent":20}}}),
            11,
        )
        .unwrap();
        assert!(observer.enqueue(&target, first.clone()));
        assert!(!observer.enqueue(&target, next));
        for index in 1..MAX_PENDING {
            let mut target = target.clone();
            target.account_id = format!("a-{index}");
            assert!(!observer.enqueue(&target, first.clone()));
        }
        let mut overflow = target.clone();
        overflow.account_id = "overflow".into();
        assert!(!observer.enqueue(&overflow, first));
        let jobs = observer.take_batch(false);
        assert_eq!(jobs.len(), MAX_PENDING);
        assert_eq!(
            jobs[0]
                .snapshot
                .primary
                .used_percent
                .as_ref()
                .unwrap()
                .value,
            0.0
        );
        assert_eq!(
            jobs[0]
                .snapshot
                .secondary
                .used_percent
                .as_ref()
                .unwrap()
                .value,
            20.0
        );
        drop(jobs);
        let snapshot =
            frame_snapshot(&json!({"type":"codex.rate_limits","plan_type":"pro"}), 12).unwrap();
        assert!(!observer.enqueue(&target, snapshot.clone()));
        assert!(observer.take_batch(true).is_empty());
        assert!(!observer.queue.lock().unwrap().running);
        let disabled = CodexQuotaObserver {
            enabled: false,
            ..Default::default()
        };
        assert!(!disabled.enqueue(&target, snapshot));
        assert!(disabled.queue.lock().unwrap().pending.is_empty());
    }
}
