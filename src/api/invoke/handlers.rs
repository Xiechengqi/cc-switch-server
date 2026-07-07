use super::super::*;
use std::collections::BTreeMap;

use crate::domain::sharing::router_contract::ShareSettingsPatch;

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
    {
        let mut store = state.ui_settings.write().await;
        let mut proxy_app_configs = store
            .value
            .get("proxyAppConfigs")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(map) = proxy_app_configs.as_object_mut() {
            map.insert(app.as_str().to_string(), config.clone());
        }
        store.apply_patch(json!({ "proxyAppConfigs": proxy_app_configs }));
    }
    state.save_ui_settings().await.map_err(ApiError::internal)?;

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
    if let Some(current) = live_import::read_current_provider_id(&ui_settings, app) {
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
                return Ok(Value::Null);
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
    Ok(json!(response))
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
    let mut bindings = Vec::new();
    for app_name in ["claude", "codex", "gemini"] {
        let Some(provider_id) = binding_map
            .get(app_name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|provider_id| !provider_id.is_empty())
        else {
            continue;
        };
        let app = parse_app_kind(app_name)?;
        let provider_type = web_provider_type_for_binding(state, app, provider_id).await?;
        bindings.push(ShareBinding {
            app,
            provider_id: provider_id.to_string(),
            provider_type,
        });
    }
    let primary = bindings
        .first()
        .cloned()
        .ok_or_else(|| ApiError::bad_request("at least one share binding is required"))?;
    let expires_at = web_optional_i64(value, &["expiresAt", "expires_at"]).or_else(|| {
        web_optional_i64(value, &["expiresInSecs", "expires_in_secs"]).and_then(|seconds| {
            (seconds > 0).then(|| (now_ms() as i64).saturating_add(seconds.saturating_mul(1000)))
        })
    });

    Ok(UpsertShareInput {
        id: web_optional_string_any(value, &["id", "shareId", "share_id"]),
        owner_email: web_optional_string_any(value, &["ownerEmail", "owner_email"]),
        app: primary.app,
        provider_id: primary.provider_id.clone(),
        provider_type: primary.provider_type,
        display_name: web_optional_string_any(value, &["displayName", "name"]),
        enabled: web_optional_bool(value, &["enabled"]),
        status: web_optional_string_any(value, &["status"]),
        subscription_level: None,
        account_email: None,
        quota_percent: None,
        tunnel_subdomain: web_optional_string_any(value, &["tunnelSubdomain", "subdomain"]),
        acl: None,
        token_limit: web_optional_u64(value, &["tokenLimit", "token_limit"]),
        parallel_limit: web_optional_u32(value, &["parallelLimit", "parallel_limit"]),
        expires_at,
        for_sale: web_optional_share_for_sale(value),
        sale_market_kind: web_optional_string_any(value, &["saleMarketKind", "sale_market_kind"]),
        access_by_app: BTreeMap::new(),
        app_settings: BTreeMap::new(),
        for_sale_official_price_percent_by_app: BTreeMap::new(),
        official_price_percent: None,
        auto_start: web_optional_bool(value, &["autoStart", "auto_start"]),
        description: web_optional_string_any(value, &["description"]),
        bindings,
        runtime_snapshot: None,
        market_grant: None,
    })
}

