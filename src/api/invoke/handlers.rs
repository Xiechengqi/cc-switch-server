use super::super::*;
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::runtime::authoritative_managed_account;

use crate::domain::accounts::oauth::{CLAUDE_WEB_PASTE_REDIRECT_URI, XAI_LOOPBACK_REDIRECT_URI};
use crate::domain::sharing::retired_fields::find_retired_share_field;
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareSettingsPatch, ShareTotalUsageEdit,
    ShareUserGrant, ShareUserUsageEdit, ShareUserUsageEditAction,
};

pub(in crate::api) async fn web_provider_health_json(
    state: &ServerState,
    app: AppKind,
    provider_id: &str,
) -> Result<Value, ApiError> {
    let providers = state.providers.read().await;
    let provider = providers
        .providers
        .iter()
        .find(|provider| provider.app == app && provider.provider.id == provider_id)
        .ok_or_else(|| ApiError::not_found("provider not found"))?;
    let plan = providers.runtime_plan(app, provider_id);
    let usage = state.usage.read().await;
    let health = crate::domain::health::provider_health_for_plan(provider, &usage, plan.as_deref());
    Ok(web_provider_health_value(&health))
}

pub(in crate::api) async fn web_provider_health_list_json(
    state: &ServerState,
    app: AppKind,
) -> Value {
    let providers = state.providers.read().await;
    let usage = state.usage.read().await;
    Value::Array(
        providers
            .providers
            .iter()
            .filter(|provider| provider.app == app)
            .map(|provider| {
                let plan = providers.runtime_plan(app, &provider.provider.id);
                let health = crate::domain::health::provider_health_for_plan(
                    provider,
                    &usage,
                    plan.as_deref(),
                );
                web_provider_health_value(&health)
            })
            .collect(),
    )
}

fn web_provider_health_value(health: &crate::domain::health::ProviderHealth) -> Value {
    use crate::domain::health::ProviderHealthStatus;

    let checked_at = health.checked_at_ms.map(|value| value.to_string());
    let successful = matches!(
        health.status,
        ProviderHealthStatus::Healthy | ProviderHealthStatus::Degraded
    );
    let failed = health.status == ProviderHealthStatus::Unhealthy;
    json!({
        "provider_id": health.provider_id,
        "app_type": health.app.as_str(),
        "status": health.status,
        "probe_support": health.probe_support,
        "available": health.available,
        "is_healthy": successful,
        "consecutive_successes": health.consecutive_successes,
        "consecutive_failures": health.consecutive_failures,
        "confirmation_pending": health.confirmation_pending,
        "last_success_at": successful.then(|| checked_at.clone()).flatten(),
        "last_failure_at": failed.then(|| checked_at.clone()).flatten(),
        "last_error": health.reason,
        "updated_at": checked_at.clone().unwrap_or_else(|| "0".to_string()),
        "checked_at": checked_at,
        "stale_at": health.stale_at_ms.map(|value| value.to_string()),
        "source": health.source,
        "latency_ms": health.probe_latency_ms,
        "model": health.model,
        "status_code": health.last_status_code,
        "error_category": health.error_category,
    })
}

#[cfg(test)]
mod provider_health_response_tests {
    use super::*;
    use crate::domain::health::{ProviderHealth, ProviderHealthStatus, ProviderProbeSupport};

    fn health(status: ProviderHealthStatus) -> ProviderHealth {
        ProviderHealth {
            provider_id: "p1".to_string(),
            app: AppKind::Codex,
            requests: 0,
            successes: 0,
            failures: 0,
            success_rate: None,
            avg_latency_ms: None,
            last_status_code: None,
            last_request_at_ms: None,
            healthy: status != ProviderHealthStatus::Unhealthy,
            available: true,
            status,
            probe_support: ProviderProbeSupport::Supported,
            checked_at_ms: None,
            stale_at_ms: None,
            source: None,
            probe_latency_ms: None,
            model: None,
            error_category: None,
            consecutive_successes: 0,
            consecutive_failures: 0,
            confirmation_pending: false,
            reason: None,
        }
    }

    #[test]
    fn unknown_health_is_not_fabricated_as_normal() {
        let value = web_provider_health_value(&health(ProviderHealthStatus::Unknown));
        assert_eq!(value["status"], "unknown");
        assert_eq!(value["is_healthy"], false);
        assert_eq!(value["available"], true);
        assert_eq!(value["updated_at"], "0");
    }

    #[test]
    fn unsupported_probe_capability_is_explicit() {
        let mut health = health(ProviderHealthStatus::Unknown);
        health.probe_support = ProviderProbeSupport::Unsupported;
        let value = web_provider_health_value(&health);
        assert_eq!(value["probe_support"], "unsupported");
        assert_eq!(value["status"], "unknown");
    }
}

pub(in crate::api) async fn web_resolve_stored_provider(
    state: &ServerState,
    args: &Value,
) -> Result<StoredProvider, ApiError> {
    let app = web_arg_app_type(args)?;
    let provider_id = web_arg_string_any(args, &["providerId", "provider_id"])?;
    resolve_provider_by_key(state, app, &provider_id).await
}

pub(in crate::api) async fn web_provider_health_check_config(
    state: &ServerState,
) -> crate::domain::providers::runtime::ProviderHealthCheckConfig {
    state.config.read().await.provider_health_check.clone()
}

pub(in crate::api) async fn web_proxy_target_provider_ids(
    state: &ServerState,
    app: AppKind,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    state
        .providers
        .read()
        .await
        .providers
        .iter()
        .filter(|provider| {
            provider.app == app
                && crate::domain::providers::bundle::surface_enabled(&provider.provider)
        })
        .map(|provider| provider.provider.id.clone())
        .collect::<HashSet<_>>()
}

pub(in crate::api) fn map_provider_test_to_stream_check_result(
    response: &TestProviderResponse,
    config: &crate::domain::providers::runtime::ProviderHealthCheckConfig,
) -> crate::domain::stream_check::StreamCheckResult {
    use crate::domain::stream_check::{HealthStatus, StreamCheckResult};
    let success = response.network_checked
        && response.network_error.is_none()
        && response
            .network_status_code
            .is_some_and(|status| (200..400).contains(&status))
        && response.network_stream_completed.unwrap_or(true);
    let latency = response
        .network_latency_ms
        .map(|value| value.min(u64::MAX as u128) as u64);
    let status = if !success {
        HealthStatus::Failed
    } else if latency.unwrap_or(0) > config.degraded_threshold_ms() {
        HealthStatus::Degraded
    } else {
        HealthStatus::Operational
    };
    StreamCheckResult {
        status,
        success,
        provider_revision: Some(response.provider_revision),
        message: if success {
            "Check succeeded".to_string()
        } else {
            response
                .network_error
                .clone()
                .unwrap_or_else(|| response.message.clone())
        },
        response_time_ms: latency,
        http_status: response.network_status_code,
        model_used: response.model.clone(),
        tested_at: chrono::Utc::now().timestamp(),
        retry_count: 0,
        error_category: provider_test_error_category(response),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
}

fn provider_test_error_category(response: &TestProviderResponse) -> Option<String> {
    if response.network_stream_completed == Some(false) {
        return Some("streamIncomplete".to_string());
    }
    if response.network_status_code == Some(404) {
        return Some("modelNotFound".to_string());
    }
    let category = match response.outcome {
        ProviderOperationOutcome::Success => return None,
        ProviderOperationOutcome::Unsupported => "unsupported",
        ProviderOperationOutcome::InvalidConfig => "invalidConfig",
        ProviderOperationOutcome::MissingCredential => "missingCredential",
        ProviderOperationOutcome::Auth => "auth",
        ProviderOperationOutcome::RateLimit => "rateLimit",
        ProviderOperationOutcome::Quota => "quotaExceeded",
        ProviderOperationOutcome::Timeout => "timeout",
        ProviderOperationOutcome::Network => "network",
        ProviderOperationOutcome::Upstream => "upstream",
        ProviderOperationOutcome::Protocol => "protocol",
    };
    Some(category.to_string())
}

pub(in crate::api) async fn web_fetch_models_for_config(
    state: &ServerState,
    args: &Value,
) -> Result<Value, ApiError> {
    let base_url = web_arg_string_any(args, &["baseUrl", "base_url"])?;
    let api_key = web_arg_string_any(args, &["apiKey", "api_key"])?;
    let models_url = web_optional_string_any(args, &["modelsUrl", "models_url"]);
    let url = models_url.unwrap_or_else(|| format!("{}/models", base_url.trim_end_matches('/')));
    let http_client = state.http_client().await;
    let response = http_client
        .get(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "fetch models failed: {}",
                redact_provider_test_error(&error.to_string())
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch models failed: {status}: {}",
            redact_provider_test_error(&body)
        )));
    }
    let raw = response
        .json::<Value>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse models failed: {error}")))?;
    let models = parse_provider_models(&raw)
        .into_iter()
        .map(|model| {
            json!({
                "id": model.id,
                "ownedBy": Value::Null,
                "displayName": model.display_name,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!(models))
}

pub(in crate::api) async fn web_patch_share_settings(
    state: &ServerState,
    args: &Value,
    patch: ShareSettingsPatch,
) -> Result<Share, ApiError> {
    let share_id = web_arg_share_id(args)?;
    let share = state
        .apply_share_settings_patch_immediate(&share_id, patch)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "settings_patched");
    Ok(share)
}

pub(in crate::api) async fn web_provider_quota(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
    provider_type: ProviderType,
) -> Result<Value, ApiError> {
    let account_id = web_optional_string_any(args, &["accountId", "account_id"]);
    let account_id = match account_id {
        Some(account_id) => account_id,
        None => {
            let accounts = state.accounts.read().await;
            let account = if provider_type == ProviderType::CodexOAuth {
                accounts.active_codex_oauth_account()
            } else {
                accounts.find_for_provider(provider_type, None)
            };
            let Some(account) = account else {
                return Ok(subscription_quota_not_found(managed_auth_provider_label(
                    provider_type,
                )));
            };
            account.id.clone()
        }
    };
    let account = state
        .find_account_by_id(&account_id)
        .await
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    ensure_provider_quota_account_type(&account, provider_type)?;
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id.clone()),
        Query(AccountQuotaQuery {
            refresh: Some(false),
            force: None,
        }),
    )
    .await?
    .0;
    let Some(account) = state.find_account_by_id(&account_id).await else {
        return Ok(Value::Null);
    };
    Ok(subscription_quota_from_response(
        &account,
        &response,
        managed_auth_provider_label(provider_type),
    ))
}

fn ensure_provider_quota_account_type(
    account: &Account,
    expected_provider_type: ProviderType,
) -> Result<(), ApiError> {
    if account.provider_type == expected_provider_type {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "account does not belong to {}",
        managed_auth_provider_label(expected_provider_type)
    )))
}

pub(in crate::api) async fn web_cached_oauth_quota(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
    refresh: bool,
    force: Option<bool>,
) -> Result<Value, ApiError> {
    require_session(state, headers).await?;
    let expected_provider_type = web_optional_auth_provider_type(args)?;
    let account_id = web_resolve_account_id(state, args).await?;
    let Some(account_id) = account_id else {
        return Ok(Value::Null);
    };
    let expected_auth_identity_generation = web_optional_u64(
        args,
        &["authIdentityGeneration", "auth_identity_generation"],
    );
    let account = state
        .find_account_by_id(&account_id)
        .await
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    if let Some(expected_provider_type) = expected_provider_type {
        if account.provider_type != expected_provider_type {
            return Err(ApiError::bad_request(format!(
                "account does not belong to {}",
                managed_auth_provider_label(expected_provider_type)
            )));
        }
    }
    if expected_auth_identity_generation
        .is_some_and(|expected| account.auth_identity_generation != expected)
    {
        return Err(ApiError::conflict(
            "OAuth account identity changed; reload the Provider binding",
        ));
    }
    let auth_provider = expected_provider_type
        .map(|provider_type| managed_auth_provider_label(provider_type).to_string())
        .or_else(|| web_optional_string_any(args, &["authProvider", "auth_provider"]))
        .unwrap_or_else(|| "unknown".to_string());
    let provider_id = web_optional_string_any(args, &["providerId", "provider_id"]);
    let app_type = web_optional_string_any(args, &["appType", "app_type", "app"]);
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id.clone()),
        Query(AccountQuotaQuery {
            refresh: Some(refresh),
            force,
        }),
    )
    .await?
    .0;
    let Some(account) = state.find_account_by_id(&account_id).await else {
        return Ok(Value::Null);
    };
    if expected_auth_identity_generation
        .is_some_and(|expected| account.auth_identity_generation != expected)
    {
        return Err(ApiError::conflict(
            "OAuth account identity changed while quota was loading",
        ));
    }
    Ok(cached_oauth_quota_from_response(
        &auth_provider,
        &account,
        &response,
        provider_id.as_deref(),
        app_type.as_deref(),
        if refresh { "refresh" } else { "cache" },
    ))
}

pub(in crate::api) async fn web_subscription_quota(
    state: &ServerState,
    headers: &HeaderMap,
    tool: &str,
    force: bool,
) -> Result<Value, ApiError> {
    let Some(provider_type) = subscription_tool_provider_type(tool) else {
        return Err(ApiError::bad_request(format!(
            "unsupported subscription quota tool: {tool}"
        )));
    };
    let account_id = {
        let accounts = state.accounts.read().await;
        managed_auth_default_account_id(&accounts, provider_type).map(str::to_string)
    };
    let Some(account_id) = account_id else {
        return Ok(subscription_quota_not_found(tool));
    };
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id.clone()),
        Query(AccountQuotaQuery {
            refresh: Some(true),
            force: Some(force),
        }),
    )
    .await?
    .0;
    let Some(account) = state.find_account_by_id(&account_id).await else {
        return Ok(subscription_quota_not_found(tool));
    };
    Ok(subscription_quota_from_response(&account, &response, tool))
}

