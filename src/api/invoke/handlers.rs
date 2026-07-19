use super::super::*;
use std::collections::BTreeMap;

use crate::domain::providers::current_provider;

use crate::domain::accounts::oauth::{CLAUDE_WEB_PASTE_REDIRECT_URI, XAI_LOOPBACK_REDIRECT_URI};
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareSettingsPatch, ShareUserGrant,
};
use crate::domain::usage::store::UsageLog;

pub(in crate::api) fn web_provider_health_json(
    app: AppKind,
    provider_id: &str,
    breaker: Option<&crate::domain::failover::ProviderBreaker>,
) -> Value {
    let breaker = breaker
        .cloned()
        .unwrap_or_else(|| crate::domain::failover::ProviderBreaker::new(app, provider_id));
    let is_healthy = breaker.state == crate::domain::failover::BreakerState::Closed;
    json!({
        "provider_id": provider_id,
        "app_type": app.as_str(),
        "is_healthy": is_healthy,
        "consecutive_failures": breaker.consecutive_failures,
        "last_success_at": breaker.last_success_at_ms.map(|value| value.to_string()),
        "last_failure_at": breaker.last_failure_at_ms.map(|value| value.to_string()),
        "last_error": breaker.last_error,
        "updated_at": breaker.last_success_at_ms
            .or(breaker.last_failure_at_ms)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "0".to_string()),
    })
}

pub(in crate::api) fn web_circuit_breaker_stats_json(
    breaker: Option<&crate::domain::failover::ProviderBreaker>,
) -> Value {
    let Some(breaker) = breaker else {
        return json!({
            "state": "closed",
            "consecutiveFailures": 0,
            "consecutiveSuccesses": 0,
            "totalRequests": 0,
            "failedRequests": 0,
        });
    };
    let state = match breaker.state {
        crate::domain::failover::BreakerState::Closed => "closed",
        crate::domain::failover::BreakerState::Open => "open",
        crate::domain::failover::BreakerState::HalfOpen => "half_open",
    };
    json!({
        "state": state,
        "consecutiveFailures": breaker.consecutive_failures,
        "consecutiveSuccesses": 0,
        "totalRequests": breaker.consecutive_failures,
        "failedRequests": breaker.consecutive_failures,
    })
}

pub(in crate::api) async fn web_update_proxy_config_for_app(
    state: &ServerState,
    args: &Value,
) -> Result<Value, ApiError> {
    let config: Value = web_arg_value(args, "config")?;
    let app = config
        .get("appType")
        .or_else(|| config.get("app_type"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("config.appType is required"))?;
    let app = parse_app_kind(app)?;
    state
        .mutate_ui_settings_immediate(|store| {
            let mut proxy_app_configs = store
                .value
                .get("proxyAppConfigs")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(map) = proxy_app_configs.as_object_mut() {
                map.insert(app.as_str().to_string(), config.clone());
            }
            store.apply_patch(json!({ "proxyAppConfigs": proxy_app_configs }));
        })
        .await
        .map_err(ApiError::internal)?;

    let failure_threshold = config
        .get("circuitFailureThreshold")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let timeout_seconds = config.get("circuitTimeoutSeconds").and_then(Value::as_u64);
    let auto_enabled = config.get("autoFailoverEnabled").and_then(Value::as_bool);
    let providers = state.providers.read().await.clone();
    state
        .mutate_failover_immediate(|failover| {
            failover.update_app_config(
                app,
                UpdateFailoverAppInput {
                    enabled: auto_enabled,
                    failure_threshold,
                    open_duration_ms: timeout_seconds.map(|seconds| (seconds * 1000) as u128),
                    ..Default::default()
                },
                &providers,
            );
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(json!(true))
}

pub(in crate::api) async fn web_resolve_stored_provider(
    state: &ServerState,
    args: &Value,
) -> Result<StoredProvider, ApiError> {
    let app = web_arg_app_type(args)?;
    let provider_id = web_arg_string_any(args, &["providerId", "provider_id"])?;
    resolve_provider_by_id(state, &provider_id, Some(app)).await
}

pub(in crate::api) async fn web_stream_check_config(
    state: &ServerState,
) -> crate::domain::stream_check::StreamCheckConfig {
    let store = state.ui_settings.read().await;
    let value = ui_settings::stream_check_config_for_frontend(&store);
    crate::domain::stream_check::stream_check_config_from_value(&value)
}

pub(in crate::api) async fn web_proxy_target_provider_ids(
    state: &ServerState,
    app: AppKind,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let mut ids = HashSet::new();
    let ui_settings = state.ui_settings.read().await.for_frontend();
    if let Some(current) = current_provider::read_current_provider_id(&ui_settings, app) {
        ids.insert(current);
    }
    let failover = state.failover.read().await;
    if let Some(config) = failover.apps.get(&app) {
        for provider_id in &config.provider_queue {
            ids.insert(provider_id.clone());
        }
    }
    ids
}

pub(in crate::api) fn map_provider_test_to_stream_check_result(
    response: &TestProviderResponse,
    config: &crate::domain::stream_check::StreamCheckConfig,
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
    } else if latency.unwrap_or(0) > config.degraded_threshold_ms {
        HealthStatus::Degraded
    } else {
        HealthStatus::Operational
    };
    StreamCheckResult {
        status,
        success,
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
        error_category: response
            .network_status_code
            .and_then(|status| (status == 404).then_some("modelNotFound".to_string())),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
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
        .map_err(|error| ApiError::bad_gateway(format!("fetch models failed: {error}")))?;
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
        .mutate_shares_immediate(|store| {
            store
                .apply_settings_patch(&share_id, patch)
                .map_err(map_share_patch_error)
        })
        .await
        .map_err(ApiError::internal)??;
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
            let Some(account) = accounts.find_for_provider(provider_type, None) else {
                return Ok(subscription_quota_not_found(managed_auth_provider_label(
                    provider_type,
                )));
            };
            account.id.clone()
        }
    };
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id),
        Query(AccountQuotaQuery {
            refresh: Some(false),
            force: None,
        }),
    )
    .await?
    .0;
    let Some(account) = response.account.as_ref() else {
        return Ok(Value::Null);
    };
    Ok(subscription_quota_from_response(
        account,
        &response,
        managed_auth_provider_label(provider_type),
    ))
}