pub(in crate::api) async fn web_share_binding_input(
    state: &ServerState,
    args: &Value,
) -> Result<(String, ShareBinding), ApiError> {
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    if let Some(binding_value) = value.get("binding") {
        let binding = serde_json::from_value::<ShareBinding>(binding_value.clone())
            .map_err(ApiError::bad_request)?;
        return Ok((share_id, binding));
    }

    let app = web_arg_string_any(value, &["appType", "app", "app_type"])
        .and_then(|value| parse_app_kind(&value))?;
    let provider_id = web_arg_string_any(value, &["providerId", "provider_id"])?;
    let provider_type = web_optional_string_any(value, &["providerType", "provider_type"])
        .map(|value| web_parse_provider_type(&value))
        .transpose()?
        .unwrap_or(web_provider_type_for_binding(state, app, &provider_id).await?);
    Ok((
        share_id,
        ShareBinding {
            app,
            provider_id,
            provider_type,
        },
    ))
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
    ensure_share_owner_target_verified(state, &owner_email).await?;
    let share = state
        .try_mutate_shares_immediate(|store| store.update_owner_email(&share_id, &owner_email))
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "owner_email_updated");
    Ok(share)
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
    ensure_share_owner_target_verified(state, &target_email).await?;
    let share = state
        .try_mutate_shares_immediate(|store| store.transfer_owner_email(&share_id, &target_email))
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "owner_transferred");
    Ok(share)
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
    state: &ServerState,
    args: &Value,
) -> Result<crate::clients::router::email_auth::EmailCodeRequestResponse, ApiError> {
    let router_domain = web_optional_string_any(args, &["routerDomain", "router_domain"]);
    let current_email = web_arg_string_any(args, &["currentEmail", "current_email"])?;
    let new_email = web_arg_string_any(args, &["newEmail", "new_email"])?;
    let config = ensure_email_router_config(state).await?;
    ensure_router_domain_matches(&config, router_domain.as_deref())?;
    let (current_email, new_email) =
        ensure_owner_change_allowed(state, &config, &current_email, &new_email).await?;
    let http_client = state.http_client().await;
    crate::clients::router::email_auth::request_code(&http_client, &config, &new_email)
        .await
        .map_err(map_email_auth_error)
        .map(|response| {
            tracing::info!(
                old_owner = %current_email,
                new_owner = %new_email,
                "requested share owner change email code"
            );
            response
        })
}