pub(in crate::api) async fn web_resolve_account_id(
    state: &ServerState,
    args: &Value,
) -> Result<Option<String>, ApiError> {
    if let Some(account_id) = web_optional_string_any(args, &["accountId", "account_id", "id"]) {
        return Ok(Some(account_id));
    }

    let provider_id = web_optional_string_any(args, &["providerId", "provider_id"]);
    let app = web_optional_string_any(args, &["appType", "app", "app_type"])
        .map(|app| parse_app_kind(&app))
        .transpose()?;
    if let (Some(app), Some(provider_id)) = (app, provider_id.as_deref()) {
        let provider = {
            let providers = state.providers.read().await;
            providers
                .providers
                .iter()
                .find(|provider| provider.app == app && provider.provider.id == provider_id)
                .cloned()
        };
        if let Some(provider) = provider {
            let accounts = state.accounts.read().await;
            return Ok(authoritative_managed_account(&provider, &accounts)
                .map(|account| account.id.clone()));
        }
    }

    let provider_type = web_optional_auth_provider_type(args)?;
    if let Some(provider_type) = provider_type {
        let accounts = state.accounts.read().await;
        let account = if provider_type == ProviderType::CodexOAuth {
            accounts.active_codex_oauth_account()
        } else {
            accounts.find_for_provider(provider_type, None)
        };
        return Ok(account.map(|account| account.id.clone()));
    }

    Ok(None)
}

pub(in crate::api) async fn web_share_upsert_input(
    state: &ServerState,
    args: &Value,
) -> Result<UpsertShareInput, ApiError> {
    let value = web_payload(args, &["params", "input", "share"]);
    reject_retired_share_input_fields(value)?;
    if let Ok(input) = serde_json::from_value::<UpsertShareInput>(value.clone()) {
        return Ok(input);
    }

    let bindings_value = value.get("bindings").ok_or_else(|| {
        ApiError::bad_request("share params.bindings or app/providerId is required")
    })?;
    let binding_map = serde_json::from_value::<BTreeMap<String, String>>(bindings_value.clone())
        .map_err(ApiError::bad_request)?;
    if binding_map.is_empty() || binding_map.len() > 3 {
        return Err(ApiError::bad_request(
            "share must have between one and three bindings",
        ));
    }
    let app_name = web_optional_string_any(value, &["appType", "app", "app_type"])
        .or_else(|| binding_map.keys().next().cloned())
        .ok_or_else(|| ApiError::bad_request("share app is required"))?;
    let app = parse_app_kind(&app_name)?;
    let provider_id = binding_map
        .get(app.as_str())
        .cloned()
        .or_else(|| web_optional_string_any(value, &["providerId", "provider_id"]))
        .ok_or_else(|| ApiError::bad_request("share providerId is required"))?;
    let provider_id = provider_id.trim().to_string();
    if provider_id.is_empty() {
        return Err(ApiError::bad_request("share providerId is required"));
    }
    let provider_type = web_provider_type_for_binding(state, app, &provider_id).await?;
    let mut bindings = Vec::with_capacity(binding_map.len());
    for (binding_app, binding_provider_id) in binding_map {
        let binding_app = parse_app_kind(&binding_app)?;
        let binding_provider_id = binding_provider_id.trim().to_string();
        if binding_provider_id.is_empty() {
            return Err(ApiError::bad_request("share providerId is required"));
        }
        let binding_provider_type =
            web_provider_type_for_binding(state, binding_app, &binding_provider_id).await?;
        bindings.push(ShareBinding {
            app: binding_app,
            provider_id: binding_provider_id,
            provider_type: binding_provider_type,
        });
    }
    let expires_at = web_optional_i64(value, &["expiresAt", "expires_at"]).or_else(|| {
        web_optional_i64(value, &["expiresInSecs", "expires_in_secs"]).and_then(|seconds| {
            (seconds > 0).then(|| (now_ms() as i64).saturating_add(seconds.saturating_mul(1000)))
        })
    });

    let user_grants =
        web_optional_deserialize::<BTreeMap<String, ShareUserGrant>>(value, "userGrants")?
            .unwrap_or_default();
    Ok(UpsertShareInput {
        id: web_optional_string_any(value, &["id", "shareId", "share_id"]),
        owner_email: web_optional_string_any(value, &["ownerEmail", "owner_email"]),
        app,
        provider_id,
        provider_type,
        display_name: web_optional_string_any(value, &["displayName", "name"]),
        enabled: web_optional_bool(value, &["enabled"]),
        status: web_optional_string_any(value, &["status"]),
        subscription_level: None,
        account_email: None,
        quota_percent: None,
        tunnel_subdomain: web_optional_string_any(value, &["tunnelSubdomain", "subdomain"]),
        token_limit: web_optional_u64(value, &["tokenLimit", "token_limit"]),
        parallel_limit: web_optional_u32(value, &["parallelLimit", "parallel_limit"]),
        expires_at,
        free_access: web_share_free_access(value)?,
        allow_personal_credits: web_optional_bool(
            value,
            &["allowPersonalCredits", "allow_personal_credits"],
        ),
        auto_consume_banked_reset: web_optional_bool(
            value,
            &["autoConsumeBankedReset", "auto_consume_banked_reset"],
        ),
        banked_reset_expiry_lead_minutes: web_optional_u32(
            value,
            &[
                "bankedResetExpiryLeadMinutes",
                "banked_reset_expiry_lead_minutes",
            ],
        ),
        previous_response_cache_enabled: web_optional_bool(
            value,
            &[
                "previousResponseCacheEnabled",
                "previous_response_cache_enabled",
            ],
        ),
        grok_media_policy: web_optional_deserialize(value, "grokMediaPolicy")?,
        auto_start: web_optional_bool(value, &["autoStart", "auto_start"]),
        description: web_optional_string_any(value, &["description"]),
        enabled_apps: None,
        bindings,
        runtime_snapshot: None,
        user_grants,
    })
}

pub(in crate::api) async fn web_update_share_owner_email(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
) -> Result<Share, ApiError> {
    require_session(state, headers).await?;
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let owner_email = web_arg_string_any(value, &["ownerEmail", "owner_email"])?;
    web_require_client_owner_target(state, &owner_email).await?;
    state
        .shares
        .read()
        .await
        .get(&share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))
}

pub(in crate::api) async fn web_transfer_share_owner(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
) -> Result<Share, ApiError> {
    require_session(state, headers).await?;
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let target_email = web_arg_string_any(value, &["targetEmail", "target_email"])?;
    web_require_client_owner_target(state, &target_email).await?;
    state
        .shares
        .read()
        .await
        .get(&share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))
}

async fn web_require_client_owner_target(
    state: &ServerState,
    target: &str,
) -> Result<String, ApiError> {
    let target =
        crate::domain::settings::config::normalize_email(target).map_err(ApiError::bad_request)?;
    let owner = state
        .config
        .read()
        .await
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::conflict("client owner email is not configured"))?;
    if !owner.eq_ignore_ascii_case(&target) {
        return Err(ApiError::conflict(
            "share owner is managed by the client owner",
        ));
    }
    Ok(owner)
}

pub(in crate::api) async fn web_email_auth_request_code(
    state: &ServerState,
    args: &Value,
) -> Result<crate::clients::router::email_auth::EmailCodeRequestResponse, ApiError> {
    let router_domain = web_optional_string_any(args, &["routerDomain", "router_domain"]);
    let email = web_arg_string_any(args, &["email"])?;
    let config = ensure_email_router_config(state).await?;
    ensure_router_domain_matches(&config, router_domain.as_deref())?;
    let email = require_configured_owner_email(&config, &email)?;
    let http_client = state.http_client().await;
    crate::clients::router::email_auth::request_code(&http_client, &config, &email)
        .await
        .map_err(map_email_auth_error)
}

pub(in crate::api) async fn web_email_auth_verify_code(
    state: &ServerState,
    args: &Value,
) -> Result<crate::clients::router::email_auth::EmailAuthStatus, ApiError> {
    let router_domain = web_optional_string_any(args, &["routerDomain", "router_domain"]);
    let email = web_arg_string_any(args, &["email"])?;
    let code = web_arg_string_any(args, &["code"])?;
    let config = ensure_email_router_config(state).await?;
    ensure_router_domain_matches(&config, router_domain.as_deref())?;
    let email = require_configured_owner_email(&config, &email)?;
    let http_client = state.http_client().await;
    let router_session = crate::clients::router::email_auth::verify_client_web_code(
        &http_client,
        &config,
        &email,
        &code,
    )
    .await
    .map_err(map_email_auth_error)?;
    bind_verified_email_session(state, &config, &email, &router_session).await
}

pub(in crate::api) async fn web_email_auth_request_owner_change_code(
    _state: &ServerState,
    _args: &Value,
) -> Result<crate::clients::router::email_auth::EmailCodeRequestResponse, ApiError> {
    Err(ApiError::bad_request(
        "owner change no longer requires email verification; call email_auth_change_owner_email directly",
    ))
}

pub(in crate::api) async fn web_email_auth_change_owner_email(
    state: &ServerState,
    args: &Value,
) -> Result<crate::clients::router::email_auth::EmailAuthStatus, ApiError> {
    let router_domain = web_optional_string_any(args, &["routerDomain", "router_domain"]);
    let current_email = web_arg_string_any(args, &["currentEmail", "current_email"])?;
    let new_email = web_arg_string_any(args, &["newEmail", "new_email"])?;
    let config = ensure_email_router_config(state).await?;
    ensure_router_domain_matches(&config, router_domain.as_deref())?;
    let (current_email, new_email) =
        ensure_owner_change_allowed(&config, &current_email, &new_email)?;
    let http_client = state.http_client().await;
    let remote = crate::clients::router::email_auth::change_owner_email_at_installation(
        &http_client,
        &config,
        &current_email,
        &new_email,
    )
    .await
    .map_err(map_email_auth_error)?;
    if !remote.ok
        || !remote.old_email.eq_ignore_ascii_case(&current_email)
        || !remote.new_email.eq_ignore_ascii_case(&new_email)
    {
        return Err(ApiError::bad_gateway(
            "router accepted owner change but returned mismatched owner emails",
        ));
    }

    let mut next_config = config.clone();
    next_config.owner.email = Some(new_email.clone());
    state
        .replace_config(next_config.clone())
        .await
        .map_err(ApiError::internal)?;

    let updated_shares = state
        .try_mutate_shares_immediate(|store| store.bind_all_to_client_owner(&new_email))
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    for share in &updated_shares {
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(state, "share.changed", share, "owner_email_changed");
    }

    let email_state = crate::clients::router::email_auth::load_state(&state.config_dir)
        .map_err(ApiError::internal)?;
    if let Some(email_state) = email_state {
        if email_state.email.eq_ignore_ascii_case(&current_email) {
            crate::clients::router::email_auth::clear_state(&state.config_dir)
                .map_err(ApiError::internal)?;
        }
    }

    Ok(crate::clients::router::email_auth::EmailAuthStatus {
        authenticated: false,
        email: Some(new_email),
        expires_at: None,
        router_domain: config.router.domain.clone(),
    })
}

pub(in crate::api) fn web_email_auth_get_status(
    state: &ServerState,
) -> Result<crate::clients::router::email_auth::EmailAuthStatus, ApiError> {
    crate::clients::router::email_auth::get_status(&state.config_dir).map_err(ApiError::internal)
}

pub(in crate::api) async fn web_email_auth_session_me(
    state: &ServerState,
) -> Result<crate::clients::router::email_auth::EmailSessionMeResponse, ApiError> {
    let config = state.config.read().await.clone();
    crate::clients::router::email_auth::session_me(&state.config_dir, &config)
        .map_err(ApiError::internal)
}

pub(in crate::api) async fn web_email_auth_logout(state: &ServerState) -> Result<Value, ApiError> {
    if state
        .shares
        .read()
        .await
        .shares
        .iter()
        .any(|share| share.owner_email.is_some())
    {
        return Err(ApiError::bad_request(
            "this server has shares; owner email auth cannot be logged out",
        ));
    }
    crate::clients::router::email_auth::clear_state(&state.config_dir)
        .map_err(ApiError::internal)?;
    Ok(json!({ "ok": true }))
}

async fn bind_verified_email_session(
    state: &ServerState,
    config: &ServerConfig,
    email: &str,
    router_session: &crate::clients::router::email_auth::RouterVerifyEmailCodeResponse,
) -> Result<crate::clients::router::email_auth::EmailAuthStatus, ApiError> {
    let verified_email =
        crate::clients::router::email_auth::normalize_email(&router_session.user.email)
            .map_err(map_email_auth_error)?;
    if verified_email != email {
        return Err(ApiError::unauthorized(
            "verified email does not match configured owner email",
        ));
    }
    let http_client = state.http_client().await;
    let owner_binding = crate::clients::router::email_auth::bind_owner_email(
        &http_client,
        config,
        email,
        &router_session.access_token,
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            crate::clients::router::email_auth::humanize_remote_owner_binding_error(&error.message),
        )
    })?;
    let bound_email =
        crate::clients::router::email_auth::normalize_email(&owner_binding.owner_email)
            .map_err(map_email_auth_error)?;
    if !owner_binding.ok || bound_email != email {
        return Err(ApiError::bad_gateway(
            "router accepted email code but did not bind the configured owner email",
        ));
    }
    let email_state =
        crate::clients::router::email_auth::state_from_router_session(config, router_session)
            .map_err(map_email_auth_error)?;
    crate::clients::router::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;
    Ok(crate::clients::router::email_auth::EmailAuthStatus {
        authenticated: true,
        email: Some(email.to_string()),
        expires_at: email_state.expires_at,
        router_domain: email_state.router_domain,
    })
}

