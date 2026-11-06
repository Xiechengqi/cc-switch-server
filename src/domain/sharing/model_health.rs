use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::accounts::store::{
    active_account_usage_block, active_account_usage_block_for_share, Account, AccountStore,
    AccountUsageBlock,
};
use crate::domain::health::ProviderHealthStatus;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::model_routing::{policy_from_settings, ModelRoutingMode};
use crate::domain::providers::probe::{automatic_probe_app, provider_has_probe};
use crate::domain::providers::runtime::{
    authoritative_managed_account, ProviderRuntimePlan, RuntimeModelPolicy,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::shares::Share;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::infra::time::now_ms;

const RECENT_RESULT_LIMIT: usize = 3;

/// Additive control-runtime evidence; never inserted into a Router cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareManualModelHealthObservation {
    pub observation_id: String,
    pub app_type: String,
    pub provider_id: String,
    pub health_fingerprint: String,
    pub requested_model: String,
    pub actual_model: String,
    pub status: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub checked_at: i64,
}

pub fn manual_observations_for_share(
    share: &Share,
    providers: &ProviderStore,
    usage: &UsageStore,
) -> Vec<ShareManualModelHealthObservation> {
    if !share.enabled || share.status != "active" {
        return Vec::new();
    }
    let Some(provider) = automatic_share_probe(share, providers) else {
        return Vec::new();
    };
    let Some(plan) = providers.runtime_plan(provider.app, &provider.provider.id) else {
        return Vec::new();
    };
    let fingerprint = plan.health_fingerprint();
    let Some(model) = plan.test_model.as_deref() else {
        return Vec::new();
    };
    let now = now_ms();
    let matching = || {
        usage.logs.iter().filter(|log| {
            log.is_health_check
                && log.data_source.as_deref() == Some("cc-switch-manual")
                && log.share_id.as_deref() == Some(share.id.as_str())
                && log.app == provider.app
                && log.provider_id == provider.provider.id
                && log.provider_health_fingerprint.as_deref() == Some(fingerprint.as_str())
                && log.requested_model.as_deref() == Some(model)
                && log.created_at_ms > 0
                && log.created_at_ms <= now
                && now.saturating_sub(log.created_at_ms) < 65 * 60 * 1000
                && log.provider_health_result.as_ref().is_some_and(|result| {
                    matches!(
                        result.status.as_str(),
                        "operational" | "degraded" | "failed"
                    )
                })
        })
    };
    let rank = |log: &&UsageLog| {
        (
            log.created_at_ms,
            log.provider_health_result
                .as_ref()
                .map(|result| result.status != "operational")
                .unwrap_or(false),
            log.request_id.clone(),
        )
    };
    let Some(latest) = matching().max_by_key(rank) else {
        return Vec::new();
    };
    // Also deliver the latest adverse manual result: a disconnected Router
    // must not miss a failure just because the user clicked success repeatedly
    // before notification delivery recovered. This is not a cycle projection.
    let adverse = matching()
        .filter(|log| {
            log.provider_health_result
                .as_ref()
                .is_some_and(|result| result.status != "operational")
        })
        .max_by_key(rank)
        .filter(|log| log.request_id != latest.request_id);
    // The accepted raw result, not effective scheduling availability. In
    // particular a tolerated first 429 must remain a failed observation.
    [Some(latest), adverse]
        .into_iter()
        .flatten()
        .map(|log| {
            let result = log
                .provider_health_result
                .as_ref()
                .expect("filtered health result");
            let status = match result.status.as_str() {
                "operational" => "success",
                "degraded" => "degraded",
                _ => "failed",
            };
            ShareManualModelHealthObservation {
                observation_id: log.request_id.clone(),
                app_type: provider.app.as_str().into(),
                provider_id: provider.provider.id.clone(),
                health_fingerprint: fingerprint.clone(),
                requested_model: model.into(),
                actual_model: match &plan.model_policy {
                    RuntimeModelPolicy::Single { upstream_model } => upstream_model.clone(),
                    RuntimeModelPolicy::Passthrough => {
                        crate::domain::providers::runtime::build_provider_model_probe(
                            provider.app,
                            provider.provider_type,
                            model,
                            crate::domain::providers::runtime::PROVIDER_MODEL_PROBE_PROMPT,
                            true,
                            plan.health_fingerprint(),
                        )
                        .wire_model
                    }
                },
                status: status.into(),
                status_code: result.http_status,
                latency_ms: u64::try_from(log.duration_ms).unwrap_or(u64::MAX),
                checked_at: ms_to_seconds(log.created_at_ms),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareModelHealthSummary {
    #[serde(default)]
    pub claude: Vec<ShareModelHealthResult>,
    #[serde(default)]
    pub codex: Vec<ShareModelHealthResult>,
    #[serde(default)]
    pub gemini: Vec<ShareModelHealthResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareModelHealthResult {
    pub app_type: String,
    pub requested_model: String,
    pub actual_model: String,
    pub status: String,
    #[serde(default)]
    pub recent_results: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(alias = "lastCheckedAt")]
    pub checked_at: i64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
}

pub fn summary_for_share(
    share: &Share,
    providers: &ProviderStore,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareModelHealthSummary {
    let mut summary = ShareModelHealthSummary::default();
    let selected = automatic_share_probe(share, providers).map(|provider| provider.app);
    for (app, provider_id) in share_bindings(share) {
        if selected != Some(app) {
            continue;
        }
        let Some(provider) = providers
            .providers
            .iter()
            .find(|item| item.app == app && item.provider.id == provider_id)
        else {
            continue;
        };
        let result = if let Some(block) = quota_block_for_share_provider(share, provider, accounts)
        {
            Some(quota_blocked_result(
                app,
                provider,
                usage.and_then(|usage| latest_quota_health_log(share, app, provider, usage)),
                &block,
            ))
        } else {
            let runtime_plan = providers.runtime_plan(app, &provider.provider.id);
            usage.and_then(|usage| {
                provider_snapshot_result(share, app, provider, usage, runtime_plan.as_deref())
            })
        };

        if let Some(result) = result {
            push_result(&mut summary, app, result);
        }
    }
    summary
}

/// The same eligibility used by Router's fixed-priority selection. Health and
/// quota failures are observations, not reasons to switch to another App.
pub fn automatic_share_probe<'a>(
    share: &Share,
    providers: &'a ProviderStore,
) -> Option<&'a StoredProvider> {
    let bindings = share_bindings(share);
    let app = automatic_probe_app(|app| {
        crate::domain::sharing::shares::share_app_api_enabled(share, app)
            && bindings.iter().any(|(bound_app, id)| {
                *bound_app == app
                    && providers.providers.iter().any(|provider| {
                        provider.app == app
                            && provider.provider.id == *id
                            && provider_has_probe(providers, provider)
                    })
            })
    })?;
    providers.providers.iter().find(|provider| {
        provider.app == app
            && bindings
                .iter()
                .any(|(bound_app, id)| *bound_app == app && provider.provider.id == *id)
    })
}

fn provider_snapshot_result(
    share: &Share,
    app: AppKind,
    provider: &StoredProvider,
    usage: &UsageStore,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> Option<ShareModelHealthResult> {
    let health = crate::domain::health::provider_health_for_plan(provider, usage, runtime_plan);
    if health.status == ProviderHealthStatus::Unknown {
        return None;
    }

    let logs = health_logs_for_binding(share, app, provider, usage);
    // effective availability deliberately tolerates a first transient failure.
    // That scheduling decision must never turn a real 429/failed probe into a
    // successful observation (or overwrite Router's immutable cycle evidence).
    let status = match health.status {
        ProviderHealthStatus::Healthy => "success",
        ProviderHealthStatus::Degraded => "degraded",
        ProviderHealthStatus::Unhealthy => "failed",
        ProviderHealthStatus::Unknown => return None,
    };
    let requested_model = health
        .model
        .clone()
        .or_else(|| default_model_for_provider(provider))
        .unwrap_or_else(|| app.as_str().to_string());
    let mut recent_results = vec![status.to_string()];
    recent_results.extend(
        logs.iter()
            .filter(|log| {
                ms_to_seconds(log.created_at_ms)
                    < health.checked_at_ms.map(ms_to_seconds).unwrap_or_default()
            })
            .take(RECENT_RESULT_LIMIT.saturating_sub(1))
            .map(|log| status_for_log(log).to_string()),
    );
    Some(ShareModelHealthResult {
        app_type: app.as_str().to_string(),
        actual_model: actual_model_for_provider(provider, &requested_model),
        requested_model,
        status: status.to_string(),
        recent_results,
        status_code: health.last_status_code,
        latency_ms: health.probe_latency_ms.unwrap_or(0),
        error_message: (status == "failed").then_some(health.reason).flatten(),
        checked_at: health.checked_at_ms.map(ms_to_seconds).unwrap_or_default(),
        source: health
            .source
            .unwrap_or_else(|| "cc-switch-health-check".to_string()),
        provider_id: Some(provider.provider.id.clone()),
        provider_name: Some(provider.provider.name.clone()),
    })
}

fn health_logs_for_binding<'a>(
    share: &Share,
    app: AppKind,
    provider: &StoredProvider,
    usage: &'a UsageStore,
) -> Vec<&'a UsageLog> {
    let mut logs = usage
        .logs
        .iter()
        .filter(|log| {
            log.is_health_check
                && log.app == app
                && log.provider_id == provider.provider.id
                && log.share_id.as_deref() == Some(share.id.as_str())
        })
        .collect::<Vec<_>>();
    logs.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    logs
}

fn latest_quota_health_log<'a>(
    share: &Share,
    app: AppKind,
    provider: &StoredProvider,
    usage: &'a UsageStore,
) -> Option<&'a UsageLog> {
    usage
        .logs
        .iter()
        .filter(|log| {
            log.is_health_check
                && log.app == app
                && log.provider_id == provider.provider.id
                && log.share_id.as_deref() == Some(share.id.as_str())
                && log.data_source.as_deref() == Some("cc-switch-quota")
        })
        .max_by_key(|log| log.created_at_ms)
}