pub(in crate::api) async fn web_cached_oauth_quota(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
    refresh: bool,
    force: Option<bool>,
) -> Result<Value, ApiError> {
    let account_id = web_resolve_account_id(state, args).await?;
    let Some(account_id) = account_id else {
        return Ok(Value::Null);
    };
    let auth_provider = web_optional_string_any(args, &["authProvider", "auth_provider"])
        .or_else(|| {
            web_optional_auth_provider_type(args)
                .ok()
                .flatten()
                .map(|provider_type| managed_auth_provider_label(provider_type).to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let provider_id = web_optional_string_any(args, &["providerId", "provider_id"]);
    let app_type = web_optional_string_any(args, &["appType", "app_type", "app"]);
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id),
        Query(AccountQuotaQuery {
            refresh: Some(refresh),
            force,
        }),
    )
    .await?
    .0;
    let Some(account) = response.account.as_ref() else {
        return Ok(Value::Null);
    };
    Ok(cached_oauth_quota_from_response(
        &auth_provider,
        account,
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
) -> Result<Value, ApiError> {
    let Some(provider_type) = subscription_tool_provider_type(tool) else {
        return Err(ApiError::bad_request(format!(
            "unsupported subscription quota tool: {tool}"
        )));
    };
    let account_id = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(provider_type, None)
            .map(|account| account.id.clone())
    };
    let Some(account_id) = account_id else {
        return Ok(subscription_quota_not_found(tool));
    };
    let response = account_quota(
        State(state.clone()),
        headers.clone(),
        Path(account_id),
        Query(AccountQuotaQuery {
            refresh: Some(true),
            force: None,
        }),
    )
    .await?
    .0;
    let Some(account) = response.account.as_ref() else {
        return Ok(subscription_quota_not_found(tool));
    };
    Ok(subscription_quota_from_response(account, &response, tool))
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
            let account_id_hint = provider
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref());
            let accounts = state.accounts.read().await;
            return Ok(accounts
                .find_for_provider(provider.provider_type, account_id_hint)
                .map(|account| account.id.clone()));
        }
    }

    let provider_type = web_optional_auth_provider_type(args)?;
    if let Some(provider_type) = provider_type {
        let accounts = state.accounts.read().await;
        return Ok(accounts
            .find_for_provider(provider_type, None)
            .map(|account| account.id.clone()));
    }

    Ok(None)
}