fn ensure_owner_change_allowed(
    config: &ServerConfig,
    current_email: &str,
    new_email: &str,
) -> Result<(String, String), ApiError> {
    let current_email = crate::clients::router::email_auth::normalize_email(current_email)
        .map_err(map_email_auth_error)?;
    let new_email = crate::clients::router::email_auth::normalize_email(new_email)
        .map_err(map_email_auth_error)?;
    if current_email == new_email {
        return Err(ApiError::bad_request(
            "new owner email must be different from current owner email",
        ));
    }
    let configured_owner = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("owner email is not configured"))?
        .trim()
        .to_ascii_lowercase();
    if configured_owner != current_email {
        return Err(ApiError::unauthorized(
            "current email does not match configured owner email",
        ));
    }
    Ok((current_email, new_email))
}

fn ensure_router_domain_matches(
    config: &ServerConfig,
    router_domain: Option<&str>,
) -> Result<(), ApiError> {
    let Some(router_domain) = router_domain
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let Some(configured_domain) = config.router.domain.as_deref() else {
        return Ok(());
    };
    if configured_domain.trim().eq_ignore_ascii_case(router_domain) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "router domain does not match configured router",
        ))
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveProviderBundleShareCommand {
    bundle_id: String,
    #[serde(default)]
    share_id: Option<String>,
    #[serde(default)]
    expected_config_revision: Option<u64>,
    enabled: bool,
    subdomain: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    free_access: Option<bool>,
    token_limit: i64,
    parallel_limit: i64,
    expires_at: String,
    #[serde(default)]
    user_grants: Option<BTreeMap<String, ShareUserGrant>>,
    #[serde(default)]
    user_usage_edits: Option<BTreeMap<String, ShareUserUsageEdit>>,
    #[serde(default)]
    share_usage_edit: Option<ShareTotalUsageEdit>,
    #[serde(default)]
    allow_personal_credits: bool,
    #[serde(default)]
    auto_consume_banked_reset: bool,
    #[serde(default = "default_provider_bundle_banked_reset_expiry_lead_minutes")]
    banked_reset_expiry_lead_minutes: u32,
    #[serde(default = "default_provider_bundle_previous_response_cache_enabled")]
    previous_response_cache_enabled: bool,
    #[serde(default)]
    grok_media_policy: crate::domain::sharing::router_contract::GrokMediaPolicy,
}

fn default_provider_bundle_banked_reset_expiry_lead_minutes() -> u32 {
    crate::domain::sharing::shares::DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES
}

fn default_provider_bundle_previous_response_cache_enabled() -> bool {
    true
}

fn provider_bundle_share_conflict(message: impl Into<String>) -> ApiError {
    ApiError::conflict_code("cc_switch_provider_bundle_share_conflict", message)
}

fn provider_bundle_share_target<'a>(
    store: &'a ShareStore,
    bundle_keys: &BTreeSet<(AppKind, String)>,
    command: &SaveProviderBundleShareCommand,
) -> Result<Option<&'a Share>, ApiError> {
    let mut matches = store.shares.iter().filter(|share| {
        share.status != "deleted"
            && share
                .bindings
                .iter()
                .any(|binding| bundle_keys.contains(&(binding.app, binding.provider_id.clone())))
    });
    let current = matches.next();
    if matches.next().is_some() {
        return Err(provider_bundle_share_conflict(
            "Provider Bundle is referenced by more than one Share",
        ));
    }
    match current {
        Some(current) => {
            if command.share_id.as_deref() != Some(current.id.as_str()) {
                return Err(provider_bundle_share_conflict(
                    "Provider Bundle Share changed since this editor was opened",
                ));
            }
            let expected = command.expected_config_revision.ok_or_else(|| {
                provider_bundle_share_conflict(
                    "expectedConfigRevision is required for an existing Provider Bundle Share",
                )
            })?;
            if current.config_revision != expected {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    format!(
                        "Share changed since this editor was opened (expected revision {expected}, current revision {})",
                        current.config_revision
                    ),
                ));
            }
            Ok(Some(current))
        }
        None => {
            if command.share_id.is_some() || command.expected_config_revision.is_some() {
                return Err(provider_bundle_share_conflict(
                    "Provider Bundle Share no longer exists",
                ));
            }
            Ok(None)
        }
    }
}

fn provider_bundle_share_limits(
    command: &SaveProviderBundleShareCommand,
) -> Result<(Option<u64>, Option<u32>), ApiError> {
    let token_limit = match command.token_limit {
        -1 => None,
        value if value >= 0 => Some(value as u64),
        _ => {
            return Err(ApiError::bad_request(
                "tokenLimit must be -1 or a non-negative integer",
            ));
        }
    };
    let parallel_limit = match command.parallel_limit {
        -1 => None,
        value if value >= 0 => Some(
            u32::try_from(value)
                .map_err(|_| ApiError::bad_request("parallelLimit exceeds the supported range"))?,
        ),
        _ => {
            return Err(ApiError::bad_request(
                "parallelLimit must be -1 or a non-negative integer",
            ));
        }
    };
    Ok((token_limit, parallel_limit))
}

fn provider_bundle_share_expiration(value: &str) -> Result<i64, ApiError> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .map(|value| value.timestamp_millis())
        .map_err(|_| ApiError::bad_request("expiresAt must be an RFC3339 timestamp"))
}

/// Operator-attributable trail for manual quota corrections.
///
/// The snapshot a rebase produces is indistinguishable from real traffic, so
/// who changed what has to be recorded at the moment it is applied.
fn log_share_usage_edits(
    share_id: &str,
    operator: Option<&str>,
    edits: &BTreeMap<String, ShareUserUsageEdit>,
) {
    for (email, edit) in edits {
        tracing::info!(
            share_id,
            operator = operator.unwrap_or("unattributed"),
            user_email = email.as_str(),
            action = ?edit.action,
            target_tokens = edit.target_tokens,
            source = ?edit.source,
            "Share user consumed-token baseline changed by an operator"
        );
    }
}

fn log_share_total_usage_edit(share_id: &str, operator: Option<&str>, edit: &ShareTotalUsageEdit) {
    tracing::info!(
        share_id,
        operator = operator.unwrap_or("unattributed"),
        action = ?edit.action,
        tokens_used = edit.tokens_used,
        "Share total consumed-token counter changed by an operator"
    );
}

#[allow(clippy::too_many_arguments)]
fn stage_provider_bundle_share(
    store: &mut ShareStore,
    bundle_keys: &BTreeSet<(AppKind, String)>,
    bindings: &[ShareBinding],
    enabled_apps: &BTreeSet<AppKind>,
    capacity_pool_id: &str,
    bundle_name: &str,
    owner_email: &str,
    command: &SaveProviderBundleShareCommand,
    create_share_id: Option<&str>,
    usage: &crate::domain::usage::store::UsageStore,
    applied_at_ms: i64,
    operator: Option<&str>,
) -> Result<Option<Share>, ApiError> {
    let current = provider_bundle_share_target(store, bundle_keys, command)?.cloned();
    if current.is_none() && !command.enabled {
        return Ok(None);
    }
    let (token_limit, parallel_limit) = provider_bundle_share_limits(command)?;
    let expires_at = provider_bundle_share_expiration(&command.expires_at)?;
    let free_access = command.free_access.unwrap_or(false);
    let subdomain =
        (!command.subdomain.trim().is_empty()).then(|| command.subdomain.trim().to_string());
    let description = command
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(current) = current {
        let share = store
            .replace_bundle_configuration_with_usage(
                &current.id,
                bindings.to_vec(),
                capacity_pool_id.to_string(),
                subdomain,
                ShareSettingsPatch {
                    description: Some(description),
                    free_access: Some(free_access),
                    token_limit: Some(command.token_limit),
                    parallel_limit: Some(command.parallel_limit),
                    expires_at: Some(command.expires_at.clone()),
                    allow_personal_credits: Some(command.allow_personal_credits),
                    auto_consume_banked_reset: Some(command.auto_consume_banked_reset),
                    banked_reset_expiry_lead_minutes: Some(
                        command.banked_reset_expiry_lead_minutes,
                    ),
                    previous_response_cache_enabled: Some(command.previous_response_cache_enabled),
                    support: Some(crate::domain::sharing::shares::support_from_enabled_apps(
                        enabled_apps,
                    )),
                    user_grants: command.user_grants.clone(),
                    ..ShareSettingsPatch::default()
                },
                command.user_usage_edits.as_ref(),
                command.enabled,
                usage,
                applied_at_ms,
                operator,
            )
            .map_err(map_share_patch_error)?;
        if let Some(edits) = command.user_usage_edits.as_ref() {
            log_share_usage_edits(&share.id, operator, edits);
        }
        if let Some(edit) = command.share_usage_edit.as_ref() {
            store
                .apply_share_total_usage_edit(&share.id, edit, applied_at_ms)
                .map_err(map_share_patch_error)?;
            log_share_total_usage_edit(&share.id, operator, edit);
            return Ok(Some(
                store
                    .get(&share.id)
                    .cloned()
                    .expect("Provider Bundle Share remains in the candidate store"),
            ));
        }
        return Ok(Some(share));
    }

    let primary = bindings
        .iter()
        .find(|binding| enabled_apps.contains(&binding.app))
        .or(bindings.first())
        .expect("Provider Bundle has at least one Surface");
    let share = store
        .upsert_with_capacity(
            UpsertShareInput {
                id: create_share_id.map(str::to_string),
                owner_email: Some(owner_email.to_string()),
                app: primary.app,
                provider_id: primary.provider_id.clone(),
                provider_type: primary.provider_type,
                display_name: Some(bundle_name.to_string()),
                enabled: Some(true),
                status: Some("active".to_string()),
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: subdomain,
                token_limit,
                parallel_limit,
                expires_at: Some(expires_at),
                free_access: Some(free_access),
                allow_personal_credits: Some(command.allow_personal_credits),
                auto_consume_banked_reset: Some(command.auto_consume_banked_reset),
                banked_reset_expiry_lead_minutes: Some(command.banked_reset_expiry_lead_minutes),
                previous_response_cache_enabled: Some(command.previous_response_cache_enabled),
                grok_media_policy: Some(command.grok_media_policy),
                auto_start: Some(true),
                description,
                enabled_apps: {
                    let bound = bindings
                        .iter()
                        .map(|binding| binding.app)
                        .collect::<BTreeSet<_>>();
                    if *enabled_apps == bound {
                        None
                    } else {
                        Some(enabled_apps.clone())
                    }
                },
                bindings: bindings.to_vec(),
                runtime_snapshot: None,
                user_grants: command.user_grants.clone().unwrap_or_default(),
            },
            Some(capacity_pool_id.to_string()),
        )
        .map_err(map_share_patch_error)?;
    if let Some(edits) = command.user_usage_edits.as_ref() {
        let original = store
            .get(&share.id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("share not found"))?;
        store
            .apply_user_usage_edits(&share.id, &original, edits, usage, applied_at_ms, operator)
            .map_err(map_share_patch_error)?;
        log_share_usage_edits(&share.id, operator, edits);
    }
    if let Some(edit) = command.share_usage_edit.as_ref() {
        store
            .apply_share_total_usage_edit(&share.id, edit, applied_at_ms)
            .map_err(map_share_patch_error)?;
        log_share_total_usage_edit(&share.id, operator, edit);
    }
    store
        .rebuild_user_usage_from_history(&share.id, usage, applied_at_ms)
        .map_err(map_share_patch_error)?;
    Ok(Some(store.get(&share.id).cloned().expect(
        "new Provider Bundle Share remains in the candidate store",
    )))
}