pub(in crate::api) async fn web_email_auth_change_owner_email(
    state: &ServerState,
    args: &Value,
) -> Result<crate::clients::router::email_auth::EmailAuthStatus, ApiError> {
    let router_domain = web_optional_string_any(args, &["routerDomain", "router_domain"]);
    let current_email = web_arg_string_any(args, &["currentEmail", "current_email"])?;
    let new_email = web_arg_string_any(args, &["newEmail", "new_email"])?;
    let code = web_arg_string_any(args, &["code"])?;
    let config = ensure_email_router_config(state).await?;
    ensure_router_domain_matches(&config, router_domain.as_deref())?;
    let (current_email, new_email) =
        ensure_owner_change_allowed(state, &config, &current_email, &new_email).await?;
    let http_client = state.http_client().await;
    let router_session = crate::clients::router::email_auth::verify_client_web_code(
        &http_client,
        &config,
        &new_email,
        &code,
    )
    .await
    .map_err(map_email_auth_error)?;
    let verified_email =
        crate::clients::router::email_auth::normalize_email(&router_session.user.email)
            .map_err(map_email_auth_error)?;
    if verified_email != new_email {
        return Err(ApiError::unauthorized(
            "verified email does not match new owner email",
        ));
    }
    let remote = crate::clients::router::email_auth::change_owner_email(
        &http_client,
        &config,
        &current_email,
        &new_email,
        &router_session.access_token,
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
        .try_mutate_shares_immediate(|store| {
            store.change_owner_email_for_all(&current_email, &new_email)
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(map_share_patch_error)?;
    for share in &updated_shares {
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(
            state,
            "share.changed",
            share,
            "owner_email_verified_changed",
        );
    }

    let email_state = crate::clients::router::email_auth::state_from_router_session(
        &next_config,
        &router_session,
    )
    .map_err(map_email_auth_error)?;
    crate::clients::router::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;

    Ok(crate::clients::router::email_auth::EmailAuthStatus {
        authenticated: true,
        email: Some(new_email),
        expires_at: email_state.expires_at,
        router_domain: email_state.router_domain,
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

async fn ensure_owner_change_allowed(
    state: &ServerState,
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
    let email_state = crate::clients::router::email_auth::load_state(&state.config_dir)
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::unauthorized("owner change requires current owner email auth login")
        })?;
    if email_state.email.trim().to_ascii_lowercase() != current_email {
        return Err(ApiError::unauthorized(
            "current email auth state does not match share owner",
        ));
    }
    if !state.shares.read().await.shares.iter().any(|share| {
        share
            .owner_email
            .as_deref()
            .is_some_and(|email| email.eq_ignore_ascii_case(&current_email))
    }) {
        return Err(ApiError::not_found(
            "this server has no share owned by the current email",
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

async fn ensure_share_owner_target_verified(
    state: &ServerState,
    target_email: &str,
) -> Result<(), ApiError> {
    let target_email = crate::clients::router::email_auth::normalize_email(target_email)
        .map_err(map_email_auth_error)?;
    let config = state.config.read().await;
    let configured_owner = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("owner email is not configured"))?
        .trim()
        .to_ascii_lowercase();
    if configured_owner != target_email {
        return Err(ApiError::forbidden(
            "share owner email change requires email_auth_change_owner_email verification",
        ));
    }
    drop(config);
    let status = crate::clients::router::email_auth::get_status(&state.config_dir)
        .map_err(ApiError::internal)?;
    if !status.authenticated || status.email.as_deref() != Some(target_email.as_str()) {
        return Err(ApiError::forbidden(
            "share owner email change requires verified owner email auth",
        ));
    }
    Ok(())
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
            "healthy": matches!(
                status.status.as_str(),
                "connected" | "running" | "active"
            ),
        }))
    });
    json!({
        "info": info,
        "lastError": last_error,
        "requiresOwnerLogin": false,
    })
}

pub(in crate::api) async fn web_client_tunnel_state(state: &ServerState) -> Value {
    let config = state.config.read().await;
    let runtime = state
        .tunnels
        .status(&crate::clients::router::tunnel::client_tunnel_key())
        .await;
    let tunnel_url = runtime
        .as_ref()
        .and_then(|status| status.tunnel_url.clone());
    let subdomain = config.client.tunnel_subdomain.clone().unwrap_or_default();
    let owner_email = config.owner.email.clone().unwrap_or_default();
    let enabled = matches!(
        config.client.tunnel_status.as_deref(),
        Some("active") | Some("running") | Some("connected")
    ) || runtime
        .as_ref()
        .is_some_and(|status| matches!(status.status.as_str(), "connected" | "running" | "active"));
    let status = web_client_tunnel_share_status(runtime);
    let mut response = json!({
        "config": {
            "ownerEmail": owner_email,
            "subdomain": subdomain,
            "enabled": enabled,
            "autoStart": true,
            "tunnelUrl": tunnel_url,
        }
    });
    if let Value::Object(ref mut map) = response {
        map.insert("status".into(), status);
    }
    response
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
    Ok(json!({
        "shareId": share.id,
        "status": share.status,
        "lastError": share.last_error,
        "runtimeStatus": runtime_status,
        "requiresOwnerLogin": false
    }))
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

pub(in crate::api) fn web_proxy_status_json(state: &ServerState) -> Value {
    json!({
        "running": true,
        "address": state.bind_addr.ip().to_string(),
        "port": state.bind_addr.port(),
        "active_connections": 0,
        "total_requests": 0,
        "success_requests": 0,
        "failed_requests": 0,
        "success_rate": 100.0,
        "uptime_seconds": 0,
        "current_provider": Value::Null,
        "current_provider_id": Value::Null,
        "last_request_at": Value::Null,
        "last_error": Value::Null,
        "failover_count": 0,
        "active_targets": [],
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

pub(in crate::api) fn web_optional_share_for_sale(args: &Value) -> Option<bool> {
    if let Some(value) = web_optional_bool(args, &["forSale", "for_sale"]) {
        return Some(value);
    }
    web_optional_string_any(args, &["forSale", "for_sale"]).map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "1" | "free"
        )
    })
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

pub(in crate::api) fn map_managed_auth_account(
    account: &Account,
    provider_label: &str,
    default_account_id: Option<&str>,
) -> Value {
    json!({
        "id": account.id,
        "provider": provider_label,
        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
        "email": account.email,
        "avatar_url": Value::Null,
        "authenticated_at": account_authenticated_at(account),
        "is_default": default_account_id == Some(account.id.as_str()),
        "github_domain": "github.com"
    })
}

const CLAUDE_WEB_PASTE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";

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
                Ok(OAuthSessionPollState::Ready) => {}
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
    state
        .try_mutate_accounts_immediate(|store| {
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
            Ok(())
        })
        .await
        .map_err(ApiError::internal)??;
    Ok(Value::Null)
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