pub(in crate::api) async fn web_share_upsert_input(
    state: &ServerState,
    args: &Value,
) -> Result<UpsertShareInput, ApiError> {
    let value = web_payload(args, &["params", "input", "share"]);
    if let Ok(input) = serde_json::from_value::<UpsertShareInput>(value.clone()) {
        return Ok(input);
    }

    let bindings_value = value.get("bindings").ok_or_else(|| {
        ApiError::bad_request("share params.bindings or app/providerId is required")
    })?;
    let binding_map = serde_json::from_value::<BTreeMap<String, String>>(bindings_value.clone())
        .map_err(ApiError::bad_request)?;
    if binding_map.len() > 1 {
        return Err(ApiError::bad_request("share must have exactly one binding"));
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
    let bindings = vec![ShareBinding {
        app,
        provider_id: provider_id.clone(),
        provider_type,
    }];
    let expires_at = web_optional_i64(value, &["expiresAt", "expires_at"]).or_else(|| {
        web_optional_i64(value, &["expiresInSecs", "expires_in_secs"]).and_then(|seconds| {
            (seconds > 0).then(|| (now_ms() as i64).saturating_add(seconds.saturating_mul(1000)))
        })
    });

    let shared_with_emails =
        web_optional_deserialize::<Vec<String>>(value, "sharedWithEmails")?.unwrap_or_default();
    let market_access_mode =
        web_optional_string_any(value, &["marketAccessMode", "market_access_mode"]);
    let access_by_app = web_optional_deserialize(value, "accessByApp")?.unwrap_or_default();
    let app_settings = web_optional_deserialize(value, "appSettings")?.unwrap_or_default();
    let for_sale_official_price_percent_by_app =
        web_optional_deserialize(value, "forSaleOfficialPricePercentByApp")?.unwrap_or_default();
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
        acl: Some(ShareAcl {
            shared_with_emails,
            public_market_email: None,
            market_access_mode,
        }),
        token_limit: web_optional_u64(value, &["tokenLimit", "token_limit"]),
        parallel_limit: web_optional_u32(value, &["parallelLimit", "parallel_limit"]),
        expires_at,
        for_sale: {
            let (for_sale, _) = web_share_for_sale_flags(value);
            for_sale
        },
        free_access: {
            let (_, free_access) = web_share_for_sale_flags(value);
            free_access
        },
        sale_market_kind: web_optional_string_any(value, &["saleMarketKind", "sale_market_kind"]),
        access_by_app,
        app_settings,
        for_sale_official_price_percent_by_app,
        official_price_percent: None,
        auto_start: web_optional_bool(value, &["autoStart", "auto_start"]),
        description: web_optional_string_any(value, &["description"]),
        bindings,
        runtime_snapshot: None,
        market_grant: None,
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

    if let Ok(Some(email_state)) = crate::clients::router::email_auth::load_state(&state.config_dir)
    {
        if email_state.email.eq_ignore_ascii_case(&current_email) {
            let _ = std::fs::remove_file(crate::clients::router::email_auth::email_auth_path(
                &state.config_dir,
            ));
        }
    }
    if let Err(error) = crate::state::reconcile_payout_profile_to_router(state.clone()).await {
        tracing::warn!(error = %error, "router payout profile reconcile after owner email change failed");
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
    if let Err(error) = crate::state::reconcile_payout_profile_to_router(state.clone()).await {
        tracing::warn!(error = %error, "router payout profile reconcile after owner email verification failed");
    }
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

pub(in crate::api) async fn web_update_share_acl(
    state: &ServerState,
    args: &Value,
) -> Result<Share, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    if let Some(acl_value) = value.get("acl") {
        let acl =
            serde_json::from_value::<ShareAcl>(acl_value.clone()).map_err(ApiError::bad_request)?;
        let share = state
            .try_mutate_shares_immediate(|store| {
                store
                    .replace_acl(&share_id, acl)
                    .ok_or_else(|| ApiError::not_found("share not found"))
            })
            .await
            .map_err(ApiError::internal)??;
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(state, "share.changed", &share, "acl_replaced");
        return Ok(share);
    }

    let patch = ShareSettingsPatch {
        shared_with_emails: web_optional_deserialize(value, "sharedWithEmails")?,
        user_grants: web_optional_deserialize(value, "userGrants")?,
        market_access_mode: web_optional_string_any(value, &["marketAccessMode"]),
        access_by_app: web_optional_deserialize(value, "accessByApp")?,
        app_settings: web_optional_deserialize(value, "appSettings")?,
        sale_market_kind: web_optional_string_any(value, &["saleMarketKind"]),
        ..ShareSettingsPatch::default()
    };
    let share = state
        .try_mutate_shares_immediate(|store| store.apply_settings_patch(&share_id, patch))
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "acl_replaced");
    Ok(share)
}

pub(in crate::api) async fn web_save_provider_share(
    state: &ServerState,
    args: &Value,
) -> Result<Share, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let subdomain = web_arg_string_any(value, &["subdomain"])?;
    let description = web_optional_string_any(value, &["description"]);
    let for_sale = web_arg_string_any(value, &["forSale", "for_sale"])?;
    let sale_market_kind = web_arg_string_any(value, &["saleMarketKind", "sale_market_kind"])?;
    let market_access_mode =
        web_arg_string_any(value, &["marketAccessMode", "market_access_mode"])?;
    let shared_with_emails =
        web_optional_deserialize::<Vec<String>>(value, "sharedWithEmails")?.unwrap_or_default();
    let access_by_app = web_optional_deserialize(value, "accessByApp")?.unwrap_or_default();
    let app_settings = web_optional_deserialize(value, "appSettings")?.unwrap_or_default();
    let for_sale_official_price_percent_by_app =
        web_optional_deserialize(value, "forSaleOfficialPricePercentByApp")?.unwrap_or_default();
    let user_grants =
        web_optional_deserialize::<BTreeMap<String, ShareUserGrant>>(value, "userGrants")?
            .unwrap_or_default();
    let token_limit = web_optional_i64(value, &["tokenLimit", "token_limit"])
        .ok_or_else(|| ApiError::bad_request("tokenLimit is required"))?;
    let parallel_limit = web_optional_i64(value, &["parallelLimit", "parallel_limit"])
        .ok_or_else(|| ApiError::bad_request("parallelLimit is required"))?;
    let expires_at = web_arg_string_any(value, &["expiresAt", "expires_at"])?;

    let mut staged = state.shares.read().await.clone();
    let current = staged
        .get(&share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let subdomain_changed = current.tunnel_subdomain.as_deref() != Some(subdomain.as_str());
    let was_running = current.enabled && current.status == "active";

    staged
        .update_subdomain(&share_id, subdomain)
        .map_err(map_share_patch_error)?;
    staged
        .apply_settings_patch(
            &share_id,
            ShareSettingsPatch {
                description: Some(description),
                for_sale: Some(for_sale),
                sale_market_kind: Some(sale_market_kind),
                market_access_mode: Some(market_access_mode),
                shared_with_emails: Some(shared_with_emails),
                access_by_app: Some(access_by_app),
                app_settings: Some(app_settings),
                for_sale_official_price_percent_by_app: Some(
                    for_sale_official_price_percent_by_app,
                ),
                token_limit: Some(token_limit),
                parallel_limit: Some(parallel_limit),
                expires_at: Some(expires_at),
                user_grants: Some(user_grants),
                ..ShareSettingsPatch::default()
            },
        )
        .map_err(map_share_patch_error)?;
    let candidate = staged
        .canonicalize_primary_app_settings(&share_id)
        .map_err(map_share_patch_error)?;
    staged
        .replace_configured_share(candidate.clone())
        .map_err(map_share_patch_error)?;

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
            crate::clients::router::client::claim_share_subdomain(
                &http_client,
                &config,
                descriptor,
            )
            .await
            .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
        }
    }

    let saved = state
        .try_mutate_shares_immediate(|store| store.replace_configured_share(candidate))
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
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