pub(in crate::api) async fn web_save_provider_bundle_share(
    state: &ServerState,
    args: &Value,
    operator: Option<String>,
) -> Result<Option<Share>, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    reject_retired_share_input_fields(value)?;
    let command = serde_json::from_value::<SaveProviderBundleShareCommand>(value.clone())
        .map_err(ApiError::bad_request)?;
    if command.bundle_id.trim().is_empty() || command.bundle_id != command.bundle_id.trim() {
        return Err(ApiError::bad_request(
            "bundleId must be non-empty and trimmed",
        ));
    }
    if command
        .share_id
        .as_deref()
        .is_some_and(|share_id| share_id.trim().is_empty() || share_id != share_id.trim())
    {
        return Err(ApiError::bad_request(
            "shareId must be non-empty and trimmed",
        ));
    }
    provider_bundle_share_limits(&command)?;
    provider_bundle_share_expiration(&command.expires_at)?;
    if !(crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES
        ..=crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES)
        .contains(&command.banked_reset_expiry_lead_minutes)
    {
        return Err(ApiError::bad_request(format!(
            "bankedResetExpiryLeadMinutes must be between {} and {}",
            crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES,
            crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES
        )));
    }

    let config = state.config.read().await.clone();
    let owner_email = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::conflict("client owner email is not configured"))?
        .to_string();
    let reference_guard = state.lock_reference_mutations().await;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares = state.shares.read().await.clone();

    let surfaces = providers
        .providers
        .iter()
        .filter(|stored| {
            crate::domain::providers::bundle::is_explicit_bundle_surface(&stored.provider)
                && crate::domain::providers::bundle::bundle_id(&stored.provider)
                    == Some(command.bundle_id.as_str())
        })
        .collect::<Vec<_>>();
    if surfaces.is_empty() {
        return Err(ApiError::not_found("Provider Bundle not found"));
    }
    let bundle_name = surfaces[0].provider.name.clone();
    let bundle_keys = surfaces
        .iter()
        .map(|stored| (stored.app, stored.provider.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut bindings = surfaces
        .iter()
        .map(|stored| ShareBinding {
            app: stored.app,
            provider_id: stored.provider.id.clone(),
            provider_type: stored.provider_type,
        })
        .collect::<Vec<_>>();
    bindings.sort_by_key(|binding| binding.app);
    let enabled_apps = surfaces
        .iter()
        .filter(|stored| crate::domain::providers::bundle::surface_enabled(&stored.provider))
        .map(|stored| stored.app)
        .collect::<BTreeSet<_>>();
    if enabled_apps.is_empty() {
        return Err(provider_bundle_share_conflict(
            "Provider Bundle has no enabled Surface",
        ));
    }
    let root_key =
        crate::infra::credentials::load_root_key(&state.config_dir).map_err(ApiError::internal)?;
    let capacity_pool_id =
        crate::domain::sharing::credential_source::capacity_pool_id_for_bindings(
            &providers,
            &accounts,
            &bindings,
            &root_key.key,
        )
        .map_err(map_credential_source_error)?;

    let previous = provider_bundle_share_target(&shares, &bundle_keys, &command)?.cloned();
    let mut staged = shares.clone();
    let staged_share = stage_provider_bundle_share(
        &mut staged,
        &bundle_keys,
        &bindings,
        &enabled_apps,
        &capacity_pool_id,
        &bundle_name,
        &owner_email,
        &command,
        None,
        &usage,
        crate::infra::time::now_ms() as i64,
        operator.as_deref(),
    )?;
    if staged_share.is_none() {
        return Ok(None);
    }
    crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
        &providers,
        &accounts,
        &shares,
        &providers,
        &accounts,
        &staged,
    )
    .map_err(map_subscription_binding_error)?;
    let staged_share = staged_share.expect("staged Provider Bundle Share exists");
    let create_share_id = previous.is_none().then_some(staged_share.id.clone());
    let subdomain_changed = previous
        .as_ref()
        .and_then(|share| share.tunnel_subdomain.as_deref())
        != staged_share.tunnel_subdomain.as_deref();
    let was_running = previous
        .as_ref()
        .is_some_and(crate::state::should_restore_share_tunnel);
    let _binding_mutation = if previous
        .as_ref()
        .is_some_and(|share| share.bindings != staged_share.bindings)
    {
        Some(
            state
                .lock_share_binding_mutation(&staged_share.id)
                .await
                .ok_or_else(share_in_flight_error)?,
        )
    } else {
        None
    };

    let mut remote_subdomain_claimed = false;
    if subdomain_changed && previous.is_some() && config.has_registered_router_identity() {
        let descriptor = descriptor_for_share_with_accounts_and_usage(
            &staged_share,
            &providers,
            Some(&accounts),
            Some(&usage),
        );
        let http_client = state.http_client().await;
        if let Err(error) =
            crate::clients::router::client::claim_share_subdomain(&http_client, &config, descriptor)
                .await
        {
            if let Err(reconcile_error) =
                crate::state::reconcile_router_share_after_failed_claim(state, &staged_share.id)
                    .await
            {
                tracing::warn!(
                    share_id = %staged_share.id,
                    error = %reconcile_error,
                    "Router Share reconciliation after an uncertain Bundle subdomain claim failed"
                );
            }
            return Err(ApiError::bad_gateway(error.to_string()));
        }
        remote_subdomain_claimed = true;
    }

    let command_for_commit = command.clone();
    let bundle_keys_for_commit = bundle_keys.clone();
    let bindings_for_commit = bindings.clone();
    let enabled_apps_for_commit = enabled_apps.clone();
    let capacity_pool_id_for_commit = capacity_pool_id.clone();
    let bundle_name_for_commit = bundle_name.clone();
    let owner_email_for_commit = owner_email.clone();
    let providers_for_commit = providers.clone();
    let accounts_for_commit = accounts.clone();
    let saved = match state
        .try_mutate_share_quota_immediate(|store, current_usage, applied_at_ms| {
            let current = store.clone();
            let saved = stage_provider_bundle_share(
                store,
                &bundle_keys_for_commit,
                &bindings_for_commit,
                &enabled_apps_for_commit,
                &capacity_pool_id_for_commit,
                &bundle_name_for_commit,
                &owner_email_for_commit,
                &command_for_commit,
                create_share_id.as_deref(),
                current_usage,
                applied_at_ms,
                operator.as_deref(),
            )?;
            crate::domain::sharing::subscription_identity::validate_subscription_reference_graph_transition(
                &providers_for_commit,
                &accounts_for_commit,
                &current,
                &providers_for_commit,
                &accounts_for_commit,
                store,
            )
            .map_err(map_subscription_binding_error)?;
            saved.ok_or_else(|| {
                provider_bundle_share_conflict(
                    "Provider Bundle Share disappeared before it could be committed",
                )
            })
        })
        .await
    {
        Ok(Ok(saved)) => saved,
        Ok(Err(error)) => {
            if remote_subdomain_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(state, &staged_share.id)
                        .await
                {
                    tracing::warn!(
                        share_id = %staged_share.id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a rejected Bundle Share save failed"
                    );
                }
            }
            return Err(error);
        }
        Err(error) => {
            if remote_subdomain_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(state, &staged_share.id)
                        .await
                {
                    tracing::warn!(
                        share_id = %staged_share.id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a failed Bundle Share commit failed"
                    );
                }
            }
            return Err(ApiError::internal(error));
        }
    };
    drop(reference_guard);

    if !crate::state::should_restore_share_tunnel(&saved) {
        if was_running {
            crate::state::stop_share_tunnel(state, &saved.id).await;
        }
    } else if subdomain_changed && previous.is_some() {
        crate::state::force_reconnect_share_tunnel(
            state.clone(),
            saved.id.clone(),
            "provider_bundle_share_subdomain_changed",
        )
        .await;
    } else {
        crate::state::ensure_share_tunnel_running_for(
            state.clone(),
            &saved.id,
            "provider_bundle_share_saved",
        )
        .await;
    }
    if let Err(error) = crate::api::router::sync_share_upsert(state.clone(), saved.clone()).await {
        tracing::warn!(
            share_id = %saved.id,
            %error,
            "Provider Bundle Share was saved locally; Router sync remains pending"
        );
    }
    let saved = state
        .shares
        .read()
        .await
        .get(&saved.id)
        .cloned()
        .unwrap_or(saved);
    emit_share_event(
        state,
        "share.changed",
        &saved,
        "provider_bundle_share_saved",
    );
    Ok(Some(saved))
}

pub(in crate::api) async fn web_save_provider_share(
    state: &ServerState,
    args: &Value,
    operator: Option<String>,
) -> Result<Share, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    reject_retired_share_input_fields(value)?;
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let expected_config_revision = web_optional_i64(
        value,
        &["expectedConfigRevision", "expected_config_revision"],
    )
    .and_then(|revision| u64::try_from(revision).ok())
    .ok_or_else(|| ApiError::bad_request("expectedConfigRevision is required"))?;
    let subdomain = web_arg_string_any(value, &["subdomain"])?;
    let description = web_optional_string_any(value, &["description"]);
    let free_access = web_share_free_access(value)?.unwrap_or(false);
    let user_grants =
        web_optional_deserialize::<BTreeMap<String, ShareUserGrant>>(value, "userGrants")?
            .unwrap_or_default();
    let user_usage_edits =
        web_optional_deserialize::<BTreeMap<String, ShareUserUsageEdit>>(value, "userUsageEdits")?;
    let share_usage_edit =
        web_optional_deserialize::<ShareTotalUsageEdit>(value, "shareUsageEdit")?;
    // Fail fast, before the remote subdomain claim below can produce an
    // external side effect.  The authoritative write still happens inside the
    // quota lock so a concurrent invocation cannot interleave with it.
    if share_usage_edit.as_ref().is_some_and(|edit| {
        edit.action == ShareUserUsageEditAction::Set && edit.tokens_used.is_none()
    }) {
        return Err(ApiError::bad_request(
            "shareUsageEdit.tokensUsed is required for action set",
        ));
    }
    let token_limit = web_optional_i64(value, &["tokenLimit", "token_limit"])
        .ok_or_else(|| ApiError::bad_request("tokenLimit is required"))?;
    let parallel_limit = web_optional_i64(value, &["parallelLimit", "parallel_limit"])
        .ok_or_else(|| ApiError::bad_request("parallelLimit is required"))?;
    let expires_at = web_arg_string_any(value, &["expiresAt", "expires_at"])?;

    let usage_for_quota = state.usage.read().await.clone();
    let mut staged = state.shares.read().await.clone();
    let current = staged
        .get(&share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    if current.config_revision != expected_config_revision {
        return Err(ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            format!(
                "Share changed since this editor was opened (expected revision {}, current revision {})",
                expected_config_revision, current.config_revision
            ),
        ));
    }
    let subdomain_changed = current.tunnel_subdomain.as_deref() != Some(subdomain.as_str());
    let was_running = current.enabled && current.status == "active";

    staged
        .update_subdomain(&share_id, subdomain)
        .map_err(map_share_patch_error)?;
    staged
        .apply_settings_patch_with_usage(
            &share_id,
            ShareSettingsPatch {
                description: Some(description),
                free_access: Some(free_access),
                token_limit: Some(token_limit),
                parallel_limit: Some(parallel_limit),
                expires_at: Some(expires_at),
                user_grants: Some(user_grants),
                ..ShareSettingsPatch::default()
            },
            &usage_for_quota,
            crate::infra::time::now_ms() as i64,
        )
        .map_err(map_share_patch_error)?;
    // Reject an impossible baseline before the remote subdomain claim below can
    // produce an external side effect.  This pass is validation only: the
    // authoritative rebase is written inside the quota lock, against the locked
    // Usage snapshot and the commit timestamp, so a request that lands between
    // this check and the commit still counts on top of the operator baseline
    // and a window rollover cannot strand the rebase in a stale window.
    if let Some(edits) = user_usage_edits.as_ref() {
        staged
            .clone()
            .apply_user_usage_edits(
                &share_id,
                &current,
                edits,
                &usage_for_quota,
                crate::infra::time::now_ms() as i64,
                operator.as_deref(),
            )
            .map_err(map_share_patch_error)?;
    }
    let candidate = staged
        .get(&share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    staged
        .replace_configured_share(candidate.clone())
        .map_err(map_share_patch_error)?;

    let mut remote_subdomain_claimed = false;
    if subdomain_changed {
        let config = state.config.read().await.clone();
        if config.has_registered_router_identity() {
            let providers = state.providers.read().await.clone();
            let accounts = state.accounts.read().await.clone();
            let usage = state.usage.read().await.clone();
            let descriptor = descriptor_for_share_with_accounts_and_usage(
                &candidate,
                &providers,
                Some(&accounts),
                Some(&usage),
            );
            let http_client = state.http_client().await;
            if let Err(error) = crate::clients::router::client::claim_share_subdomain(
                &http_client,
                &config,
                descriptor,
            )
            .await
            {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(state, &share_id).await
                {
                    tracing::warn!(
                        share_id,
                        error = %reconcile_error,
                        "Router Share reconciliation after an uncertain subdomain claim failed"
                    );
                }
                return Err(ApiError::bad_gateway(error.to_string()));
            }
            remote_subdomain_claimed = true;
        }
    }

    let saved = match state
        .try_mutate_share_quota_immediate(|store, usage, applied_at_ms| {
            let current = store
                .get(&share_id)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            if current.config_revision != expected_config_revision {
                return Err(ApiError::conflict_code(
                    "cc_switch_share_revision_conflict",
                    format!(
                        "Share changed since this editor was opened (expected revision {}, current revision {})",
                        expected_config_revision, current.config_revision
                    ),
                ));
            }
            let original = current.clone();
            store
                .replace_configured_share(candidate)
                .map_err(map_share_patch_error)?;
            if let Some(edits) = user_usage_edits.as_ref() {
                store
                    .apply_user_usage_edits(
                        &share_id,
                        &original,
                        edits,
                        usage,
                        applied_at_ms,
                        operator.as_deref(),
                    )
                    .map_err(map_share_patch_error)?;
                log_share_usage_edits(&share_id, operator.as_deref(), edits);
            }
            if let Some(edit) = share_usage_edit.as_ref() {
                store
                    .apply_share_total_usage_edit(&share_id, edit, applied_at_ms)
                    .map_err(map_share_patch_error)?;
                log_share_total_usage_edit(&share_id, operator.as_deref(), edit);
            }
            store
                .rebuild_user_usage_from_history(&share_id, usage, applied_at_ms)
                .map_err(map_share_patch_error)?;
            store
                .get(&share_id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
    {
        Ok(Ok(saved)) => saved,
        Ok(Err(error)) => {
            if remote_subdomain_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(state, &share_id).await
                {
                    tracing::warn!(
                        share_id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a rejected local save failed"
                    );
                }
            }
            return Err(error);
        }
        Err(error) => {
            if remote_subdomain_claimed {
                if let Err(reconcile_error) =
                    crate::state::reconcile_router_share_after_failed_claim(state, &share_id).await
                {
                    tracing::warn!(
                        share_id,
                        error = %reconcile_error,
                        "Router Share reconciliation after a failed local save failed"
                    );
                }
            }
            return Err(ApiError::internal(error));
        }
    };
    if subdomain_changed && was_running {
        crate::state::force_reconnect_share_tunnel(
            state.clone(),
            share_id.clone(),
            "share_subdomain_changed",
        )
        .await;
    }
    crate::api::router::sync_share_upsert(state.clone(), saved.clone())
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!(
                "share was saved locally but router sync is pending: {error}"
            ))
        })?;
    let saved = state
        .shares
        .read()
        .await
        .get(&share_id)
        .cloned()
        .unwrap_or(saved);
    emit_share_event(
        state,
        "share.changed",
        &saved,
        "provider_share_settings_saved",
    );
    Ok(saved)
}

pub(in crate::api) fn expected_client_tunnel_url(
    client_subdomain: &str,
    router_domain: &str,
) -> Option<String> {
    let client_subdomain = client_subdomain.trim();
    let router_domain = router_domain.trim();
    if client_subdomain.is_empty() || router_domain.is_empty() {
        None
    } else {
        Some(format!("https://{client_subdomain}.{router_domain}"))
    }
}

