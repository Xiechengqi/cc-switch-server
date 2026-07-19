use serde::{Deserialize, Serialize};

use crate::domain::accounts::store::{Account, AccountStore};
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::shares::Share;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::infra::time::now_ms;

const RECENT_RESULT_LIMIT: usize = 3;

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
    for (app, provider_id) in share_bindings(share) {
        let Some(provider) = providers
            .providers
            .iter()
            .find(|item| item.app == app && item.provider.id == provider_id)
        else {
            continue;
        };

        let result = if quota_blocked_for_provider(share, provider, accounts) {
            Some(quota_blocked_result(
                app,
                provider,
                usage.and_then(|usage| latest_quota_health_log(share, app, provider, usage)),
            ))
        } else {
            usage.and_then(|usage| latest_health_result(share, app, provider, usage))
        };

        if let Some(result) = result {
            push_result(&mut summary, app, result);
        }
    }
    summary
}

fn latest_health_result(
    share: &Share,
    app: AppKind,
    provider: &StoredProvider,
    usage: &UsageStore,
) -> Option<ShareModelHealthResult> {
    let logs = health_logs_for_binding(share, app, provider, usage);
    let latest = logs.first()?;
    let mut result = result_from_log(app, provider, latest);
    result.recent_results = logs
        .iter()
        .take(RECENT_RESULT_LIMIT)
        .map(|log| status_for_log(log).to_string())
        .collect();
    Some(result)
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

fn result_from_log(
    app: AppKind,
    provider: &StoredProvider,
    log: &UsageLog,
) -> ShareModelHealthResult {
    let requested_model = requested_model_from_log_or_provider(log, provider);
    let actual_model = log
        .actual_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| actual_model_for_provider(provider, &requested_model));
    let status = status_for_log(log);
    ShareModelHealthResult {
        app_type: app.as_str().to_string(),
        requested_model,
        actual_model,
        status: status.to_string(),
        recent_results: Vec::new(),
        status_code: Some(log.status_code),
        latency_ms: saturating_u128_to_u64(log.duration_ms),
        error_message: log
            .error_message
            .clone()
            .or_else(|| error_message_for_log(log, status)),
        checked_at: ms_to_seconds(log.created_at_ms),
        source: log
            .data_source
            .as_deref()
            .filter(|source| source.starts_with("cc-switch-"))
            .unwrap_or("cc-switch-health-check")
            .to_string(),
        provider_id: Some(provider.provider.id.clone()),
        provider_name: Some(provider.provider.name.clone()),
    }
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
        error_message: log
            .and_then(|log| log.error_message.clone())
            .or_else(|| Some("quota blocked".to_string())),
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

fn error_message_for_log(log: &UsageLog, status: &str) -> Option<String> {
    if status == "success" {
        return None;
    }
    if log.is_streaming {
        let stream_status = log.stream_status.as_deref().unwrap_or("unknown");
        return Some(format!("stream {stream_status}"));
    }
    Some(format!("HTTP {}", log.status_code))
}

fn requested_model_from_log_or_provider(log: &UsageLog, provider: &StoredProvider) -> String {
    log.requested_model
        .as_deref()
        .or(log.model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| default_model_for_provider(provider))
        .unwrap_or_else(|| provider.app.as_str().to_string())
}

fn actual_model_for_provider(provider: &StoredProvider, requested_model: &str) -> String {
    let settings = &provider.provider.settings_config;
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
    let env = settings.get("env");
    [
        "/env/ANTHROPIC_MODEL",
        "/env/ANTHROPIC_DEFAULT_SONNET_MODEL",
        "/env/OPENAI_MODEL",
        "/env/GEMINI_MODEL",
        "/env/GOOGLE_GEMINI_MODEL",
        "/modelMapping/upstreamModel",
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
    .or_else(|| {
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

pub(crate) fn quota_blocked_for_provider(
    share: &Share,
    provider: &StoredProvider,
    accounts: Option<&AccountStore>,
) -> bool {
    quota_percent_for_provider(share, provider, accounts).is_some_and(|value| value >= 100.0)
}

fn quota_percent_for_provider(
    share: &Share,
    provider: &StoredProvider,
    accounts: Option<&AccountStore>,
) -> Option<f64> {
    account_for_provider(accounts, provider)
        .and_then(|account| account.quota_percent)
        .or(share.quota_percent)
}

fn account_for_provider<'a>(
    accounts: Option<&'a AccountStore>,
    provider: &StoredProvider,
) -> Option<&'a Account> {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    accounts.and_then(|accounts| accounts.find_for_provider(provider.provider_type, account_id))
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
    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::sharing::shares::{ShareAcl, ShareBinding, ShareMarketGrantStatus};
    use crate::domain::usage::store::{UsageLogContext, UsageModelMetadata};

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
        let usage = UsageStore {
            logs: vec![other_share_log, own_log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));

        assert_eq!(summary.codex.len(), 1);
        assert_eq!(summary.codex[0].provider_id.as_deref(), Some("p-codex"));
        assert!(summary.claude.is_empty());
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
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.requested_model, "gpt-5.5");
        assert_eq!(result.actual_model, "glm-5.2");
        assert_eq!(result.status, "success");
        assert_eq!(result.recent_results, vec!["success"]);
    }

    #[test]
    fn quota_block_uses_account_quota_without_health_log() {
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
        };
        let accounts = AccountStore {
            accounts: vec![Account {
                id: "acct-1".to_string(),
                provider_type: ProviderType::CodexOAuth,
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
                quota: Some(AccountQuota::default()),
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: None,
                manual_subscription_expires_at_ms: None,
                manual_subscription_expiry_updated_at_ms: None,
                rate_limited_until: None,
                last_refresh_error: None,
                refresh_consecutive_failures: 0,
                needs_relogin: false,
            }],
        };

        let summary = summary_for_share(&share, &providers, Some(&accounts), None);
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "quota_blocked");
        assert_eq!(result.source, "cc-switch-quota");
        assert_eq!(result.recent_results, vec!["quota_blocked"]);
    }

    #[test]
    fn quota_block_reuses_persisted_health_log_metadata() {
        let share = test_share(
            "share-1",
            AppKind::Codex,
            "p-codex",
            ProviderType::Codex,
            Some(100.0),
        );
        let provider = test_provider(AppKind::Codex, "p-codex", ProviderType::Codex);
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
        };

        let first = summary_for_share(&share, &providers, None, Some(&usage));
        let second = summary_for_share(&share, &providers, None, Some(&usage));
        let first = first.codex.first().unwrap();
        let second = second.codex.first().unwrap();

        assert_eq!(first.status, "quota_blocked");
        assert_eq!(first.checked_at, 2);
        assert_eq!(second.checked_at, first.checked_at);
        assert_eq!(first.source, "cc-switch-quota");
        assert_eq!(
            first.error_message.as_deref(),
            Some("quota blocked until reset")
        );
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
        log.created_at_ms = 1_783_917_271_880;
        log.data_source = Some("cc-switch-scheduled".to_string());
        log.error_message = Some("upstream connection timed out".to_string());
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let summary = summary_for_share(&share, &providers, None, Some(&usage));
        let result = summary.codex.first().unwrap();

        assert_eq!(result.status, "failed");
        assert_eq!(result.checked_at, 1_783_917_271);
        assert_eq!(result.source, "cc-switch-scheduled");
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
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
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
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
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
            acl: ShareAcl::default(),
            token_limit: None,
            parallel_limit: None,
            tokens_used: 0,
            requests_count: 0,
            expires_at: None,
            created_at_ms: 0,
            for_sale: false,
            free_access: false,
            sale_market_kind: "token".to_string(),
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: false,
            description: None,
            bindings: vec![ShareBinding {
                app,
                provider_id: provider_id.to_string(),
                provider_type,
            }],
            binding_history: Vec::new(),
            runtime_snapshot: None,
            market_grant: None::<ShareMarketGrantStatus>,
            last_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
            config_revision: 0,
            router_synced_revision: 0,
            user_grants: BTreeMap::new(),
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
                pricing_model: None,
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