pub(in crate::api) fn web_upstream_proxy_input(
    args: &Value,
) -> Result<UpdateUpstreamProxyInput, ApiError> {
    let value = web_payload(args, &["config", "input", "params"]);
    if let Ok(input) = serde_json::from_value::<UpdateUpstreamProxyInput>(value.clone()) {
        return Ok(input);
    }
    Ok(UpdateUpstreamProxyInput {
        url: web_optional_string_any(value, &["url", "proxyUrl", "proxy_url"]),
        clear: web_optional_bool(value, &["clear"]).or_else(|| {
            web_optional_bool(value, &["enabled", "proxyEnabled"]).map(|enabled| !enabled)
        }),
        follow_system_proxy: web_optional_bool(value, &["followSystemProxy"]),
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

pub(in crate::api) fn web_global_proxy_config_json(state: &ServerState) -> Value {
    json!({
        "proxyEnabled": true,
        "listenAddress": state.bind_addr.ip().to_string(),
        "listenPort": state.bind_addr.port(),
        "enableLogging": true,
    })
}

pub(in crate::api) async fn web_proxy_takeover_status_json(state: &ServerState) -> Value {
    let providers = state.providers.read().await;
    let ui_settings = state.ui_settings.read().await.for_frontend();

    fn app_takeover(
        providers: &crate::domain::providers::store::ProviderStore,
        ui_settings: &Value,
        app: AppKind,
    ) -> (bool, bool) {
        let has_provider =
            current_provider::resolve_current_provider_id(providers, ui_settings, app).is_some();
        // Server-native routing is always on for the three core apps.
        (has_provider, !has_provider)
    }

    let (claude, claude_pending) = app_takeover(&providers, &ui_settings, AppKind::Claude);
    let (codex, codex_pending) = app_takeover(&providers, &ui_settings, AppKind::Codex);
    let (gemini, gemini_pending) = app_takeover(&providers, &ui_settings, AppKind::Gemini);

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
    let ui_settings = state.ui_settings.read().await.for_frontend();
    let mut active_targets = Vec::new();
    for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
        let Some(provider_id) =
            current_provider::resolve_current_provider_id(&providers, &ui_settings, app)
        else {
            continue;
        };
        let Some(stored) = providers
            .providers
            .iter()
            .find(|provider| provider.app == app && provider.provider.id == provider_id)
        else {
            continue;
        };
        active_targets.push(json!({
            "app_type": app.as_str(),
            "provider_id": provider_id,
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
        "current_provider": active_targets.first().and_then(|target| target.get("provider_name")).cloned().unwrap_or(Value::Null),
        "current_provider_id": active_targets.first().and_then(|target| target.get("provider_id")).cloned().unwrap_or(Value::Null),
        "last_request_at": Value::Null,
        "last_error": Value::Null,
        "failover_count": 0,
        "active_targets": active_targets,
    })
}

pub(in crate::api) async fn web_upstream_proxy_status_json(state: &ServerState) -> Value {
    let config = state.config.read().await;
    let url = config.upstream_proxy.url.clone();
    let enabled = url.as_ref().is_some_and(|value| !value.trim().is_empty());
    json!({
        "enabled": enabled,
        "proxyUrl": url,
    })
}

pub(in crate::api) async fn web_test_proxy_url(url: &str) -> Value {
    if url.trim().is_empty() {
        return json!({
            "success": false,
            "latencyMs": 0,
            "error": "Proxy URL is empty",
        });
    }

    let start = Instant::now();
    let proxy = match reqwest::Proxy::all(url) {
        Ok(proxy) => proxy,
        Err(error) => {
            return json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("Invalid proxy URL: {error}"),
            });
        }
    };

    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("Failed to build client: {error}"),
            });
        }
    };

    let test_urls = [
        "https://httpbin.org/get",
        "https://www.google.com",
        "https://api.anthropic.com",
    ];
    let mut last_error = None;
    for test_url in test_urls {
        match client.head(test_url).send().await {
            Ok(_) => {
                return json!({
                    "success": true,
                    "latencyMs": start.elapsed().as_millis() as u64,
                    "error": Value::Null,
                });
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    json!({
        "success": false,
        "latencyMs": start.elapsed().as_millis() as u64,
        "error": last_error.unwrap_or_else(|| "All test targets failed".to_string()),
    })
}

pub(in crate::api) async fn web_scan_local_proxies() -> Vec<Value> {
    const PROXY_PORTS: &[(u16, &str, bool)] = &[
        (7890, "http", true),
        (7891, "socks5", false),
        (1080, "socks5", false),
        (8080, "http", false),
        (8888, "http", false),
        (3128, "http", false),
        (10808, "socks5", false),
        (10809, "http", false),
    ];

    tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        for &(port, primary_type, is_mixed) in PROXY_PORTS {
            let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            if TcpStream::connect_timeout(&addr.into(), Duration::from_millis(100)).is_ok() {
                found.push(json!({
                    "url": format!("{primary_type}://127.0.0.1:{port}"),
                    "proxyType": primary_type,
                    "port": port,
                }));
                if is_mixed {
                    let alt_type = if primary_type == "http" {
                        "socks5"
                    } else {
                        "http"
                    };
                    found.push(json!({
                        "url": format!("{alt_type}://127.0.0.1:{port}"),
                        "proxyType": alt_type,
                        "port": port,
                    }));
                }
            }
        }
        found
    })
    .await
    .unwrap_or_default()
}