pub(in crate::api) fn web_client_tunnel_share_status(
    runtime: Option<crate::clients::router::tunnel::TunnelRuntimeStatus>,
) -> Value {
    let last_error = runtime
        .as_ref()
        .and_then(|status| status.last_error.clone());
    let info = runtime.and_then(|status| {
        let tunnel_url = status.tunnel_url.clone()?;
        Some(json!({
            "tunnelUrl": tunnel_url,
            "subdomain": status.subdomain.clone().unwrap_or_default(),
            "remotePort": status.remote_port.unwrap_or(0),
            "healthy": tunnel_runtime_is_healthy(status.status.as_str()),
            "status": status.status,
            "kind": status.kind,
            "generation": status.generation,
            "desiredGeneration": status.desired_generation,
            "transportState": status.transport_state,
            "startReason": status.start_reason,
        }))
    });
    json!({
        "info": info,
        "lastError": last_error,
        "requiresOwnerLogin": false,
    })
}

pub(in crate::api) async fn web_configure_share_tunnel(
    state: &ServerState,
    args: &Value,
) -> Result<(), ApiError> {
    let value = web_payload(args, &["config", "params", "input"]);
    let domain = web_optional_string_any(value, &["domain"])
        .ok_or_else(|| ApiError::bad_request("domain is required"))?;
    let domain =
        crate::domain::sharing::share_router_domain::normalize_share_router_domain(&domain)
            .map_err(ApiError::bad_request)?;

    state
        .apply_ui_settings_patch_immediate(json!({ "shareRouterDomain": domain }))
        .await
        .map_err(ApiError::internal)?;

    let region =
        crate::domain::sharing::share_router_domain::share_router_region_for_domain(&domain);
    let mut config = state.config.read().await.clone();
    config.router.domain = Some(domain.clone());
    if let Some(region) = region {
        config.router.region = Some(region.to_string());
    }
    if config
        .router
        .url
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        config.router.url = Some(format!("https://{domain}"));
    }
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

pub(in crate::api) async fn web_client_tunnel_state(state: &ServerState) -> Value {
    let config = state.config.read().await;
    let ui_settings = state.ui_settings.read().await;
    let router_domain = ui_settings
        .settings_for_frontend(&config)
        .get("shareRouterDomain")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let runtime = state
        .tunnels
        .status(&crate::clients::router::tunnel::client_tunnel_key())
        .await;
    let active_tunnel_url = runtime
        .as_ref()
        .and_then(|status| status.tunnel_url.clone());
    let subdomain = config.client.tunnel_subdomain.clone().unwrap_or_default();
    let expected_tunnel_url = expected_client_tunnel_url(&subdomain, &router_domain);
    let tunnel_url = active_tunnel_url
        .clone()
        .or_else(|| expected_tunnel_url.clone());
    let owner_email = config.owner.email.clone().unwrap_or_default();
    let enabled = matches!(
        config.client.tunnel_status.as_deref(),
        Some("active") | Some("running") | Some("connected")
    ) || runtime
        .as_ref()
        .is_some_and(|status| tunnel_runtime_is_healthy(status.status.as_str()));
    let status = web_client_tunnel_share_status(runtime);
    let mut response = json!({
        "config": {
            "ownerEmail": owner_email,
            "subdomain": subdomain,
            "enabled": enabled,
            "autoStart": true,
            "tunnelUrl": tunnel_url,
            "expectedUrl": expected_tunnel_url,
        }
    });
    if let Value::Object(ref mut map) = response {
        map.insert("status".into(), status);
    }
    response
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ShareHealthLevel {
    Healthy,
    Warning,
    Unhealthy,
}

impl ShareHealthLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Unhealthy => "unhealthy",
        }
    }
}

fn tunnel_runtime_is_healthy(status: &str) -> bool {
    matches!(
        status,
        "connected" | "running" | "active" | "renewing" | "renewal_retrying"
    )
}

fn share_health_level(
    enabled: bool,
    share_status: &str,
    router_sync_error: Option<&str>,
    tunnel_error: Option<&str>,
) -> ShareHealthLevel {
    if router_sync_error.is_some() || (enabled && tunnel_error.is_some()) {
        return ShareHealthLevel::Unhealthy;
    }
    if !enabled {
        return ShareHealthLevel::Warning;
    }
    if share_status != "active" {
        return ShareHealthLevel::Warning;
    }
    ShareHealthLevel::Healthy
}

pub(in crate::api) async fn web_share_health_status(state: &ServerState) -> Value {
    use crate::client_tunnel_provision::{
        derive_client_tunnel_claim_status, derive_client_tunnel_connectivity_status,
    };

    let config = state.config.read().await;
    let shares_store = state.shares.read().await;
    let ui_settings = state.ui_settings.read().await;
    let router_domain = ui_settings
        .settings_for_frontend(&config)
        .get("shareRouterDomain")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let client_runtime = state
        .tunnels
        .status(&crate::clients::router::tunnel::client_tunnel_key())
        .await;
    let tunnel_statuses = state.tunnels.statuses().await;
    let tunnel_by_key: BTreeMap<String, crate::clients::router::tunnel::TunnelRuntimeStatus> =
        tunnel_statuses
            .into_iter()
            .map(|status| (status.key.clone(), status))
            .collect();

    let router_last_error = shares_store
        .last_router_error
        .as_deref()
        .or(config.router.last_register_error.as_deref());
    let router_level = if router_last_error.is_some() {
        ShareHealthLevel::Unhealthy
    } else if shares_store.router_registered {
        ShareHealthLevel::Healthy
    } else {
        ShareHealthLevel::Warning
    };

    let client_subdomain = config.client.tunnel_subdomain.clone().unwrap_or_default();
    let expected_tunnel_url = expected_client_tunnel_url(&client_subdomain, &router_domain);
    let active_tunnel_url = client_runtime
        .as_ref()
        .and_then(|status| status.tunnel_url.clone());
    let client_last_error = client_runtime
        .as_ref()
        .and_then(|status| status.last_error.clone())
        .or_else(|| {
            router_last_error
                .filter(|_| !shares_store.router_registered)
                .map(str::to_string)
        });
    let claim_status = derive_client_tunnel_claim_status(&config, router_last_error);
    let connectivity_status = derive_client_tunnel_connectivity_status(
        client_runtime.as_ref().map(|status| status.status.as_str()),
        client_last_error.as_deref(),
        claim_status,
    );
    let client_tunnel_level = match claim_status {
        "conflict" | "error" => ShareHealthLevel::Unhealthy,
        "unclaimed" => ShareHealthLevel::Warning,
        "claimed" => match connectivity_status {
            "connected" => ShareHealthLevel::Healthy,
            "connecting" => ShareHealthLevel::Warning,
            _ => ShareHealthLevel::Warning,
        },
        _ => ShareHealthLevel::Warning,
    };

    let mut share_items = Vec::new();
    let mut share_aggregate_level = ShareHealthLevel::Healthy;
    for share in &shares_store.shares {
        let runtime =
            tunnel_by_key.get(&crate::clients::router::tunnel::share_tunnel_key(&share.id));
        let tunnel_status = runtime.map(|status| status.status.as_str()).unwrap_or("");
        let tunnel_error = runtime
            .and_then(|status| status.last_error.clone())
            .or_else(|| share.last_error.clone());
        let level = if runtime.is_some_and(|status| status.status == "renewal_retrying")
            && share.enabled
            && share.status == "active"
            && share.router_last_sync_error.is_none()
        {
            ShareHealthLevel::Warning
        } else {
            share_health_level(
                share.enabled,
                share.status.as_str(),
                share.router_last_sync_error.as_deref(),
                tunnel_error.as_deref(),
            )
        };
        share_aggregate_level = share_aggregate_level.max(level);
        share_items.push(json!({
            "id": share.id,
            "name": share
                .display_name
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| share.id.clone()),
            "status": level.as_str(),
            "shareStatus": share.status,
            "enabled": share.enabled,
            "routerLastSyncError": share.router_last_sync_error,
            "routerLastSyncedAtMs": share.router_last_synced_at_ms,
            "tunnelStatus": if tunnel_status.is_empty() { Value::Null } else { json!(tunnel_status) },
            "tunnelError": tunnel_error,
        }));
    }

    let overall = [router_level, client_tunnel_level, share_aggregate_level]
        .into_iter()
        .max()
        .unwrap_or(ShareHealthLevel::Healthy);
    let issue_count = [router_level, client_tunnel_level, share_aggregate_level]
        .into_iter()
        .filter(|level| *level != ShareHealthLevel::Healthy)
        .count();

    json!({
        "overall": overall.as_str(),
        "issueCount": issue_count,
        "shareIssueCount": share_items
            .iter()
            .filter(|item| {
                item.get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| status != "healthy")
            })
            .count(),
        "router": {
            "status": router_level.as_str(),
            "domain": router_domain,
            "registered": shares_store.router_registered,
            "lastHeartbeatMs": shares_store.last_router_heartbeat_ms,
            "lastError": router_last_error,
        },
        "clientTunnel": {
            "status": client_tunnel_level.as_str(),
            "subdomain": client_subdomain,
            "claimStatus": claim_status,
            "connectivityStatus": connectivity_status,
            "expectedUrl": expected_tunnel_url,
            "activeUrl": active_tunnel_url,
            "tunnelUrl": active_tunnel_url.clone().or(expected_tunnel_url.clone()),
            "lastError": client_last_error,
        },
        "shares": share_items,
    })
}

pub(in crate::api) async fn web_share_tunnel_status(
    state: &ServerState,
    share_id: &str,
) -> Result<Value, ApiError> {
    let share = state
        .shares
        .read()
        .await
        .get(share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let runtime_status = state
        .tunnels
        .status(&crate::clients::router::tunnel::share_tunnel_key(share_id))
        .await;
    let mut payload = web_client_tunnel_share_status(runtime_status);
    if let Some(object) = payload.as_object_mut() {
        object.insert("shareId".to_string(), json!(share.id));
        object.insert("status".to_string(), json!(share.status));
        if share.last_error.is_some() {
            object.insert("lastError".to_string(), json!(share.last_error));
        }
    }
    Ok(payload)
}

pub(in crate::api) async fn web_provider_type_for_binding(
    state: &ServerState,
    app: AppKind,
    provider_id: &str,
) -> Result<ProviderType, ApiError> {
    state
        .providers
        .read()
        .await
        .providers
        .iter()
        .find(|provider| provider.app == app && provider.provider.id == provider_id)
        .map(|provider| provider.provider_type)
        .ok_or_else(|| ApiError::not_found(format!("provider not found: {provider_id}")))
}

pub(in crate::api) fn web_create_backup_request(
    args: &Value,
) -> Result<Option<Json<CreateBackupRequest>>, ApiError> {
    if !web_has_payload(args) {
        return Ok(None);
    }
    let value = web_payload(args, &["input", "params"]);
    let request = serde_json::from_value::<CreateBackupRequest>(value.clone())
        .map_err(ApiError::bad_request)?;
    Ok(Some(Json(request)))
}

pub(in crate::api) fn web_client_tunnel_input(
    args: &Value,
) -> Result<UpdateClientTunnelInput, ApiError> {
    let value = web_payload(args, &["params", "input", "config"]);
    Ok(UpdateClientTunnelInput {
        tunnel_subdomain: web_optional_string_any(value, &["tunnelSubdomain", "subdomain"]),
        tunnel_status: web_optional_string_any(value, &["tunnelStatus", "status"]),
    })
}

pub(in crate::api) fn web_arg_share_id(args: &Value) -> Result<String, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    web_arg_string_any(value, &["shareId", "share_id", "id"])
}

pub(in crate::api) fn web_share_json(
    config: &ServerConfig,
    share: &Share,
) -> Result<Value, ApiError> {
    let mut value = serde_json::to_value(share).map_err(ApiError::internal)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("share did not serialize as an object"))?;
    let Some(slug) = share.tunnel_subdomain.as_deref() else {
        return Ok(value);
    };
    let slug = crate::domain::router::ShareSlug::parse(slug)
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    let client = config
        .client
        .tunnel_subdomain
        .as_deref()
        .ok_or_else(|| ApiError::conflict("client subdomain is not configured"))
        .and_then(|value| {
            crate::domain::router::ClientSubdomain::parse(value)
                .map_err(|error| ApiError::conflict(error.to_string()))
        })?;
    let label = format!("{slug}--{client}");
    object.insert("shareSlug".into(), json!(slug.as_str()));
    object.insert("subdomain".into(), json!(label));
    if let Some(domain) = config.router.domain.as_deref() {
        object.insert(
            "tunnelUrl".into(),
            json!(format!("https://{label}.{}", domain.trim())),
        );
    }
    Ok(value)
}

pub(in crate::api) fn web_payload<'a>(args: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter().find_map(|key| args.get(*key)).unwrap_or(args)
}

pub(in crate::api) fn web_has_payload(args: &Value) -> bool {
    args.as_object().is_some_and(|object| !object.is_empty())
}

pub(in crate::api) fn web_arg_value_any<T>(args: &Value, keys: &[&str]) -> Result<T, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = web_payload(args, keys).clone();
    serde_json::from_value(value).map_err(ApiError::bad_request)
}

pub(in crate::api) fn web_arg_string_any(args: &Value, keys: &[&str]) -> Result<String, ApiError> {
    web_optional_string_any(args, keys).ok_or_else(|| {
        ApiError::bad_request(format!("{} is required", keys.first().unwrap_or(&"value")))
    })
}

