use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::domain::providers::model::AppKind;
use crate::domain::providers::registry::{provider_registry, OperationSupport};
use crate::domain::providers::runtime::ProviderRuntimePlan;
use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::infra::time::now_ms;

const PROVIDER_HEALTH_FILE_NAME: &str = "provider-health.json";
const PROVIDER_HEALTH_SCHEMA_VERSION: u8 = 1;
pub const PROVIDER_HEALTH_STALE_AFTER_MS: u128 = 65 * 60 * 1000;
pub const PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS: u128 = 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRequestOutcome {
    Success { status_code: u16 },
    Failure { status_code: u16 },
    RateLimited { status_code: u16 },
    NetworkFailure,
}

impl ProviderRequestOutcome {
    pub fn from_status(status_code: u16) -> Self {
        if status_code == 429 || (500..=599).contains(&status_code) {
            Self::Failure { status_code }
        } else {
            Self::Success { status_code }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

impl ProviderHealthStatus {
    pub fn is_success(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProbeSupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct ProviderHealthObservation {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub status: ProviderHealthStatus,
    pub checked_at_ms: u128,
    pub source: String,
    pub status_code: Option<u16>,
    pub latency_ms: Option<u64>,
    pub model: Option<String>,
    pub error_category: Option<String>,
    pub error_message: Option<String>,
    pub transient_failure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealthSnapshot {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub status: ProviderHealthStatus,
    pub checked_at_ms: u128,
    pub stale_at_ms: u128,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub consecutive_successes: u32,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default = "default_true")]
    pub effective_available: bool,
    #[serde(default)]
    pub confirmation_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealthStore {
    #[serde(default = "provider_health_schema_version")]
    pub schema_version: u8,
    #[serde(default)]
    snapshots: BTreeMap<String, ProviderHealthSnapshot>,
}

impl Default for ProviderHealthStore {
    fn default() -> Self {
        Self {
            schema_version: PROVIDER_HEALTH_SCHEMA_VERSION,
            snapshots: BTreeMap::new(),
        }
    }
}

impl ProviderHealthStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = provider_health_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("read Provider health store {}", path.display()))?;
        let store: Self = serde_json::from_str(&content)
            .with_context(|| format!("parse Provider health store {}", path.display()))?;
        anyhow::ensure!(
            store.schema_version == PROVIDER_HEALTH_SCHEMA_VERSION,
            "unsupported Provider health schema version {}",
            store.schema_version
        );
        Ok(store)
    }

    pub fn load_rebuildable(config_dir: &Path) -> Self {
        match Self::load_or_default(config_dir) {
            Ok(store) => store,
            Err(error) => {
                tracing::warn!(error = %error, "ignored invalid rebuildable Provider health store");
                Self::default()
            }
        }
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        let path = provider_health_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write Provider health store {}", path.display()))
    }

    pub fn record(&mut self, observation: ProviderHealthObservation) -> ProviderHealthSnapshot {
        debug_assert_ne!(observation.status, ProviderHealthStatus::Unknown);
        let key = provider_health_key(observation.app, &observation.provider_id);
        if let Some(existing) = self.snapshots.get(&key) {
            if existing.checked_at_ms > observation.checked_at_ms {
                return existing.clone();
            }
            if !observation.status.is_success()
                && observation.transient_failure
                && existing.confirmation_pending
                && existing.provider_revision == observation.provider_revision
                && existing.runtime_fingerprint == observation.runtime_fingerprint
                && existing.stale_at_ms > observation.checked_at_ms
                && observation.checked_at_ms
                    < existing
                        .checked_at_ms
                        .saturating_add(PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS)
            {
                return existing.clone();
            }
        }

        let previous = self.snapshots.get(&key).filter(|snapshot| {
            snapshot.provider_revision == observation.provider_revision
                && snapshot.runtime_fingerprint == observation.runtime_fingerprint
                && snapshot.stale_at_ms > observation.checked_at_ms
        });
        let successful = observation.status.is_success();
        let consecutive_successes = if successful {
            previous
                .map(|snapshot| snapshot.consecutive_successes)
                .unwrap_or_default()
                .saturating_add(1)
        } else {
            0
        };
        let consecutive_failures = if successful {
            0
        } else {
            previous
                .map(|snapshot| snapshot.consecutive_failures)
                .unwrap_or_default()
                .saturating_add(1)
        };
        let confirmation_pending =
            !successful && observation.transient_failure && consecutive_failures < 2;
        let effective_available = if successful {
            true
        } else if !observation.transient_failure || consecutive_failures >= 2 {
            false
        } else {
            previous
                .map(|snapshot| snapshot.effective_available)
                .unwrap_or(true)
        };
        let snapshot = ProviderHealthSnapshot {
            app: observation.app,
            provider_id: observation.provider_id,
            provider_revision: observation.provider_revision,
            runtime_fingerprint: observation.runtime_fingerprint,
            status: observation.status,
            checked_at_ms: observation.checked_at_ms,
            stale_at_ms: observation
                .checked_at_ms
                .saturating_add(PROVIDER_HEALTH_STALE_AFTER_MS),
            source: observation.source,
            status_code: observation.status_code,
            latency_ms: observation.latency_ms,
            model: observation.model,
            error_category: observation.error_category,
            error_message: observation.error_message,
            consecutive_successes,
            consecutive_failures,
            effective_available,
            confirmation_pending,
        };
        self.snapshots.insert(key, snapshot.clone());
        snapshot
    }

    pub fn get(&self, app: AppKind, provider_id: &str) -> Option<&ProviderHealthSnapshot> {
        self.snapshots.get(&provider_health_key(app, provider_id))
    }

    pub fn retain_providers(&mut self, providers: &[StoredProvider]) -> bool {
        let before = self.snapshots.len();
        self.snapshots.retain(|_, snapshot| {
            providers.iter().any(|provider| {
                provider.app == snapshot.app && provider.provider.id == snapshot.provider_id
            })
        });
        before != self.snapshots.len()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider_id: String,
    pub app: AppKind,
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub success_rate: Option<f64>,
    pub avg_latency_ms: Option<f64>,
    pub last_status_code: Option<u16>,
    pub last_request_at_ms: Option<u128>,
    pub healthy: bool,
    pub available: bool,
    pub status: ProviderHealthStatus,
    pub probe_support: ProviderProbeSupport,
    pub checked_at_ms: Option<u128>,
    pub stale_at_ms: Option<u128>,
    pub source: Option<String>,
    pub probe_latency_ms: Option<u64>,
    pub model: Option<String>,
    pub error_category: Option<String>,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
    pub confirmation_pending: bool,
    pub reason: Option<String>,
}

pub fn provider_health(provider: &StoredProvider, usage: &UsageStore) -> ProviderHealth {
    provider_health_with_runtime(provider, usage, ProviderProbeSupport::Supported, None)
}

pub fn provider_probe_support(plan: &ProviderRuntimePlan) -> ProviderProbeSupport {
    provider_probe_support_for_driver(plan.driver_id.as_str())
}

fn provider_probe_support_for_driver(driver_id: &str) -> ProviderProbeSupport {
    provider_registry()
        .drivers
        .iter()
        .find(|driver| driver.driver_id.as_str() == driver_id)
        .map(|driver| {
            if driver.operations.test == OperationSupport::Supported {
                ProviderProbeSupport::Supported
            } else {
                ProviderProbeSupport::Unsupported
            }
        })
        .unwrap_or(ProviderProbeSupport::Unsupported)
}

pub fn provider_health_for_plan(
    provider: &StoredProvider,
    usage: &UsageStore,
    plan: Option<&ProviderRuntimePlan>,
) -> ProviderHealth {
    let probe_support = plan
        .map(provider_probe_support)
        .unwrap_or(ProviderProbeSupport::Supported);
    provider_health_with_runtime(
        provider,
        usage,
        probe_support,
        plan.map(|plan| plan.runtime_fingerprint.as_str()),
    )
}

fn provider_health_with_runtime(
    provider: &StoredProvider,
    usage: &UsageStore,
    probe_support: ProviderProbeSupport,
    runtime_fingerprint: Option<&str>,
) -> ProviderHealth {
    let logs = usage.logs.iter().filter(|log| {
        !log.is_health_check && log.provider_id == provider.provider.id && log.app == provider.app
    });
    health_from_logs(
        provider,
        logs,
        &usage.provider_health,
        probe_support,
        runtime_fingerprint,
        now_ms(),
    )
}

fn health_from_logs<'a>(
    provider: &StoredProvider,
    logs: impl Iterator<Item = &'a UsageLog>,
    store: &ProviderHealthStore,
    probe_support: ProviderProbeSupport,
    runtime_fingerprint: Option<&str>,
    now: u128,
) -> ProviderHealth {
    let mut requests = 0_u64;
    let mut successes = 0_u64;
    let mut failures = 0_u64;
    let mut latency_total = 0_u128;
    let mut last_request_at_ms = None;

    for log in logs {
        requests = requests.saturating_add(1);
        latency_total = latency_total.saturating_add(log.duration_ms);
        if (200..400).contains(&log.status_code) {
            successes = successes.saturating_add(1);
        } else {
            failures = failures.saturating_add(1);
        }
        if last_request_at_ms.is_none_or(|value| log.created_at_ms >= value) {
            last_request_at_ms = Some(log.created_at_ms);
        }
    }

    let snapshot = store.get(provider.app, &provider.provider.id);
    let current = snapshot.filter(|snapshot| {
        probe_support == ProviderProbeSupport::Supported
            && snapshot.provider_revision == provider.resource.revision
            && runtime_fingerprint
                .is_none_or(|fingerprint| snapshot.runtime_fingerprint == fingerprint)
            && snapshot.checked_at_ms <= now
            && snapshot.stale_at_ms > now
    });
    let status = current
        .map(|snapshot| snapshot.status)
        .unwrap_or(ProviderHealthStatus::Unknown);
    let reason = if probe_support == ProviderProbeSupport::Unsupported {
        Some("provider driver does not support model health probes".to_string())
    } else if let Some(snapshot) = current {
        snapshot.error_message.clone()
    } else if snapshot
        .is_some_and(|snapshot| snapshot.provider_revision != provider.resource.revision)
    {
        Some("provider changed since its last health check".to_string())
    } else if snapshot.is_some_and(|snapshot| {
        runtime_fingerprint.is_some_and(|fingerprint| snapshot.runtime_fingerprint != fingerprint)
    }) {
        Some("provider runtime changed since its last health check".to_string())
    } else if snapshot.is_some() {
        Some("provider health check is stale".to_string())
    } else {
        Some("provider has not been health checked".to_string())
    };
    let healthy = status != ProviderHealthStatus::Unhealthy;
    let available = current
        .map(|snapshot| snapshot.effective_available)
        .unwrap_or(true);

    ProviderHealth {
        provider_id: provider.provider.id.clone(),
        app: provider.app,
        requests,
        successes,
        failures,
        success_rate: (requests > 0).then_some(successes as f64 / requests as f64),
        avg_latency_ms: (requests > 0).then_some(latency_total as f64 / requests as f64),
        last_status_code: current.and_then(|snapshot| snapshot.status_code),
        last_request_at_ms,
        healthy,
        available,
        status,
        probe_support,
        checked_at_ms: snapshot.map(|snapshot| snapshot.checked_at_ms),
        stale_at_ms: snapshot.map(|snapshot| snapshot.stale_at_ms),
        source: snapshot.map(|snapshot| snapshot.source.clone()),
        probe_latency_ms: snapshot.and_then(|snapshot| snapshot.latency_ms),
        model: snapshot.and_then(|snapshot| snapshot.model.clone()),
        error_category: snapshot.and_then(|snapshot| snapshot.error_category.clone()),
        consecutive_successes: current
            .map(|snapshot| snapshot.consecutive_successes)
            .unwrap_or_default(),
        consecutive_failures: current
            .map(|snapshot| snapshot.consecutive_failures)
            .unwrap_or_default(),
        confirmation_pending: current.is_some_and(|snapshot| snapshot.confirmation_pending),
        reason,
    }
}

pub fn provider_health_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PROVIDER_HEALTH_FILE_NAME)
}