pub(in crate::api) async fn web_app_proxy_config_json(state: &ServerState, app: AppKind) -> Value {
    let failover = state.failover.read().await;
    let auto_failover_enabled = failover
        .apps
        .get(&app)
        .map(|config| config.enabled)
        .unwrap_or(false);
    let app_config = state
        .ui_settings
        .read()
        .await
        .value
        .get("proxyAppConfigs")
        .and_then(|configs| configs.get(app.as_str()))
        .cloned();
    if let Some(config) = app_config {
        return config;
    }
    let failure_threshold = failover
        .apps
        .get(&app)
        .map(|config| config.failure_threshold)
        .unwrap_or(4);
    let timeout_seconds = failover
        .apps
        .get(&app)
        .map(|config| (config.open_duration_ms / 1000).max(1))
        .unwrap_or(60);
    json!({
        "appType": app.as_str(),
        "enabled": true,
        "autoFailoverEnabled": auto_failover_enabled,
        "maxRetries": 3,
        "streamingFirstByteTimeout": 60,
        "streamingIdleTimeout": 120,
        "nonStreamingTimeout": 600,
        "circuitFailureThreshold": failure_threshold,
        "circuitSuccessThreshold": 2,
        "circuitTimeoutSeconds": timeout_seconds,
        "circuitErrorRateThreshold": 0.6,
        "circuitMinRequests": 10,
    })
}