pub(in crate::api) fn web_runtime_auth_required_payload(
    config: &ServerConfig,
    contract: &web_runtime::WebRuntimeContract,
) -> Value {
    json!({
        "mode": "client-login",
        "appMode": "server",
        "platform": "server",
        "status": "auth-required",
        "permissions": ["login"],
        "apps": ["claude", "codex", "gemini"],
        "providerContract": {
            "version": web_runtime::PROVIDER_CONTRACT_VERSION,
            "minSupported": web_runtime::PROVIDER_CONTRACT_MIN_SUPPORTED,
            "maxSupported": web_runtime::PROVIDER_CONTRACT_MAX_SUPPORTED
        },
        "auth": {
            "authenticated": false,
            "setupRequired": false,
            "ownerEmail": config.owner.email,
            "methods": web_runtime_auth_methods(config)
        },
        "features": {
            "retained": contract.retained_features,
            "hidden": contract.hidden_features,
            "excluded": contract.excluded_features
        },
        "commands": contract.commands,
        "uiAutomation": {
            "allowed": contract.ui_automation_allowed
        }
    })
}

pub(in crate::api) fn web_runtime_auth_methods(config: &ServerConfig) -> Vec<&'static str> {
    crate::domain::web_auth::auth_methods(config).methods
}

pub(in crate::api) async fn web_proxy_takeover_status_json(state: &ServerState) -> Value {
    let providers = state.providers.read().await;
    fn app_takeover(
        providers: &crate::domain::providers::store::ProviderStore,
        app: AppKind,
    ) -> (bool, bool) {
        let has_provider = providers.providers.iter().any(|provider| {
            provider.app == app
                && crate::domain::providers::bundle::surface_enabled(&provider.provider)
        });
        // Server-native routing is always on for the three core apps.
        (has_provider, !has_provider)
    }

    let (claude, claude_pending) = app_takeover(&providers, AppKind::Claude);
    let (codex, codex_pending) = app_takeover(&providers, AppKind::Codex);
    let (gemini, gemini_pending) = app_takeover(&providers, AppKind::Gemini);

    json!({
        "claude": claude,
        "codex": codex,
        "gemini": gemini,
        "opencode": false,
        "openclaw": false,
        "hermes": false,
        "claude_pending": claude_pending,
        "codex_pending": codex_pending,
        "gemini_pending": gemini_pending,
    })
}

pub(in crate::api) async fn web_is_live_takeover_active(state: &ServerState) -> bool {
    let status = web_proxy_takeover_status_json(state).await;
    ["claude", "codex", "gemini"]
        .into_iter()
        .any(|app| status.get(app).and_then(Value::as_bool).unwrap_or(false))
}

pub(in crate::api) async fn web_proxy_status_json(state: &ServerState) -> Value {
    let providers = state.providers.read().await;
    let mut active_targets = Vec::new();
    for stored in providers
        .providers
        .iter()
        .filter(|provider| crate::domain::providers::bundle::surface_enabled(&provider.provider))
    {
        active_targets.push(json!({
            "app_type": stored.app.as_str(),
            "provider_id": stored.provider.id,
            "provider_name": stored.provider.name,
        }));
    }

    json!({
        "running": true,
        "address": state.bind_addr.ip().to_string(),
        "port": state.bind_addr.port(),
        "active_connections": 0,
        "total_requests": 0,
        "success_requests": 0,
        "failed_requests": 0,
        "success_rate": 100.0,
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "last_request_at": Value::Null,
        "last_error": Value::Null,
        "active_targets": active_targets,
    })
}

pub(in crate::api) fn web_optional_string_any(args: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(in crate::api) fn web_optional_bool(args: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_bool))
}

pub(in crate::api) fn web_optional_i64(args: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        args.get(*key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        })
    })
}

pub(in crate::api) fn web_optional_u64(args: &Value, keys: &[&str]) -> Option<u64> {
    web_optional_i64(args, keys).and_then(|value| (value >= 0).then_some(value as u64))
}

pub(in crate::api) fn web_optional_u32(args: &Value, keys: &[&str]) -> Option<u32> {
    web_optional_i64(args, keys).and_then(|value| u32::try_from(value).ok())
}

pub(in crate::api) fn web_optional_deserialize<T>(
    args: &Value,
    key: &str,
) -> Result<Option<T>, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    args.get(key)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::bad_request)
}

pub(in crate::api) fn web_share_free_access(args: &Value) -> Result<Option<bool>, ApiError> {
    Ok(web_optional_bool(args, &["freeAccess", "free_access"]))
}

fn reject_retired_share_input_fields(args: &Value) -> Result<(), ApiError> {
    if !args.is_object() {
        return Err(ApiError::bad_request("share input must be an object"));
    }
    if let Some(field) = find_retired_share_field(args) {
        return Err(ApiError::bad_request(format!(
            "retired Share field `{field}` is not accepted; use freeAccess/userGrants"
        )));
    }
    Ok(())
}

pub(in crate::api) fn web_optional_auth_provider_type(
    args: &Value,
) -> Result<Option<ProviderType>, ApiError> {
    web_optional_string_any(args, &["providerType", "provider_type", "authProvider"])
        .map(|value| web_parse_auth_provider_type(&value))
        .transpose()
}

pub(in crate::api) fn web_auth_provider_type(args: &Value) -> Result<ProviderType, ApiError> {
    web_optional_auth_provider_type(args)?
        .ok_or_else(|| ApiError::bad_request("authProvider is required"))
}

pub(in crate::api) fn web_parse_auth_provider_type(value: &str) -> Result<ProviderType, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "google_gemini_oauth" | "gemini_cli" => Ok(ProviderType::GeminiCli),
        "github_copilot" => Ok(ProviderType::GitHubCopilot),
        "codex_oauth" => Ok(ProviderType::CodexOAuth),
        "grok_oauth" => Ok(ProviderType::GrokOAuth),
        "kimi_code" => Ok(ProviderType::KimiCode),
        "claude_oauth" => Ok(ProviderType::ClaudeOAuth),
        "antigravity_oauth" => Ok(ProviderType::AntigravityOAuth),
        "cursor_oauth" => Ok(ProviderType::CursorOAuth),
        "kiro_oauth" => Ok(ProviderType::KiroOAuth),
        "agy_oauth" => Ok(ProviderType::AgyOAuth),
        other => web_parse_provider_type(other),
    }
}

pub(in crate::api) fn web_parse_provider_type(value: &str) -> Result<ProviderType, ApiError> {
    serde_json::from_value(Value::String(value.trim().to_string()))
        .map_err(|_| ApiError::bad_request(format!("invalid providerType: {value}")))
}

pub(in crate::api) fn managed_auth_provider_label(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::GitHubCopilot => "github_copilot",
        ProviderType::CodexOAuth => "codex_oauth",
        ProviderType::GrokOAuth => "grok_oauth",
        ProviderType::KimiCode => "kimi_code",
        ProviderType::ClaudeOAuth => "claude_oauth",
        ProviderType::GeminiCli => "google_gemini_oauth",
        ProviderType::AntigravityOAuth => "antigravity_oauth",
        ProviderType::AgyOAuth => "agy_oauth",
        ProviderType::CursorOAuth => "cursor_oauth",
        ProviderType::KiroOAuth => "kiro_oauth",
        ProviderType::QoderCosy => "qoder_cosy",
        ProviderType::DeepSeekAccount => "deepseek_account",
        _ => "unknown",
    }
}

pub(in crate::api) fn account_is_authenticated(account: &Account) -> bool {
    account
        .access_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || account
            .refresh_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

pub(in crate::api) fn account_authenticated_at(account: &Account) -> i64 {
    account.quota_refreshed_at.unwrap_or(0)
}

pub(in crate::api) fn deepseek_account_status_json(accounts: &AccountStore) -> Value {
    let matching = accounts
        .accounts
        .iter()
        .filter(|account| account.provider_type == ProviderType::DeepSeekAccount)
        .collect::<Vec<_>>();
    let default_account_id = matching.first().map(|account| account.id.as_str());
    let mapped = matching
        .iter()
        .map(|account| deepseek_account_json(account, default_account_id))
        .collect::<Vec<_>>();
    json!({
        "authenticated": matching
            .iter()
            .any(|account| account_is_authenticated(account)),
        "default_account_id": default_account_id,
        "accounts": mapped,
    })
}

pub(in crate::api) fn deepseek_account_json(
    account: &Account,
    default_account_id: Option<&str>,
) -> Value {
    json!({
        "id": account.id,
        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
        "authenticated_at": account_authenticated_at(account),
        "is_default": default_account_id == Some(account.id.as_str()),
        "has_password": false,
        "credential_kind": "access_token",
    })
}

#[cfg(test)]
mod managed_auth_provider_label_tests {
    use super::*;

    #[test]
    fn canonical_free_access_is_read_directly() {
        assert_eq!(
            web_share_free_access(&json!({ "freeAccess": true })).unwrap(),
            Some(true)
        );
        assert_eq!(
            web_share_free_access(&json!({ "freeAccess": false })).unwrap(),
            Some(false)
        );
        assert_eq!(web_share_free_access(&json!({})).unwrap(), None);
    }

    #[test]
    fn agy_and_antigravity_keep_distinct_auth_provider_labels() {
        assert_eq!(
            managed_auth_provider_label(ProviderType::AntigravityOAuth),
            "antigravity_oauth"
        );
        assert_eq!(
            managed_auth_provider_label(ProviderType::AgyOAuth),
            "agy_oauth"
        );
        assert_eq!(
            managed_auth_provider_label(ProviderType::DeepSeekAccount),
            "deepseek_account"
        );
        assert_eq!(
            managed_auth_provider_label(ProviderType::KimiCode),
            "kimi_code"
        );
        assert_eq!(
            managed_auth_provider_label(ProviderType::QoderCosy),
            "qoder_cosy"
        );
        assert_eq!(
            web_parse_auth_provider_type("kimi_code").unwrap(),
            ProviderType::KimiCode
        );
        assert_eq!(
            web_parse_auth_provider_type("qoder_cosy").unwrap(),
            ProviderType::QoderCosy
        );
    }

    #[test]
    fn deepseek_status_maps_real_account_store_records() {
        let accounts: AccountStore = serde_json::from_value(json!({
            "accounts": [
                {
                    "id": "deepseek-1",
                    "providerType": "deepseek_account",
                    "email": "owner@example.com",
                    "accessToken": "deepseek-token"
                },
                {
                    "id": "codex-1",
                    "providerType": "codex_oauth",
                    "accessToken": "codex-token"
                }
            ]
        }))
        .expect("account store");

        let status = deepseek_account_status_json(&accounts);

        assert_eq!(status["authenticated"], true);
        assert_eq!(status["default_account_id"], "deepseek-1");
        assert_eq!(status["accounts"].as_array().map(Vec::len), Some(1));
        assert_eq!(status["accounts"][0]["id"], "deepseek-1");
        assert_eq!(status["accounts"][0]["login"], "owner@example.com");
        assert_eq!(status["accounts"][0]["is_default"], true);
        assert_eq!(status["accounts"][0]["has_password"], false);
        assert_eq!(status["accounts"][0]["credential_kind"], "access_token");
    }

    #[test]
    fn codex_default_account_uses_explicit_active_selection() {
        let accounts: AccountStore = serde_json::from_value(json!({
            "accounts": [
                {
                    "id": "codex-first",
                    "providerType": "codex_oauth",
                    "accessToken": "first-token"
                },
                {
                    "id": "codex-active",
                    "providerType": "codex_oauth",
                    "accessToken": "active-token"
                }
            ],
            "activeCodexOauthAccountId": "codex-active"
        }))
        .expect("account store");

        assert_eq!(
            managed_auth_default_account_id(&accounts, ProviderType::CodexOAuth),
            Some("codex-active")
        );
    }

    #[test]
    fn provider_quota_rejects_an_account_from_another_auth_provider() {
        let account: Account = serde_json::from_value(json!({
            "id": "gemini-account",
            "providerType": "gemini_cli",
            "accessToken": "gemini-token"
        }))
        .expect("account");

        let error = ensure_provider_quota_account_type(&account, ProviderType::CodexOAuth)
            .expect_err("Codex quota must reject a Gemini account id");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("codex_oauth"));
    }
}

pub(in crate::api) fn managed_auth_default_account_id(
    accounts: &AccountStore,
    provider_type: ProviderType,
) -> Option<&str> {
    if provider_type == ProviderType::CodexOAuth {
        return accounts
            .active_codex_oauth_account()
            .map(|account| account.id.as_str());
    }
    accounts
        .accounts
        .iter()
        .find(|account| account.provider_type == provider_type)
        .map(|account| account.id.as_str())
}

fn subscription_expiry_rfc3339(timestamp_ms: Option<i64>) -> Option<String> {
    timestamp_ms.and_then(|timestamp_ms| {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
            .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    })
}

