use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::domain::providers::model::ProviderType;

#[derive(Debug, Default)]
pub struct ShareInFlightTracker {
    counts: Mutex<BTreeMap<String, u32>>,
}

#[derive(Debug)]
pub struct ShareInFlightGuard {
    tracker: Arc<ShareInFlightTracker>,
    share_id: String,
    user_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareInFlightAcquireError {
    ShareLimit { current: u32, limit: u32 },
    UserLimit { current: u32, limit: u32 },
}

#[derive(Debug, Default)]
pub struct AccountInFlightTracker {
    pub(super) counts: Mutex<AccountInFlightCounts>,
}

#[derive(Debug, Default)]
pub(super) struct AccountInFlightCounts {
    by_account: BTreeMap<String, u32>,
    pub(super) by_provider_type: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Default)]
pub struct AccountInFlightSnapshot {
    counts: BTreeMap<String, u32>,
}

#[derive(Debug)]
pub struct AccountInFlightGuard {
    tracker: Arc<AccountInFlightTracker>,
    key: String,
    provider_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountInFlightAcquireError {
    pub current: u32,
    pub limit: u32,
}

impl ShareInFlightTracker {
    pub fn has_in_flight(&self, share_id: &str) -> bool {
        self.counts
            .lock()
            .map(|counts| counts.get(share_id).is_some_and(|count| *count > 0))
            .unwrap_or(true)
    }

    pub fn try_acquire(
        self: &Arc<Self>,
        share_id: &str,
        parallel_limit: Option<u32>,
    ) -> Option<ShareInFlightGuard> {
        self.try_acquire_for_user(share_id, parallel_limit, None, None)
            .ok()
    }

    pub fn try_acquire_for_user(
        self: &Arc<Self>,
        share_id: &str,
        parallel_limit: Option<u32>,
        user_email: Option<&str>,
        user_parallel_limit: Option<u32>,
    ) -> Result<ShareInFlightGuard, ShareInFlightAcquireError> {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = *counts.get(share_id).unwrap_or(&0);
        if let Some(limit) = parallel_limit.filter(|limit| current >= *limit) {
            return Err(ShareInFlightAcquireError::ShareLimit { current, limit });
        }
        let user_key =
            user_email.map(|email| format!("{share_id}\u{1f}{}", email.to_ascii_lowercase()));
        if let Some(user_key) = user_key.as_deref() {
            let user_current = *counts.get(user_key).unwrap_or(&0);
            if let Some(limit) = user_parallel_limit.filter(|limit| user_current >= *limit) {
                return Err(ShareInFlightAcquireError::UserLimit {
                    current: user_current,
                    limit,
                });
            }
        }
        counts.insert(share_id.to_string(), current.saturating_add(1));
        if let Some(user_key) = user_key.as_deref() {
            let current = *counts.get(user_key).unwrap_or(&0);
            counts.insert(user_key.to_string(), current.saturating_add(1));
        }
        Ok(ShareInFlightGuard {
            tracker: self.clone(),
            share_id: share_id.to_string(),
            user_key,
        })
    }

    fn release(&self, share_id: &str, user_key: Option<&str>) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        for key in std::iter::once(share_id).chain(user_key) {
            let Some(current) = counts.get_mut(key) else {
                continue;
            };
            if *current <= 1 {
                counts.remove(key);
            } else {
                *current -= 1;
            }
        }
    }
}

impl Drop for ShareInFlightGuard {
    fn drop(&mut self) {
        self.tracker
            .release(&self.share_id, self.user_key.as_deref());
    }
}

impl AccountInFlightTracker {
    pub fn snapshot(&self) -> AccountInFlightSnapshot {
        let counts = self
            .counts
            .lock()
            .map(|counts| counts.by_account.clone())
            .unwrap_or_default();
        AccountInFlightSnapshot { counts }
    }

    pub fn try_acquire(
        self: &Arc<Self>,
        provider_type: ProviderType,
        account_id: &str,
        max_concurrent: u32,
    ) -> Result<AccountInFlightGuard, AccountInFlightAcquireError> {
        let key = account_in_flight_key(provider_type, account_id);
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = *counts.by_account.get(&key).unwrap_or(&0);
        if current >= max_concurrent {
            crate::metrics::record_account_lease(provider_type.as_str(), "rejected");
            return Err(AccountInFlightAcquireError {
                current,
                limit: max_concurrent,
            });
        }
        let next = current.saturating_add(1);
        counts.by_account.insert(key.clone(), next);
        crate::metrics::record_account_lease(provider_type.as_str(), "acquired");
        let provider_type_label = provider_type.as_str().to_string();
        let provider_total = counts
            .by_provider_type
            .entry(provider_type_label.clone())
            .or_default();
        *provider_total = provider_total.saturating_add(1);
        crate::metrics::record_account_inflight(&provider_type_label, *provider_total);
        Ok(AccountInFlightGuard {
            tracker: self.clone(),
            key,
            provider_type: provider_type_label,
        })
    }

    fn release(&self, key: &str, provider_type: &str) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        let Some(current) = counts.by_account.get_mut(key) else {
            return;
        };
        let next = current.saturating_sub(1);
        if next == 0 {
            counts.by_account.remove(key);
        } else {
            *current = next;
        }
        let provider_total = counts
            .by_provider_type
            .get_mut(provider_type)
            .map(|current| {
                *current = current.saturating_sub(1);
                *current
            })
            .unwrap_or_default();
        if provider_total == 0 {
            counts.by_provider_type.remove(provider_type);
        }
        crate::metrics::record_account_inflight(provider_type, provider_total);
    }
}

impl AccountInFlightSnapshot {
    pub fn current(&self, provider_type: ProviderType, account_id: &str) -> u32 {
        self.counts
            .get(&account_in_flight_key(provider_type, account_id))
            .copied()
            .unwrap_or_default()
    }
}

impl Drop for AccountInFlightGuard {
    fn drop(&mut self) {
        self.tracker.release(&self.key, &self.provider_type);
    }
}

fn account_in_flight_key(provider_type: ProviderType, account_id: &str) -> String {
    format!("{}:{account_id}", provider_type.as_str())
}