pub(in crate::api) async fn web_available_providers_for_failover(
    state: &ServerState,
    app: AppKind,
) -> Vec<Provider> {
    let failover = state.failover.read().await;
    let providers = state.providers.read().await;
    let queue_ids = failover
        .apps
        .get(&app)
        .map(|config| config.provider_queue.as_slice())
        .unwrap_or(&[]);
    providers
        .providers
        .iter()
        .filter(|stored| stored.app == app)
        .filter(|stored| !queue_ids.iter().any(|id| id == &stored.provider.id))
        .map(|stored| stored.provider.clone())
        .collect()
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

pub(in crate::api) fn web_optional_u128(args: &Value, keys: &[&str]) -> Option<u128> {
    keys.iter().find_map(|key| {
        args.get(*key).and_then(|value| {
            value
                .as_u64()
                .map(|number| number as u128)
                .or_else(|| value.as_i64().map(|number| number.max(0) as u128))
        })
    })
}

pub(in crate::api) fn web_usage_stats_filter_from_args(args: &Value) -> UsageStatsFilter {
    let app = web_optional_string_any(args, &["appType", "app", "app_type"])
        .as_deref()
        .and_then(|value| parse_app_kind(value).ok());
    UsageStatsFilter {
        from_ms: web_optional_u128(args, &["startDate", "fromMs", "from_ms"]),
        to_ms: web_optional_u128(args, &["endDate", "toMs", "to_ms"]),
        app,
        provider_id: web_optional_string_any(args, &["providerName", "providerId", "provider_id"]),
        ..UsageStatsFilter::default()
    }
}

pub(in crate::api) fn web_request_logs_json(usage: &UsageStore, args: &Value) -> Value {
    const DEFAULT_PAGE_SIZE: usize = 20;
    const MAX_PAGE_SIZE: usize = 200;

    let filters = args
        .get("filters")
        .filter(|value| value.is_object())
        .unwrap_or(args);
    let app_type = web_optional_string_any(filters, &["appType", "app_type", "app"])
        .filter(|value| value != "all");
    let provider_name = web_optional_string_any(filters, &["providerName", "provider_name"]);
    let model = web_optional_string_any(filters, &["model"]);
    let share_id = web_optional_string_any(filters, &["shareId", "share_id"]);
    let status_code = web_optional_u32(filters, &["statusCode", "status_code"])
        .and_then(|value| u16::try_from(value).ok());
    let from_ms = web_request_log_date_bound_ms(filters, true);
    let to_ms = web_request_log_date_bound_ms(filters, false);

    let page = web_optional_u64(args, &["page"])
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let page_size = web_optional_u64(args, &["pageSize", "page_size"])
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);

    let matches = |log: &&UsageLog| {
        from_ms.is_none_or(|from| log.created_at_ms >= from)
            && to_ms.is_none_or(|to| log.created_at_ms <= to)
            && app_type
                .as_deref()
                .is_none_or(|app_type| log.app.as_str() == app_type)
            && provider_name
                .as_deref()
                .is_none_or(|provider_name| log.provider_name == provider_name)
            && model
                .as_deref()
                .is_none_or(|model| web_request_log_effective_model(log) == model)
            && share_id
                .as_deref()
                .is_none_or(|share_id| log.share_id.as_deref() == Some(share_id))
            && status_code.is_none_or(|status_code| log.status_code == status_code)
    };
    let mut matching = usage.logs.iter().filter(matches).collect::<Vec<_>>();
    matching.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    let total = matching.len();
    let offset = page.saturating_mul(page_size);
    let data = matching
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(web_request_log_json)
        .collect::<Vec<_>>();

    json!({
        "data": data,
        "total": total,
        "page": page,
        "pageSize": page_size,
    })
}

fn web_request_log_date_bound_ms(filters: &Value, is_start: bool) -> Option<u128> {
    let (seconds_keys, milliseconds_keys) = if is_start {
        (["startDate", "start_date"], ["fromMs", "from_ms"])
    } else {
        (["endDate", "end_date"], ["toMs", "to_ms"])
    };
    if let Some(seconds) = web_optional_u64(filters, &seconds_keys) {
        let milliseconds = u128::from(seconds).saturating_mul(1_000);
        return Some(if is_start {
            milliseconds
        } else {
            milliseconds.saturating_add(999)
        });
    }
    web_optional_u64(filters, &milliseconds_keys).map(u128::from)
}

fn web_request_log_effective_model(log: &UsageLog) -> &str {
    log.pricing_model
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(log.model.as_deref())
        .unwrap_or_default()
}