pub(in crate::api) fn map_managed_auth_account(
    account: &Account,
    provider_label: &str,
    default_account_id: Option<&str>,
) -> Value {
    let workspaces = crate::domain::accounts::store::codex_workspace_options(account);
    let selected_workspace_id =
        crate::domain::accounts::store::effective_codex_workspace_id(account);
    use crate::domain::accounts::subscription_expiry::{
        automatic_subscription_expires_at_ms, recurring_subscription_expires_at_ms,
        resolved_subscription_expiry_at, supports_manual_expiry, SubscriptionExpirySource,
    };

    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let subscription_expiry = resolved_subscription_expiry_at(account, now_ms);
    let supports_manual = supports_manual_expiry(subscription_expiry.capability);
    let legacy_manual_expires_at = supports_manual
        .then_some(account.manual_subscription_expires_at_ms)
        .flatten()
        .and_then(|timestamp_ms| subscription_expiry_rfc3339(Some(timestamp_ms)));
    let rule_next_expires_at = supports_manual
        .then(|| recurring_subscription_expires_at_ms(account, now_ms))
        .flatten()
        .and_then(|timestamp_ms| subscription_expiry_rfc3339(Some(timestamp_ms)));
    let automatic_expires_at = automatic_subscription_expires_at_ms(account)
        .and_then(|timestamp_ms| subscription_expiry_rfc3339(Some(timestamp_ms)));
    let effective_expires_at = subscription_expiry_rfc3339(subscription_expiry.expires_at_ms);
    let expiry_source = subscription_expiry.source.map(|source| match source {
        SubscriptionExpirySource::Automatic => "automatic",
        SubscriptionExpirySource::RecurringRule => "recurring_rule",
        SubscriptionExpirySource::LegacyManual => "manual",
    });
    let expiry_kind = subscription_expiry.source.map(|source| match source {
        SubscriptionExpirySource::Automatic => "subscription",
        SubscriptionExpirySource::RecurringRule => "recurring_billing_period",
        SubscriptionExpirySource::LegacyManual => "billing_period",
    });
    let qoder = (account.provider_type == ProviderType::QoderCosy)
        .then(|| crate::domain::qoder::QoderAccountProfile::parse(account.profile.as_ref()).ok())
        .flatten()
        .map(|profile| {
            json!({
                "site": profile.site,
                "credentialRail": profile.credential_rail,
            })
        });
    json!({
        "id": account.id,
        "provider": provider_label,
        "authIdentityGeneration": account.auth_identity_generation,
        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
        "email": account.email,
        "subscriptionLevel": crate::api::types::account_subscription_level_public_view(account),
        "avatar_url": Value::Null,
        "authenticated_at": account_authenticated_at(account),
        "is_default": default_account_id == Some(account.id.as_str()),
        "github_domain": "github.com",
        "workspaces": workspaces,
        "selected_workspace_id": selected_workspace_id,
        "qoder": qoder,
        "subscriptionExpiry": {
            "capability": subscription_expiry.capability,
            "rule": if supports_manual { account.manual_subscription_expiry_rule.as_ref() } else { None },
            "ruleNextExpiresAt": rule_next_expires_at,
            "automaticExpiresAt": automatic_expires_at,
            "legacyManualExpiresAt": legacy_manual_expires_at,
            "manualExpiresAt": legacy_manual_expires_at,
            "effectiveExpiresAt": effective_expires_at,
            "source": expiry_source,
            "kind": expiry_kind,
        }
    })
}

pub(in crate::api) fn managed_auth_is_cli_oauth_flow(oauth_flow_mode: Option<&str>) -> bool {
    matches!(
        oauth_flow_mode
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cli") | Some("cli_manual") | Some("browser") | Some("cli_oauth") | Some("clioauth")
    )
}

pub(in crate::api) fn web_managed_auth_redirect_uri(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
    provider_type: ProviderType,
    oauth_flow_mode: Option<&str>,
) -> String {
    if provider_type == ProviderType::CodexOAuth && managed_auth_is_cli_oauth_flow(oauth_flow_mode)
    {
        return crate::domain::accounts::oauth::CODEX_CLI_REDIRECT_URI.to_string();
    }
    if provider_type == ProviderType::ClaudeOAuth
        && matches!(
            oauth_flow_mode
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("web_paste") | Some("webpaste")
        )
    {
        return CLAUDE_WEB_PASTE_REDIRECT_URI.to_string();
    }
    if provider_type == ProviderType::GrokOAuth {
        return XAI_LOOPBACK_REDIRECT_URI.to_string();
    }
    if let Some(uri) = web_optional_string_any(args, &["redirectUri", "redirect_uri"]) {
        return uri;
    }
    if let Some(host) = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
    {
        if let Ok(host_str) = host.to_str() {
            let scheme = headers
                .get("x-forwarded-proto")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("http");
            return format!("{scheme}://{host_str}/api/accounts/login/callback");
        }
    }
    default_account_login_redirect_uri(state)
}

pub(in crate::api) fn map_managed_auth_device_code(
    provider_label: &str,
    device_code: &str,
    user_code: &str,
    verification_uri: &str,
    expires_in: u64,
    interval: u64,
) -> Value {
    json!({
        "flow": "device",
        "provider": provider_label,
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "expires_in": expires_in,
        "interval": interval,
    })
}

pub(in crate::api) fn map_managed_auth_browser_login(
    provider_label: &str,
    login: &OAuthLoginStart,
    cli_prefix: bool,
    expires_in: u64,
    interval: u64,
) -> Value {
    let device_code = if cli_prefix {
        format!("manual:{}", login.session_id)
    } else {
        login.state.clone()
    };
    json!({
        "flow": if cli_prefix { "cli_manual" } else { "browser" },
        "session_id": login.session_id,
        "provider": provider_label,
        "device_code": device_code,
        "user_code": "",
        "verification_uri": login.authorize_url,
        "expires_in": expires_in,
        "interval": interval,
    })
}

pub(in crate::api) async fn web_managed_auth_account_by_id(
    state: &ServerState,
    account_id: &str,
    provider_label: &str,
) -> Result<Value, ApiError> {
    let accounts = state.accounts.read().await;
    let provider_type = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .map(|account| account.provider_type)
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    let default_account_id = managed_auth_default_account_id(&accounts, provider_type);
    let account = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    Ok(map_managed_auth_account(
        account,
        provider_label,
        default_account_id,
    ))
}

