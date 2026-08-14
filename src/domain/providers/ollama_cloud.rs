use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::registry::ProviderKey;

pub const OLLAMA_CLOUD_CACHE_TTL_MS: u64 = 5 * 60 * 1_000;
pub const OLLAMA_CLOUD_STALE_TTL_MS: u64 = 60 * 60 * 1_000;
pub const OLLAMA_CLOUD_MAX_MODELS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OllamaCloudCacheKey {
    pub credential_source_key: ProviderKey,
    pub credential_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaCloudSnapshotSource {
    Live,
    FreshCache,
    StaleCache,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaCloudSnapshotStatus {
    Complete,
    Partial,
    Stale,
    Error,
    Unconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaCloudSectionState {
    Available,
    Stale,
    Error,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaCloudErrorKind {
    Authentication,
    RateLimited,
    Transient,
    InvalidResponse,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudSection<T> {
    pub state: OllamaCloudSectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<OllamaCloudErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl<T> OllamaCloudSection<T> {
    pub fn available(data: T, observed_at_ms: i64) -> Self {
        Self {
            state: OllamaCloudSectionState::Available,
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
        error_kind: OllamaCloudErrorKind,
        reason: impl Into<String>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        Self {
            state: OllamaCloudSectionState::Stale,
            data: Some(data),
            observed_at_ms: Some(observed_at_ms),
            stale_since_ms: Some(observed_at_ms.saturating_add(OLLAMA_CLOUD_CACHE_TTL_MS as i64)),
            error_kind: Some(error_kind),
            reason: Some(reason.into()),
            retry_after_ms,
        }
    }

    pub fn error(
        error_kind: OllamaCloudErrorKind,
        reason: impl Into<String>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        Self {
            state: OllamaCloudSectionState::Error,
            data: None,
            observed_at_ms: None,
            stale_since_ms: None,
            error_kind: Some(error_kind),
            reason: Some(reason.into()),
            retry_after_ms,
        }
    }

    pub fn unconfigured(reason: impl Into<String>) -> Self {
        Self {
            state: OllamaCloudSectionState::Unavailable,
            data: None,
            observed_at_ms: None,
            stale_since_ms: None,
            error_kind: Some(OllamaCloudErrorKind::NotConfigured),
            reason: Some(reason.into()),
            retry_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudSnapshot {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub credential_source_key: ProviderKey,
    pub credential_generation: u64,
    pub source: OllamaCloudSnapshotSource,
    pub status: OllamaCloudSnapshotStatus,
    pub account: OllamaCloudSection<OllamaCloudAccountView>,
    pub usage: OllamaCloudSection<OllamaCloudUsageView>,
}

impl OllamaCloudSnapshot {
    pub fn observed_at_ms(&self) -> Option<i64> {
        self.account
            .observed_at_ms
            .into_iter()
            .chain(self.usage.observed_at_ms)
            .max()
    }
}

pub fn snapshot_status<T, U>(
    account: &OllamaCloudSection<T>,
    usage: &OllamaCloudSection<U>,
) -> OllamaCloudSnapshotStatus {
    use OllamaCloudSectionState::{Available, Error, Stale, Unavailable};
    match (account.state, usage.state) {
        (Available, Available) => OllamaCloudSnapshotStatus::Complete,
        (Unavailable, Unavailable) => OllamaCloudSnapshotStatus::Unconfigured,
        (Error, Error) | (Error, Unavailable) | (Unavailable, Error) => {
            OllamaCloudSnapshotStatus::Error
        }
        (Stale, Stale) | (Stale, Available) | (Available, Stale) => {
            OllamaCloudSnapshotStatus::Stale
        }
        _ => OllamaCloudSnapshotStatus::Partial,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudAccountView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudUsageView {
    pub limits: Vec<OllamaCloudUsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<OllamaCloudActivityView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaCloudUsageWindowKind {
    Session,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudUsageWindow {
    pub kind: OllamaCloudUsageWindowKind,
    pub utilization: f64,
    pub models: Vec<OllamaCloudModelUsage>,
    #[serde(default)]
    pub models_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudModelUsage {
    pub name: String,
    pub request_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudActivityView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<OllamaCloudActivityPeriod>,
    pub models: Vec<OllamaCloudModelUsage>,
    #[serde(default)]
    pub models_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OllamaCloudActivityPeriod {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OllamaCloudObserved<T> {
    pub data: T,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct OllamaCloudCacheEntry {
    account: Option<OllamaCloudObserved<OllamaCloudAccountView>>,
    usage: Option<OllamaCloudObserved<OllamaCloudUsageView>>,
}

#[derive(Debug, Default)]
pub struct OllamaCloudCache {
    entries: BTreeMap<OllamaCloudCacheKey, OllamaCloudCacheEntry>,
}

impl OllamaCloudCache {
    pub fn fresh_account(
        &self,
        key: &OllamaCloudCacheKey,
        now_ms: i64,
    ) -> Option<OllamaCloudObserved<OllamaCloudAccountView>> {
        self.entries
            .get(key)?
            .account
            .as_ref()
            .filter(|item| is_within(item.observed_at_ms, now_ms, OLLAMA_CLOUD_CACHE_TTL_MS))
            .cloned()
    }

    pub fn fresh_usage(
        &self,
        key: &OllamaCloudCacheKey,
        now_ms: i64,
    ) -> Option<OllamaCloudObserved<OllamaCloudUsageView>> {
        self.entries
            .get(key)?
            .usage
            .as_ref()
            .filter(|item| is_within(item.observed_at_ms, now_ms, OLLAMA_CLOUD_CACHE_TTL_MS))
            .cloned()
    }

    pub fn stale_account(
        &self,
        key: &OllamaCloudCacheKey,
        now_ms: i64,
    ) -> Option<OllamaCloudObserved<OllamaCloudAccountView>> {
        self.entries
            .get(key)?
            .account
            .as_ref()
            .filter(|item| is_within(item.observed_at_ms, now_ms, OLLAMA_CLOUD_STALE_TTL_MS))
            .cloned()
    }

    pub fn stale_usage(
        &self,
        key: &OllamaCloudCacheKey,
        now_ms: i64,
    ) -> Option<OllamaCloudObserved<OllamaCloudUsageView>> {
        self.entries
            .get(key)?
            .usage
            .as_ref()
            .filter(|item| is_within(item.observed_at_ms, now_ms, OLLAMA_CLOUD_STALE_TTL_MS))
            .cloned()
    }

    pub fn insert_account(
        &mut self,
        key: OllamaCloudCacheKey,
        data: OllamaCloudAccountView,
        observed_at_ms: i64,
    ) {
        self.retain_current(&key);
        self.entries.entry(key).or_default().account = Some(OllamaCloudObserved {
            data,
            observed_at_ms,
        });
    }

    pub fn insert_usage(
        &mut self,
        key: OllamaCloudCacheKey,
        data: OllamaCloudUsageView,
        observed_at_ms: i64,
    ) {
        self.retain_current(&key);
        self.entries.entry(key).or_default().usage = Some(OllamaCloudObserved {
            data,
            observed_at_ms,
        });
    }

    pub fn remove_account(&mut self, key: &OllamaCloudCacheKey) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.account = None;
            if entry.usage.is_none() {
                self.entries.remove(key);
            }
        }
    }

    pub fn remove_usage(&mut self, key: &OllamaCloudCacheKey) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.usage = None;
            if entry.account.is_none() {
                self.entries.remove(key);
            }
        }
    }

    pub fn retain_current(&mut self, key: &OllamaCloudCacheKey) {
        self.entries.retain(|candidate, _| {
            candidate.credential_source_key != key.credential_source_key || candidate == key
        });
    }

    pub fn retain_keys(&mut self, keys: &BTreeSet<OllamaCloudCacheKey>) {
        self.entries.retain(|key, _| keys.contains(key));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
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

    fn key(generation: u64) -> OllamaCloudCacheKey {
        OllamaCloudCacheKey {
            credential_source_key: ProviderKey::new(AppKind::Codex, "ollama-bundle").unwrap(),
            credential_generation: generation,
        }
    }

    fn account(id: &str) -> OllamaCloudAccountView {
        OllamaCloudAccountView {
            id: Some(id.to_string()),
            email: None,
            name: None,
            first_name: None,
            last_name: None,
            avatar_url: None,
            plan: Some("free".to_string()),
            created_at_ms: None,
        }
    }

    #[test]
    fn cache_fences_old_credential_generations_and_expires_stale_data() {
        let mut cache = OllamaCloudCache::default();
        cache.insert_account(key(1), account("old"), 1_000);
        cache.insert_account(key(2), account("new"), 2_000);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache
                .fresh_account(&key(2), 2_000)
                .unwrap()
                .data
                .id
                .as_deref(),
            Some("new")
        );
        assert!(cache
            .stale_account(&key(2), 2_000 + OLLAMA_CLOUD_STALE_TTL_MS as i64 + 1)
            .is_none());
    }

    #[test]
    fn status_distinguishes_zero_data_errors_stale_and_unconfigured() {
        let available = OllamaCloudSection::available((), 1);
        let stale = OllamaCloudSection::stale(
            (),
            1,
            OllamaCloudErrorKind::Transient,
            "temporary failure",
            None,
        );
        let error = OllamaCloudSection::<()>::error(
            OllamaCloudErrorKind::Authentication,
            "authentication failed",
            None,
        );
        let unavailable = OllamaCloudSection::<()>::unconfigured("missing key");
        assert_eq!(
            snapshot_status(&available, &available),
            OllamaCloudSnapshotStatus::Complete
        );
        assert_eq!(
            snapshot_status(&available, &error),
            OllamaCloudSnapshotStatus::Partial
        );
        assert_eq!(
            snapshot_status(&available, &stale),
            OllamaCloudSnapshotStatus::Stale
        );
        assert_eq!(
            snapshot_status(&unavailable, &unavailable),
            OllamaCloudSnapshotStatus::Unconfigured
        );
    }
}