fn web_request_log_json(log: &UsageLog) -> Value {
    let model = log
        .model
        .as_deref()
        .or(log.requested_model.as_deref())
        .or(log.actual_model.as_deref())
        .unwrap_or_default();
    let requested_model = log.requested_model.as_deref().unwrap_or(model);
    let actual_model = log.actual_model.as_deref().unwrap_or(model);

    json!({
        "requestId": log.request_id,
        "providerId": log.provider_id,
        "providerName": log.provider_name,
        "appType": log.app.as_str(),
        "model": model,
        "requestModel": requested_model,
        "requestAgent": log.request_agent.as_deref().unwrap_or_default(),
        "requestedModel": requested_model,
        "actualModel": actual_model,
        "actualModelSource": log.actual_model_source.as_deref().unwrap_or("server"),
        "pricingModel": log.pricing_model,
        "costMultiplier": web_request_log_multiplier(log.cost_multiplier),
        "inputTokens": web_request_log_token_count(log.input_tokens),
        "outputTokens": web_request_log_token_count(log.output_tokens),
        "cacheReadTokens": web_request_log_token_count(log.cache_read_tokens),
        "cacheCreationTokens": web_request_log_token_count(log.cache_creation_tokens),
        "inputCostUsd": web_request_log_cost(log.input_cost_usd),
        "outputCostUsd": web_request_log_cost(log.output_cost_usd),
        "cacheReadCostUsd": web_request_log_cost(log.cache_read_cost_usd),
        "cacheCreationCostUsd": web_request_log_cost(log.cache_creation_cost_usd),
        "totalCostUsd": web_request_log_cost(log.total_cost_usd),
        "isStreaming": log.is_streaming,
        "latencyMs": web_request_log_u128_to_u64(log.duration_ms),
        "firstTokenMs": log.first_token_ms.map(web_request_log_u128_to_u64),
        "durationMs": web_request_log_u128_to_u64(log.duration_ms),
        "statusCode": log.status_code,
        "errorMessage": Value::Null,
        "createdAt": web_request_log_u128_to_i64(log.created_at_ms / 1_000),
        "shareId": log.share_id,
        "shareName": log.share_name,
        "userEmail": log.user_email,
        "dataSource": log.data_source,
    })
}

fn web_request_log_token_count(value: Option<u64>) -> u32 {
    value.unwrap_or(0).min(u64::from(u32::MAX)) as u32
}

fn web_request_log_u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

fn web_request_log_u128_to_i64(value: u128) -> i64 {
    value.min(i64::MAX as u128) as i64
}

fn web_request_log_cost(value: Option<f64>) -> String {
    let value = value.filter(|value| value.is_finite()).unwrap_or(0.0);
    format!("{value:.6}")
}