pub(in crate::api) async fn web_managed_auth_start_login(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    let provider_type = web_auth_provider_type(args)?;
    let provider_label = managed_auth_provider_label(provider_type);
    let oauth_flow_mode = web_optional_string_any(args, &["oauthFlowMode", "oauth_flow_mode"]);
    let oauth_flow_mode_ref = oauth_flow_mode.as_deref();
    if provider_type == ProviderType::CodexOAuth
        && managed_auth_is_cli_oauth_flow(oauth_flow_mode_ref)
    {
        require_secure_manual_cli_origin(&state, &headers).await?;
    }

    match provider_type {
        ProviderType::GitHubCopilot => {
            let response = start_copilot_device_login(
                State(state),
                headers,
                Json(StartCopilotDeviceLoginRequest {
                    github_domain: web_optional_string_any(
                        args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(map_managed_auth_device_code(
                provider_label,
                &response.device.device_code,
                &response.device.user_code,
                &response.device.verification_uri,
                response.device.expires_in,
                response.device.interval,
            ))
        }
        ProviderType::KiroOAuth => {
            let response = start_kiro_device_login(
                State(state),
                headers,
                Json(StartKiroDeviceLoginRequest {
                    region: web_optional_string_any(args, &["region"]),
                    start_url: web_optional_string_any(args, &["startUrl", "start_url"]),
                    issuer_url: web_optional_string_any(args, &["issuerUrl", "issuer_url"]),
                    login_provider: web_optional_string_any(
                        args,
                        &["kiroLoginProvider", "kiro_login_provider", "loginProvider"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(map_managed_auth_device_code(
                provider_label,
                &response.device.device_code,
                &response.device.user_code,
                &response.device.verification_uri,
                response.device.expires_in,
                response.device.interval,
            ))
        }
        ProviderType::KimiCode => {
            let response = start_kimi_device_login(
                State(state),
                headers,
                Json(StartKimiDeviceLoginRequest {}),
            )
            .await?
            .0;
            Ok(map_managed_auth_device_code(
                provider_label,
                &response.device.device_code,
                &response.device.user_code,
                &response.device.verification_uri,
                response.device.expires_in,
                response.device.interval,
            ))
        }
        ProviderType::QoderCosy => {
            let site = crate::domain::qoder::QoderSite::parse(
                web_optional_string_any(args, &["qoderSite", "qoder_site", "site"])
                    .as_deref()
                    .unwrap_or_default(),
            )
            .map_err(ApiError::bad_request)?;
            let response = start_qoder_device_login(
                State(state),
                headers,
                Json(StartQoderDeviceLoginRequest {
                    site: Some(site.as_str().to_string()),
                }),
            )
            .await?
            .0;
            Ok(json!({
                "flow": "device",
                "provider": provider_label,
                "device_code": response.device.device_code,
                "state": response.device.state,
                "user_code": "",
                "verification_uri": response.device.verification_uri_complete,
                "verification_uri_complete": response.device.verification_uri_complete,
                "expires_in": response.device.expires_in,
                "interval": response.device.interval,
                "site": response.device.site,
            }))
        }
        ProviderType::CodexOAuth if !managed_auth_is_cli_oauth_flow(oauth_flow_mode_ref) => {
            let response = start_codex_device_login(
                State(state),
                headers,
                Json(StartCodexDeviceLoginRequest {}),
            )
            .await?
            .0;
            Ok(map_managed_auth_device_code(
                provider_label,
                &response.device.device_code,
                &response.device.user_code,
                &response.device.verification_uri,
                response.device.expires_in,
                response.device.interval,
            ))
        }
        _ => {
            let redirect_uri = Some(web_managed_auth_redirect_uri(
                &state,
                &headers,
                args,
                provider_type,
                oauth_flow_mode_ref,
            ));
            let response = start_account_login(
                State(state),
                headers,
                Json(StartAccountLoginRequest {
                    provider_type,
                    redirect_uri,
                }),
            )
            .await?
            .0;
            let (expires_in, interval, cli_prefix) = match provider_type {
                ProviderType::CodexOAuth => {
                    (300, 2, managed_auth_is_cli_oauth_flow(oauth_flow_mode_ref))
                }
                ProviderType::CursorOAuth => (300, 2, false),
                _ => (300, 5, false),
            };
            Ok(map_managed_auth_browser_login(
                provider_label,
                &response.login,
                cli_prefix,
                expires_in,
                interval,
            ))
        }
    }
}

pub(in crate::api) async fn web_managed_auth_poll_for_account(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    let provider_type = web_auth_provider_type(args)?;
    let provider_label = managed_auth_provider_label(provider_type);
    let device_code = web_arg_string_any(args, &["deviceCode", "device_code"])?;

    match provider_type {
        ProviderType::GitHubCopilot => {
            let response = poll_copilot_device_login(
                State(state.clone()),
                headers,
                Json(PollCopilotDeviceLoginRequest {
                    device_code,
                    github_domain: web_optional_string_any(
                        args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            if response.pending {
                return Ok(Value::Null);
            }
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("copilot device flow completed without account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        ProviderType::KiroOAuth => {
            let response = poll_kiro_device_login(
                State(state.clone()),
                headers,
                Json(PollKiroDeviceLoginRequest { device_code }),
            )
            .await?
            .0;
            if response.pending {
                return Ok(Value::Null);
            }
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("kiro device flow completed without account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        ProviderType::KimiCode => {
            let response = poll_kimi_device_login(
                State(state.clone()),
                headers,
                Json(PollKimiDeviceLoginRequest { device_code }),
            )
            .await?
            .0;
            if response.pending {
                return Ok(Value::Null);
            }
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("Kimi device flow completed without account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        ProviderType::QoderCosy => {
            let flow_state = web_arg_string_any(args, &["flowState", "flow_state", "state"])?;
            let response = poll_qoder_device_login(
                State(state.clone()),
                headers,
                Json(PollQoderDeviceLoginRequest {
                    device_code,
                    state: flow_state,
                }),
            )
            .await?
            .0;
            if response.pending {
                return Ok(Value::Null);
            }
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("Qoder device flow completed without account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        ProviderType::CodexOAuth
            if !device_code.starts_with("cli:") && !device_code.starts_with("manual:") =>
        {
            let response = poll_codex_device_login(
                State(state.clone()),
                headers,
                Json(PollCodexDeviceLoginRequest { device_code }),
            )
            .await?
            .0;
            if response.pending {
                return Ok(Value::Null);
            }
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("codex device flow completed without account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        _ => {
            let principal = require_web_admin_session(&state, &headers).await?;
            let principal_id = principal.oauth_binding_id();
            let manual_session_id = device_code.strip_prefix("manual:");
            let poll_state = manual_session_id.is_none().then(|| {
                device_code
                    .strip_prefix("cli:")
                    .unwrap_or(device_code.as_str())
            });
            let managed_auth_operation = state.lock_managed_auth_operations().await;
            let poll_status = state
                .mutate_oauth_logins(|store| {
                    store.poll_state_for_principal(
                        manual_session_id,
                        poll_state,
                        &principal_id,
                        now_ms() as i64,
                    )
                })
                .await;
            drop(managed_auth_operation);
            match poll_status {
                Ok(OAuthSessionPollState::Pending) => return Ok(Value::Null),
                Err(OAuthLoginError::NotFound) => {
                    return Err(ApiError::bad_request("oauth login session not found"));
                }
                Err(OAuthLoginError::Expired) => {
                    return Err(ApiError::conflict("oauth login session expired"));
                }
                Err(OAuthLoginError::AlreadyConsumed) => return Ok(Value::Null),
                Err(error) => return Err(oauth_login_api_error(error)),
                Ok(OAuthSessionPollState::Ready | OAuthSessionPollState::Completed) => {}
            }

            let finish_result = finish_account_login(
                State(state.clone()),
                headers,
                Json(FinishAccountLoginRequest {
                    state: poll_state.map(str::to_string),
                    session_id: manual_session_id.map(str::to_string),
                    code: None,
                    execute_token_exchange: Some(true),
                    expected_provider_type: Some(provider_type),
                }),
            )
            .await;

            match finish_result {
                Ok(response) => {
                    let account_id = response
                        .0
                        .account
                        .as_ref()
                        .map(|account| account.id.as_str())
                        .ok_or_else(|| {
                            ApiError::bad_gateway("oauth login did not import account")
                        })?;
                    web_managed_auth_account_by_id(&state, account_id, provider_label).await
                }
                Err(error)
                    if error.status == StatusCode::CONFLICT
                        || error.message.contains("authorization_pending") =>
                {
                    Ok(Value::Null)
                }
                Err(error) => Err(error),
            }
        }
    }
}

pub(in crate::api) async fn web_managed_auth_cancel_login(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    let provider_type = web_auth_provider_type(args)?;
    let device_code = web_arg_string_any(args, &["deviceCode", "device_code"])?;
    if provider_type == ProviderType::CodexOAuth
        && !device_code.starts_with("cli:")
        && !device_code.starts_with("manual:")
    {
        let response = cancel_codex_device_login(
            State(state),
            headers,
            Json(CancelCodexDeviceLoginRequest { device_code }),
        )
        .await?
        .0;
        return Ok(json!(response));
    }
    if provider_type == ProviderType::QoderCosy {
        let response = cancel_qoder_device_login(
            State(state),
            headers,
            Json(CancelQoderDeviceLoginRequest { device_code }),
        )
        .await?
        .0;
        return Ok(json!(response));
    }
    if matches!(
        provider_type,
        ProviderType::GitHubCopilot | ProviderType::KiroOAuth | ProviderType::KimiCode
    ) {
        let principal = require_web_admin_session(&state, &headers).await?;
        let managed_auth_operation = state.lock_managed_auth_operations().await;
        let cancelled = state
            .remove_device_flow_for_principal_under_managed_auth_guard(
                &managed_auth_operation,
                provider_type,
                &device_code,
                &principal.oauth_binding_id(),
                now_ms() as i64,
            )
            .await;
        drop(managed_auth_operation);
        return Ok(json!({"ok": true, "cancelled": cancelled}));
    }
    let manual_session_id = device_code.strip_prefix("manual:").map(str::to_string);
    let oauth_state = manual_session_id.is_none().then(|| {
        device_code
            .strip_prefix("cli:")
            .unwrap_or(device_code.as_str())
            .to_string()
    });
    let response = cancel_account_login(
        State(state),
        headers,
        Json(CancelAccountLoginRequest {
            session_id: manual_session_id,
            state: oauth_state,
            expected_provider_type: Some(provider_type),
        }),
    )
    .await?
    .0;
    Ok(json!({
        "ok": response.ok,
        "cancelled": response.login.status == OAuthLoginStatus::Cancelled,
        "status": response.login.status,
    }))
}

pub(in crate::api) async fn require_secure_manual_cli_origin(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let direct_authority = first_header_authority(headers, header::HOST);
    let forwarded_authority = first_header_authority(headers, "x-forwarded-host");

    if state.bind_addr.ip().is_loopback()
        && forwarded_authority.is_none()
        && direct_authority.as_ref().is_some_and(authority_is_loopback)
    {
        return Ok(());
    }

    let presented_authority = forwarded_authority.or(direct_authority).ok_or_else(|| {
        ApiError::forbidden("manual CLI OAuth requires an identifiable Client URL origin")
    })?;
    let scheme = first_header_value(headers, "x-forwarded-proto");
    if scheme.as_deref() != Some("https") {
        return Err(ApiError::forbidden(
            "manual CLI OAuth requires HTTPS when accessed through a non-loopback Client URL",
        ));
    }

    let expected_authority = configured_client_authority(state).await.ok_or_else(|| {
        ApiError::forbidden(
            "manual CLI OAuth through a remote URL requires a configured Client URL",
        )
    })?;
    let signed_ingress_authority =
        first_header_authority(headers, "x-cc-switch-client-tunnel-host");
    if signed_ingress_authority.as_ref() != Some(&expected_authority)
        || presented_authority != expected_authority
    {
        return Err(ApiError::forbidden(
            "manual CLI OAuth is only available through the signed configured Client URL",
        ));
    }

    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| url::Url::parse(value.trim()).ok());
    let origin_is_expected = origin.as_ref().is_some_and(|origin| {
        origin.scheme() == "https"
            && url_authority(origin).as_ref() == Some(&expected_authority)
            && origin.path() == "/"
            && origin.query().is_none()
            && origin.fragment().is_none()
    });
    if !origin_is_expected {
        return Err(ApiError::forbidden(
            "manual CLI OAuth requires a same-origin HTTPS Client URL request",
        ));
    }
    Ok(())
}

async fn configured_client_authority(state: &ServerState) -> Option<(String, Option<u16>)> {
    let config = state.config_snapshot().await;
    let router_domain = state
        .ui_settings
        .read()
        .await
        .settings_for_frontend(&config)
        .get("shareRouterDomain")
        .and_then(Value::as_str)
        .map(str::to_string)?;
    let client_subdomain = config.client.tunnel_subdomain.as_deref()?;
    let url = expected_client_tunnel_url(client_subdomain, &router_domain)?;
    url::Url::parse(&url)
        .ok()
        .and_then(|url| url_authority(&url))
}

fn first_header_value(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn first_header_authority(
    headers: &HeaderMap,
    name: impl header::AsHeaderName,
) -> Option<(String, Option<u16>)> {
    let value = first_header_value(headers, name)?;
    let url = url::Url::parse(&format!("http://{value}")).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let (host, port) = url_authority(&url)?;
    Some((host, port.filter(|port| *port != 443)))
}

fn url_authority(url: &url::Url) -> Option<(String, Option<u16>)> {
    Some((url.host_str()?.to_ascii_lowercase(), url.port()))
}

fn authority_is_loopback(authority: &(String, Option<u16>)) -> bool {
    let hostname = authority.0.as_str();
    hostname.eq_ignore_ascii_case("localhost")
        || hostname.to_ascii_lowercase().ends_with(".localhost")
        || hostname
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(in crate::api) async fn web_managed_auth_remove_account(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    let provider_type = web_auth_provider_type(args)?;
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    let exists = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .any(|account| account.id == account_id && account.provider_type == provider_type);
    if !exists {
        return Err(ApiError::not_found("account not found"));
    }
    let _ = delete_account(State(state), headers, Path(account_id)).await?;
    Ok(Value::Null)
}

pub(in crate::api) async fn web_managed_auth_set_default_account(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    if provider_type == ProviderType::CodexOAuth {
        state
            .select_active_codex_oauth_account_command(&account_id)
            .await
            .map_err(ApiError::internal)?
            .map_err(map_codex_active_account_selection_error)?;
        return Ok(Value::Null);
    }
    let default_changed = state
        .try_mutate_accounts_immediate(|store| {
            let default_changed = store
                .accounts
                .iter()
                .find(|account| account.provider_type == provider_type)
                .is_none_or(|account| account.id != account_id);
            let Some(index) = store.accounts.iter().position(|account| {
                account.id == account_id && account.provider_type == provider_type
            }) else {
                return Err(ApiError::not_found("account not found"));
            };
            let account = store.accounts.remove(index);
            let insert_at = store
                .accounts
                .iter()
                .position(|item| item.provider_type == provider_type)
                .unwrap_or(store.accounts.len());
            store.accounts.insert(insert_at, account);
            Ok(default_changed)
        })
        .await
        .map_err(map_account_write_error)??;
    if default_changed {
        state
            .refresh_account_subscription_metadata(provider_type, None)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Value::Null)
}

pub(in crate::api) async fn web_managed_auth_set_manual_subscription_expiry(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let provider_label = managed_auth_provider_label(provider_type);
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    let expires_at = args
        .get("expiresAt")
        .or_else(|| args.get("expires_at"))
        .ok_or_else(|| ApiError::bad_request("expiresAt is required"))?;
    let expires_at_ms = match expires_at {
        Value::Null => None,
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(ApiError::bad_request(
                    "expiresAt must be an RFC3339 timestamp or null",
                ));
            }
            Some(
                chrono::DateTime::parse_from_rfc3339(value)
                    .map_err(|_| {
                        ApiError::bad_request("expiresAt must be a valid RFC3339 timestamp")
                    })?
                    .timestamp_millis(),
            )
        }
        _ => {
            return Err(ApiError::bad_request(
                "expiresAt must be an RFC3339 timestamp or null",
            ));
        }
    };

    state
        .set_manual_subscription_expiry_and_sync(provider_type, &account_id, expires_at_ms)
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| match error {
            crate::domain::accounts::store::ManualSubscriptionExpiryError::NotFound(_) => {
                ApiError::not_found("account not found")
            }
            crate::domain::accounts::store::ManualSubscriptionExpiryError::Unsupported(_) => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
            }
            crate::domain::accounts::store::ManualSubscriptionExpiryError::InvalidTimestamp => {
                ApiError::bad_request(error)
            }
            crate::domain::accounts::store::ManualSubscriptionExpiryError::InvalidRule(_) => {
                ApiError::bad_request(error)
            }
        })?;

    web_managed_auth_account_by_id(&state, &account_id, provider_label).await
}

pub(in crate::api) async fn web_managed_auth_set_subscription_expiry_rule(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let provider_label = managed_auth_provider_label(provider_type);
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    let rule = args
        .get("rule")
        .ok_or_else(|| ApiError::bad_request("rule is required"))?;
    let draft = if rule.is_null() {
        None
    } else {
        Some(
            serde_json::from_value::<
                crate::domain::accounts::subscription_expiry::SubscriptionExpiryRuleDraft,
            >(rule.clone())
            .map_err(|error| {
                ApiError::bad_request(format!("invalid subscription expiry rule: {error}"))
            })?,
        )
    };

    state
        .set_subscription_expiry_rule_and_sync(provider_type, &account_id, draft)
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| match error {
            crate::domain::accounts::store::ManualSubscriptionExpiryError::NotFound(_) => {
                ApiError::not_found("account not found")
            }
            crate::domain::accounts::store::ManualSubscriptionExpiryError::Unsupported(_) => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
            }
            crate::domain::accounts::store::ManualSubscriptionExpiryError::InvalidTimestamp
            | crate::domain::accounts::store::ManualSubscriptionExpiryError::InvalidRule(_) => {
                ApiError::bad_request(error)
            }
        })?;

    web_managed_auth_account_by_id(&state, &account_id, provider_label).await
}

pub(in crate::api) async fn web_managed_auth_set_workspace(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    if provider_type != ProviderType::CodexOAuth {
        return Err(ApiError::bad_request(
            "workspace selection is only available for codex_oauth accounts",
        ));
    }
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    let workspace_id = web_arg_string_any(args, &["workspaceId", "workspace_id"])?;
    if let Some(share_id) = web_optional_string_any(args, &["shareId", "share_id"]) {
        let expected_config_revision = web_optional_i64(
            args,
            &["expectedConfigRevision", "expected_config_revision"],
        )
        .and_then(|revision| u64::try_from(revision).ok())
        .ok_or_else(|| ApiError::bad_request("expectedConfigRevision is required"))?;
        let result = state
            .rebind_codex_workspace_for_share_command(
                &share_id,
                &account_id,
                &workspace_id,
                expected_config_revision,
            )
            .await
            .map_err(ApiError::internal)?
            .map_err(map_codex_workspace_rebind_error)?;
        crate::state::stop_share_tunnel(&state, &share_id).await;
        if result.identity_changed {
            spawn_share_upsert_sync(state.clone(), result.share.clone());
            emit_share_event(
                &state,
                "share.changed",
                &result.share,
                "codex_workspace_rebound",
            );
        }
        return Ok(map_managed_auth_account(
            &result.account,
            managed_auth_provider_label(ProviderType::CodexOAuth),
            None,
        ));
    }

    let account = state
        .select_codex_workspace_command(&account_id, &workspace_id)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_codex_workspace_rebind_error)?;
    Ok(map_managed_auth_account(
        &account,
        managed_auth_provider_label(ProviderType::CodexOAuth),
        None,
    ))
}

pub(in crate::api) async fn web_managed_auth_logout(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let reference_guard = state.lock_reference_mutations().await;
    let account_ids = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .filter(|account| account.provider_type == provider_type)
        .map(|account| account.id.clone())
        .collect::<Vec<_>>();
    let account_id_set = account_ids.iter().cloned().collect::<BTreeSet<_>>();
    let provider_keys = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .filter(|stored| {
            crate::domain::providers::runtime::managed_account_binding(stored).is_some_and(
                |(account_provider_type, account_id)| {
                    account_provider_type == provider_type && account_id_set.contains(account_id)
                },
            )
        })
        .map(|stored| (stored.app, stored.provider.id.clone()))
        .collect::<BTreeSet<_>>();
    if !provider_keys.is_empty() {
        return Err(ApiError::conflict_code(
            "cc_switch_account_in_use",
            format!(
                "account is still referenced by {} Provider(s)",
                provider_keys.len()
            ),
        ));
    }
    if !account_ids.is_empty() {
        state
            .try_mutate_accounts_immediate_under_reference_guard(|store| {
                for account_id in &account_ids {
                    manager_for(provider_type)
                        .revoke_or_delete(store, account_id)
                        .map_err(ApiError::bad_request)?;
                }
                Ok(())
            })
            .await
            .map_err(map_account_write_error)??;
    }
    state
        .cancel_managed_auth_for_principal_under_operation_guard(
            &managed_auth_operation,
            provider_type,
            &principal.oauth_binding_id(),
            now_ms() as i64,
        )
        .await;
    drop(reference_guard);
    drop(managed_auth_operation);
    Ok(Value::Null)
}

pub(in crate::api) fn web_arg_app_type(args: &Value) -> Result<AppKind, ApiError> {
    let app = web_arg_string_any(args, &["appType", "app", "app_type"])?;
    parse_app_kind(&app)
}

pub(in crate::api) fn web_arg_app(args: &Value) -> Result<AppKind, ApiError> {
    web_arg_string(args, "app")
        .or_else(|_| web_arg_string(args, "appType"))
        .and_then(|value| parse_app_kind(&value))
}

pub(in crate::api) fn web_arg_string(args: &Value, key: &str) -> Result<String, ApiError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::bad_request(format!("{key} is required")))
}

pub(in crate::api) fn web_arg_value<T>(args: &Value, key: &str) -> Result<T, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| ApiError::bad_request(format!("{key} is required")))?;
    serde_json::from_value(value).map_err(ApiError::bad_request)
}

pub(in crate::api) fn web_runtime_support_label(support: WebRuntimeCommandSupport) -> &'static str {
    match support {
        WebRuntimeCommandSupport::Native => "native",
        WebRuntimeCommandSupport::Shim => "shim",
        WebRuntimeCommandSupport::Excluded => "excluded",
    }
}