fn quota_blocked_result(
    app: AppKind,
    provider: &StoredProvider,
    log: Option<&UsageLog>,
    block: &AccountUsageBlock,
) -> ShareModelHealthResult {
    let requested_model = log
        .and_then(|log| log.requested_model.as_deref().or(log.model.as_deref()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| default_model_for_provider(provider))
        .unwrap_or_else(|| app.as_str().into());
    let actual_model = log
        .and_then(|log| log.actual_model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| actual_model_for_provider(provider, &requested_model));
    ShareModelHealthResult {
        app_type: app.as_str().to_string(),
        requested_model,
        actual_model,
        status: "quota_blocked".to_string(),
        recent_results: vec!["quota_blocked".to_string()],
        status_code: None,
        latency_ms: log
            .map(|log| saturating_u128_to_u64(log.duration_ms))
            .unwrap_or(0),
        error_message: Some(quota_block_message(block)),
        checked_at: log
            .map(|log| ms_to_seconds(log.created_at_ms))
            .unwrap_or_else(|| ms_to_seconds(now_ms())),
        source: log
            .and_then(|log| log.data_source.clone())
            .unwrap_or_else(|| "cc-switch-quota".to_string()),
        provider_id: Some(provider.provider.id.clone()),
        provider_name: Some(provider.provider.name.clone()),
    }
}

fn status_for_log(log: &UsageLog) -> &'static str {
    if (200..400).contains(&log.status_code)
        && (!log.is_streaming || log.stream_status.as_deref() == Some("completed"))
    {
        "success"
    } else {
        "failed"
    }
}

