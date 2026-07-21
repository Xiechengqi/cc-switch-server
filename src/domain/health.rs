use serde::Serialize;

use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::infra::time::now_ms;

const RECENT_WINDOW_MS: u128 = 10 * 60 * 1000;

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
    pub reason: Option<String>,
}

pub fn provider_health(provider: &StoredProvider, usage: &UsageStore) -> ProviderHealth {
    let logs = usage
        .logs
        .iter()
        .filter(|log| log.provider_id == provider.provider.id && log.app == provider.app);
    health_from_logs(provider, logs)
}

pub fn provider_health_list(
    providers: &[StoredProvider],
    usage: &UsageStore,
) -> Vec<ProviderHealth> {
    providers
        .iter()
        .map(|provider| provider_health(provider, usage))
        .collect()
}

pub fn is_provider_healthy(provider: &StoredProvider, usage: &UsageStore) -> bool {
    provider_health(provider, usage).healthy
}

fn health_from_logs<'a>(
    provider: &StoredProvider,
    logs: impl Iterator<Item = &'a UsageLog>,
) -> ProviderHealth {
    let now = now_ms();
    let mut requests = 0_u64;
    let mut successes = 0_u64;
    let mut failures = 0_u64;
    let mut latency_total = 0_u128;
    let mut last_status_code = None;
    let mut last_request_at_ms = None;
    let mut recent_blocking_failure = false;

    for log in logs {
        requests += 1;
        latency_total += log.duration_ms;
        if (200..400).contains(&log.status_code) {
            successes += 1;
        } else {
            failures += 1;
        }
        if last_request_at_ms.is_none_or(|value| log.created_at_ms >= value) {
            last_request_at_ms = Some(log.created_at_ms);
            last_status_code = Some(log.status_code);
        }
        if now.saturating_sub(log.created_at_ms) <= RECENT_WINDOW_MS
            && matches!(log.status_code, 429 | 500..=599)
        {
            recent_blocking_failure = true;
        }
    }

    let healthy = !recent_blocking_failure;
    ProviderHealth {
        provider_id: provider.provider.id.clone(),
        app: provider.app,
        requests,
        successes,
        failures,
        success_rate: (requests > 0).then_some(successes as f64 / requests as f64),
        avg_latency_ms: (requests > 0).then_some(latency_total as f64 / requests as f64),
        last_status_code,
        last_request_at_ms,
        healthy,
        reason: (!healthy).then(|| "recent 429/5xx response".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::providers::model::{AppKind, Provider, ProviderType};
    use crate::domain::providers::store::StoredProvider;
    use crate::domain::usage::store::{UsageLog, UsageModelMetadata, UsageStore};

    use super::*;

    #[test]
    fn marks_recent_5xx_unhealthy() {
        let provider = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "p1".to_string(),
                settings_config: serde_json::json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
            resource: Default::default(),
        };
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "p1".to_string(),
            ProviderType::Codex,
            500,
            100,
            UsageModelMetadata::default(),
            Default::default(),
        );
        log.created_at_ms = now_ms();
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };

        let health = provider_health(&provider, &usage);

        assert!(!health.healthy);
        assert_eq!(health.failures, 1);
    }

    #[test]
    fn marks_recent_429_unhealthy_for_quota_or_rate_limit() {
        let provider = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "p1".to_string(),
                settings_config: serde_json::json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
            resource: Default::default(),
        };
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "p1".to_string(),
            ProviderType::Codex,
            429,
            100,
            UsageModelMetadata::default(),
            Default::default(),
        );
        log.created_at_ms = now_ms();
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };

        let health = provider_health(&provider, &usage);

        assert!(!health.healthy);
        assert_eq!(health.last_status_code, Some(429));
        assert_eq!(health.reason.as_deref(), Some("recent 429/5xx response"));
    }
}
