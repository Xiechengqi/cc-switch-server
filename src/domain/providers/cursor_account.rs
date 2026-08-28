use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::domain::accounts::store::AccountQuota;

use super::registry::ProviderKey;

pub const CURSOR_ACCOUNT_CACHE_TTL_MS: u64 = 5 * 60 * 1_000;
pub const CURSOR_ACCOUNT_STALE_TTL_MS: u64 = 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CursorAccountCacheKey {
    pub provider_key: ProviderKey,
    pub credential_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorAccountSnapshotSource {
    Live,
    FreshCache,
    StaleCache,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorAccountSnapshotStatus {
    Complete,
    Partial,
    Stale,
    Error,
    Unconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorAccountSectionState {
    Available,
    Stale,
    Error,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorAccountErrorKind {
    Authentication,
    RateLimited,
    Transient,
    InvalidResponse,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorAccountSection<T> {
    pub state: CursorAccountSectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<CursorAccountErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl<T> CursorAccountSection<T> {
    pub fn available(data: T, observed_at_ms: i64) -> Self {
        Self {
            state: CursorAccountSectionState::Available,
            data: Some(data),
            observed_at_ms: Some(observed_at_ms),
            stale_since_ms: None,
            error_kind: None,
            reason: None,
            retry_after_ms: None,
        }
    }

    pub fn stale(
        data: T,
        observed_at_ms: i64,
        error_kind: CursorAccountErrorKind,
        reason: impl Into<String>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        Self {
            state: CursorAccountSectionState::Stale,
            data: Some(data),
            observed_at_ms: Some(observed_at_ms),
            stale_since_ms: Some(observed_at_ms.saturating_add(CURSOR_ACCOUNT_CACHE_TTL_MS as i64)),
            error_kind: Some(error_kind),
            reason: Some(reason.into()),
            retry_after_ms,
        }
    }

    pub fn error(error_kind: CursorAccountErrorKind, reason: impl Into<String>) -> Self {
        Self {
            state: CursorAccountSectionState::Error,
            data: None,
            observed_at_ms: None,
            stale_since_ms: None,
            error_kind: Some(error_kind),
            reason: Some(reason.into()),
            retry_after_ms: None,
        }
    }

    pub fn unconfigured(reason: impl Into<String>) -> Self {
        Self {
            state: CursorAccountSectionState::Unavailable,
            data: None,
            observed_at_ms: None,
            stale_since_ms: None,
            error_kind: Some(CursorAccountErrorKind::NotConfigured),
            reason: Some(reason.into()),
            retry_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorAccountView {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorAccountSnapshot {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub credential_generation: u64,
    pub source: CursorAccountSnapshotSource,
    pub status: CursorAccountSnapshotStatus,
    pub account: CursorAccountSection<CursorAccountView>,
    pub quota: CursorAccountSection<AccountQuota>,
}

pub fn snapshot_status<T, U>(
    account: &CursorAccountSection<T>,
    quota: &CursorAccountSection<U>,
) -> CursorAccountSnapshotStatus {
    use CursorAccountSectionState::{Available, Error, Stale, Unavailable};
    match (account.state, quota.state) {
        (Available, Available) => CursorAccountSnapshotStatus::Complete,
        (Unavailable, Unavailable) => CursorAccountSnapshotStatus::Unconfigured,
        (Error, Error) | (Error, Unavailable) | (Unavailable, Error) => {
            CursorAccountSnapshotStatus::Error
        }
        (Stale, _) | (_, Stale) => CursorAccountSnapshotStatus::Stale,
        _ => CursorAccountSnapshotStatus::Partial,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CursorAccountObserved<T> {
    pub data: T,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct CursorAccountCacheEntry {
    account: Option<CursorAccountObserved<CursorAccountView>>,
    quota: Option<CursorAccountObserved<AccountQuota>>,
}

#[derive(Debug, Default)]
pub struct CursorAccountCache {
    entries: BTreeMap<CursorAccountCacheKey, CursorAccountCacheEntry>,
}

impl CursorAccountCache {
    pub fn fresh(
        &self,
        key: &CursorAccountCacheKey,
        now_ms: i64,
    ) -> Option<(
        CursorAccountObserved<CursorAccountView>,
        CursorAccountObserved<AccountQuota>,
    )> {
        let entry = self.entries.get(key)?;
        let account = entry.account.as_ref()?;
        let quota = entry.quota.as_ref()?;
        if !is_within(account.observed_at_ms, now_ms, CURSOR_ACCOUNT_CACHE_TTL_MS)
            || !is_within(quota.observed_at_ms, now_ms, CURSOR_ACCOUNT_CACHE_TTL_MS)
        {
            return None;
        }
        Some((account.clone(), quota.clone()))
    }

    pub fn stale_account(
        &self,
        key: &CursorAccountCacheKey,
        now_ms: i64,
    ) -> Option<CursorAccountObserved<CursorAccountView>> {
        self.entries
            .get(key)?
            .account
            .as_ref()
            .filter(|value| is_within(value.observed_at_ms, now_ms, CURSOR_ACCOUNT_STALE_TTL_MS))
            .cloned()
    }

    pub fn stale_quota(
        &self,
        key: &CursorAccountCacheKey,
        now_ms: i64,
    ) -> Option<CursorAccountObserved<AccountQuota>> {
        self.entries
            .get(key)?
            .quota
            .as_ref()
            .filter(|value| is_within(value.observed_at_ms, now_ms, CURSOR_ACCOUNT_STALE_TTL_MS))
            .cloned()
    }

    pub fn insert_account(
        &mut self,
        key: CursorAccountCacheKey,
        data: CursorAccountView,
        observed_at_ms: i64,
    ) {
        self.retain_current(&key);
        self.entries.entry(key).or_default().account = Some(CursorAccountObserved {
            data,
            observed_at_ms,
        });
    }

    pub fn insert_quota(
        &mut self,
        key: CursorAccountCacheKey,
        data: AccountQuota,
        observed_at_ms: i64,
    ) {
        self.retain_current(&key);
        self.entries.entry(key).or_default().quota = Some(CursorAccountObserved {
            data,
            observed_at_ms,
        });
    }

    pub fn clear(&mut self, key: &CursorAccountCacheKey) {
        self.entries.remove(key);
    }

    pub fn retain_current(&mut self, key: &CursorAccountCacheKey) {
        self.entries
            .retain(|candidate, _| candidate.provider_key != key.provider_key || candidate == key);
    }

    pub fn retain_keys(&mut self, keys: &BTreeSet<CursorAccountCacheKey>) {
        self.entries.retain(|key, _| keys.contains(key));
    }
}

fn is_within(observed_at_ms: i64, now_ms: i64, ttl_ms: u64) -> bool {
    now_ms
        .checked_sub(observed_at_ms)
        .is_some_and(|age| age >= 0 && age <= ttl_ms.min(i64::MAX as u64) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::AppKind;

    fn key(generation: u64) -> CursorAccountCacheKey {
        CursorAccountCacheKey {
            provider_key: ProviderKey::new(AppKind::Codex, "cursor-key").unwrap(),
            credential_generation: generation,
        }
    }

    #[test]
    fn cache_fences_credential_generations() {
        let mut cache = CursorAccountCache::default();
        let account = |id: &str| CursorAccountView {
            account_id: id.to_string(),
            email: None,
            display_name: None,
            credential_name: None,
            subscription_level: None,
        };
        cache.insert_account(key(1), account("old"), 1_000);
        cache.insert_account(key(2), account("new"), 2_000);
        assert!(cache.stale_account(&key(1), 2_000).is_none());
        assert_eq!(
            cache.stale_account(&key(2), 2_000).unwrap().data.account_id,
            "new"
        );
    }

    #[test]
    fn status_preserves_partial_account_information() {
        let account = CursorAccountSection::available((), 1);
        let quota = CursorAccountSection::<()>::error(
            CursorAccountErrorKind::Transient,
            "usage unavailable",
        );
        assert_eq!(
            snapshot_status(&account, &quota),
            CursorAccountSnapshotStatus::Partial
        );
    }
}