fn provider_health_key(app: AppKind, provider_id: &str) -> String {
    format!("{}:{provider_id}", app.as_str())
}

const fn provider_health_schema_version() -> u8 {
    PROVIDER_HEALTH_SCHEMA_VERSION
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::providers::store::ProviderResourceMetadata;
    use crate::domain::usage::store::{UsageLog, UsageModelMetadata};

    use super::*;

    fn provider() -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "p1".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
            resource: ProviderResourceMetadata {
                revision: 3,
                ..Default::default()
            },
        }
    }

    fn observation(status: ProviderHealthStatus, checked_at_ms: u128) -> ProviderHealthObservation {
        ProviderHealthObservation {
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_revision: 3,
            runtime_fingerprint: "runtime-1".to_string(),
            status,
            checked_at_ms,
            source: "test".to_string(),
            status_code: status.is_success().then_some(200),
            latency_ms: Some(42),
            model: Some("gpt-test".to_string()),
            error_category: (!status.is_success()).then(|| "network".to_string()),
            error_message: (!status.is_success()).then(|| "failed".to_string()),
            transient_failure: !status.is_success(),
        }
    }

    #[test]
    fn latest_probe_controls_health_and_success_recovers_immediately() {
        let provider = provider();
        let mut usage = UsageStore::default();
        usage
            .provider_health
            .record(observation(ProviderHealthStatus::Unhealthy, 1_000));
        let failed = health_from_logs(
            &provider,
            std::iter::empty(),
            &usage.provider_health,
            ProviderProbeSupport::Supported,
            None,
            1_001,
        );
        assert_eq!(failed.status, ProviderHealthStatus::Unhealthy);
        assert_eq!(failed.consecutive_failures, 1);
        assert!(failed.available);

        usage
            .provider_health
            .record(observation(ProviderHealthStatus::Healthy, 2_000));
        let recovered = health_from_logs(
            &provider,
            std::iter::empty(),
            &usage.provider_health,
            ProviderProbeSupport::Supported,
            None,
            2_001,
        );
        assert_eq!(recovered.status, ProviderHealthStatus::Healthy);
        assert_eq!(recovered.consecutive_failures, 0);
        assert!(recovered.available);
    }

    #[test]
    fn second_transient_failure_makes_provider_unavailable() {
        let mut store = ProviderHealthStore::default();
        let first = store.record(observation(ProviderHealthStatus::Unhealthy, 1_000));
        assert!(first.effective_available);
        assert!(first.confirmation_pending);
        let second = store.record(observation(
            ProviderHealthStatus::Unhealthy,
            1_000 + PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS,
        ));
        assert!(!second.effective_available);
        assert!(!second.confirmation_pending);
    }

    #[test]
    fn transient_failure_cannot_confirm_before_the_confirmation_window() {
        let mut store = ProviderHealthStore::default();
        let first = store.record(observation(ProviderHealthStatus::Unhealthy, 1_000));
        let early = store.record(observation(
            ProviderHealthStatus::Unhealthy,
            1_000 + PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS - 1,
        ));

        assert_eq!(early, first);
        assert_eq!(early.consecutive_failures, 1);
        assert!(early.effective_available);
        assert!(early.confirmation_pending);
    }

    #[test]
    fn stale_failure_does_not_confirm_a_new_transient_failure() {
        let mut store = ProviderHealthStore::default();
        store.record(observation(ProviderHealthStatus::Unhealthy, 1_000));

        let next = store.record(observation(
            ProviderHealthStatus::Unhealthy,
            1_000 + PROVIDER_HEALTH_STALE_AFTER_MS + 1,
        ));

        assert_eq!(next.consecutive_failures, 1);
        assert!(next.effective_available);
        assert!(next.confirmation_pending);
    }

    #[test]
    fn stale_or_revision_mismatched_snapshot_is_unknown_and_fail_open() {
        let mut store = ProviderHealthStore::default();
        store.record(observation(ProviderHealthStatus::Healthy, 1_000));
        let provider = provider();
        let stale = health_from_logs(
            &provider,
            std::iter::empty(),
            &store,
            ProviderProbeSupport::Supported,
            None,
            1_000 + PROVIDER_HEALTH_STALE_AFTER_MS,
        );
        assert_eq!(stale.status, ProviderHealthStatus::Unknown);
        assert!(stale.available);

        let mut changed = provider.clone();
        changed.resource.revision = 4;
        let mismatched = health_from_logs(
            &changed,
            std::iter::empty(),
            &store,
            ProviderProbeSupport::Supported,
            None,
            2_000,
        );
        assert_eq!(mismatched.status, ProviderHealthStatus::Unknown);
        assert!(mismatched.available);

        let runtime_mismatched = health_from_logs(
            &provider,
            std::iter::empty(),
            &store,
            ProviderProbeSupport::Supported,
            Some("runtime-2"),
            2_000,
        );
        assert_eq!(runtime_mismatched.status, ProviderHealthStatus::Unknown);
        assert!(runtime_mismatched.available);
        assert_eq!(
            runtime_mismatched.reason.as_deref(),
            Some("provider runtime changed since its last health check")
        );
    }

    #[test]
    fn health_checks_do_not_pollute_business_request_statistics() {
        let provider = provider();
        let mut usage = UsageStore::default();
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "p1".to_string(),
            ProviderType::Codex,
            599,
            100,
            UsageModelMetadata::default(),
            Default::default(),
        );
        log.is_health_check = true;
        usage.logs.push(log);
        let health = provider_health(&provider, &usage);
        assert_eq!(health.requests, 0);
        assert_eq!(health.status, ProviderHealthStatus::Unknown);
    }

    #[test]
    fn terminal_failure_is_unavailable_without_confirmation() {
        let mut terminal = observation(ProviderHealthStatus::Unhealthy, 1_000);
        terminal.transient_failure = false;
        terminal.error_category = Some("auth".to_string());
        let snapshot = ProviderHealthStore::default().record(terminal);
        assert!(!snapshot.effective_available);
        assert!(!snapshot.confirmation_pending);
    }

    #[test]
    fn unsupported_drivers_are_reported_without_probing() {
        assert_eq!(
            provider_probe_support_for_driver("special.kiro"),
            ProviderProbeSupport::Unsupported
        );
        assert_eq!(
            provider_probe_support_for_driver("http.openai_responses"),
            ProviderProbeSupport::Supported
        );
        assert_eq!(
            provider_probe_support_for_driver("missing.driver"),
            ProviderProbeSupport::Unsupported
        );
    }

    #[test]
    fn provider_health_store_round_trips_separate_snapshot_file() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-provider-health-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut store = ProviderHealthStore::default();
        let expected = store.record(observation(ProviderHealthStatus::Healthy, now_ms()));
        store.save(&dir).unwrap();

        let loaded = ProviderHealthStore::load_or_default(&dir).unwrap();
        assert_eq!(loaded.get(AppKind::Codex, "p1"), Some(&expected));

        fs::remove_dir_all(dir).unwrap();
    }
}