fn actual_model_for_provider(provider: &StoredProvider, requested_model: &str) -> String {
    let settings = &provider.provider.settings_config;
    if let Some(policy) = crate::domain::providers::model_routing::policy_from_settings(settings) {
        return match policy.mode {
            crate::domain::providers::model_routing::ModelRoutingMode::Single => policy
                .upstream_model
                .unwrap_or_else(|| requested_model.to_string()),
            crate::domain::providers::model_routing::ModelRoutingMode::Passthrough => {
                requested_model.to_string()
            }
        };
    }
    if let Some(mapped) = settings
        .pointer(&format!(
            "/modelMapping/{}",
            requested_model.replace('~', "~0").replace('/', "~1")
        ))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return mapped.to_string();
    }
    settings
        .pointer("/modelMapping/upstreamModel")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| requested_model.to_string())
}

fn default_model_for_provider(provider: &StoredProvider) -> Option<String> {
    let settings = &provider.provider.settings_config;
    let passthrough = policy_from_settings(settings)
        .is_some_and(|policy| policy.mode == ModelRoutingMode::Passthrough);
    let configured_model = if passthrough {
        None
    } else {
        let env = settings.get("env");
        [
            "/modelMapping/upstreamModel",
            "/env/ANTHROPIC_MODEL",
            "/env/ANTHROPIC_DEFAULT_SONNET_MODEL",
            "/env/OPENAI_MODEL",
            "/env/GEMINI_MODEL",
            "/env/GOOGLE_GEMINI_MODEL",
        ]
        .into_iter()
        .find_map(|pointer| {
            settings
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            env.and_then(|value| value.get("MODEL"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    configured_model.or_else(|| {
        settings
            .get("models")
            .and_then(serde_json::Value::as_array)
            .and_then(|models| models.first())
            .and_then(|value| {
                value.as_str().or_else(|| {
                    value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
                })
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn quota_block_for_provider(
    provider: &StoredProvider,
    accounts: Option<&AccountStore>,
) -> Option<AccountUsageBlock> {
    let now = now_ms().min(i64::MAX as u128) as i64;
    account_for_provider(accounts, provider)
        .and_then(|account| active_account_usage_block(account, now))
}

pub(crate) fn quota_block_for_share_provider(
    share: &Share,
    provider: &StoredProvider,
    accounts: Option<&AccountStore>,
) -> Option<AccountUsageBlock> {
    let now = now_ms().min(i64::MAX as u128) as i64;
    account_for_provider(accounts, provider).and_then(|account| {
        active_account_usage_block_for_share(account, now, share.allow_personal_credits)
    })
}

pub(crate) fn quota_block_message(block: &AccountUsageBlock) -> String {
    let until = Utc
        .timestamp_millis_opt(block.until_ms)
        .single()
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| block.until_ms.to_string());
    format!("quota blocked: {} until {until}", block.reason)
}

fn account_for_provider<'a>(
    accounts: Option<&'a AccountStore>,
    provider: &StoredProvider,
) -> Option<&'a Account> {
    authoritative_managed_account(provider, accounts?)
}

pub(crate) fn share_bindings(share: &Share) -> Vec<(AppKind, String)> {
    if share.bindings.is_empty() {
        vec![(share.app, share.provider_id.clone())]
    } else {
        share
            .bindings
            .iter()
            .map(|binding| (binding.app, binding.provider_id.clone()))
            .collect()
    }
}

fn push_result(
    summary: &mut ShareModelHealthSummary,
    app: AppKind,
    result: ShareModelHealthResult,
) {
    match app {
        AppKind::Claude => summary.claude.push(result),
        AppKind::Codex => summary.codex.push(result),
        AppKind::Gemini => summary.gemini.push(result),
    }
}

fn ms_to_seconds(value: u128) -> i64 {
    (value / 1000).min(i64::MAX as u128) as i64
}

fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::AccountQuota;
    use crate::domain::health::{ProviderHealthObservation, ProviderHealthStatus};
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta, ProviderType};
    use crate::domain::sharing::shares::{ShareBinding, SharePolicy};
    use crate::domain::usage::store::{UsageLogContext, UsageModelMetadata};

    fn manual_fixture(app: AppKind, kind: ProviderType) -> (Share, ProviderStore, UsageStore) {
        let mut provider = test_provider(app, "manual-provider", kind);
        provider.provider.meta = None;
        provider.provider.settings_config = json!({
            "testModel":"health-model", "modelMapping":{"mode":"passthrough"},
            "env":{"OPENAI_BASE_URL":"https://fixture.invalid/v1"}
        });
        let share = test_share("manual-share", app, "manual-provider", kind, None);
        let mut providers = ProviderStore {
            providers: vec![provider.clone()],
            ..Default::default()
        };
        providers
            .rebuild_runtime_index(&AccountStore::default())
            .unwrap();
        let plan = providers.runtime_plan(app, "manual-provider").unwrap();
        let mut log = health_log(
            &share,
            &provider,
            200,
            true,
            Some("completed"),
            plan.test_model.as_deref(),
            None,
        );
        log.provider_health_fingerprint = Some(plan.health_fingerprint());
        log.provider_health_result = Some(crate::domain::usage::store::UsageProviderHealthResult {
            status: "operational".into(),
            http_status: Some(200),
        });
        log.data_source = Some("cc-switch-manual".into());
        log.created_at_ms = now_ms().saturating_sub(1000);
        (
            share,
            providers,
            UsageStore {
                logs: vec![log],
                ..Default::default()
            },
        )
    }

    #[test]
    fn manual_evidence_covers_all_apps_and_excludes_private_error_and_account_data() {
        for (app, kind) in [
            (AppKind::Codex, ProviderType::OpenRouter),
            (AppKind::Claude, ProviderType::Claude),
            (AppKind::Gemini, ProviderType::Gemini),
        ] {
            let (share, providers, mut usage) = manual_fixture(app, kind);
            usage.logs[0].error_message = Some("private@example.test secret-key".into());
            let results = manual_observations_for_share(&share, &providers, &usage);
            assert_eq!(results.len(), 1, "{app:?}");
            assert_eq!(results[0].app_type, app.as_str());
            assert_eq!(results[0].requested_model, "health-model");
            assert_eq!(results[0].status, "success");
            let wire = serde_json::to_string(&results[0]).unwrap();
            assert!(wire.contains("\"healthFingerprint\""));
            assert!(wire.contains("\"checkedAt\""));
            assert!(!wire.contains("private@example.test"));
            assert!(!wire.contains("secret-key"));
        }
    }

    #[test]
    fn manual_evidence_preserves_raw_slow_and_failed_checks_instead_of_scheduling_availability() {
        let (share, providers, mut usage) =
            manual_fixture(AppKind::Codex, ProviderType::OpenRouter);
        usage.logs[0]
            .provider_health_result
            .as_mut()
            .unwrap()
            .status = "degraded".into();
        usage.logs[0].duration_ms = 11321;
        let result = manual_observations_for_share(&share, &providers, &usage).remove(0);
        assert_eq!(result.status, "degraded");
        assert_eq!(result.latency_ms, 11321);
        assert_eq!(result.status_code, Some(200));
        usage.logs[0].provider_health_result =
            Some(crate::domain::usage::store::UsageProviderHealthResult {
                status: "failed".into(),
                http_status: Some(429),
            });
        let result = manual_observations_for_share(&share, &providers, &usage).remove(0);
        assert_eq!(result.status, "failed");
        assert_eq!(result.status_code, Some(429));
    }

    #[test]
    fn manual_evidence_rejects_old_future_mismatched_and_non_manual_records() {
        let (mut share, providers, usage) =
            manual_fixture(AppKind::Codex, ProviderType::OpenRouter);
        for variant in 0..9 {
            let mut bad = usage.clone();
            let log = &mut bad.logs[0];
            match variant {
                0 => log.created_at_ms = now_ms().saturating_sub(65 * 60 * 1000),
                1 => log.created_at_ms = now_ms() + 3600000,
                2 => log.provider_health_fingerprint = Some("old-configuration".into()),
                3 => log.share_id = Some("another-share".into()),
                4 => log.provider_id = "another-provider".into(),
                5 => log.requested_model = Some("another-model".into()),
                6 => log.is_health_check = false,
                7 => log.data_source = Some("cc-switch-scheduled".into()),
                _ => log.provider_health_result = None,
            }
            assert!(
                manual_observations_for_share(&share, &providers, &bad).is_empty(),
                "{variant}"
            );
        }
        share.enabled = false;
        assert!(manual_observations_for_share(&share, &providers, &usage).is_empty());
    }

    #[test]
    fn manual_evidence_selects_latest_original_event_and_fixed_execution_model() {
        let (share, mut providers, mut usage) =
            manual_fixture(AppKind::Codex, ProviderType::OpenRouter);
        providers.providers[0].provider.settings_config["modelMapping"] =
            json!({"mode":"single","upstreamModel":"fixed-model"});
        providers
            .rebuild_runtime_index(&AccountStore::default())
            .unwrap();
        let plan = providers
            .runtime_plan(AppKind::Codex, "manual-provider")
            .unwrap();
        usage.logs[0].provider_health_fingerprint = Some(plan.health_fingerprint());
        usage.logs[0].requested_model = plan.test_model.clone();
        let mut older = usage.logs[0].clone();
        older.request_id = "older".into();
        older.created_at_ms -= 1000;
        usage.logs.push(older);
        let result = manual_observations_for_share(&share, &providers, &usage).remove(0);
        assert_eq!(result.observation_id, usage.logs[0].request_id);
        assert_eq!(result.actual_model, "fixed-model");
    }

    #[test]
    fn manual_evidence_delivers_latest_adverse_event_even_after_repeated_success_and_deduplicates()
    {
        let (share, providers, mut usage) =
            manual_fixture(AppKind::Codex, ProviderType::OpenRouter);
        let mut failed = usage.logs[0].clone();
        failed.request_id = "failed-check".into();
        failed.created_at_ms -= 2000;
        failed.provider_health_result =
            Some(crate::domain::usage::store::UsageProviderHealthResult {
                status: "failed".into(),
                http_status: Some(429),
            });
        let mut older = failed.clone();
        older.request_id = "older-failure".into();
        older.created_at_ms -= 1000;
        usage.logs.extend([failed.clone(), older]);
        for index in 1..4 {
            let mut success = usage.logs[0].clone();
            success.request_id = format!("success-{index}");
            success.created_at_ms -= index * 100;
            usage.logs.push(success);
        }
        let result = manual_observations_for_share(&share, &providers, &usage);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].observation_id, usage.logs[0].request_id);
        assert_eq!(result[0].status, "success");
        assert_eq!(result[1].observation_id, "failed-check");
        assert_eq!(result[1].status, "failed");
        usage.logs.reverse();
        assert_eq!(
            manual_observations_for_share(&share, &providers, &usage),
            result
        );
        failed.created_at_ms = now_ms().saturating_sub(1);
        usage.logs.push(failed);
        let result = manual_observations_for_share(&share, &providers, &usage);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].observation_id, "failed-check");
    }

    #[test]
    fn summary_filters_by_share_id_and_bound_apps() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let mut own_log = health_log(&share, &provider, 200, false, None, Some("gpt-5.5"), None);
        own_log.created_at_ms = 2000;
        let mut other_share_log = own_log.clone();
        other_share_log.share_id = Some("share-2".to_string());
        other_share_log.created_at_ms = 3000;
        let mut usage = UsageStore {
            logs: vec![other_share_log, own_log],
            ..Default::default()
        };
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Healthy,
            now_ms(),
            "cc-switch-scheduled",
            false,
            Some(200),
            None,
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));

        assert_eq!(summary.codex.len(), 1);
        assert_eq!(summary.codex[0].provider_id.as_deref(), Some("p-codex"));
        assert!(summary.claude.is_empty());
        assert!(summary.gemini.is_empty());
    }

    #[test]
    fn bundle_summary_only_projects_the_selected_test_app() {
        let mut share = test_share(
            "share-1",
            AppKind::Claude,
            "grok-bundle",
            ProviderType::GrokOAuth,
            None,
        );
        share.bindings.push(ShareBinding {
            app: AppKind::Codex,
            provider_id: "grok-bundle".to_string(),
            provider_type: ProviderType::GrokOAuth,
        });
        let mut claude = test_provider(AppKind::Claude, "grok-bundle", ProviderType::GrokOAuth);
        let mut codex = test_provider(AppKind::Codex, "grok-bundle", ProviderType::GrokOAuth);
        for provider in [&mut claude, &mut codex] {
            provider.provider.extra.extend([
                ("bundleId".to_string(), json!("grok-bundle")),
                ("familyId".to_string(), json!("family.grok_oauth")),
                ("surfaceEnabled".to_string(), json!(true)),
                ("modelPolicyScope".to_string(), json!("global")),
                ("testApp".to_string(), json!("codex")),
            ]);
        }
        let mut usage = UsageStore::default();
        record_snapshot(
            &mut usage,
            &claude,
            ProviderHealthStatus::Healthy,
            now_ms(),
            "cc-switch-scheduled",
            false,
            Some(200),
            None,
        );
        record_snapshot(
            &mut usage,
            &codex,
            ProviderHealthStatus::Healthy,
            now_ms(),
            "cc-switch-scheduled",
            false,
            Some(200),
            None,
        );
        let providers = ProviderStore {
            providers: vec![claude, codex],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));

        assert!(summary.claude.is_empty());
        assert_eq!(summary.codex.len(), 1);
        assert!(summary.gemini.is_empty());
    }

    #[test]
    fn health_check_log_maps_requested_and_actual_model() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let log = health_log(&share, &provider, 200, false, None, Some("gpt-5.5"), None);
        let checked_at = log.created_at_ms;
        let mut usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Healthy,
            checked_at,
            "cc-switch-scheduled",
            false,
            Some(200),
            None,
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.requested_model, "gpt-5.5");
        assert_eq!(result.actual_model, "glm-5.2");
        assert_eq!(result.status, "success");
        assert_eq!(result.recent_results, vec!["success"]);
    }

    #[test]
    fn passthrough_health_fallback_ignores_stale_fixed_model_environment() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::OpenRouter,
            None,
        );
        let mut provider = test_provider(AppKind::Codex, "p-codex", ProviderType::OpenRouter);
        provider.provider.settings_config = json!({
            "env": {"OPENAI_MODEL": "stale-fixed-model"},
            "modelMapping": {"mode": "passthrough"}
        });
        let mut usage = UsageStore::default();
        usage.provider_health.record(ProviderHealthObservation {
            app: provider.app,
            provider_id: provider.provider.id.clone(),
            provider_revision: provider.resource.revision,
            runtime_fingerprint: "runtime-test".to_string(),
            status: ProviderHealthStatus::Healthy,
            checked_at_ms: now_ms(),
            source: "cc-switch-scheduled".to_string(),
            status_code: Some(200),
            latency_ms: Some(123),
            model: None,
            error_category: None,
            error_message: None,
            transient_failure: false,
        });
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.requested_model, "codex");
        assert_eq!(result.actual_model, "codex");
    }

    #[test]
    fn first_transient_429_remains_a_failed_observation_even_while_scheduling_is_allowed() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let mut usage = UsageStore::default();
        let checked_at = now_ms();
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            checked_at,
            "cc-switch-router-cycle:utc-test",
            true,
            Some(429),
            Some("rate limited"),
        );
        let snapshot = usage
            .provider_health
            .get(AppKind::Codex, "p-codex")
            .unwrap();
        assert!(snapshot.effective_available);
        assert!(snapshot.confirmation_pending);
        let mut log = health_log(&share, &provider, 429, false, None, Some("gpt-5.5"), None);
        log.created_at_ms = checked_at;
        usage.logs.push(log);
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };
        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = &summary.codex[0];
        assert_eq!(result.status, "failed");
        assert_eq!(result.status_code, Some(429));
        assert_eq!(result.error_message.as_deref(), Some("rate limited"));
        assert_eq!(
            result.recent_results,
            ["failed"],
            "same probe must not be counted twice"
        );
    }

    #[test]
    fn degraded_snapshot_remains_available_and_is_projected_to_router() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let mut usage = UsageStore::default();
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Degraded,
            now_ms(),
            "cc-switch-scheduled",
            false,
            Some(200),
            None,
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "degraded");
        assert_eq!(result.recent_results, vec!["degraded"]);
        assert!(result.error_message.is_none());
    }

    #[test]
    fn quota_block_uses_fresh_explicit_account_evidence_without_health_log() {
        let now = now_ms().min(i64::MAX as u128) as i64;
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::CodexOAuth,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::CodexOAuth);
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };
        let accounts = blocked_codex_accounts(now);

        let summary = summary_for_share(&share, &providers, Some(&accounts), None);
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "quota_blocked");
        assert_eq!(result.source, "cc-switch-quota");
        assert_eq!(result.recent_results, vec!["quota_blocked"]);
        assert!(result
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("until ")));
    }

    #[test]
    fn quota_block_reuses_persisted_timestamp_but_refreshes_bounded_message() {
        let now = now_ms().min(i64::MAX as u128) as i64;
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::CodexOAuth,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::CodexOAuth);
        let mut log = health_log(&share, &provider, 429, false, None, Some("gpt-5.5"), None);
        log.created_at_ms = 2_000;
        log.data_source = Some("cc-switch-quota".to_string());
        log.error_message = Some("quota blocked until reset".to_string());
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let accounts = blocked_codex_accounts(now);
        let first = summary_for_share(&share, &providers, Some(&accounts), Some(&usage));
        let second = summary_for_share(&share, &providers, Some(&accounts), Some(&usage));
        let first = first.codex.first().unwrap();
        let second = second.codex.first().unwrap();

        assert_eq!(first.status, "quota_blocked");
        assert_eq!(first.checked_at, 2);
        assert_eq!(second.checked_at, first.checked_at);
        assert_eq!(first.source, "cc-switch-quota");
        assert!(first
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("until ")));
        assert_ne!(
            first.error_message.as_deref(),
            Some("quota blocked until reset")
        );
    }

    #[test]
    fn share_quota_percent_alone_does_not_block_health() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            Some(100.0),
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, None);
        assert!(summary.codex.is_empty());
    }

    #[test]
    fn scheduled_health_log_preserves_source_error_and_seconds_timestamp() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let mut log = health_log(
            &share,
            &provider,
            599,
            true,
            Some("failed"),
            Some("gpt-5.5"),
            None,
        );
        let checked_at = now_ms();
        log.created_at_ms = checked_at;
        log.data_source = Some("cc-switch-scheduled".to_string());
        log.error_message = Some("upstream connection timed out".to_string());
        let mut usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            checked_at
                .saturating_sub(crate::domain::health::PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS),
            "cc-switch-scheduled",
            true,
            Some(599),
            Some("upstream connection timed out"),
        );
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            checked_at,
            "cc-switch-scheduled-confirmation",
            true,
            Some(599),
            Some("upstream connection timed out"),
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "failed");
        assert_eq!(result.checked_at, ms_to_seconds(checked_at));
        assert_eq!(result.source, "cc-switch-scheduled-confirmation");
        assert_eq!(
            result.error_message.as_deref(),
            Some("upstream connection timed out")
        );
    }

    #[test]
    fn streaming_health_check_requires_completed_stream() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            None,
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
        let log = health_log(
            &share,
            &provider,
            200,
            true,
            Some("interrupted"),
            Some("gpt-5.5"),
            Some("glm-5.2"),
        );
        let mut usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let checked_at = now_ms();
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            checked_at
                .saturating_sub(crate::domain::health::PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS),
            "cc-switch-scheduled",
            true,
            Some(200),
            Some("stream interrupted"),
        );
        record_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            checked_at,
            "cc-switch-scheduled-confirmation",
            true,
            Some(200),
            Some("stream interrupted"),
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "failed");
        assert_eq!(result.status_code, Some(200));
        assert_eq!(result.error_message.as_deref(), Some("stream interrupted"));
    }

    fn test_provider(app: AppKind, id: &str, provider_type: ProviderType) -> StoredProvider {
        StoredProvider {
            app,
            provider: Provider {
                id: id.to_string(),
                name: "provider 1".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": "https://upstream.example/v1"
                    },
                    "modelMapping": {
                        "upstreamModel": "glm-5.2",
                        "gpt-5.5": "glm-5.2"
                    },
                    "models": ["gpt-5.5"]
                }),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("managed_account".to_string()),
                        auth_provider: Some(provider_type.as_str().to_string()),
                        account_id: Some("acct-1".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    fn test_share(
        id: &str,
        app: AppKind,
        provider_id: &str,
        provider_type: ProviderType,
        quota_percent: Option<f64>,
    ) -> Share {
        Share {
            id: id.to_string(),
            capacity_pool_id: format!("cp-{id}"),
            owner_email: Some("owner@example.com".to_string()),
            app,
            provider_id: provider_id.to_string(),
            provider_type,
            display_name: Some(id.to_string()),
            enabled: true,
            status: "active".to_string(),
            subscription_level: None,
            account_email: None,
            quota_percent,
            tunnel_subdomain: Some(id.to_string()),
            policy: SharePolicy::default(),
            tokens_used: 0,
            requests_count: 0,
            created_at_ms: 0,
            auto_start: false,
            description: None,
            enabled_apps: None,
            bindings: vec![ShareBinding {
                app,
                provider_id: provider_id.to_string(),
                provider_type,
            }],
            binding_history: Vec::new(),
            runtime_snapshot: None,
            last_error: None,
            integrity_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
            config_revision: 0,
            router_synced_revision: 0,
            descriptor_generation: 0,
            descriptor_fingerprint: None,
            router_synced_descriptor_generation: 0,
            router_synced_descriptor_fingerprint: None,
            user_grants: BTreeMap::new(),
            market_entitlements: BTreeMap::new(),
            market_requests: BTreeMap::new(),
            pending_market_termination_acks: BTreeMap::new(),
            market_compacted_usage: BTreeMap::new(),
            market_receipts_before_ms: 0,
        }
    }

    fn blocked_codex_accounts(now_ms: i64) -> AccountStore {
        AccountStore {
            accounts: vec![Account {
                id: "acct-1".to_string(),
                provider_type: ProviderType::CodexOAuth,
                auth_identity_generation: 1,
                token_refresh_generation: 1,
                email: Some("owner@example.com".to_string()),
                access_token: Some("token".to_string()),
                refresh_token: None,
                id_token: None,
                token_type: Some("Bearer".to_string()),
                api_key: None,
                extra_headers: Default::default(),
                scopes: Vec::new(),
                profile: None,
                raw: None,
                subscription_level: Some("pro".to_string()),
                entitlement_status: None,
                quota_percent: Some(100.0),
                quota: Some(AccountQuota {
                    success: true,
                    credential_message: None,
                    tiers: Vec::new(),
                    extra_usage: Some(json!({
                        "subscriptionEvidence": {
                            "usageAllowed": false,
                            "usageLimitReached": true
                        }
                    })),
                }),
                quota_refreshed_at: Some(now_ms),
                quota_next_refresh_at: Some(now_ms + 30 * 60 * 1000),
                expires_at: None,
                manual_subscription_expires_at_ms: None,
                manual_subscription_expiry_updated_at_ms: None,
                manual_subscription_expiry_rule: None,
                rate_limited_until: None,
                last_refresh_error: None,
                refresh_consecutive_failures: 0,
                needs_relogin: false,
                capacity_pool_limits: Default::default(),
                capability_observations: Default::default(),
                quota_window_observations: Default::default(),
                codex_passive_quota: None,
            }],
            ..Default::default()
        }
    }

    fn health_log(
        share: &Share,
        provider: &StoredProvider,
        status_code: u16,
        is_streaming: bool,
        stream_status: Option<&str>,
        requested_model: Option<&str>,
        actual_model: Option<&str>,
    ) -> UsageLog {
        let mut log = UsageLog::new(
            share.app,
            provider.provider.id.clone(),
            provider.provider.name.clone(),
            provider.provider_type,
            status_code,
            123,
            UsageModelMetadata {
                model: requested_model.map(str::to_string),
                requested_model: requested_model.map(str::to_string),
                actual_model: actual_model.map(str::to_string),
                actual_model_source: None,
            },
            Default::default(),
        );
        log.apply_context(UsageLogContext {
            share_id: Some(share.id.clone()),
            share_name: share.display_name.clone(),
            is_health_check: true,
            is_streaming,
            stream_status: stream_status.map(str::to_string),
            ..UsageLogContext::default()
        });
        log
    }

    #[allow(clippy::too_many_arguments)]
    fn record_snapshot(
        usage: &mut UsageStore,
        provider: &StoredProvider,
        status: ProviderHealthStatus,
        checked_at_ms: u128,
        source: &str,
        transient_failure: bool,
        status_code: Option<u16>,
        error_message: Option<&str>,
    ) {
        usage.provider_health.record(ProviderHealthObservation {
            app: provider.app,
            provider_id: provider.provider.id.clone(),
            provider_revision: provider.resource.revision,
            runtime_fingerprint: "runtime-test".to_string(),
            status,
            checked_at_ms,
            source: source.to_string(),
            status_code,
            latency_ms: Some(123),
            model: Some("gpt-5.5".to_string()),
            error_category: (!status.is_success()).then(|| "network".to_string()),
            error_message: error_message.map(str::to_string),
            transient_failure,
        });
    }

    #[test]
    fn model_health_result_uses_checked_at_wire_name() {
        let result = ShareModelHealthResult {
            app_type: "codex".to_string(),
            requested_model: "gpt-5".to_string(),
            actual_model: "gpt-5".to_string(),
            status: "healthy".to_string(),
            recent_results: vec!["healthy".to_string()],
            status_code: Some(200),
            latency_ms: 120,
            error_message: None,
            checked_at: 1_783_917_271,
            source: "health_check".to_string(),
            provider_id: Some("provider-1".to_string()),
            provider_name: Some("Provider".to_string()),
        };
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(serialized.contains("\"checkedAt\":1783917271"));
        assert!(!serialized.contains("lastCheckedAt"));

        let from_router: ShareModelHealthResult = serde_json::from_value(serde_json::json!({
            "appType": "codex",
            "requestedModel": "gpt-5",
            "actualModel": "gpt-5",
            "status": "healthy",
            "latencyMs": 120,
            "lastCheckedAt": 99,
            "source": "health_check"
        }))
        .unwrap();
        assert_eq!(from_router.checked_at, 99);
    }
}