fn web_request_log_multiplier(value: Option<f64>) -> String {
    let value = value.filter(|value| value.is_finite()).unwrap_or(1.0);
    value.to_string()
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

pub(in crate::api) fn web_share_for_sale_flags(args: &Value) -> (Option<bool>, Option<bool>) {
    if let Some(value) = web_optional_string_any(args, &["forSale", "for_sale"]) {
        return match value.trim().to_ascii_lowercase().as_str() {
            "free" => (Some(false), Some(true)),
            "yes" | "true" | "1" | "share" => (Some(true), Some(false)),
            _ => (Some(false), Some(false)),
        };
    }
    if let Some(value) = web_optional_bool(args, &["forSale", "for_sale"]) {
        return (Some(value), Some(false));
    }
    (None, None)
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

pub(in crate::api) fn provider_extra_string(provider: &Provider, key: &str) -> Option<String> {
    provider
        .extra
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

pub(in crate::api) fn provider_record_for_app(
    providers: &[StoredProvider],
    app: AppKind,
) -> BTreeMap<String, Provider> {
    providers
        .iter()
        .filter(|provider| provider.app == app)
        .map(|provider| (provider.provider.id.clone(), provider.provider.clone()))
        .collect()
}

pub(in crate::api) fn managed_auth_provider_label(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::GitHubCopilot => "github_copilot",
        ProviderType::CodexOAuth => "codex_oauth",
        ProviderType::GrokOAuth => "grok_oauth",
        ProviderType::ClaudeOAuth => "claude_oauth",
        ProviderType::GeminiCli => "google_gemini_oauth",
        ProviderType::AntigravityOAuth => "antigravity_oauth",
        ProviderType::CursorOAuth => "cursor_oauth",
        ProviderType::KiroOAuth => "kiro_oauth",
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
    let subscription_expiry =
        crate::domain::accounts::subscription_expiry::resolved_subscription_expiry(account);
    let manual_expires_at = (subscription_expiry.capability
        == crate::domain::accounts::subscription_expiry::SubscriptionExpiryCapability::ManualRequired)
        .then_some(account.manual_subscription_expires_at_ms)
        .flatten()
        .and_then(|timestamp_ms| subscription_expiry_rfc3339(Some(timestamp_ms)));
    let effective_expires_at = subscription_expiry_rfc3339(subscription_expiry.expires_at_ms);
    let expiry_kind = subscription_expiry.source.map(|source| match source {
        crate::domain::accounts::subscription_expiry::SubscriptionExpirySource::Automatic => {
            "subscription"
        }
        crate::domain::accounts::subscription_expiry::SubscriptionExpirySource::Manual => {
            "billing_period"
        }
    });
    json!({
        "id": account.id,
        "provider": provider_label,
        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
        "email": account.email,
        "avatar_url": Value::Null,
        "authenticated_at": account_authenticated_at(account),
        "is_default": default_account_id == Some(account.id.as_str()),
        "github_domain": "github.com",
        "workspaces": workspaces,
        "selected_workspace_id": selected_workspace_id,
        "subscriptionExpiry": {
            "capability": subscription_expiry.capability,
            "manualExpiresAt": manual_expires_at,
            "effectiveExpiresAt": effective_expires_at,
            "source": subscription_expiry.source,
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
        Some("cli") | Some("browser") | Some("cli_oauth") | Some("clioauth")
    )
}

pub(in crate::api) fn web_managed_auth_redirect_uri(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
    provider_type: ProviderType,
    oauth_flow_mode: Option<&str>,
) -> String {
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
    if let Some(uri) = web_optional_string_any(
        args,
        &[
            "redirectUri",
            "redirect_uri",
            "codexCallbackUrl",
            "codex_callback_url",
        ],
    ) {
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
        format!("cli:{}", login.state)
    } else {
        login.state.clone()
    };
    json!({
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
    let default_account_id = accounts
        .accounts
        .iter()
        .filter(|account| account.provider_type == provider_type)
        .map(|account| account.id.as_str())
        .next();
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
        ProviderType::CodexOAuth if !device_code.starts_with("cli:") => {
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
            let poll_state = device_code
                .strip_prefix("cli:")
                .unwrap_or(device_code.as_str());
            let poll_status = state
                .mutate_oauth_logins(|store| {
                    store.poll_state_by_oauth_state(poll_state, now_ms() as i64)
                })
                .await;
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
                    session_id: None,
                    state: Some(poll_state.to_string()),
                    code: None,
                    execute_token_exchange: Some(true),
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
    if provider_type == ProviderType::CodexOAuth && !device_code.starts_with("cli:") {
        let response = cancel_codex_device_login(
            State(state),
            headers,
            Json(CancelCodexDeviceLoginRequest { device_code }),
        )
        .await?
        .0;
        return Ok(json!(response));
    }
    if matches!(
        provider_type,
        ProviderType::GitHubCopilot | ProviderType::KiroOAuth
    ) {
        return Ok(json!({"ok": true, "cancelled": false}));
    }
    let oauth_state = device_code
        .strip_prefix("cli:")
        .unwrap_or(device_code.as_str())
        .to_string();
    let response = cancel_account_login(
        State(state),
        headers,
        Json(CancelAccountLoginRequest {
            session_id: None,
            state: Some(oauth_state),
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
        .map_err(ApiError::internal)??;
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
    // Serialize workspace changes with token/quota refreshes for the same
    // account. Otherwise an in-flight workspace A response could be persisted
    // after workspace B has cleared the old cache.
    let _refresh_guard = state
        .account_refresh_locks
        .lock(ProviderType::CodexOAuth, &account_id)
        .await;
    let (account_before_workspace_change, account) = state
        .try_mutate_accounts_immediate(|store| {
            let before = store
                .accounts
                .iter()
                .find(|account| {
                    account.id == account_id && account.provider_type == ProviderType::CodexOAuth
                })
                .cloned()
                .ok_or_else(|| "codex account not found".to_string())?;
            let account = store.select_codex_workspace(&account_id, &workspace_id)?;
            Ok::<_, String>((before, account))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    state
        .refresh_automatic_subscription_metadata_if_changed(
            &account_before_workspace_change,
            &account,
        )
        .await
        .map_err(ApiError::internal)?;
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
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let account_ids = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .filter(|account| account.provider_type == provider_type)
        .map(|account| account.id.clone())
        .collect::<Vec<_>>();
    for account_id in account_ids {
        let _ = delete_account(State(state.clone()), headers.clone(), Path(account_id)).await?;
    }
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
