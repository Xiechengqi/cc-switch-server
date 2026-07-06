use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use axum::body::{Body, Bytes};
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::Datelike;
use futures_util::Stream;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::Sha256;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::build_info::{build_info, BuildInfo};
use crate::core::account_managers::{manager_for, AccountManager};
use crate::core::account_refresh::{
    account_needs_native_refresh, execute_native_account_refresh, execute_oauth_json_request,
    execute_oauth_token_request, provider_native_refresh_available, AccountRefreshFailure,
};
use crate::core::accounts::{
    Account, AccountQuota, AccountRefreshUpdate, AccountStore, UpsertAccountInput,
};
use crate::core::config::{
    mask_proxy_url, RouterConfig, ServerConfig, SetupInput, UpdateClientTunnelInput,
    UpdateRouterConfigInput, UpdateUpstreamProxyInput,
};
use crate::core::copilot_device::CopilotDeviceError;
use crate::core::email_auth::EmailAuthError;
use crate::core::failover::{FailoverAppConfig, FailoverSnapshot, UpdateFailoverAppInput};
use crate::core::health::ProviderHealth;
use crate::core::kiro_device::KiroDeviceError;
use crate::core::live_config_import;
use crate::core::oauth_clients::{
    build_profile_request, build_refresh_request, oauth_provider_spec, token_expires_soon,
    upsert_input_from_login_response, OAuthAuthorizeFlow, OAuthHttpRequest, OAuthQuotaStrategy,
    OAuthSupportStage,
};
use crate::core::oauth_login::{
    OAuthLoginError, OAuthLoginFinish, OAuthLoginStart, OAuthLoginStatus, OAuthSessionPollState,
};
use crate::core::pricing::{ModelPricingEntry, UpdateModelPricingInput};
use crate::core::provider::{
    classify_provider_response, AppKind, Provider, ProviderType, ProviderTypeRequest,
    ProviderTypeResponse,
};
use crate::core::providers::{ProviderSortUpdate, StoredProvider};
use crate::core::quota::{refresh_account_quota, QuotaRefreshFailure, QuotaRefreshResult};
use crate::core::router_client::RouterRegisterResult;
use crate::core::router_client::{
    ShareAppAvailability, ShareAppProviders, ShareAppRuntimes, ShareRequestLogEntry,
    ShareSettingsPatch, ShareSupport,
};
use crate::core::shares::{
    Share, ShareAcl, ShareBinding, ShareMarketGrantStatus, ShareStore, ShareUpdateError,
    UpsertShareInput,
};
use crate::core::ui_settings;
use crate::core::universal_providers::{
    provider_from_universal, universal_provider_presets, UniversalProvider,
    UniversalProviderPreset, UniversalProviderSyncResult,
};
use crate::core::usage::{
    ModelUsageStats, ProviderUsageStats, UsageLog, UsageLogFilter, UsageRollup, UsageStatsFilter,
    UsageStore, UsageTrendPoint,
};
use crate::coverage::ProviderCoverage;
use crate::proxy::adapters::ProviderAdapter;
use crate::proxy::{self, ProxyRoute};
use crate::state::{
    save_accounts_debounced, save_shares_debounced, ServerEvent, ServerState, Session,
};
use crate::web_assets;
use crate::web_runtime::{self, WebRuntimeCommandSupport};

type HmacSha256 = Hmac<Sha256>;

const APPLY_SHARE_SETTINGS_PATH: &str = "/_ctl/apply_share_settings";
const REFRESH_SHARE_USAGE_PATH: &str = "/_ctl/refresh_share_usage";
const CONTROL_SIGNATURE_WINDOW_MS: i64 = 5 * 60 * 1000;
const SHARE_ROUTER_REQUEST_LOGS_LIMIT: usize = 10;

pub async fn serve(state: ServerState) -> anyhow::Result<()> {
    let app = app_router(state.clone());

    let listener = tokio::net::TcpListener::bind(state.bind_addr)
        .await
        .with_context(|| format!("bind {}", state.bind_addr))?;

    tracing::info!("cc-switch-server listening on {}", state.bind_addr);
    axum::serve(listener, app).await.context("serve http")
}

pub fn app_router(state: ServerState) -> Router {
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/_share-router/health", get(share_router_health))
        .route(
            "/_share-router/request-logs",
            get(share_router_request_logs),
        )
        .route("/_share-router/share-runtime", get(share_router_runtime))
        .route(
            "/_share-router/model-health",
            post(share_router_model_health),
        )
        .route(
            APPLY_SHARE_SETTINGS_PATH,
            post(control_apply_share_settings),
        )
        .route(REFRESH_SHARE_USAGE_PATH, post(control_refresh_share_usage))
        .route("/api/setup/status", get(setup_status))
        .route("/api/setup", post(setup))
        .route("/api/auth/login", post(login))
        .route("/api/auth/password", put(change_password))
        .route(
            "/api/auth/email/request-code",
            post(request_email_login_code),
        )
        .route("/api/auth/email/verify-code", post(verify_email_login_code))
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/api-token", post(rotate_api_token))
        .route("/api/events", get(events))
        .route("/api/backup", get(list_backups).post(create_backup))
        .route("/api/backups", get(list_backups).post(create_backup))
        .route("/api/backup/:id/restore", post(restore_backup))
        .route("/api/backups/:id/restore", post(restore_backup))
        .route("/api/config", get(config_snapshot))
        .route(
            "/api/upstream-proxy",
            get(upstream_proxy).put(update_upstream_proxy),
        )
        .route("/api/providers", get(list_providers).post(create_provider))
        .route("/api/providers/export", get(export_providers))
        .route("/api/providers/import", post(import_providers))
        .route(
            "/api/universal-providers",
            get(list_universal_providers).post(upsert_universal_provider),
        )
        .route(
            "/api/universal-providers/export",
            get(export_universal_providers),
        )
        .route(
            "/api/universal-providers/import",
            post(import_universal_providers),
        )
        .route(
            "/api/universal-providers/:id",
            get(get_universal_provider).delete(delete_universal_provider),
        )
        .route(
            "/api/universal-providers/:id/sync",
            post(sync_universal_provider),
        )
        .route(
            "/api/universal-provider-presets",
            get(universal_provider_presets_route),
        )
        .route("/api/providers/health", get(provider_health))
        .route("/api/failover", get(failover_snapshot))
        .route("/api/failover/apps/:app", put(update_failover_app))
        .route(
            "/api/failover/providers/:provider_id/reset",
            post(reset_failover_provider),
        )
        .route("/api/providers/test", post(test_providers))
        .route("/api/providers/:id/test", post(test_provider))
        .route(
            "/api/providers/:id/fetch-models",
            post(fetch_provider_models),
        )
        .route(
            "/api/providers/from-preset",
            post(create_provider_from_preset),
        )
        .route("/api/provider-presets", get(provider_presets))
        .route("/api/provider-coverage", get(provider_coverage))
        .route("/api/provider-matrix", get(provider_matrix))
        .route("/api/provider-type", post(provider_type))
        .route("/api/accounts", get(list_accounts).post(upsert_account))
        .route("/api/accounts/capabilities", get(account_capabilities))
        .route(
            "/api/accounts/import-templates",
            get(account_import_templates),
        )
        .route("/api/accounts/login/start", post(start_account_login))
        .route("/api/accounts/login/callback", get(account_login_callback))
        .route("/api/accounts/login/finish", post(finish_account_login))
        .route(
            "/api/accounts/copilot/device/start",
            post(start_copilot_device_login),
        )
        .route(
            "/api/accounts/copilot/device/poll",
            post(poll_copilot_device_login),
        )
        .route(
            "/api/accounts/kiro/device/start",
            post(start_kiro_device_login),
        )
        .route(
            "/api/accounts/kiro/device/poll",
            post(poll_kiro_device_login),
        )
        .route("/api/accounts/:id", delete(delete_account))
        .route("/api/accounts/:id/refresh", post(refresh_account))
        .route("/api/accounts/:id/refresh-plan", get(account_refresh_plan))
        .route("/api/accounts/:id/quota", get(account_quota))
        .route("/api/usage/trends", get(usage_trends))
        .route("/api/usage/provider-stats", get(usage_provider_stats))
        .route("/api/usage/model-stats", get(usage_model_stats))
        .route("/api/usage/logs/:id", get(usage_log_detail))
        .route("/api/usage/logs", get(usage_logs))
        .route("/api/usage/summary", get(usage_summary))
        .route("/api/usage/backfill-costs", post(backfill_usage_costs))
        .route(
            "/api/pricing/models",
            get(list_model_pricing).post(upsert_model_pricing),
        )
        .route(
            "/api/pricing/models/*model_id",
            put(update_model_pricing).delete(delete_model_pricing),
        )
        .route("/api/provider-limits", get(provider_limits))
        .route(
            "/api/providers/:id/limits",
            get(provider_limits_for_provider),
        )
        .route(
            "/api/usage/router-sync/retry",
            post(retry_usage_router_sync),
        )
        .route("/api/shares", get(list_shares).post(upsert_share))
        .route("/api/shares/export", get(export_shares))
        .route("/api/shares/import", post(import_shares))
        .route("/api/shares/:id", delete(delete_share))
        .route("/api/shares/:id/connect-info", get(share_connect_info))
        .route("/api/shares/:id/subdomain", post(update_share_subdomain))
        .route("/api/shares/:id/pause", post(pause_share))
        .route("/api/shares/:id/resume", post(resume_share))
        .route("/api/shares/:id/tunnel/start", post(start_share_tunnel))
        .route("/api/shares/:id/tunnel/stop", post(stop_share_tunnel))
        .route("/api/shares/tunnels/restore", post(restore_share_tunnels))
        .route("/api/shares/:id/reset-usage", post(reset_share_usage))
        .route("/api/shares/:id/binding", post(update_share_binding))
        .route("/api/shares/:id/acl", post(replace_share_acl))
        .route(
            "/api/shares/:id/market-grant",
            post(update_share_market_grant),
        )
        .route("/api/share-markets", get(list_share_markets))
        .route(
            "/api/shares/:id/authorize-market",
            post(authorize_share_market),
        )
        .route(
            "/api/shares/runtime-snapshot",
            post(refresh_share_snapshots),
        )
        .route(
            "/api/router/config",
            get(router_config).post(update_router_config),
        )
        .route(
            "/api/router/client-tunnel",
            get(client_tunnel_status).post(update_client_tunnel),
        )
        .route("/api/router/client-tunnel/claim", post(claim_client_tunnel))
        .route(
            "/api/router/client-tunnel/lease",
            post(issue_client_tunnel_lease),
        )
        .route("/api/router/client-tunnel/stop", post(stop_client_tunnel))
        .route("/api/router/tunnels", get(router_tunnels))
        .route("/api/router/heartbeat", post(router_heartbeat))
        .route("/api/router/status", get(router_status))
        .route("/api/router/diagnostics", get(router_diagnostics))
        .route("/api/router/register", post(router_register))
        .route("/api/router/batch-sync", post(router_batch_sync))
        .route(
            "/api/router/share-edits/pull",
            post(router_pull_share_edits),
        )
        .route(
            "/api/router/shares/delete-all",
            post(router_delete_all_shares),
        )
        .route("/api/proxy/capabilities", get(proxy_capabilities))
        .route(
            "/web-api/auth/email/request-code",
            post(request_email_login_code),
        )
        .route(
            "/web-api/auth/email/verify-code",
            post(web_verify_email_login_code),
        )
        .route("/web-api/auth/methods", get(web_auth_methods))
        .route("/web-api/auth/password/login", post(web_password_login))
        .route("/web-api/auth/password/setup", post(web_password_setup))
        .route("/web-api/auth/password/refresh", post(web_password_refresh))
        .route("/web-api/auth/password/logout", post(web_password_logout))
        .route("/web-api/auth/password/change", post(web_password_change))
        .route("/web-api/auth/session/refresh", post(web_session_refresh))
        .route("/web-api/context", get(web_runtime_context))
        .route("/web-api/invoke/*command", post(web_invoke_compat))
        .route("/v1/models", get(proxy_models))
        .route("/models", get(proxy_models))
        .route("/v1/messages", post(proxy_claude_messages))
        .route("/claude/v1/messages", post(proxy_claude_messages))
        .route("/v1/chat/completions", post(proxy_codex_chat_completions))
        .route(
            "/v1/v1/chat/completions",
            post(proxy_codex_chat_completions),
        )
        .route("/chat/completions", post(proxy_codex_chat_completions))
        .route(
            "/codex/v1/chat/completions",
            post(proxy_codex_chat_completions),
        )
        .route("/v1/responses", post(proxy_codex_responses))
        .route("/v1/responses/compact", post(proxy_codex_responses))
        .route("/v1/v1/responses", post(proxy_codex_responses))
        .route("/v1/v1/responses/compact", post(proxy_codex_responses))
        .route("/responses", post(proxy_codex_responses))
        .route("/responses/compact", post(proxy_codex_responses))
        .route("/codex/v1/responses", post(proxy_codex_responses))
        .route("/codex/v1/responses/compact", post(proxy_codex_responses))
        .route("/backend-api/codex/responses", post(proxy_codex_responses))
        .route(
            "/backend-api/codex/responses/compact",
            post(proxy_codex_responses),
        )
        .route("/v1beta/*path", any(proxy_gemini))
        .route("/gemini/v1/*path", any(proxy_gemini))
        .route("/gemini/v1beta/*path", any(proxy_gemini))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    match state.web_dist_dir.as_ref() {
        Some(web_dist_dir) if web_dist_dir.is_dir() => {
            app = app.fallback_service(ServeDir::new(web_dist_dir));
        }
        Some(web_dist_dir) => {
            tracing::warn!(
                web_dist_dir = %web_dist_dir.display(),
                "configured web dist directory is missing; using embedded web assets"
            );
            app = app.fallback(embedded_web_asset);
        }
        None if web_assets::asset_count() > 0 => {
            app = app.fallback(embedded_web_asset);
        }
        None => {
            app = app.fallback(web_dist_missing);
        }
    }
    app
}

async fn embedded_web_asset(method: Method, uri: Uri) -> Response {
    if !matches!(method, Method::GET | Method::HEAD) {
        return web_dist_missing_response();
    }
    let Some(asset) = web_assets::asset_for_uri_path(uri.path()) else {
        return web_dist_missing_response();
    };

    let cache_control = if asset.path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(Bytes::from_static(asset.bytes))
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(body)
        .unwrap_or_else(|_| web_dist_missing_response())
}

fn web_dist_missing_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            ok: false,
            error: "web dist asset not found".to_string(),
            code: None,
            error_type: None,
            status: Some(StatusCode::NOT_FOUND.as_u16()),
            retryable: None,
        }),
    )
        .into_response()
}

async fn health(State(state): State<ServerState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        config_dir: state.config_dir.display().to_string(),
        web_dist_dir: state
            .web_dist_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        embedded_web_assets: web_assets::asset_count(),
        unix_ms: now_ms(),
    })
}

async fn version() -> Json<VersionResponse> {
    Json(build_info())
}

async fn share_router_health(
    headers: HeaderMap,
) -> Result<Json<ShareRouterHealthResponse>, ApiError> {
    require_share_router_probe(&headers)?;
    Ok(Json(ShareRouterHealthResponse {
        ok: true,
        status: "healthy".to_string(),
        timestamp_ms: now_ms(),
    }))
}

async fn share_router_request_logs(
    State(state): State<ServerState>,
    Query(query): Query<ShareRouterRequestLogsQuery>,
) -> Result<Json<ShareRouterRequestLogsResponse>, ApiError> {
    let limit = query.limit.unwrap_or(SHARE_ROUTER_REQUEST_LOGS_LIMIT);
    let usage = state.usage.read().await.clone();
    let mut logs = Vec::new();
    for log in usage.logs.iter().rev().filter(|log| {
        log.share_id.is_some()
            && query
                .share_id
                .as_deref()
                .is_none_or(|share_id| log.share_id.as_deref() == Some(share_id))
    }) {
        if logs.len() >= limit {
            break;
        }
        if let Some(entry) = crate::state::share_request_log_entry(&state, log).await {
            logs.push(entry);
        }
    }
    Ok(Json(ShareRouterRequestLogsResponse {
        share_id: query.share_id,
        logs,
    }))
}

async fn share_router_runtime(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ShareRouterRuntimeQuery>,
) -> Result<Json<ShareRouterRuntimeResponse>, ApiError> {
    require_share_router_probe(&headers)?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = resolve_share_for_internal_request(&state, query.share_id.as_deref()).await?;
    let descriptor = crate::core::router_client::descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    Ok(Json(runtime_response_from_descriptor(descriptor)))
}

async fn share_router_model_health(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ShareRouterModelHealthRequest>,
) -> Result<Json<ShareRouterModelHealthResponse>, ApiError> {
    require_share_router_health_check(&headers)?;
    let app = parse_app_kind(&input.app_type)?;
    let providers = state.providers.read().await.clone();
    let usage = state.usage.read().await.clone();
    let provider = providers
        .providers
        .iter()
        .find(|provider| provider.app == app)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no provider selected"))?;
    let health = crate::core::health::provider_health(&provider, &usage);
    let latest = usage
        .logs
        .iter()
        .rev()
        .find(|log| log.app == app && log.provider_id == provider.provider.id);
    Ok(Json(ShareRouterModelHealthResponse {
        ok: true,
        success: health.healthy,
        status: if health.healthy { "healthy" } else { "failed" }.to_string(),
        message: health
            .reason
            .clone()
            .unwrap_or_else(|| "derived from server usage health".to_string()),
        status_code: latest.map(|log| log.status_code),
        model_used: latest
            .and_then(|log| {
                log.actual_model
                    .clone()
                    .or_else(|| log.requested_model.clone())
            })
            .or_else(|| {
                provider
                    .provider
                    .settings_config
                    .get("model")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        response_time_ms: latest.map(|log| clamp_u128_to_u64(log.duration_ms)),
        tested_at: (latest.map(|log| log.created_at_ms).unwrap_or_else(now_ms) / 1000) as i64,
        retry_count: 0,
        error_category: None,
        provider_id: provider.provider.id,
        provider_name: provider.provider.name,
    }))
}

async fn control_apply_share_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlApplyShareSettingsResponse>, ApiError> {
    verify_control_request(&state, APPLY_SHARE_SETTINGS_PATH, &headers, &body).await?;
    let input: ControlApplyShareSettingsInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let share = {
        let mut shares = state.shares.write().await;
        shares
            .apply_settings_patch(&input.share_id, input.patch)
            .map_err(|error| match error {
                crate::core::shares::SharePatchError::NotFound => {
                    ApiError::not_found("share not found")
                }
                crate::core::shares::SharePatchError::Invalid(message) => {
                    ApiError::bad_request(message)
                }
            })?
    };
    state.save_shares().await.map_err(ApiError::internal)?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|item| item.id == share.id)
        .cloned()
        .unwrap_or(share);
    let descriptor = crate::core::router_client::descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    Ok(Json(ControlApplyShareSettingsResponse {
        ok: true,
        share: descriptor,
    }))
}

async fn control_refresh_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlRefreshShareUsageResponse>, ApiError> {
    verify_control_request(&state, REFRESH_SHARE_USAGE_PATH, &headers, &body).await?;
    let input: ControlRefreshShareUsageInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let providers = state.providers.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|item| item.id == input.share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let refreshed =
        refresh_share_usage_items(&state, &share, input.app.as_deref(), &providers).await;
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    {
        let mut shares = state.shares.write().await;
        shares.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
    }
    state.save_shares().await.map_err(ApiError::internal)?;
    Ok(Json(ControlRefreshShareUsageResponse {
        ok: true,
        refreshed,
    }))
}

async fn provider_coverage(State(state): State<ServerState>) -> Json<ProviderCoverage> {
    Json(state.provider_coverage.clone())
}

async fn provider_matrix() -> Json<crate::core::provider_matrix::ProviderMatrix> {
    Json(crate::core::provider_matrix::provider_matrix())
}

async fn provider_type(Json(input): Json<ProviderTypeRequest>) -> Json<ProviderTypeResponse> {
    Json(classify_provider_response(input.app, &input.provider))
}

async fn setup_status(State(state): State<ServerState>) -> Json<SetupStatusResponse> {
    let config = state.config.read().await;
    Json(SetupStatusResponse::from_config(&config))
}

async fn setup(
    State(state): State<ServerState>,
    Json(input): Json<SetupInput>,
) -> Result<Json<SetupResponse>, ApiError> {
    if state.config.read().await.is_setup_complete() {
        return Err(ApiError::conflict("server setup is already complete"));
    }

    let config = ServerConfig::from_setup(input).map_err(ApiError::bad_request)?;
    let response = SetupResponse::from_config(&config);
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    crate::state::start_client_tunnel(state.clone()).await;

    Ok(Json(response))
}

async fn login(
    State(state): State<ServerState>,
    Json(input): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let config = state.config.read().await;
    if !config.is_setup_complete() {
        return Err(ApiError::forbidden("setup is required before login"));
    }
    match input.method.as_str() {
        "password" => {
            if !config.verify_password(&input.password) {
                return Err(ApiError::unauthorized("invalid password"));
            }
        }
        "api_token" => {
            let api_token = input
                .api_token
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("api token is required"))?;
            if !config.verify_api_token(api_token) {
                return Err(ApiError::unauthorized("invalid api token"));
            }
        }
        "email" => {
            let email = input
                .email
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("email is required"))?;
            let code = input
                .code
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("email verification code is required"))?;
            drop(config);
            return complete_email_login(&state, email, code).await;
        }
        _ => return Err(ApiError::bad_request("unsupported auth method")),
    }
    drop(config);

    Ok(Json(issue_login_response(&state).await))
}

async fn request_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginCodeRequest>,
) -> Result<Json<crate::core::email_auth::EmailCodeRequestResponse>, ApiError> {
    let config = ensure_email_router_config(&state).await?;
    let email = require_configured_owner_email(&config, &input.email)?;
    let http_client = state.http_client().await;
    let response = crate::core::email_auth::request_code(&http_client, &config, &email)
        .await
        .map_err(map_email_auth_error)?;
    Ok(Json(response))
}

async fn verify_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginVerifyCodeRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    complete_email_login(&state, &input.email, &input.code).await
}

async fn web_verify_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginVerifyCodeRequest>,
) -> Result<Json<crate::core::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    complete_client_web_email_login(&state, &input.email, &input.code).await
}

async fn complete_client_web_email_login(
    state: &ServerState,
    email: &str,
    code: &str,
) -> Result<Json<crate::core::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    let config = ensure_email_router_config(state).await?;
    let email = require_configured_owner_email(&config, email)?;
    let http_client = state.http_client().await;
    let router_session =
        crate::core::email_auth::verify_client_web_code(&http_client, &config, &email, code)
            .await
            .map_err(map_email_auth_error)?;
    let verified_email = crate::core::email_auth::normalize_email(&router_session.user.email)
        .map_err(map_email_auth_error)?;
    if verified_email != email {
        return Err(ApiError::unauthorized(
            "verified email does not match configured owner email",
        ));
    }
    let owner_binding = crate::core::email_auth::bind_owner_email(
        &http_client,
        &config,
        &email,
        &router_session.access_token,
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            crate::core::email_auth::humanize_remote_owner_binding_error(&error.message),
        )
    })?;
    let bound_email = crate::core::email_auth::normalize_email(&owner_binding.owner_email)
        .map_err(map_email_auth_error)?;
    if !owner_binding.ok || bound_email != email {
        return Err(ApiError::bad_gateway(
            "router accepted email code but did not bind the configured owner email",
        ));
    }
    let email_state = crate::core::email_auth::state_from_router_session(&config, &router_session)
        .map_err(map_email_auth_error)?;
    crate::core::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;

    Ok(Json(router_session))
}

async fn web_session_refresh(
    State(state): State<ServerState>,
    Json(input): Json<WebSessionRefreshRequest>,
) -> Result<Json<crate::core::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    let config = ensure_email_router_config(&state).await?;
    let http_client = state.http_client().await;
    let response =
        crate::core::email_auth::refresh_session(&http_client, &config, &input.refresh_token)
            .await
            .map_err(map_email_auth_error)?;
    Ok(Json(response))
}

async fn web_auth_methods(
    State(state): State<ServerState>,
) -> Result<Json<crate::core::web_auth::AuthMethods>, ApiError> {
    let config = state.config.read().await;
    Ok(Json(crate::core::web_auth::auth_methods(&config)))
}

async fn web_password_login(
    State(state): State<ServerState>,
    Json(input): Json<WebPasswordRequest>,
) -> Result<Json<crate::core::web_auth::PasswordLoginResponse>, ApiError> {
    let config = state.config.read().await.clone();
    state
        .web_auth
        .login(&config, &input.password)
        .map(Json)
        .map_err(map_web_auth_error)
}

async fn web_password_setup(
    State(state): State<ServerState>,
    Json(input): Json<WebPasswordRequest>,
) -> Result<Json<crate::core::web_auth::PasswordLoginResponse>, ApiError> {
    let mut config = state.config.read().await.clone();
    let response = state
        .web_auth
        .setup_password(&mut config, &input.password)
        .map_err(map_web_auth_error)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(response))
}

async fn web_password_refresh(
    State(state): State<ServerState>,
    Json(input): Json<WebSessionRefreshRequest>,
) -> Result<Json<crate::core::web_auth::PasswordLoginResponse>, ApiError> {
    state
        .web_auth
        .refresh(&input.refresh_token)
        .map(Json)
        .map_err(map_web_auth_error)
}

async fn web_password_logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = bearer_token(&headers)
        .ok_or_else(|| ApiError::unauthorized("authorization bearer token is required"))?;
    state.web_auth.logout(token).map_err(map_web_auth_error)?;
    Ok(Json(json!({ "ok": true })))
}

async fn web_password_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<WebPasswordChangeRequest>,
) -> Result<Json<Value>, ApiError> {
    require_web_admin_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    state
        .web_auth
        .change_password(&mut config, &input.current_password, &input.new_password)
        .map_err(map_web_auth_error)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    state.sessions.write().await.clear();
    Ok(Json(json!({ "ok": true })))
}

async fn complete_email_login(
    state: &ServerState,
    email: &str,
    code: &str,
) -> Result<Json<LoginResponse>, ApiError> {
    let config = ensure_email_router_config(state).await?;
    let email = require_configured_owner_email(&config, email)?;
    let http_client = state.http_client().await;
    let router_session =
        crate::core::email_auth::verify_client_web_code(&http_client, &config, &email, code)
            .await
            .map_err(map_email_auth_error)?;
    let verified_email = crate::core::email_auth::normalize_email(&router_session.user.email)
        .map_err(map_email_auth_error)?;
    if verified_email != email {
        return Err(ApiError::unauthorized(
            "verified email does not match configured owner email",
        ));
    }
    let owner_binding = crate::core::email_auth::bind_owner_email(
        &http_client,
        &config,
        &email,
        &router_session.access_token,
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            crate::core::email_auth::humanize_remote_owner_binding_error(&error.message),
        )
    })?;
    let bound_email = crate::core::email_auth::normalize_email(&owner_binding.owner_email)
        .map_err(map_email_auth_error)?;
    if !owner_binding.ok || bound_email != email {
        return Err(ApiError::bad_gateway(
            "router accepted email code but did not bind the configured owner email",
        ));
    }
    let email_state = crate::core::email_auth::state_from_router_session(&config, &router_session)
        .map_err(map_email_auth_error)?;
    crate::core::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;

    Ok(Json(issue_login_response(state).await))
}

async fn ensure_email_router_config(state: &ServerState) -> Result<ServerConfig, ApiError> {
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        return Err(ApiError::forbidden("setup is required before email login"));
    }
    let has_identity = config.router.identity.as_ref().is_some_and(|identity| {
        !identity.installation_id.trim().is_empty() && !identity.private_key.trim().is_empty()
    });
    if has_identity {
        return Ok(config);
    }

    let http_client = state.http_client().await;
    match crate::core::router_client::register_installation(&http_client, &mut config).await {
        Ok(_) => {
            state
                .replace_config(config.clone())
                .await
                .map_err(ApiError::internal)?;
            {
                let mut shares = state.shares.write().await;
                shares.router_registered = true;
                shares.last_router_error = None;
            }
            state.save_shares().await.map_err(ApiError::internal)?;
            Ok(config)
        }
        Err(error) => {
            {
                let mut shares = state.shares.write().await;
                shares.router_registered = false;
                shares.last_router_error = Some(error.to_string());
            }
            state.save_shares().await.map_err(ApiError::internal)?;
            Err(ApiError::bad_gateway(format!(
                "router installation register failed: {error}"
            )))
        }
    }
}

fn require_configured_owner_email(config: &ServerConfig, email: &str) -> Result<String, ApiError> {
    let email = crate::core::email_auth::normalize_email(email).map_err(map_email_auth_error)?;
    let owner_email = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("owner email is not configured"))?
        .trim()
        .to_ascii_lowercase();
    if owner_email != email {
        return Err(ApiError::unauthorized(
            "email does not match configured owner email",
        ));
    }
    Ok(email)
}

async fn issue_login_response(state: &ServerState) -> LoginResponse {
    let token = generate_session_token();
    state.sessions.write().await.push(Session {
        token: token.clone(),
    });
    LoginResponse {
        ok: true,
        token,
        token_type: "bearer",
    }
}

async fn change_password(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    config
        .set_password(&input.new_password)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    state.sessions.write().await.clear();
    state
        .web_auth
        .revoke_all_sessions()
        .map_err(ApiError::internal)?;
    Ok(Json(ChangePasswordResponse { ok: true }))
}

async fn rotate_api_token(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ApiTokenResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let api_token = generate_session_token();
    let mut config = state.config.read().await.clone();
    config
        .set_api_token(&api_token)
        .map_err(ApiError::internal)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ApiTokenResponse {
        ok: true,
        api_token,
    }))
}

async fn auth_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AuthMeResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(AuthMeResponse {
        ok: true,
        owner_email: config.owner.email.clone(),
    }))
}

async fn events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_event_session(&state, &headers, query.token.as_deref()).await?;
    let receiver = state.subscribe_events();
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let event_name = event.event_type.clone();
                    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                    return Some((Ok(Event::default().event(event_name).data(data)), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

async fn list_backups(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BackupListResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let backups =
        crate::core::backup::list_backups(&state.config_dir).map_err(ApiError::internal)?;
    Ok(Json(BackupListResponse { ok: true, backups }))
}

async fn create_backup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Option<Json<CreateBackupRequest>>,
) -> Result<Json<BackupCreateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let reason = body.and_then(|Json(input)| input.reason);
    let backup = crate::core::backup::create_backup(&state.config_dir, reason)
        .map_err(ApiError::internal)?;
    state.emit_event(
        ServerEvent::new("backup.created", "backup")
            .id(backup.id.clone())
            .message("manual"),
    );
    Ok(Json(BackupCreateResponse { ok: true, backup }))
}

async fn restore_backup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BackupRestoreResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let result = crate::core::backup::restore_backup(&state.config_dir, &id)
        .map_err(ApiError::bad_request)?;
    state
        .reload_persistent_stores()
        .await
        .map_err(ApiError::internal)?;
    state.emit_event(
        ServerEvent::new("backup.restored", "backup")
            .id(result.restored.id.clone())
            .message("restored"),
    );
    Ok(Json(BackupRestoreResponse { ok: true, result }))
}

async fn config_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ConfigSnapshotResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(ConfigSnapshotResponse::from_config(&config)))
}

async fn upstream_proxy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UpstreamProxyResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(UpstreamProxyResponse {
        ok: true,
        upstream_proxy: UpstreamProxyView::from_config(&config),
    }))
}

async fn update_upstream_proxy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateUpstreamProxyInput>,
) -> Result<Json<UpstreamProxyResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    config
        .update_upstream_proxy(input)
        .map_err(ApiError::bad_request)?;
    let upstream_proxy = UpstreamProxyView::from_config(&config);
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpstreamProxyResponse {
        ok: true,
        upstream_proxy,
    }))
}

async fn router_config(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterConfigResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(RouterConfigResponse {
        ok: true,
        router: RouterConfigView::from_config(&config.router),
    }))
}

async fn update_router_config(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateRouterConfigInput>,
) -> Result<Json<RouterConfigResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    config.update_router(input).map_err(ApiError::bad_request)?;
    let router = RouterConfigView::from_config(&config.router);
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(RouterConfigResponse { ok: true, router }))
}

async fn client_tunnel_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(ClientTunnelResponse {
        ok: true,
        tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        tunnel_status: config.client.tunnel_status.clone(),
        last_heartbeat_ms: config.client.last_heartbeat_ms,
        runtime_status: state
            .tunnels
            .status(&crate::core::tunnel::client_tunnel_key())
            .await,
    }))
}

async fn update_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateClientTunnelInput>,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    config
        .update_client_tunnel(input)
        .map_err(ApiError::bad_request)?;
    let response = ClientTunnelResponse {
        ok: true,
        tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        tunnel_status: config.client.tunnel_status.clone(),
        last_heartbeat_ms: config.client.last_heartbeat_ms,
        runtime_status: state
            .tunnels
            .status(&crate::core::tunnel::client_tunnel_key())
            .await,
    };
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(response))
}

async fn claim_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelClaimResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        let http_client = state.http_client().await;
        crate::core::router_client::register_installation(&http_client, &mut config)
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("router installation register failed: {error}"))
            })?;
        state
            .replace_config(config.clone())
            .await
            .map_err(ApiError::internal)?;
    }
    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::bad_request("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::bad_request("client tunnel subdomain is not configured"))?;
    let tunnel = crate::core::router_client::ClientTunnelConfig {
        owner_email,
        subdomain,
        enabled: true,
    };
    let http_client = state.http_client().await;
    match crate::core::router_client::claim_client_tunnel(&http_client, &config, tunnel).await {
        Ok(()) => {
            let mut next = config;
            next.client.tunnel_status = Some("claimed_remote".to_string());
            next.router.last_register_error = None;
            state
                .replace_config(next)
                .await
                .map_err(ApiError::internal)?;
            emit_tunnel_event(&state, "tunnel.changed", "client", "claimed_remote");
            Ok(Json(ClientTunnelClaimResponse {
                ok: true,
                status: "claimed_remote".to_string(),
                error: None,
            }))
        }
        Err(error) => {
            let mut next = config;
            next.client.tunnel_status = Some("claim_failed".to_string());
            next.router.last_register_error = Some(error.to_string());
            state
                .replace_config(next)
                .await
                .map_err(ApiError::internal)?;
            Err(ApiError::bad_gateway(format!(
                "router client tunnel claim failed: {error}"
            )))
        }
    }
}

async fn issue_client_tunnel_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelLeaseResponse>, ApiError> {
    require_session(&state, &headers).await?;
    crate::state::start_client_tunnel(state.clone()).await;
    emit_tunnel_event(&state, "tunnel.changed", "client", "started");
    Ok(Json(ClientTunnelLeaseResponse {
        ok: true,
        status: state
            .tunnels
            .status(&crate::core::tunnel::client_tunnel_key())
            .await,
        message: "client tunnel supervisor started".to_string(),
    }))
}

async fn stop_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    crate::state::stop_client_tunnel(&state).await;
    let mut config = state.config.read().await.clone();
    config.client.tunnel_status = Some("stopped".to_string());
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    emit_tunnel_event(&state, "tunnel.changed", "client", "stopped");
    client_tunnel_status(State(state), headers).await
}

async fn router_tunnels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterTunnelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(RouterTunnelsResponse {
        ok: true,
        tunnels: state.tunnels.statuses().await,
    }))
}

async fn list_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ListProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    Ok(Json(ListProvidersResponse {
        ok: true,
        providers,
    }))
}

async fn create_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateProviderRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.provider.name.trim().is_empty() {
        return Err(ApiError::bad_request("provider name is required"));
    }

    let stored = {
        let mut store = state.providers.write().await;
        store.upsert(input.app, input.provider)
    };
    state.save_providers().await.map_err(ApiError::internal)?;

    Ok(Json(CreateProviderResponse { ok: true, stored }))
}

async fn export_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ExportProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    Ok(Json(ExportProvidersResponse {
        ok: true,
        providers,
    }))
}

async fn import_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportProvidersRequest>,
) -> Result<Json<ImportProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    for item in &input.providers {
        if item.provider.name.trim().is_empty() {
            return Err(ApiError::bad_request("provider name is required"));
        }
    }
    let imported = {
        let mut store = state.providers.write().await;
        input
            .providers
            .into_iter()
            .map(|item| {
                store.upsert(item.app, item.provider);
                1usize
            })
            .sum()
    };
    state.save_providers().await.map_err(ApiError::internal)?;
    Ok(Json(ImportProvidersResponse { ok: true, imported }))
}

async fn list_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.universal_providers.read().await.providers.clone();
    Ok(Json(ListUniversalProvidersResponse {
        ok: true,
        providers,
    }))
}

async fn universal_provider_presets_route() -> Json<UniversalProviderPresetsResponse> {
    Json(UniversalProviderPresetsResponse {
        ok: true,
        presets: universal_provider_presets(),
    })
}

async fn export_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ExportUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state
        .universal_providers
        .read()
        .await
        .providers
        .values()
        .cloned()
        .collect();
    Ok(Json(ExportUniversalProvidersResponse {
        ok: true,
        providers,
    }))
}

async fn import_universal_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<ImportUniversalProvidersRequest>,
) -> Result<Json<ImportUniversalProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    for provider in &mut input.providers {
        if provider.id.trim().is_empty() {
            provider.id = format!("universal-{}", &generate_session_token()[..16]);
        }
        if provider.name.trim().is_empty() {
            return Err(ApiError::bad_request("universal provider name is required"));
        }
        if provider.base_url.trim().is_empty() {
            return Err(ApiError::bad_request(
                "universal provider baseUrl is required",
            ));
        }
    }
    let imported = {
        let mut store = state.universal_providers.write().await;
        input
            .providers
            .into_iter()
            .map(|provider| {
                store.upsert(provider);
                1usize
            })
            .sum()
    };
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ImportUniversalProvidersResponse {
        ok: true,
        imported,
    }))
}

async fn get_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<GetUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let provider = state
        .universal_providers
        .read()
        .await
        .providers
        .get(&id)
        .cloned();
    Ok(Json(GetUniversalProviderResponse { ok: true, provider }))
}

async fn upsert_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertUniversalProviderRequest>,
) -> Result<Json<UpsertUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.provider.id.trim().is_empty() {
        input.provider.id = format!("universal-{}", &generate_session_token()[..16]);
    }
    if input.provider.name.trim().is_empty() {
        return Err(ApiError::bad_request("universal provider name is required"));
    }
    if input.provider.base_url.trim().is_empty() {
        return Err(ApiError::bad_request(
            "universal provider baseUrl is required",
        ));
    }

    let provider = {
        let mut store = state.universal_providers.write().await;
        store.upsert(input.provider)
    };
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpsertUniversalProviderResponse { ok: true, provider }))
}

async fn delete_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = state.universal_providers.write().await.delete(&id);
    state
        .save_universal_providers()
        .await
        .map_err(ApiError::internal)?;
    if deleted {
        state
            .providers
            .write()
            .await
            .remove_universal_derivatives(&id);
        state.save_providers().await.map_err(ApiError::internal)?;
    }
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

async fn sync_universal_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<SyncUniversalProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let universal = state
        .universal_providers
        .read()
        .await
        .providers
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("universal provider not found"))?;

    let mut result = UniversalProviderSyncResult::default();
    {
        let mut providers = state.providers.write().await;
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            if let Some(provider) = provider_from_universal(&universal, app) {
                providers.upsert_merging_settings(app, provider);
                result.synced.push(app.as_str().to_string());
            } else {
                if providers.remove_universal_derivative(&universal.id, app) {
                    result.removed.push(app.as_str().to_string());
                }
                result.skipped.push(app.as_str().to_string());
            }
        }
    }
    state.save_providers().await.map_err(ApiError::internal)?;

    Ok(Json(SyncUniversalProviderResponse { ok: true, result }))
}

async fn provider_health(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListProvidersQuery>,
) -> Result<Json<ProviderHealthResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.list(query.app);
    let usage = state.usage.read().await;
    Ok(Json(ProviderHealthResponse {
        ok: true,
        providers: crate::core::health::provider_health_list(&providers, &usage),
    }))
}

async fn failover_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<FailoverResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let failover = state.failover.read().await;
    Ok(Json(FailoverResponse {
        ok: true,
        failover: failover.snapshot_for_providers(&providers),
    }))
}

async fn update_failover_app(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(app): Path<AppKind>,
    Json(input): Json<UpdateFailoverAppInput>,
) -> Result<Json<UpdateFailoverAppResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let config = {
        let mut failover = state.failover.write().await;
        failover.update_app_config(app, input, &providers)
    };
    state.save_failover().await.map_err(ApiError::internal)?;
    Ok(Json(UpdateFailoverAppResponse {
        ok: true,
        app,
        config,
    }))
}

async fn reset_failover_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<FailoverProviderResetQuery>,
) -> Result<Json<ResetFailoverProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let app = resolve_failover_provider_app(&state, &provider_id, query.app).await?;
    let breaker = {
        let mut failover = state.failover.write().await;
        failover.reset_provider(app, &provider_id)
    };
    state.save_failover().await.map_err(ApiError::internal)?;
    Ok(Json(ResetFailoverProviderResponse { ok: true, breaker }))
}

async fn resolve_failover_provider_app(
    state: &ServerState,
    provider_id: &str,
    requested_app: Option<AppKind>,
) -> Result<AppKind, ApiError> {
    let providers = state.providers.read().await;
    if let Some(app) = requested_app {
        if providers
            .providers
            .iter()
            .any(|provider| provider.app == app && provider.provider.id == provider_id)
        {
            return Ok(app);
        }
        return Err(ApiError::not_found("provider not found for app"));
    }

    let matches = providers
        .providers
        .iter()
        .filter(|provider| provider.provider.id == provider_id)
        .map(|provider| provider.app)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [app] => Ok(*app),
        [] => Err(ApiError::not_found("provider not found")),
        _ => Err(ApiError::bad_request(
            "provider id is used by multiple apps; specify app query",
        )),
    }
}

async fn test_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<TestProviderQuery>,
) -> Result<Json<TestProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let stored = resolve_provider_by_id(&state, &id, query.app).await?;
    Ok(Json(test_provider_inner(&state, stored, &query).await?))
}

async fn fetch_provider_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FetchProviderModelsRequest>,
) -> Result<Json<FetchProviderModelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let stored = resolve_provider_by_id(&state, &id, input.app).await?;
    let fetched = fetch_provider_models_inner(&state, &stored, input.timeout_ms).await?;
    let mut provider = None;
    let mut merged_count = 0usize;
    if input.merge.unwrap_or(false) {
        {
            let mut providers = state.providers.write().await;
            let item = providers
                .providers
                .iter_mut()
                .find(|item| item.app == stored.app && item.provider.id == stored.provider.id)
                .ok_or_else(|| ApiError::not_found("provider not found"))?;
            merged_count = merge_fetched_models_into_provider(item, &fetched.models);
            provider = Some(item.clone());
        }
        state.save_providers().await.map_err(ApiError::internal)?;
    }
    Ok(Json(FetchProviderModelsResponse {
        ok: true,
        provider_id: stored.provider.id,
        app: stored.app,
        provider_type: stored.provider_type,
        url: fetched.url,
        merged: input.merge.unwrap_or(false),
        merged_count,
        models: fetched.models,
        provider,
    }))
}

async fn test_providers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<TestProvidersRequest>,
) -> Result<Json<TestProvidersResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let query = TestProviderQuery {
        app: None,
        network: input.network,
        timeout_ms: input.timeout_ms,
        model: input.model,
        stream: input.stream,
    };
    let providers = state.providers.read().await.providers.clone();
    let selected = providers
        .into_iter()
        .filter(|item| input.app.is_none_or(|app| item.app == app))
        .filter(|item| {
            input
                .provider_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == &item.provider.id))
        })
        .collect::<Vec<_>>();
    let mut results = Vec::new();
    for stored in selected {
        results.push(test_provider_inner(&state, stored, &query).await?);
    }
    Ok(Json(TestProvidersResponse { ok: true, results }))
}

async fn test_provider_inner(
    state: &ServerState,
    stored: StoredProvider,
    query: &TestProviderQuery,
) -> Result<TestProviderResponse, ApiError> {
    let accounts = state.accounts.read().await.clone();
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let route = default_test_route(stored.app);
    let stream = query.stream.unwrap_or(false);
    let model = provider_test_model(stored.app, &stored, query.model.as_deref());
    let endpoint = adapter
        .resolve_endpoint(
            route,
            default_gemini_test_path(stored.app, &model, stream),
            &stored,
        )
        .map_err(ApiError::proxy)?;
    let target_headers = adapter
        .build_headers(stored.app, &stored, &accounts)
        .map_err(ApiError::proxy)?;
    let capability = adapter.capability(stored.app, stored.provider_type);
    let mut network_status_code = None;
    let mut network_latency_ms = None;
    let mut network_error = None;
    let mut network_stream_completed = None;
    if query.network.unwrap_or(false) {
        let started = std::time::Instant::now();
        let body = provider_test_body(stored.app, &stored, Some(&model), stream);
        let http_client = state.http_client().await;
        let mut request = http_client
            .post(&endpoint)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body);
        if stream {
            request = request.header(axum::http::header::ACCEPT, "text/event-stream");
        }
        for (name, value) in &target_headers {
            request = request.header(*name, value);
        }
        match request
            .timeout(provider_test_timeout(query.timeout_ms))
            .send()
            .await
        {
            Ok(response) => {
                network_status_code = Some(response.status().as_u16());
                network_latency_ms = Some(started.elapsed().as_millis());
                if !response.status().is_success() {
                    let body = response.text().await.unwrap_or_default();
                    network_error = Some(redact_provider_test_error(&body));
                } else if stream {
                    let body = response.text().await.unwrap_or_default();
                    let completed = provider_test_stream_completed(stored.app, &body);
                    network_stream_completed = Some(completed);
                    if !completed {
                        network_error = Some(
                            "stream probe did not observe a provider completion marker".to_string(),
                        );
                    }
                }
            }
            Err(error) => {
                network_error = Some(error.to_string());
            }
        }
    }

    Ok(TestProviderResponse {
        ok: true,
        provider_id: stored.provider.id,
        app: stored.app,
        provider_type: stored.provider_type,
        adapter: capability.adapter,
        support: capability.support,
        endpoint,
        model,
        stream,
        header_names: target_headers
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect(),
        network_checked: query.network.unwrap_or(false),
        network_status_code,
        network_latency_ms,
        network_stream_completed,
        network_error,
        message: if query.network.unwrap_or(false) {
            "configuration check passed; upstream network/model call executed".to_string()
        } else {
            "configuration check passed; upstream network/model call is not executed".to_string()
        },
    })
}

async fn resolve_provider_by_id(
    state: &ServerState,
    provider_id: &str,
    app: Option<AppKind>,
) -> Result<StoredProvider, ApiError> {
    let matches = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .filter(|item| item.provider.id == provider_id && app.is_none_or(|app| item.app == app))
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [stored] => Ok(stored.clone()),
        [] => Err(ApiError::not_found("provider not found")),
        _ => Err(ApiError::bad_request(
            "provider id is used by multiple apps; pass app in the request body",
        )),
    }
}

struct ProviderModelsFetchResult {
    url: String,
    models: Vec<FetchedProviderModel>,
}

async fn fetch_provider_models_inner(
    state: &ServerState,
    stored: &StoredProvider,
    timeout_ms: Option<u64>,
) -> Result<ProviderModelsFetchResult, ApiError> {
    let accounts = state.accounts.read().await.clone();
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let model = provider_test_model(stored.app, stored, None);
    let endpoint = adapter
        .resolve_endpoint(
            default_test_route(stored.app),
            default_gemini_test_path(stored.app, &model, false),
            stored,
        )
        .map_err(ApiError::proxy)?;
    let url = model_list_url_from_endpoint(&endpoint).ok_or_else(|| {
        ApiError::bad_request("provider endpoint cannot be mapped to a model list URL")
    })?;
    let target_headers = adapter
        .build_headers(stored.app, stored, &accounts)
        .map_err(ApiError::proxy)?;
    let http_client = state.http_client().await;
    let mut request = http_client.get(&url);
    for (name, value) in target_headers {
        request = request.header(name, value);
    }
    let response = request
        .timeout(provider_test_timeout(timeout_ms))
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("fetch provider models failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch provider models failed: {status}: {}",
            redact_provider_test_error(&body)
        )));
    }
    let raw = response
        .json::<Value>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse provider models failed: {error}")))?;
    Ok(ProviderModelsFetchResult {
        url,
        models: parse_provider_models(&raw),
    })
}

fn model_list_url_from_endpoint(endpoint: &str) -> Option<String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }
    if let Some(index) = endpoint.find("/models/") {
        return Some(format!("{}/models", &endpoint[..index]));
    }
    for suffix in [
        "/chat/completions",
        "/responses",
        "/messages",
        "/completions",
    ] {
        if let Some(index) = endpoint.rfind(suffix) {
            return Some(format!("{}/models", &endpoint[..index]));
        }
    }
    endpoint.ends_with("/models").then(|| endpoint.to_string())
}

fn parse_provider_models(raw: &Value) -> Vec<FetchedProviderModel> {
    let models = raw
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.get("models").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    models
        .into_iter()
        .filter_map(|model| {
            let upstream_model = model
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| model.get("name").and_then(Value::as_str))?
                .trim()
                .to_string();
            if upstream_model.is_empty() {
                return None;
            }
            let id = upstream_model
                .strip_prefix("models/")
                .unwrap_or(&upstream_model)
                .to_string();
            let display_name = model
                .get("displayName")
                .or_else(|| model.get("display_name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(FetchedProviderModel {
                id,
                upstream_model,
                display_name,
                raw: model,
            })
        })
        .collect()
}

fn merge_fetched_models_into_provider(
    stored: &mut StoredProvider,
    models: &[FetchedProviderModel],
) -> usize {
    if !stored.provider.settings_config.is_object() {
        stored.provider.settings_config = json!({});
    }
    let settings = stored
        .provider
        .settings_config
        .as_object_mut()
        .expect("settings_config object");
    let catalog = settings
        .entry("modelCatalog".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !catalog.is_object() {
        *catalog = Value::Object(Map::new());
    }
    let catalog = catalog.as_object_mut().expect("modelCatalog object");
    let mut merged = 0usize;
    for model in models {
        if catalog.contains_key(&model.id) {
            continue;
        }
        catalog.insert(
            model.id.clone(),
            json!({
                "upstreamModel": model.upstream_model.clone(),
                "displayName": model.display_name.clone(),
            }),
        );
        merged += 1;
    }
    merged
}

async fn create_provider_from_preset(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateProviderFromPresetRequest>,
) -> Result<Json<CreateProviderResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let fixtures = fixtures_for_app(&state.provider_coverage, input.app);
    let fixture = fixtures
        .into_iter()
        .find(|item| item.name == input.name)
        .ok_or_else(|| ApiError::not_found("provider preset not found"))?;
    let stored = {
        let mut store = state.providers.write().await;
        store.upsert(input.app, fixture.provider.clone())
    };
    state.save_providers().await.map_err(ApiError::internal)?;
    Ok(Json(CreateProviderResponse { ok: true, stored }))
}

async fn provider_presets(
    State(state): State<ServerState>,
    Query(query): Query<ProviderPresetsQuery>,
) -> Json<ProviderPresetsResponse> {
    let presets = match query.app {
        Some(AppKind::Claude) => state.provider_coverage.presets.claude.clone(),
        Some(AppKind::Codex) => state.provider_coverage.presets.codex.clone(),
        Some(AppKind::Gemini) => state.provider_coverage.presets.gemini.clone(),
        None => Vec::new(),
    };
    Json(ProviderPresetsResponse { ok: true, presets })
}

fn default_test_route(app: AppKind) -> ProxyRoute {
    match app {
        AppKind::Claude => ProxyRoute::ClaudeMessages,
        AppKind::Codex => ProxyRoute::CodexResponses,
        AppKind::Gemini => ProxyRoute::Gemini,
    }
}

fn default_gemini_test_path(app: AppKind, model: &str, stream: bool) -> Option<String> {
    (app == AppKind::Gemini).then(|| {
        let method = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        format!("{}:{method}", gemini_model_name(model))
    })
}

fn provider_test_model(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
) -> String {
    override_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/testModel")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/testConfig/model")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.test_config.as_ref())
                .and_then(|value| value.get("testModel").or_else(|| value.get("model")))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .pointer("/modelMapping/upstreamModel")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            stored
                .provider
                .settings_config
                .get("models")
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|value| {
                    value.as_str().or_else(|| {
                        value
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
                    })
                })
        })
        .unwrap_or(match app {
            AppKind::Claude => "claude-3-5-haiku-latest",
            AppKind::Codex => "gpt-4.1-mini",
            AppKind::Gemini => "gemini-2.5-flash",
        })
        .to_string()
}

fn provider_test_body(
    app: AppKind,
    stored: &StoredProvider,
    override_model: Option<&str>,
    stream: bool,
) -> String {
    let model = provider_test_model(app, stored, override_model);
    let value = match app {
        AppKind::Claude => serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
            "stream": stream
        }),
        AppKind::Codex => serde_json::json!({
            "model": model,
            "input": "ping",
            "max_output_tokens": 1,
            "stream": stream
        }),
        AppKind::Gemini => serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "ping"}]}],
            "generationConfig": {"maxOutputTokens": 1}
        }),
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

fn provider_test_stream_completed(app: AppKind, body: &str) -> bool {
    match app {
        AppKind::Claude => body.contains("message_stop") || body.contains("[DONE]"),
        AppKind::Codex => {
            body.contains("response.completed")
                || body.contains("\"status\":\"completed\"")
                || body.contains("[DONE]")
        }
        AppKind::Gemini => body.contains("finishReason") || body.contains("\"candidates\""),
    }
}

fn redact_provider_test_error(value: &str) -> String {
    let mut redacted = value.to_string();
    for marker in ["sk-", "ya29.", "Bearer "] {
        while let Some(index) = redacted.find(marker) {
            let end = redacted[index..]
                .find(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'')
                .map(|offset| index + offset)
                .unwrap_or_else(|| redacted.len());
            redacted.replace_range(index..end, "[REDACTED]");
        }
    }
    redacted.chars().take(800).collect()
}

async fn list_accounts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListAccountsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ListAccountsResponse {
        ok: true,
        accounts: state.accounts.read().await.accounts.clone(),
    }))
}

async fn upsert_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpsertAccountInput>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let account = {
        let mut store = state.accounts.write().await;
        let manager = manager_for(input.provider_type);
        manager
            .finish_login(&mut store, input)
            .map_err(ApiError::bad_request)?
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse { ok: true, account }))
}

async fn account_capabilities() -> Json<AccountCapabilitiesResponse> {
    Json(AccountCapabilitiesResponse {
        ok: true,
        capabilities: crate::core::account_managers::all_capabilities(),
    })
}

async fn account_import_templates(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AccountImportTemplatesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(AccountImportTemplatesResponse {
        ok: true,
        templates: crate::core::account_managers::account_import_templates(),
    }))
}

async fn start_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartAccountLoginRequest>,
) -> Result<Json<StartAccountLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let redirect_uri = input
        .redirect_uri
        .or_else(|| Some(default_account_login_redirect_uri(&state)));
    let login = {
        let mut store = state.oauth_logins.write().await;
        store
            .start(input.provider_type, redirect_uri, now_ms() as i64)
            .map_err(oauth_login_api_error)?
    };
    Ok(Json(StartAccountLoginResponse { ok: true, login }))
}

async fn account_login_callback(
    State(state): State<ServerState>,
    Query(query): Query<AccountLoginCallbackQuery>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    let AccountLoginCallbackQuery {
        session_id,
        state: oauth_state,
        code,
        error,
        error_description,
    } = query;
    if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
        let message = error_description
            .filter(|value| !value.trim().is_empty())
            .map(|description| format!("{error}: {description}"))
            .unwrap_or(error);
        return Err(ApiError::bad_request(message));
    }
    let finish = {
        let mut store = state.oauth_logins.write().await;
        store
            .finish(
                session_id.as_deref(),
                oauth_state.as_deref(),
                code.as_deref(),
                false,
                now_ms() as i64,
            )
            .map_err(oauth_login_api_error)?
    };
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account: None,
    }))
}

async fn finish_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<FinishAccountLoginRequest>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut finish = {
        let mut store = state.oauth_logins.write().await;
        store
            .finish(
                input.session_id.as_deref(),
                input.state.as_deref(),
                input.code.as_deref(),
                input.execute_token_exchange.unwrap_or(false),
                now_ms() as i64,
            )
            .map_err(oauth_login_api_error)?
    };
    let account = if input.execute_token_exchange.unwrap_or(false) {
        Some(execute_account_login_token_exchange(&state, &mut finish).await?)
    } else {
        None
    };
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account,
    }))
}

async fn start_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartCopilotDeviceLoginRequest>,
) -> Result<Json<StartCopilotDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let device = crate::core::copilot_device::start_device_flow(
        &http_client,
        input.github_domain.as_deref(),
    )
    .await
    .map_err(map_copilot_device_error)?;
    Ok(Json(StartCopilotDeviceLoginResponse { ok: true, device }))
}

async fn poll_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollCopilotDeviceLoginRequest>,
) -> Result<Json<PollCopilotDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let result = crate::core::copilot_device::poll_device_flow(
        &http_client,
        &input.device_code,
        input.github_domain.as_deref(),
        now_ms() as i64,
    )
    .await
    .map_err(map_copilot_device_error)?;
    if result.pending {
        return Ok(Json(PollCopilotDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("copilot device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(provider_type).finish_login(&mut store, account_input)
    };
    let account = account_result.map_err(ApiError::bad_request)?;
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(PollCopilotDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

async fn start_kiro_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartKiroDeviceLoginRequest>,
) -> Result<Json<StartKiroDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    let (device, flow) = crate::core::kiro_device::start_device_flow(
        &http_client,
        input.region.as_deref(),
        input.start_url.as_deref(),
        now,
    )
    .await
    .map_err(map_kiro_device_error)?;
    {
        let mut store = state.kiro_device_flows.write().await;
        store.insert(device.device_code.clone(), flow, now);
    }
    Ok(Json(StartKiroDeviceLoginResponse { ok: true, device }))
}

async fn poll_kiro_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollKiroDeviceLoginRequest>,
) -> Result<Json<PollKiroDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let flow = {
        let mut store = state.kiro_device_flows.write().await;
        store
            .get(&input.device_code, now)
            .ok_or_else(|| ApiError::unauthorized("kiro device flow is expired or unknown"))?
    };
    let http_client = state.http_client().await;
    let result = match crate::core::kiro_device::poll_device_flow(
        &http_client,
        &input.device_code,
        flow,
        now,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            if matches!(
                error.status,
                StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
            ) {
                state
                    .kiro_device_flows
                    .write()
                    .await
                    .remove(&input.device_code);
            }
            return Err(map_kiro_device_error(error));
        }
    };
    if result.pending {
        return Ok(Json(PollKiroDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    state
        .kiro_device_flows
        .write()
        .await
        .remove(&input.device_code);
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("kiro device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(provider_type).finish_login(&mut store, account_input)
    };
    let account = account_result.map_err(ApiError::bad_request)?;
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(PollKiroDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

async fn execute_account_login_token_exchange(
    state: &ServerState,
    finish: &mut OAuthLoginFinish,
) -> Result<AccountLoginAccountSummary, ApiError> {
    let request = finish
        .token_request
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("token exchange request is unavailable"))?;
    let http_client = state.http_client().await;
    let (token_response, raw) = match execute_oauth_token_request(
        &http_client,
        finish.provider_type,
        request,
        format!("{} OAuth token exchange", finish.provider_type.as_str()),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(account_refresh_api_error(error));
        }
    };
    let profile_raw = match execute_account_login_profile_request(
        state,
        finish.provider_type,
        finish.flow,
        &token_response.access_token,
    )
    .await
    {
        Ok(profile) => profile,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(account_refresh_api_error(error));
        }
    };
    let input = match upsert_input_from_login_response(
        finish.provider_type,
        &token_response,
        raw,
        profile_raw,
        now_ms() as i64,
    ) {
        Ok(input) => input,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error.message));
        }
    };

    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(input.provider_type).finish_login(&mut store, input)
    };
    let account = match account_result {
        Ok(account) => account,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error));
        }
    };
    if let Err(error) = state.save_accounts().await {
        mark_account_login_exchange_failed(state, &finish.session_id).await;
        return Err(ApiError::internal(error));
    }
    state
        .oauth_logins
        .write()
        .await
        .mark_exchanged(&finish.session_id)
        .map_err(oauth_login_api_error)?;

    finish.status = OAuthLoginStatus::TokenExchanged;
    finish.method = "token_exchange_completed";
    finish.token_request = None;
    finish.account_import_hint = None;
    finish.message = format!(
        "{} OAuth token exchange completed and account was imported",
        finish.provider_type.as_str()
    );

    Ok(AccountLoginAccountSummary::from_account(&account))
}

async fn execute_account_login_profile_request(
    state: &ServerState,
    provider_type: ProviderType,
    flow: OAuthAuthorizeFlow,
    access_token: &str,
) -> Result<Option<serde_json::Value>, AccountRefreshFailure> {
    if flow == OAuthAuthorizeFlow::CursorDeepControl {
        return Ok(None);
    }
    if !matches!(
        provider_type,
        ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        return Ok(None);
    }
    let Some(request) = build_profile_request(provider_type, access_token) else {
        return Ok(None);
    };
    let http_client = state.http_client().await;
    execute_oauth_json_request(
        &http_client,
        provider_type,
        &request,
        format!("{} OAuth profile fetch", provider_type.as_str()),
    )
    .await
    .map(Some)
}

async fn mark_account_login_exchange_failed(state: &ServerState, session_id: &str) {
    state
        .oauth_logins
        .write()
        .await
        .mark_exchange_failed(session_id);
}

async fn delete_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = {
        let mut store = state.accounts.write().await;
        let provider_type = store
            .accounts
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.provider_type);
        match provider_type {
            Some(provider_type) => manager_for(provider_type)
                .revoke_or_delete(&mut store, &id)
                .map_err(ApiError::bad_request)?,
            None => false,
        }
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

async fn refresh_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let existing = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;

    if provider_native_refresh_available(existing.provider_type) {
        let now = now_ms() as i64;
        let _refresh_guard = state
            .account_refresh_locks
            .try_lock(existing.provider_type, &existing.id)
            .ok_or_else(|| ApiError::conflict("account refresh is already in progress"))?;
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(&http_client, &existing, now).await {
            Ok(update) => update,
            Err(error) => {
                {
                    let mut store = state.accounts.write().await;
                    store.mark_refresh_failure(&id, error.message.clone());
                }
                state.save_accounts().await.map_err(ApiError::internal)?;
                return Err(account_refresh_api_error(error));
            }
        };
        let account = {
            let mut store = state.accounts.write().await;
            store
                .mark_refresh_success(&id, update)
                .ok_or_else(|| ApiError::not_found("account not found"))?
        };
        state.save_accounts().await.map_err(ApiError::internal)?;
        return Ok(Json(UpsertAccountResponse { ok: true, account }));
    }

    let account = {
        let mut store = state.accounts.write().await;
        manager_for(existing.provider_type)
            .refresh_token(&mut store, &id, now_ms() as i64)
            .map_err(ApiError::bad_request)?
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse { ok: true, account }))
}

fn account_refresh_api_error(error: AccountRefreshFailure) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

async fn account_refresh_plan(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AccountRefreshPlanResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let account = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    let spec = oauth_provider_spec(account.provider_type);
    let refresh_request = build_refresh_request(account.provider_type, &account)
        .ok()
        .map(redact_oauth_request);
    let profile_request = account
        .access_token
        .as_deref()
        .and_then(|token| build_profile_request(account.provider_type, token))
        .map(redact_oauth_request);
    let refresh_required = token_expires_soon(&account, now_ms() as i64);
    let message = if spec.is_some_and(|item| item.server_native_refresh_enabled())
        && refresh_request.is_some()
    {
        "native refresh/profile execution is available after importing refresh credentials"
            .to_string()
    } else if refresh_request.is_some() {
        "refresh request shape is available; native refresh execution remains disabled".to_string()
    } else if spec.is_some_and(|item| item.token_urls.is_empty()) {
        "provider has no OAuth refresh endpoint; manual import/API key mode only".to_string()
    } else {
        "refresh request shape is unavailable; account likely lacks a refresh token or provider credentials".to_string()
    };

    Ok(Json(AccountRefreshPlanResponse {
        ok: true,
        account_id: account.id,
        provider_type: account.provider_type,
        refresh_required,
        server_native_stage: spec.map(|item| item.stage),
        quota_strategy: spec.map(|item| item.quota_strategy),
        refresh_request,
        profile_request,
        message,
    }))
}

async fn account_quota(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<AccountQuotaQuery>,
) -> Result<Json<AccountQuotaResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let existing = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    if !query.refresh.unwrap_or(false) {
        let store = state.accounts.read().await;
        let quota = manager_for(existing.provider_type)
            .query_quota(&store, &id)
            .map_err(ApiError::bad_request)?;
        let next_refresh_at = existing.quota_next_refresh_at;
        return Ok(Json(AccountQuotaResponse {
            ok: true,
            quota,
            account: Some(existing),
            refreshed: false,
            message: Some(
                "quota snapshot returned; use refresh=true to query upstream".to_string(),
            ),
            next_refresh_at,
        }));
    }

    let now = now_ms() as i64;
    let force = query.force.unwrap_or(false);
    if !force {
        if let Some(next_refresh_at) = existing.quota_next_refresh_at {
            if next_refresh_at > now {
                return Ok(Json(AccountQuotaResponse {
                    ok: true,
                    quota: existing.quota.clone(),
                    account: Some(existing),
                    refreshed: false,
                    message: Some(format!("quota refresh skipped until {next_refresh_at}")),
                    next_refresh_at: Some(next_refresh_at),
                }));
            }
        }
    }

    let mut active_account = existing;
    let mut account_mutated = false;
    if account_needs_native_refresh(&active_account, now) {
        let _refresh_guard = state
            .account_refresh_locks
            .try_lock(active_account.provider_type, &active_account.id)
            .ok_or_else(|| ApiError::conflict("account refresh is already in progress"))?;
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(&http_client, &active_account, now).await
        {
            Ok(update) => update,
            Err(error) => {
                {
                    let mut store = state.accounts.write().await;
                    store.mark_refresh_failure(&id, error.message.clone());
                }
                state.save_accounts().await.map_err(ApiError::internal)?;
                return Err(account_refresh_api_error(error));
            }
        };
        active_account = {
            let mut store = state.accounts.write().await;
            store
                .mark_refresh_success(&id, update)
                .ok_or_else(|| ApiError::not_found("account not found"))?
        };
        account_mutated = true;
    }

    let http_client = state.http_client().await;
    match refresh_account_quota(&http_client, &active_account, now, force).await {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let account = {
                let mut store = state.accounts.write().await;
                store
                    .mark_refresh_success(&id, update)
                    .ok_or_else(|| ApiError::not_found("account not found"))?
            };
            state.save_accounts().await.map_err(ApiError::internal)?;
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: account.quota.clone(),
                account: Some(account.clone()),
                refreshed: true,
                message: Some(message),
                next_refresh_at: account.quota_next_refresh_at,
            }))
        }
        Ok(QuotaRefreshResult::SkippedCooldown {
            next_refresh_at,
            message,
        }) => {
            if account_mutated {
                state.save_accounts().await.map_err(ApiError::internal)?;
            }
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: active_account.quota.clone(),
                account: Some(active_account),
                refreshed: false,
                message: Some(message),
                next_refresh_at: Some(next_refresh_at),
            }))
        }
        Err(error) => {
            {
                let mut store = state.accounts.write().await;
                store.mark_refresh_success(
                    &id,
                    AccountRefreshUpdate {
                        quota_next_refresh_at: error.next_refresh_at,
                        last_refresh_error: Some(error.message.clone()),
                        ..Default::default()
                    },
                );
            }
            state.save_accounts().await.map_err(ApiError::internal)?;
            Err(ApiError::new(
                StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
                error.message,
            ))
        }
    }
}

fn redact_oauth_request(mut request: OAuthHttpRequest) -> OAuthHttpRequest {
    for (name, value) in &mut request.headers {
        if name.eq_ignore_ascii_case("authorization") {
            *value = "[REDACTED]".to_string();
        }
    }
    request.url = redact_oauth_url(&request.url);
    redact_oauth_json(&mut request.body);
    request
}

fn redact_oauth_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let redacted_query = query
        .split('&')
        .map(|part| {
            let Some((key, _value)) = part.split_once('=') else {
                return part.to_string();
            };
            if is_oauth_secret_key(key) {
                format!("{key}=[REDACTED]")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{redacted_query}")
}

fn redact_oauth_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if is_oauth_secret_key(key) {
                    *item = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_oauth_json(item);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_oauth_json(item);
            }
        }
        _ => {}
    }
}

fn is_oauth_secret_key(key: &str) -> bool {
    let key_lower = key.to_ascii_lowercase();
    key_lower.contains("token")
        || key_lower.contains("secret")
        || key_lower.contains("api_key")
        || key_lower == "password"
        || key_lower == "code"
        || key_lower == "code_verifier"
        || key_lower == "verifier"
}

async fn usage_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageLogsQuery>,
) -> Result<Json<UsageLogsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(UsageLogsResponse {
        ok: true,
        logs: state.usage.read().await.latest_filtered(query.into()),
    }))
}

async fn usage_summary(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageSummaryResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageSummaryResponse {
        ok: true,
        summary: state.usage.read().await.rollup_filtered(&filter),
    }))
}

async fn usage_trends(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageTrendsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageTrendsResponse {
        ok: true,
        trends: state.usage.read().await.trends(&filter),
    }))
}

async fn usage_provider_stats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageProviderStatsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageProviderStatsResponse {
        ok: true,
        providers: state.usage.read().await.provider_stats(&filter),
    }))
}

async fn usage_model_stats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageModelStatsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageModelStatsResponse {
        ok: true,
        models: state.usage.read().await.model_stats(&filter),
    }))
}

async fn usage_log_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UsageLogDetailResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let log = state
        .usage
        .read()
        .await
        .request_detail(&id)
        .ok_or_else(|| ApiError::not_found("usage request not found"))?;
    Ok(Json(UsageLogDetailResponse { ok: true, log }))
}

async fn backfill_usage_costs(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UsageBackfillResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let pricing = state.pricing.read().await.clone();
    let updated = state
        .usage
        .write()
        .await
        .backfill_costs(&providers, &pricing);
    state.save_usage().await.map_err(ApiError::internal)?;
    Ok(Json(UsageBackfillResponse { ok: true, updated }))
}

async fn list_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ModelPricingListResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ModelPricingListResponse {
        ok: true,
        models: state.pricing.read().await.list(),
    }))
}

async fn upsert_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateModelPricingInput>,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let model_id = input
        .model_id
        .clone()
        .ok_or_else(|| ApiError::bad_request("modelId is required"))?;
    update_model_pricing_inner(state, model_id, input).await
}

async fn update_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(input): Json<UpdateModelPricingInput>,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    update_model_pricing_inner(state, model_id, input).await
}

async fn update_model_pricing_inner(
    state: ServerState,
    model_id: String,
    input: UpdateModelPricingInput,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    let entry = {
        let mut pricing = state.pricing.write().await;
        pricing
            .upsert(model_id, input)
            .map_err(ApiError::bad_request)?
    };
    state.save_pricing().await.map_err(ApiError::internal)?;

    let providers = state.providers.read().await.clone();
    let pricing = state.pricing.read().await.clone();
    let updated =
        state
            .usage
            .write()
            .await
            .backfill_costs_for_model(&providers, &pricing, &entry.model_id);
    if updated > 0 {
        state.save_usage().await.map_err(ApiError::internal)?;
    }

    Ok(Json(ModelPricingUpdateResponse {
        ok: true,
        model: entry,
        backfilled: updated,
    }))
}

async fn delete_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> Result<Json<ModelPricingDeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = {
        let mut pricing = state.pricing.write().await;
        pricing.delete(&model_id)
    };
    state.save_pricing().await.map_err(ApiError::internal)?;
    Ok(Json(ModelPricingDeleteResponse { ok: true, deleted }))
}

async fn provider_limits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ProviderLimitsQuery>,
) -> Result<Json<ProviderLimitsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let shares = state.shares.read().await.clone();
    let usage = state.usage.read().await.clone();
    let limits = providers
        .providers
        .iter()
        .filter(|provider| query.app.is_none_or(|app| provider.app == app))
        .filter(|provider| {
            query
                .provider_id
                .as_deref()
                .is_none_or(|id| provider.provider.id == id)
        })
        .map(|provider| provider_limit_status(provider, &accounts, &shares, &usage))
        .collect::<Vec<_>>();
    Ok(Json(ProviderLimitsResponse { ok: true, limits }))
}

async fn provider_limits_for_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<ProviderLimitsQuery>,
) -> Result<Json<ProviderLimitResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let provider = providers
        .providers
        .iter()
        .find(|provider| {
            provider.provider.id == provider_id && query.app.is_none_or(|app| provider.app == app)
        })
        .cloned()
        .ok_or_else(|| ApiError::not_found("provider not found"))?;
    let accounts = state.accounts.read().await.clone();
    let shares = state.shares.read().await.clone();
    let usage = state.usage.read().await.clone();
    Ok(Json(ProviderLimitResponse {
        ok: true,
        limit: provider_limit_status(&provider, &accounts, &shares, &usage),
    }))
}

fn provider_limit_status(
    provider: &StoredProvider,
    accounts: &AccountStore,
    shares: &ShareStore,
    usage: &UsageStore,
) -> ProviderLimitStatusView {
    let daily_limit_usd = provider_number_setting(provider, &["limitDailyUsd", "dailyLimitUsd"]);
    let monthly_limit_usd =
        provider_number_setting(provider, &["limitMonthlyUsd", "monthlyLimitUsd"]);
    let quota_dispatch_limit_percent = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.quota_dispatch_limit_percent)
        .map(|value| value as f64)
        .or_else(|| {
            provider_number_setting(
                provider,
                &["quotaDispatchLimitPercent", "quota_dispatch_limit_percent"],
            )
        });
    let daily_usage_usd = provider_usage_cost_since(
        usage,
        provider,
        current_utc_day_start_ms().unwrap_or(0) as u128,
    );
    let monthly_usage_usd = provider_usage_cost_since(
        usage,
        provider,
        current_utc_month_start_ms().unwrap_or(0) as u128,
    );
    let daily_exceeded = daily_limit_usd
        .map(|limit| daily_usage_usd >= limit)
        .unwrap_or(false);
    let monthly_exceeded = monthly_limit_usd
        .map(|limit| monthly_usage_usd >= limit)
        .unwrap_or(false);
    let account = accounts
        .find_for_provider(
            provider.provider_type,
            provider
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref()),
        )
        .cloned();
    let account_quota_percent = account
        .as_ref()
        .and_then(|account| account.quota_percent)
        .or_else(|| account.as_ref().and_then(account_tier_quota_percent));
    let quota_dispatch_exceeded = quota_dispatch_limit_percent
        .zip(account_quota_percent)
        .map(|(limit, quota)| quota >= limit)
        .unwrap_or(false);
    let share_limits = shares
        .shares
        .iter()
        .filter(|share| share_uses_provider(share, provider))
        .map(share_limit_status)
        .collect::<Vec<_>>();
    let share_blocked = share_limits.iter().any(|share| share.blocked);

    let mut warnings = Vec::new();
    if daily_exceeded {
        warnings.push("daily_cost_limit_exceeded".to_string());
    }
    if monthly_exceeded {
        warnings.push("monthly_cost_limit_exceeded".to_string());
    }
    if quota_dispatch_exceeded {
        warnings.push("quota_dispatch_limit_exceeded".to_string());
    }
    if account
        .as_ref()
        .and_then(|account| account.last_refresh_error.clone())
        .is_some()
    {
        warnings.push("account_quota_refresh_error".to_string());
    }
    if share_blocked {
        warnings.push("share_limit_blocks_usage".to_string());
    }
    if account
        .as_ref()
        .and_then(|account| account.quota.as_ref())
        .is_some_and(|quota| {
            quota
                .tiers
                .iter()
                .filter_map(|tier| tier.utilization)
                .any(|value| normalize_quota_utilization_percent(value) >= 95.0)
        })
    {
        warnings.push("account_quota_near_limit".to_string());
    }

    ProviderLimitStatusView {
        app: provider.app,
        provider_id: provider.provider.id.clone(),
        provider_name: provider.provider.name.clone(),
        provider_type: provider.provider_type,
        daily_usage_usd,
        daily_limit_usd,
        daily_exceeded,
        monthly_usage_usd,
        monthly_limit_usd,
        monthly_exceeded,
        account_id: account.as_ref().map(|account| account.id.clone()),
        account_email: account.as_ref().and_then(|account| account.email.clone()),
        account_quota_percent,
        account_quota_refreshed_at: account
            .as_ref()
            .and_then(|account| account.quota_refreshed_at),
        account_last_refresh_error: account
            .as_ref()
            .and_then(|account| account.last_refresh_error.clone()),
        quota_dispatch_limit_percent,
        quota_dispatch_exceeded,
        shares: share_limits,
        warnings,
        blocked: daily_exceeded || monthly_exceeded || quota_dispatch_exceeded || share_blocked,
    }
}

fn provider_usage_cost_since(usage: &UsageStore, provider: &StoredProvider, start_ms: u128) -> f64 {
    usage.provider_cost_since(provider.app, &provider.provider.id, start_ms)
}

fn provider_number_setting(provider: &StoredProvider, keys: &[&str]) -> Option<f64> {
    provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| map_number_value(&meta.extra, keys))
        .or_else(|| map_number_value(&provider.provider.extra, keys))
        .or_else(|| value_number_setting(&provider.provider.settings_config, keys))
        .or_else(|| {
            provider
                .provider
                .settings_config
                .get("limits")
                .and_then(|value| value_number_setting(value, keys))
        })
        .or_else(|| {
            provider
                .provider
                .settings_config
                .get("usageLimits")
                .and_then(|value| value_number_setting(value, keys))
        })
}

fn map_number_value(map: &BTreeMap<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| map.get(*key).and_then(value_as_f64))
}

fn value_number_setting(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_as_f64))
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    })
}

fn account_tier_quota_percent(account: &Account) -> Option<f64> {
    account.quota.as_ref().and_then(|quota| {
        quota
            .tiers
            .iter()
            .filter_map(|tier| tier.utilization)
            .map(normalize_quota_utilization_percent)
            .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
    })
}

fn normalize_quota_utilization_percent(value: f64) -> f64 {
    if value <= 1.0 {
        value * 100.0
    } else {
        value
    }
}

fn share_uses_provider(share: &Share, provider: &StoredProvider) -> bool {
    (share.app == provider.app && share.provider_id == provider.provider.id)
        || share.bindings.iter().any(|binding| {
            binding.app == provider.app && binding.provider_id == provider.provider.id
        })
}

fn share_limit_status(share: &Share) -> ShareLimitStatusView {
    let now = now_ms() as i64;
    let token_exceeded = share
        .token_limit
        .map(|limit| share.tokens_used >= limit)
        .unwrap_or(false);
    let expired = share
        .expires_at
        .map(|expires_at| expires_at <= now)
        .unwrap_or(false);
    let inactive = !share.enabled || share.status != "active";
    let blocked = inactive || token_exceeded || expired;
    let mut warnings = Vec::new();
    if inactive {
        warnings.push("share_inactive".to_string());
    }
    if token_exceeded {
        warnings.push("share_token_limit_exceeded".to_string());
    } else if let Some(limit) = share.token_limit {
        if limit > 0 && (share.tokens_used as f64 / limit as f64) >= 0.9 {
            warnings.push("share_token_limit_near".to_string());
        }
    }
    if expired {
        warnings.push("share_expired".to_string());
    }
    ShareLimitStatusView {
        share_id: share.id.clone(),
        share_name: share
            .display_name
            .clone()
            .unwrap_or_else(|| share.id.clone()),
        status: share.status.clone(),
        enabled: share.enabled,
        token_limit: share.token_limit,
        tokens_used: share.tokens_used,
        parallel_limit: share.parallel_limit,
        expires_at: share.expires_at,
        token_exceeded,
        expired,
        blocked,
        warnings,
    }
}

fn current_utc_day_start_ms() -> Option<i64> {
    chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc().timestamp_millis())
}

fn current_utc_month_start_ms() -> Option<i64> {
    let now = chrono::Utc::now();
    chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|value| value.and_utc().timestamp_millis())
}

async fn retry_usage_router_sync(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UsageRouterSyncRetryResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let summary = crate::state::sync_pending_direct_share_logs(state, 200, true).await;
    Ok(Json(UsageRouterSyncRetryResponse {
        ok: true,
        attempted: summary.attempted,
        synced: summary.synced,
        failed: summary.failed,
    }))
}

async fn list_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ListSharesResponse {
        ok: true,
        shares: state.shares.read().await.shares.clone(),
    }))
}

async fn export_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    list_shares(State(state), headers).await
}

async fn import_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportSharesRequest>,
) -> Result<Json<ImportSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let imported = state.shares.write().await.import_shares(input.shares);
    state.save_shares().await.map_err(ApiError::internal)?;
    state.emit_event(
        ServerEvent::new("share.imported", "share").message(format!("imported {imported} shares")),
    );
    Ok(Json(ImportSharesResponse { ok: true, imported }))
}

async fn upsert_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertShareInput>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.owner_email.is_none() {
        input.owner_email = state.config.read().await.owner.email.clone();
    }
    let share = {
        let mut store = state.shares.write().await;
        store.upsert(input)
    };
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "upserted");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn share_connect_info(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ShareConnectInfoResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    Ok(Json(connect_info_for_share(&config, &share)?))
}

async fn update_share_subdomain(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareSubdomainRequest>,
) -> Result<Json<UpdateShareSubdomainResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let subdomain = crate::core::shares::normalize_share_subdomain(&input.subdomain)
        .map_err(ApiError::bad_request)?;
    let config = state.config.read().await.clone();
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let current = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let mut candidate = current.clone();
    candidate.tunnel_subdomain = Some(subdomain.clone());
    let descriptor = crate::core::router_client::descriptor_for_share_with_accounts_and_usage(
        &candidate,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let mut remote_claimed = false;
    if config.router.identity.is_some() {
        let http_client = state.http_client().await;
        crate::core::router_client::claim_share_subdomain(&http_client, &config, descriptor)
            .await
            .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
        remote_claimed = true;
    }
    let share = state
        .shares
        .write()
        .await
        .update_subdomain(&id, subdomain)
        .map_err(map_share_patch_error)?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "subdomain_updated");
    Ok(Json(UpdateShareSubdomainResponse {
        ok: true,
        remote_claimed,
        share,
    }))
}

async fn delete_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = state.shares.write().await.delete(&id);
    state.save_shares().await.map_err(ApiError::internal)?;
    if deleted {
        spawn_share_delete_sync(state.clone(), id.clone());
        state.emit_event(
            ServerEvent::new("share.deleted", "share")
                .id(id.clone())
                .message("deleted"),
        );
    }
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

async fn pause_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .pause(&id)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "paused");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn resume_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .resume(&id)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "resumed");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn start_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .set_share_tunnel_status(&id, "active", None)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    crate::state::start_share_tunnel(state.clone(), id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_started");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_started");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn stop_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .set_share_tunnel_status(&id, "stopped", None)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    crate::state::stop_share_tunnel(&state, &id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_stopped");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_stopped");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn restore_share_tunnels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let shares = state.shares.write().await.restore_auto_start();
    state.save_shares().await.map_err(ApiError::internal)?;
    for share in shares
        .iter()
        .filter(|share| share.auto_start && share.enabled)
    {
        crate::state::start_share_tunnel(state.clone(), share.id.clone()).await;
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(&state, "share.changed", share, "tunnel_restored");
        emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_restored");
    }
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

async fn reset_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .reset_usage(&id)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "usage_reset");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn update_share_binding(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareBindingRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .update_binding(&id, input.binding)
        .map_err(|error| match error {
            ShareUpdateError::NotFound => ApiError::not_found("share not found"),
            ShareUpdateError::MustBePaused => ApiError::conflict(error.to_string()),
        })?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "binding_updated");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn replace_share_acl(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ReplaceShareAclRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .shares
        .write()
        .await
        .replace_acl(&id, input.acl)
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "acl_replaced");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn update_share_market_grant(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareMarketGrantRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let market_grant = input
        .market_grant
        .map(normalize_share_market_grant)
        .transpose()?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = {
        let mut store = state.shares.write().await;
        store
            .update_market_grant(&id, market_grant)
            .ok_or_else(|| ApiError::not_found("share not found"))?;
        store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
        store
            .shares
            .iter()
            .find(|share| share.id == id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("share not found"))?
    };
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "market_grant_updated");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn list_share_markets(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListShareMarketsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let markets = fetch_public_markets_from_router(&state).await?;
    Ok(Json(ListShareMarketsResponse { ok: true, markets }))
}

async fn authorize_share_market(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AuthorizeShareMarketRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let market_email = crate::core::email_auth::normalize_email(&input.market_email)
        .map_err(map_email_auth_error)?;
    let markets = fetch_public_markets_from_router(&state).await?;
    let public_market_emails = markets
        .iter()
        .map(|market| market.email.trim().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let selected_market = markets.iter().find(|market| {
        market.email.eq_ignore_ascii_case(&market_email) && market.market_kind == "share"
    });
    if selected_market.is_none() {
        return Err(ApiError::bad_request(
            "marketEmail must belong to a registered share market",
        ));
    }
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = {
        let mut store = state.shares.write().await;
        let share = store
            .authorize_share_market(&id, market_email, &public_market_emails)
            .map_err(map_share_patch_error)?;
        store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
        store.get(&id).cloned().unwrap_or(share)
    };
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "share_market_authorized");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

async fn refresh_share_snapshots(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares =
        state
            .shares
            .write()
            .await
            .refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
    save_shares_debounced(&state);
    state.emit_event(ServerEvent::new("share.changed", "share").message("runtime_snapshot"));
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

fn emit_share_event(state: &ServerState, event_type: &str, share: &Share, message: &str) {
    state.emit_event(
        ServerEvent::new(event_type, "share")
            .id(share.id.clone())
            .app(share.app)
            .message(message),
    );
}

fn emit_tunnel_event(state: &ServerState, event_type: &str, tunnel_id: &str, message: &str) {
    state.emit_event(
        ServerEvent::new(event_type, "tunnel")
            .id(tunnel_id.to_string())
            .message(message),
    );
}

fn connect_info_for_share(
    config: &ServerConfig,
    share: &Share,
) -> Result<ShareConnectInfoResponse, ApiError> {
    let subdomain = share
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::conflict("share subdomain is not configured"))?;
    let router_domain = config
        .router
        .domain
        .clone()
        .or_else(|| router_domain_from_url(config.router.url.as_deref()))
        .ok_or_else(|| ApiError::conflict("router domain is not configured"))?;
    let direct_url = share
        .router_url
        .clone()
        .unwrap_or_else(|| format!("https://{subdomain}.{router_domain}"));
    let snippets = [
        (
            AppKind::Claude,
            "Claude / Anthropic",
            vec![
                ("ANTHROPIC_BASE_URL", direct_url.clone()),
                ("ANTHROPIC_AUTH_TOKEN", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Codex,
            "Codex / OpenAI-compatible",
            vec![
                (
                    "OPENAI_BASE_URL",
                    format!("{}/v1", direct_url.trim_end_matches('/')),
                ),
                ("OPENAI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Gemini,
            "Gemini",
            vec![
                ("GEMINI_BASE_URL", direct_url.clone()),
                ("GEMINI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
    ]
    .into_iter()
    .map(|(app, title, values)| {
        let env = values
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>();
        ShareConnectSnippet {
            app,
            title: title.to_string(),
            env,
        }
    })
    .collect::<Vec<_>>();
    Ok(ShareConnectInfoResponse {
        ok: true,
        share_id: share.id.clone(),
        direct_url,
        subdomain,
        router_domain,
        snippets,
        note: "The caller must use their own cc-switch user_api_token as the bearer/API key."
            .to_string(),
    })
}

fn router_domain_from_url(url: Option<&str>) -> Option<String> {
    let value = url?.trim();
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    without_scheme
        .split('/')
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

async fn fetch_public_markets_from_router(
    state: &ServerState,
) -> Result<Vec<PublicShareMarket>, ApiError> {
    let config = state.config.read().await.clone();
    let api_base = config
        .router_api_base()
        .ok_or_else(|| ApiError::conflict("router API base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let http_client = state.http_client().await;
    let response = http_client
        .get(format!("{api_base}/v1/markets"))
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("fetch share markets failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch share markets failed: {status}: {body}"
        )));
    }
    let response = response
        .json::<ListShareMarketsResponse>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse share markets failed: {error}")))?;
    Ok(response.markets)
}

async fn router_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterStatusResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let shares = state.shares.read().await;
    Ok(Json(RouterStatusResponse {
        ok: true,
        registered: shares.router_registered,
        last_error: shares.last_router_error.clone(),
        last_heartbeat_ms: shares.last_router_heartbeat_ms,
        pending_request_log_sync: crate::state::pending_router_log_count(&state).await,
    }))
}

async fn router_diagnostics(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterDiagnosticsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let shares = state.shares.read().await;
    let share_sync = shares
        .shares
        .iter()
        .map(|share| ShareSyncDiagnostic {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            status: share.status.clone(),
            enabled: share.enabled,
            router_last_synced_at_ms: share.router_last_synced_at_ms,
            router_last_sync_error: share.router_last_sync_error.clone(),
            router_url: share.router_url.clone(),
        })
        .collect();
    Ok(Json(RouterDiagnosticsResponse {
        ok: true,
        router: RouterConfigView::from_config(&config.router),
        registered: shares.router_registered,
        last_error: shares.last_router_error.clone(),
        last_heartbeat_ms: shares.last_router_heartbeat_ms,
        pending_request_log_sync: crate::state::pending_router_log_count(&state).await,
        tunnels: state.tunnels.statuses().await,
        share_sync,
    }))
}

async fn router_heartbeat(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterStatusResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let now = now_ms();
    {
        let mut shares = state.shares.write().await;
        shares.last_router_heartbeat_ms = Some(now);
        shares.router_registered = true;
        shares.last_router_error = None;
    }
    {
        let mut config = state.config.read().await.clone();
        config.client.last_heartbeat_ms = Some(now);
        state
            .replace_config(config)
            .await
            .map_err(ApiError::internal)?;
    }
    save_shares_debounced(&state);
    router_status(State(state), headers).await
}

async fn router_register(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterRegisterResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        return Err(ApiError::bad_request("setup is incomplete"));
    }

    let http_client = state.http_client().await;
    match crate::core::router_client::register_installation(&http_client, &mut config).await {
        Ok(registration) => {
            state
                .replace_config(config)
                .await
                .map_err(ApiError::internal)?;
            {
                let mut shares = state.shares.write().await;
                shares.router_registered = true;
                shares.last_router_error = None;
            }
            state.save_shares().await.map_err(ApiError::internal)?;
            Ok(Json(RouterRegisterResponse {
                ok: true,
                registration,
            }))
        }
        Err(error) => {
            {
                let mut shares = state.shares.write().await;
                shares.router_registered = false;
                shares.last_router_error = Some(error.to_string());
            }
            let mut failed_config = config;
            failed_config.router.last_register_error = Some(error.to_string());
            state
                .replace_config(failed_config)
                .await
                .map_err(ApiError::internal)?;
            state.save_shares().await.map_err(ApiError::internal)?;
            Err(ApiError::bad_gateway(format!(
                "router installation register failed: {error}"
            )))
        }
    }
}

async fn router_batch_sync(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterBatchSyncResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares =
        state
            .shares
            .write()
            .await
            .refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
    save_shares_debounced(&state);
    let config = state.config.read().await.clone();
    let mut remote_synced = false;
    let mut remote_error = None;
    if config.router.identity.is_some() {
        let ops = shares
            .iter()
            .map(|share| {
                let descriptor =
                    crate::core::router_client::descriptor_for_share_with_accounts_and_usage(
                        share,
                        &providers,
                        Some(&accounts),
                        Some(&usage),
                    );
                crate::core::router_client::ShareSyncOperation {
                    kind: "upsert".to_string(),
                    share_id: None,
                    share: Some(descriptor),
                }
            })
            .collect();
        let http_client = state.http_client().await;
        if let Err(error) =
            crate::core::router_client::batch_sync_shares(&http_client, &config, ops).await
        {
            remote_error = Some(error.to_string());
        }
        remote_synced = remote_error.is_none();
    } else {
        remote_error = Some("router installation is not registered".to_string());
    }
    if let Some(error) = remote_error.clone() {
        let mut store = state.shares.write().await;
        store.last_router_error = Some(error);
        drop(store);
        state.save_shares().await.map_err(ApiError::internal)?;
    } else {
        let mut store = state.shares.write().await;
        store.router_registered = true;
        store.last_router_error = None;
        drop(store);
        state.save_shares().await.map_err(ApiError::internal)?;
    }
    let message = if remote_synced {
        "local runtime snapshots refreshed and remote router shares synced".to_string()
    } else {
        format!(
            "local runtime snapshots refreshed; remote router sync skipped/failed: {}",
            remote_error.unwrap_or_else(|| "unknown error".to_string())
        )
    };
    Ok(Json(RouterBatchSyncResponse {
        ok: true,
        synced: shares.len(),
        remote_synced,
        message,
        shares,
    }))
}

async fn router_pull_share_edits(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterShareEditPullResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let summary = crate::state::pull_and_apply_pending_share_edits(state).await;
    Ok(Json(RouterShareEditPullResponse {
        ok: summary.error.is_none(),
        summary,
    }))
}

async fn router_delete_all_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterDeleteAllSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return Err(ApiError::bad_request(
            "router installation is not registered",
        ));
    }
    let http_client = state.http_client().await;
    crate::core::router_client::delete_all_shares(&http_client, &config)
        .await
        .map_err(|error| {
            ApiError::bad_gateway(format!("router delete_all shares failed: {error}"))
        })?;
    {
        let mut shares = state.shares.write().await;
        shares.router_registered = true;
        shares.last_router_error = None;
    }
    state.save_shares().await.map_err(ApiError::internal)?;
    Ok(Json(RouterDeleteAllSharesResponse {
        ok: true,
        message: "remote router shares for this installation were deleted".to_string(),
    }))
}

fn spawn_share_upsert_sync(state: ServerState, share: Share) {
    tokio::spawn(async move {
        let providers = state.providers.read().await.clone();
        let accounts = state.accounts.read().await.clone();
        let usage = state.usage.read().await.clone();
        let descriptor = crate::core::router_client::descriptor_for_share_with_accounts_and_usage(
            &share,
            &providers,
            Some(&accounts),
            Some(&usage),
        );
        let op = crate::core::router_client::ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(descriptor),
        };
        sync_share_ops(state, vec![op]).await;
    });
}

fn spawn_share_delete_sync(state: ServerState, share_id: String) {
    tokio::spawn(async move {
        let op = crate::core::router_client::ShareSyncOperation {
            kind: "delete".to_string(),
            share_id: Some(share_id),
            share: None,
        };
        sync_share_ops(state, vec![op]).await;
    });
}

async fn sync_share_ops(
    state: ServerState,
    ops: Vec<crate::core::router_client::ShareSyncOperation>,
) {
    let synced_share_ids = ops
        .iter()
        .filter_map(|op| {
            op.share
                .as_ref()
                .map(|share| share.share_id.clone())
                .or_else(|| op.share_id.clone())
        })
        .collect::<Vec<_>>();
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return;
    }
    let router_base = config.router_api_base().map(str::to_string);
    let http_client = state.http_client().await;
    match crate::core::router_client::batch_sync_shares(&http_client, &config, ops).await {
        Ok(()) => {
            let mut store = state.shares.write().await;
            store.router_registered = true;
            store.last_router_error = None;
            let now = now_ms();
            for share_id in &synced_share_ids {
                store.mark_router_sync(share_id, router_base.clone(), Ok(now));
            }
            drop(store);
            save_shares_debounced(&state);
        }
        Err(error) => {
            tracing::warn!(error = %error, "router share sync failed");
            let mut store = state.shares.write().await;
            let message = error.to_string();
            store.last_router_error = Some(message.clone());
            for share_id in &synced_share_ids {
                store.mark_router_sync(share_id, router_base.clone(), Err(message.clone()));
            }
            drop(store);
            save_shares_debounced(&state);
        }
    }
}

async fn proxy_capabilities() -> Json<ProxyCapabilitiesResponse> {
    Json(ProxyCapabilitiesResponse {
        ok: true,
        capabilities: proxy::capabilities(),
    })
}

async fn proxy_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ModelsQuery>,
) -> Json<OpenAiModelsResponse> {
    let provider_id = query
        .provider_id
        .as_deref()
        .or_else(|| {
            headers
                .get("x-cc-provider-id")
                .and_then(|value| value.to_str().ok())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let providers = state.providers.read().await;
    Json(OpenAiModelsResponse {
        object: "list",
        data: openai_model_list(&providers.providers, query.app, provider_id),
    })
}

async fn web_runtime_context(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let config = state.config.read().await.clone();
    let contract = web_runtime::contract();
    if !config.is_setup_complete() {
        return Ok(Json(json!({
            "mode": "client-login",
            "appMode": "server",
            "platform": "server",
            "status": "setup-required",
            "permissions": ["setup"],
            "apps": ["claude", "codex", "gemini"],
            "auth": {
                "authenticated": false,
                "setupRequired": true,
                "ownerEmail": config.owner.email,
                "methods": ["passwordSetup"]
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
        })));
    }

    if resolve_web_admin_principal(&state, &headers)
        .await?
        .is_none()
    {
        return Ok(Json(web_runtime_auth_required_payload(&config, &contract)));
    }

    Ok(Json(json!({
        "mode": "local-admin",
        "appMode": "server",
        "platform": "server",
        "status": "authenticated",
        "permissions": ["admin", "providers", "shares", "usage", "settings", "accounts"],
        "apps": ["claude", "codex", "gemini"],
        "auth": {
            "authenticated": true,
            "setupRequired": false,
            "ownerEmail": config.owner.email,
            "methods": web_runtime_auth_methods(&config)
        },
        "router": {
            "url": config.router.url,
            "domain": config.router.domain,
            "clientSubdomain": config.client.tunnel_subdomain,
            "clientTunnelStatus": config.client.tunnel_status
        },
        "runtime": {
            "configDir": state.config_dir.display().to_string(),
            "webDistDir": state.web_dist_dir.as_ref().map(|path| path.display().to_string()),
            "embeddedWebAssets": web_assets::asset_count()
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
    })))
}

async fn web_invoke_compat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(command): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let args = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?
    };
    let command_def = web_runtime::command(&command)
        .ok_or_else(|| ApiError::web_invoke_unknown(command.clone()))?;
    if command_def.support == WebRuntimeCommandSupport::Excluded {
        return Err(ApiError::feature_disabled(format!(
            "desktop invoke command '{command}' is excluded from cc-switch-server ({})",
            command_def.feature
        )));
    }

    require_session(&state, &headers).await?;
    if !command_def.implemented {
        return Err(ApiError::web_invoke_not_wired(format!(
            "desktop invoke command '{command}' is registered as {} but is not bridged yet",
            web_runtime_support_label(command_def.support)
        )));
    }

    web_invoke_dispatch(&state, &headers, &command, args)
        .await
        .map(Json)
}

fn web_provider_health_json(
    app: AppKind,
    provider_id: &str,
    breaker: Option<&crate::core::failover::ProviderBreaker>,
) -> Value {
    let breaker = breaker
        .cloned()
        .unwrap_or_else(|| crate::core::failover::ProviderBreaker::new(app, provider_id));
    let is_healthy = breaker.state == crate::core::failover::BreakerState::Closed;
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

fn web_circuit_breaker_stats_json(
    breaker: Option<&crate::core::failover::ProviderBreaker>,
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
        crate::core::failover::BreakerState::Closed => "closed",
        crate::core::failover::BreakerState::Open => "open",
        crate::core::failover::BreakerState::HalfOpen => "half_open",
    };
    json!({
        "state": state,
        "consecutiveFailures": breaker.consecutive_failures,
        "consecutiveSuccesses": 0,
        "totalRequests": breaker.consecutive_failures,
        "failedRequests": breaker.consecutive_failures,
    })
}

async fn web_update_proxy_config_for_app(
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
    {
        let mut failover = state.failover.write().await;
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
    }
    state.save_failover().await.map_err(ApiError::internal)?;
    Ok(json!(true))
}

async fn web_invoke_dispatch(
    state: &ServerState,
    headers: &HeaderMap,
    command: &str,
    args: Value,
) -> Result<Value, ApiError> {
    match command {
        "get_build_info" => Ok(json!(build_info())),
        "get_settings" => {
            let settings = state.ui_settings.read().await.for_frontend();
            Ok(settings)
        }
        "get_rectifier_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::rectifier_config_for_frontend(&store))
        }
        "get_optimizer_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::optimizer_config_for_frontend(&store))
        }
        "set_rectifier_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "rectifierConfig": config }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "set_optimizer_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "optimizerConfig": config }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "get_log_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::log_config_for_frontend(&store))
        }
        "set_log_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "logConfig": config }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "get_stream_check_config" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::stream_check_config_for_frontend(&store))
        }
        "save_stream_check_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "streamCheckConfig": config }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "save_settings" => {
            let patch =
                ui_settings::settings_patch_from_args(&args).map_err(ApiError::bad_request)?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(patch);
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "get_global_proxy_config" => Ok(json!(web_global_proxy_config_json(state))),
        "get_global_proxy_url" => {
            let config = state.config.read().await;
            Ok(json!(config.upstream_proxy.url))
        }
        "get_upstream_proxy_status" => Ok(json!(web_upstream_proxy_status_json(state).await)),
        "set_global_proxy_url" => {
            let url = web_optional_string_any(&args, &["url", "proxyUrl", "proxy_url"])
                .unwrap_or_default();
            let mut config = state.config.read().await.clone();
            let input = if url.trim().is_empty() {
                UpdateUpstreamProxyInput {
                    url: None,
                    clear: Some(true),
                    follow_system_proxy: None,
                }
            } else {
                UpdateUpstreamProxyInput {
                    url: Some(url),
                    clear: None,
                    follow_system_proxy: None,
                }
            };
            config
                .update_upstream_proxy(input)
                .map_err(ApiError::bad_request)?;
            state
                .replace_config(config)
                .await
                .map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "test_proxy_url" => {
            let url = web_arg_string_any(&args, &["url", "proxyUrl"])?;
            Ok(json!(web_test_proxy_url(&url).await))
        }
        "scan_local_proxies" => Ok(json!(web_scan_local_proxies().await)),
        "is_portable_mode" => Ok(json!(false)),
        "get_app_config_dir_override" => Ok(json!(null)),
        "get_app_config_path" => Ok(json!(state.config_dir.display().to_string())),
        "get_config_dir" => {
            let _app = web_arg_app(&args).or_else(|_| web_arg_app_type(&args))?;
            Ok(json!(""))
        }
        "get_providers" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!({})),
            };
            let providers = state.providers.read().await;
            Ok(json!(provider_record_for_app(&providers.providers, app)))
        }
        "get_current_provider" => {
            let app = match web_arg_app_for_read(&args)? {
                Some(app) => app,
                None => return Ok(json!("")),
            };
            let providers = state.providers.read().await;
            let ui_settings = state.ui_settings.read().await.for_frontend();
            let current = live_config_import::read_current_provider_id(&ui_settings, app)
                .filter(|id| {
                    providers
                        .providers
                        .iter()
                        .any(|provider| provider.app == app && provider.provider.id == *id)
                })
                .or_else(|| {
                    providers
                        .list(Some(app))
                        .into_iter()
                        .next()
                        .map(|provider| provider.provider.id)
                })
                .unwrap_or_default();
            Ok(json!(current))
        }
        "import_default_config" => {
            let app = web_arg_app(&args).or_else(|_| web_arg_app_type(&args))?;
            let ui_settings = state.ui_settings.read().await.for_frontend();
            let imported = {
                let mut providers = state.providers.write().await;
                live_config_import::import_default_config(&mut providers, app, &ui_settings)
                    .map_err(|error| ApiError::bad_request(error.to_string()))?
            };
            if imported {
                {
                    let mut ui_store = state.ui_settings.write().await;
                    ui_store.apply_patch(json!({
                        live_config_import::current_provider_settings_key(app): "default"
                    }));
                }
                state.save_providers().await.map_err(ApiError::internal)?;
                state.save_ui_settings().await.map_err(ApiError::internal)?;
            }
            Ok(json!(imported))
        }
        "add_provider" | "update_provider" => {
            let app = web_arg_app(&args)?;
            let provider: Provider = web_arg_value(&args, "provider")?;
            if provider.name.trim().is_empty() {
                return Err(ApiError::bad_request("provider name is required"));
            }
            state.providers.write().await.upsert(app, provider);
            state.save_providers().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "update_providers_sort_order" => {
            let app = web_arg_app(&args)?;
            let updates: Vec<ProviderSortUpdate> = web_arg_value(&args, "updates")?;
            let changed = state
                .providers
                .write()
                .await
                .update_sort_order(app, updates);
            if changed {
                state.save_providers().await.map_err(ApiError::internal)?;
            }
            Ok(json!(true))
        }
        "delete_provider" => {
            let app = web_arg_app(&args)?;
            let id = web_arg_string(&args, "id")?;
            let deleted = {
                let mut providers = state.providers.write().await;
                let before = providers.providers.len();
                providers
                    .providers
                    .retain(|provider| !(provider.app == app && provider.provider.id == id));
                providers.providers.len() != before
            };
            if deleted {
                state.save_providers().await.map_err(ApiError::internal)?;
            }
            Ok(json!(deleted))
        }
        "switch_provider" => {
            let app = web_arg_app(&args)?;
            let id = web_arg_string(&args, "id")?;
            let exists = state
                .providers
                .read()
                .await
                .providers
                .iter()
                .any(|provider| provider.app == app && provider.provider.id == id);
            if !exists {
                return Err(ApiError::not_found("provider not found"));
            }
            {
                let mut ui_store = state.ui_settings.write().await;
                ui_store.apply_patch(json!({
                    live_config_import::current_provider_settings_key(app): id
                }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!({ "warnings": [] }))
        }
        "get_provider_health" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let failover = state.failover.read().await;
            let key = format!("{}:{provider_id}", app.as_str());
            let breaker = failover.breakers.get(&key);
            Ok(json!(web_provider_health_json(app, &provider_id, breaker)))
        }
        "get_universal_providers" => {
            let providers = state.universal_providers.read().await.providers.clone();
            Ok(json!(providers))
        }
        "get_universal_provider" => {
            let id = web_arg_string(&args, "id")?;
            let provider = state
                .universal_providers
                .read()
                .await
                .providers
                .get(&id)
                .cloned();
            Ok(json!(provider))
        }
        "upsert_universal_provider" => {
            let provider: UniversalProvider = web_arg_value_any(&args, &["provider"])?;
            let response = upsert_universal_provider(
                State(state.clone()),
                headers.clone(),
                Json(UpsertUniversalProviderRequest { provider }),
            )
            .await?
            .0;
            Ok(json!(response.provider))
        }
        "delete_universal_provider" => {
            let id = web_arg_string(&args, "id")?;
            let response =
                delete_universal_provider(State(state.clone()), headers.clone(), Path(id))
                    .await?
                    .0;
            Ok(json!(response.deleted))
        }
        "sync_universal_provider" => {
            let id = web_arg_string(&args, "id")?;
            let response = sync_universal_provider(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.result))
        }
        "list_shares" | "export_all_shares" => {
            let shares = state.shares.read().await.shares.clone();
            Ok(json!(shares))
        }
        "get_share_detail" => {
            let id = web_arg_share_id(&args)?;
            let share = state.shares.read().await.get(&id).cloned();
            Ok(json!(share))
        }
        "get_share_connect_info" => {
            let id = web_arg_share_id(&args)?;
            let config = state.config.read().await.clone();
            let share = state
                .shares
                .read()
                .await
                .get(&id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            Ok(json!(connect_info_for_share(&config, &share)?))
        }
        "list_share_markets" => {
            let markets = fetch_public_markets_from_router(state).await?;
            Ok(json!(markets))
        }
        "create_share" => {
            let input = web_share_upsert_input(state, &args).await?;
            let response = upsert_share(State(state.clone()), headers.clone(), Json(input))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "delete_share" => {
            let id = web_arg_share_id(&args)?;
            let response = delete_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.deleted))
        }
        "pause_share" => {
            let id = web_arg_share_id(&args)?;
            let response = pause_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "resume_share" => {
            let id = web_arg_share_id(&args)?;
            let response = resume_share(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "reset_share_usage" => {
            let id = web_arg_share_id(&args)?;
            let response = reset_share_usage(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "update_share_provider_binding" => {
            let (id, binding) = web_share_binding_input(state, &args).await?;
            let response = update_share_binding(
                State(state.clone()),
                headers.clone(),
                Path(id),
                Json(UpdateShareBindingRequest { binding }),
            )
            .await?
            .0;
            Ok(json!(response.share))
        }
        "update_share_acl" => {
            let share = web_update_share_acl(state, &args).await?;
            Ok(json!(share))
        }
        "update_share_owner_email" => {
            let share = web_update_share_owner_email(state, headers, &args).await?;
            Ok(json!(share))
        }
        "transfer_share_owner" => {
            let share = web_transfer_share_owner(state, headers, &args).await?;
            Ok(json!(share))
        }
        "authorize_share_market" => {
            let id = web_arg_share_id(&args)?;
            let value = web_payload(&args, &["params", "input"]);
            let market_email = web_arg_string_any(value, &["marketEmail", "market_email"])?;
            let response = authorize_share_market(
                State(state.clone()),
                headers.clone(),
                Path(id),
                Json(AuthorizeShareMarketRequest { market_email }),
            )
            .await?
            .0;
            Ok(json!(response.share))
        }
        "start_share_tunnel" => {
            let id = web_arg_share_id(&args)?;
            let response = start_share_tunnel(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "stop_share_tunnel" => {
            let id = web_arg_share_id(&args)?;
            let response = stop_share_tunnel(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "get_tunnel_status" => {
            if let Ok(id) = web_arg_share_id(&args) {
                return Ok(json!(web_share_tunnel_status(state, &id).await?));
            }
            let response = router_tunnels(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response.tunnels))
        }
        "get_client_tunnel" => Ok(web_client_tunnel_state(state).await),
        "get_client_tunnel_status" => {
            let runtime = state
                .tunnels
                .status(&crate::core::tunnel::client_tunnel_key())
                .await;
            Ok(web_client_tunnel_share_status(runtime))
        }
        "claim_client_tunnel" => {
            if web_has_payload(&args) {
                let input = web_client_tunnel_input(&args)?;
                let _ = update_client_tunnel(State(state.clone()), headers.clone(), Json(input))
                    .await?;
            }
            let _ = claim_client_tunnel(State(state.clone()), headers.clone()).await?;
            Ok(web_client_tunnel_state(state).await)
        }
        "update_client_tunnel" => {
            let input = web_client_tunnel_input(&args)?;
            let _ =
                update_client_tunnel(State(state.clone()), headers.clone(), Json(input)).await?;
            Ok(web_client_tunnel_state(state).await)
        }
        "start_client_tunnel" => {
            let response = issue_client_tunnel_lease(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response))
        }
        "stop_client_tunnel" => {
            let response = stop_client_tunnel(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response))
        }
        "get_usage_summary" => {
            let filter = web_usage_stats_filter_from_args(&args);
            let usage = state.usage.read().await;
            Ok(json!(usage.rollup_filtered(&filter)))
        }
        "get_usage_summary_by_app" => {
            let filter = web_usage_stats_filter_from_args(&args);
            let usage = state.usage.read().await;
            Ok(json!(usage.summary_by_app(&filter)))
        }
        "get_installed_skills" => Ok(json!([])),
        "get_usage_trends" => {
            let usage = state.usage.read().await;
            let filter = UsageStatsFilter {
                window_ms: Some(24 * 60 * 60 * 1000),
                ..UsageStatsFilter::default()
            };
            Ok(json!(usage.trends(&filter)))
        }
        "get_provider_stats" => {
            let usage = state.usage.read().await;
            Ok(json!(usage.provider_stats(&UsageStatsFilter::default())))
        }
        "get_model_stats" => {
            let usage = state.usage.read().await;
            Ok(json!(usage.model_stats(&UsageStatsFilter::default())))
        }
        "get_request_logs" => {
            let usage = state.usage.read().await;
            Ok(json!(usage.latest_filtered(UsageLogFilter {
                limit: Some(200),
                ..UsageLogFilter::default()
            })))
        }
        "get_request_detail" => {
            let id = web_arg_string(&args, "id").or_else(|_| web_arg_string(&args, "requestId"))?;
            let usage = state.usage.read().await;
            let log = usage.logs.iter().find(|log| log.request_id == id).cloned();
            Ok(json!(log))
        }
        "get_proxy_config_for_app" => {
            let app = web_arg_app_type(&args)?;
            Ok(json!(web_app_proxy_config_json(state, app).await))
        }
        "get_available_providers_for_failover" => {
            let app = web_arg_app_type(&args)?;
            Ok(json!(
                web_available_providers_for_failover(state, app).await
            ))
        }
        "get_proxy_status" => Ok(json!(web_proxy_status_json(state))),
        "get_proxy_takeover_status" => Ok(json!({
            "claude": false,
            "codex": false,
            "gemini": false
        })),
        "is_proxy_running" => Ok(json!(true)),
        "is_live_takeover_active" => Ok(json!(false)),
        "update_global_proxy_config" => {
            let input = web_upstream_proxy_input(&args)?;
            let response =
                update_upstream_proxy(State(state.clone()), headers.clone(), Json(input))
                    .await?
                    .0;
            Ok(json!(response.upstream_proxy))
        }
        "list_db_backups" => {
            let response = list_backups(State(state.clone()), headers.clone()).await?.0;
            Ok(json!(crate::core::backup::backup_entries_for_frontend(
                &response.backups
            )))
        }
        "create_db_backup" => {
            let body = web_create_backup_request(&args)?;
            let response = create_backup(State(state.clone()), headers.clone(), body)
                .await?
                .0;
            Ok(json!(response.backup))
        }
        "restore_db_backup" => {
            let id = web_arg_string_any(&args, &["id", "backupId", "filename"])?;
            let response = restore_backup(State(state.clone()), headers.clone(), Path(id))
                .await?
                .0;
            Ok(json!(response.result))
        }
        "get_auto_failover_enabled" => {
            let app = web_arg_app_type(&args)?;
            let failover = state.failover.read().await;
            let enabled = failover
                .apps
                .get(&app)
                .map(|config| config.enabled)
                .unwrap_or(false);
            Ok(json!(enabled))
        }
        "get_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let failover = state.failover.read().await;
            let providers = state.providers.read().await;
            let queue = failover
                .apps
                .get(&app)
                .map(|config| config.provider_queue.as_slice())
                .unwrap_or(&[]);
            let items = queue
                .iter()
                .enumerate()
                .map(|(index, provider_id)| {
                    let stored = providers.providers.iter().find(|provider| {
                        provider.app == app && provider.provider.id == *provider_id
                    });
                    json!({
                        "providerId": provider_id,
                        "providerName": stored
                            .map(|provider| provider.provider.name.clone())
                            .unwrap_or_else(|| provider_id.clone()),
                        "sortIndex": index,
                        "providerNotes": stored.and_then(|provider| {
                            provider_extra_string(&provider.provider, "notes")
                        })
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!(items))
        }
        "deepseek_account_status" => {
            let accounts = state.accounts.read().await;
            let deepseek_accounts = accounts
                .accounts
                .iter()
                .filter(|account| account.provider_type == ProviderType::DeepSeekAccount)
                .collect::<Vec<_>>();
            let default_account_id = deepseek_accounts.first().map(|account| account.id.clone());
            let authenticated = deepseek_accounts
                .iter()
                .any(|account| account_is_authenticated(account));
            let mapped_accounts = deepseek_accounts
                .iter()
                .enumerate()
                .map(|(index, account)| {
                    json!({
                        "id": account.id,
                        "login": account.email.clone().unwrap_or_else(|| account.id.clone()),
                        "authenticated_at": account_authenticated_at(account),
                        "is_default": default_account_id
                            .as_deref()
                            .map(|id| id == account.id)
                            .unwrap_or(index == 0),
                        "has_password": true
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "authenticated": authenticated,
                "default_account_id": default_account_id,
                "accounts": mapped_accounts
            }))
        }
        "auth_get_status" => {
            let provider_type = web_auth_provider_type(&args)?;
            let provider_label = managed_auth_provider_label(provider_type);
            let accounts = state.accounts.read().await;
            let matching = accounts
                .accounts
                .iter()
                .filter(|account| account.provider_type == provider_type)
                .collect::<Vec<_>>();
            let default_account_id = matching.first().map(|account| account.id.clone());
            let authenticated = matching
                .iter()
                .any(|account| account_is_authenticated(account));
            let mapped_accounts = matching
                .iter()
                .map(|account| {
                    map_managed_auth_account(account, provider_label, default_account_id.as_deref())
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "provider": provider_label,
                "authenticated": authenticated,
                "default_account_id": default_account_id,
                "migration_error": Value::Null,
                "accounts": mapped_accounts
            }))
        }
        "auth_list_accounts" => {
            let provider_type = web_optional_auth_provider_type(&args)?;
            let accounts = state
                .accounts
                .read()
                .await
                .accounts
                .iter()
                .filter(|account| {
                    provider_type
                        .map(|provider_type| account.provider_type == provider_type)
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>();
            Ok(json!(accounts))
        }
        "auth_start_login" => {
            web_managed_auth_start_login(state.clone(), headers.clone(), &args).await
        }
        "auth_poll_for_account" => {
            web_managed_auth_poll_for_account(state.clone(), headers.clone(), &args).await
        }
        "auth_remove_account" => {
            web_managed_auth_remove_account(state.clone(), headers.clone(), &args).await
        }
        "auth_set_default_account" => {
            web_managed_auth_set_default_account(state.clone(), headers.clone(), &args).await
        }
        "auth_logout" => web_managed_auth_logout(state.clone(), headers.clone(), &args).await,
        "auth_submit_oauth_code" => {
            let provider_type = web_auth_provider_type(&args)?;
            let provider_label = managed_auth_provider_label(provider_type);
            let session_id = web_optional_string_any(&args, &["sessionId", "session_id"]);
            let state_arg = web_optional_string_any(&args, &["state"]).or_else(|| {
                session_id
                    .is_none()
                    .then(|| web_optional_string_any(&args, &["deviceCode", "device_code"]))
                    .flatten()
            });
            let code = web_optional_string_any(&args, &["code"]);
            let response = finish_account_login(
                State(state.clone()),
                headers.clone(),
                Json(FinishAccountLoginRequest {
                    session_id,
                    state: state_arg,
                    code,
                    execute_token_exchange: Some(true),
                }),
            )
            .await?
            .0;
            let account_id = response
                .account
                .as_ref()
                .map(|account| account.id.as_str())
                .ok_or_else(|| {
                    ApiError::bad_gateway("oauth code exchange did not import account")
                })?;
            web_managed_auth_account_by_id(&state, account_id, provider_label).await
        }
        "refresh_oauth_quota" => {
            let account_id = web_resolve_account_id(state, &args).await?;
            let Some(account_id) = account_id else {
                return Ok(Value::Null);
            };
            let response = account_quota(
                State(state.clone()),
                headers.clone(),
                Path(account_id),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: web_optional_bool(&args, &["force"]),
                }),
            )
            .await?
            .0;
            Ok(json!(response))
        }
        "get_cached_oauth_quota" => {
            let account_id = web_resolve_account_id(state, &args).await?;
            let Some(account_id) = account_id else {
                return Ok(Value::Null);
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
        "get_claude_oauth_quota" => {
            let response =
                web_provider_quota(state, headers, &args, ProviderType::ClaudeOAuth).await?;
            Ok(response)
        }
        "get_codex_oauth_quota" => {
            let response =
                web_provider_quota(state, headers, &args, ProviderType::CodexOAuth).await?;
            Ok(response)
        }
        "copilot_start_device_flow" => {
            let response = start_copilot_device_login(
                State(state.clone()),
                headers.clone(),
                Json(StartCopilotDeviceLoginRequest {
                    github_domain: web_optional_string_any(
                        &args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(json!(response.device))
        }
        "copilot_poll_for_auth" => {
            let device_code = web_arg_string_any(&args, &["deviceCode", "device_code"])?;
            let response = poll_copilot_device_login(
                State(state.clone()),
                headers.clone(),
                Json(PollCopilotDeviceLoginRequest {
                    device_code,
                    github_domain: web_optional_string_any(
                        &args,
                        &["githubDomain", "github_domain"],
                    ),
                }),
            )
            .await?
            .0;
            Ok(json!(response))
        }
        "start_proxy_server" => Ok(json!({
            "address": state.bind_addr.ip().to_string(),
            "port": state.bind_addr.port(),
        })),
        "stop_proxy_server" | "stop_proxy_with_restore" => Ok(json!(true)),
        "set_proxy_takeover_for_app" => Ok(json!(true)),
        "switch_proxy_provider" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let exists = state
                .providers
                .read()
                .await
                .providers
                .iter()
                .any(|provider| provider.app == app && provider.provider.id == provider_id);
            if !exists {
                return Err(ApiError::not_found("provider not found"));
            }
            Ok(json!(true))
        }
        "get_proxy_config" => Ok(json!(web_global_proxy_config_json(state))),
        "update_proxy_config" => {
            let listen_address =
                web_optional_string_any(&args, &["listenAddress", "listen_address", "address"]);
            let listen_port = args
                .get("listenPort")
                .or_else(|| args.get("listen_port"))
                .or_else(|| args.get("port"))
                .and_then(|value| value.as_u64().map(|port| port as u16));
            if listen_address.is_some() || listen_port.is_some() {
                let mut patch = serde_json::Map::new();
                let mut proxy_patch = serde_json::Map::new();
                if let Some(address) = listen_address {
                    proxy_patch.insert("listenAddress".to_string(), json!(address));
                }
                if let Some(port) = listen_port {
                    proxy_patch.insert("listenPort".to_string(), json!(port));
                }
                patch.insert("proxyRuntimeConfig".to_string(), Value::Object(proxy_patch));
                {
                    let mut store = state.ui_settings.write().await;
                    store.apply_patch(Value::Object(patch));
                }
                state.save_ui_settings().await.map_err(ApiError::internal)?;
            }
            Ok(json!(true))
        }
        "update_proxy_config_for_app" => web_update_proxy_config_for_app(state, &args).await,
        "add_to_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let providers = state.providers.read().await.clone();
            let config = {
                let mut failover = state.failover.write().await;
                let mut queue = failover
                    .apps
                    .get(&app)
                    .map(|config| config.provider_queue.clone())
                    .unwrap_or_default();
                if !queue.iter().any(|id| id == &provider_id) {
                    queue.push(provider_id);
                }
                failover.update_app_config(
                    app,
                    UpdateFailoverAppInput {
                        provider_queue: Some(queue),
                        ..Default::default()
                    },
                    &providers,
                )
            };
            state.save_failover().await.map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "remove_from_failover_queue" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let providers = state.providers.read().await.clone();
            let config = {
                let mut failover = state.failover.write().await;
                let queue = failover
                    .apps
                    .get(&app)
                    .map(|config| {
                        config
                            .provider_queue
                            .iter()
                            .filter(|id| **id != provider_id)
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                failover.update_app_config(
                    app,
                    UpdateFailoverAppInput {
                        provider_queue: Some(queue),
                        ..Default::default()
                    },
                    &providers,
                )
            };
            state.save_failover().await.map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "set_auto_failover_enabled" => {
            let app = web_arg_app_type(&args)?;
            let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let providers = state.providers.read().await.clone();
            let config = {
                let mut failover = state.failover.write().await;
                failover.update_app_config(
                    app,
                    UpdateFailoverAppInput {
                        enabled: Some(enabled),
                        ..Default::default()
                    },
                    &providers,
                )
            };
            state.save_failover().await.map_err(ApiError::internal)?;
            Ok(json!(config.enabled))
        }
        "get_circuit_breaker_config" => {
            let failover = state.failover.read().await;
            let app = web_arg_app_type(&args).unwrap_or(AppKind::Claude);
            let config = failover.apps.get(&app).cloned().unwrap_or_default();
            Ok(json!({
                "failureThreshold": config.failure_threshold,
                "successThreshold": 2,
                "timeoutSeconds": (config.open_duration_ms / 1000).max(1),
                "errorRateThreshold": 0.5,
                "minRequests": 10,
            }))
        }
        "update_circuit_breaker_config" => {
            let config: Value = web_arg_value(&args, "config")?;
            let app = web_arg_app_type(&args).unwrap_or(AppKind::Claude);
            let providers = state.providers.read().await.clone();
            let failure_threshold = config
                .get("failureThreshold")
                .and_then(Value::as_u64)
                .map(|value| value as u32);
            let timeout_seconds = config.get("timeoutSeconds").and_then(Value::as_u64);
            let updated = {
                let mut failover = state.failover.write().await;
                failover.update_app_config(
                    app,
                    UpdateFailoverAppInput {
                        failure_threshold,
                        open_duration_ms: timeout_seconds.map(|seconds| (seconds * 1000) as u128),
                        ..Default::default()
                    },
                    &providers,
                )
            };
            state.save_failover().await.map_err(ApiError::internal)?;
            Ok(json!({
                "failureThreshold": updated.failure_threshold,
                "successThreshold": 2,
                "timeoutSeconds": (updated.open_duration_ms / 1000).max(1),
                "errorRateThreshold": 0.5,
                "minRequests": 10,
            }))
        }
        "get_circuit_breaker_stats" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let failover = state.failover.read().await;
            let key = format!("{}:{}", app.as_str(), provider_id);
            let breaker = failover.breakers.get(&key);
            Ok(json!(web_circuit_breaker_stats_json(breaker)))
        }
        "reset_circuit_breaker" => {
            let app = web_arg_app_type(&args)?;
            let provider_id = web_arg_string_any(&args, &["providerId", "provider_id"])?;
            let breaker = {
                let mut failover = state.failover.write().await;
                failover.reset_provider(app, &provider_id)
            };
            state.save_failover().await.map_err(ApiError::internal)?;
            Ok(json!(web_circuit_breaker_stats_json(Some(&breaker))))
        }
        "delete_db_backup" => {
            let id = web_arg_string_any(&args, &["filename", "id", "backupId"])?;
            crate::core::backup::delete_backup(&state.config_dir, &id)
                .map_err(ApiError::bad_request)?;
            Ok(Value::Null)
        }
        "rename_db_backup" => {
            let id = web_arg_string_any(&args, &["oldFilename", "filename", "id"])?;
            let new_name = web_arg_string_any(&args, &["newName", "new_name"])?;
            let manifest = crate::core::backup::rename_backup(&state.config_dir, &id, &new_name)
                .map_err(ApiError::bad_request)?;
            Ok(json!(manifest.id))
        }
        "export_config_to_file" => {
            let bundle = crate::core::config_transfer::export_config_bundle(&state.config_dir)
                .map_err(ApiError::internal)?;
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                serde_json::to_vec(&bundle).map_err(ApiError::internal)?,
            );
            let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            Ok(json!({
                "success": true,
                "message": "config exported",
                "fileName": format!("cc-switch-server-export-{stamp}.json"),
                "contentBase64": encoded,
            }))
        }
        "import_config_from_file" => {
            if let Some(encoded) =
                web_optional_string_any(&args, &["contentBase64", "content_base64", "fileContent"])
            {
                let backup_id = crate::core::config_transfer::import_config_bundle_from_base64(
                    &state.config_dir,
                    &encoded,
                )
                .map_err(ApiError::bad_request)?;
                state
                    .reload_persistent_stores()
                    .await
                    .map_err(ApiError::internal)?;
                return Ok(json!({
                    "success": true,
                    "message": "config imported",
                    "backupId": backup_id,
                }));
            }
            Err(ApiError::bad_request(
                "import requires contentBase64 payload on server web runtime",
            ))
        }
        "open_file_dialog" | "save_file_dialog" | "pick_directory" => Ok(Value::Null),
        "open_external" => Ok(json!(true)),
        "open_config_folder" | "open_app_config_folder" => Ok(json!(true)),
        "restart_app" | "check_for_updates" | "install_update_and_restart" | "update_tray_menu" => {
            Ok(json!(true))
        }
        "has_codex_unify_history_backup" => Ok(json!(false)),
        "restore_codex_unified_history" => Ok(json!({
            "restoredJsonlFiles": 0,
            "restoredStateRows": 0,
            "skippedReason": "not_supported_on_server",
        })),
        "get_model_pricing" => {
            let response = list_model_pricing(State(state.clone()), headers.clone())
                .await?
                .0;
            Ok(json!(response.models))
        }
        "update_model_pricing" => {
            let model_id = web_arg_string_any(&args, &["modelId", "model_id"])?;
            let input = UpdateModelPricingInput {
                model_id: Some(model_id.clone()),
                display_name: web_arg_string_any(&args, &["displayName", "display_name"])?,
                input_cost_per_million: web_arg_string_any(&args, &["inputCost", "input_cost"])?,
                output_cost_per_million: web_arg_string_any(&args, &["outputCost", "output_cost"])?,
                cache_read_cost_per_million: web_arg_string_any(
                    &args,
                    &["cacheReadCost", "cache_read_cost"],
                )?,
                cache_creation_cost_per_million: web_arg_string_any(
                    &args,
                    &["cacheCreationCost", "cache_creation_cost"],
                )?,
            };
            let response = upsert_model_pricing(State(state.clone()), headers.clone(), Json(input))
                .await?
                .0;
            Ok(json!(response.model))
        }
        "delete_model_pricing" => {
            let model_id = web_arg_string_any(&args, &["modelId", "model_id"])?;
            delete_model_pricing(State(state.clone()), headers.clone(), Path(model_id)).await?;
            Ok(Value::Null)
        }
        "get_pricing_model_source" => {
            let store = state.ui_settings.read().await;
            let source = store
                .value
                .get("pricingModelSource")
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "claude": "response",
                        "codex": "response",
                        "gemini": "response",
                    })
                });
            Ok(source)
        }
        "set_pricing_model_source" => {
            let source: Value = web_arg_value(&args, "source")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "pricingModelSource": source }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!(true))
        }
        "webdav_sync_save_settings" => {
            let settings: Value = web_arg_value(&args, "settings")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "webdavSync": settings }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!({ "success": true }))
        }
        "s3_sync_save_settings" => {
            let settings: Value = web_arg_value(&args, "settings")?;
            {
                let mut store = state.ui_settings.write().await;
                store.apply_patch(json!({ "s3Sync": settings }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(json!({ "success": true }))
        }
        "webdav_test_connection" | "s3_test_connection" => Ok(json!({
            "success": true,
            "message": "connection test is not available on server web runtime; settings saved only",
        })),
        "webdav_sync_fetch_remote_info" | "s3_sync_fetch_remote_info" => {
            Ok(json!({ "empty": true }))
        }
        "webdav_sync_upload" | "s3_sync_upload" => {
            let backup = crate::core::backup::create_backup(
                &state.config_dir,
                Some("cloud-sync-upload".to_string()),
            )
            .map_err(ApiError::internal)?;
            Ok(json!({ "status": format!("uploaded:{}", backup.id) }))
        }
        "webdav_sync_download" | "s3_sync_download" => {
            Ok(json!({ "status": "download_not_configured" }))
        }
        "get_tool_versions" => Ok(json!([])),
        "probe_tool_installations" => {
            let tool_names = args
                .get("toolNames")
                .or_else(|| args.get("tool_names"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(json!(tool_names
                .into_iter()
                .map(|name| json!({
                    "toolName": name,
                    "installed": false,
                    "needs_confirmation": false,
                }))
                .collect::<Vec<_>>()))
        }
        "run_tool_lifecycle_action" => Err(ApiError::bad_request(
            "tool lifecycle actions are not available on server web runtime",
        )),
        "copilot_list_accounts" => Ok(json!([])),
        "copilot_is_authenticated" => Ok(json!(false)),
        "copilot_get_auth_status" => Ok(json!({ "authenticated": false, "accounts": [] })),
        "copilot_get_token" | "copilot_get_token_for_account" => Ok(Value::Null),
        "copilot_get_models" | "copilot_get_models_for_account" => Ok(json!([])),
        "copilot_get_usage" | "copilot_get_usage_for_account" => Ok(Value::Null),
        "copilot_logout" | "copilot_remove_account" => Ok(json!(true)),
        "copilot_set_default_account" => Ok(json!(true)),
        "copilot_poll_for_account" => Ok(Value::Null),
        "deepseek_account_add" | "deepseek_account_remove" | "deepseek_account_set_default" => {
            Ok(json!(true))
        }
        "deepseek_account_list" => Ok(json!([])),
        "get_common_config_snippet" => {
            let app_type = web_arg_common_config_app_type(&args)?;
            let store = state.ui_settings.read().await;
            Ok(ui_settings::common_config_snippet_for_frontend(
                &store, app_type,
            ))
        }
        "set_common_config_snippet" => {
            let app_type = web_arg_common_config_app_type(&args)?;
            let snippet = web_arg_string_any(&args, &["snippet", "value"])?;
            {
                let mut store = state.ui_settings.write().await;
                let mut snippets = store
                    .value
                    .get("commonConfigSnippets")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let Some(map) = snippets.as_object_mut() {
                    if snippet.trim().is_empty() {
                        map.remove(app_type);
                    } else {
                        map.insert(app_type.to_string(), json!(snippet));
                    }
                }
                store.apply_patch(json!({ "commonConfigSnippets": snippets }));
            }
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "extract_common_config_snippet" => {
            let _app_type = web_arg_common_config_app_type(&args)?;
            if let Some(settings_config) = args.get("settingsConfig").and_then(Value::as_str) {
                let trimmed = settings_config.trim();
                if trimmed.is_empty() {
                    return Ok(json!("{}"));
                }
                return Ok(json!(trimmed));
            }
            Ok(json!("{}"))
        }
        "stream_check_provider" => {
            let stored = web_resolve_stored_provider(state, &args).await?;
            let config = web_stream_check_config(state).await;
            let http_client = state.http_client().await;
            let result = crate::core::stream_check::check_provider_reachability(
                &http_client,
                &stored,
                &config,
            )
            .await;
            Ok(json!(result))
        }
        "stream_check_all_providers" => {
            let app = web_arg_app_type(&args)?;
            let proxy_targets_only = args
                .get("proxyTargetsOnly")
                .or_else(|| args.get("proxy_targets_only"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let config = web_stream_check_config(state).await;
            let http_client = state.http_client().await;
            let allowed_ids = if proxy_targets_only {
                Some(web_proxy_target_provider_ids(state, app).await)
            } else {
                None
            };
            let providers = state.providers.read().await.providers.clone();
            let mut results = Vec::new();
            for stored in providers.into_iter().filter(|item| item.app == app) {
                if allowed_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&stored.provider.id))
                {
                    continue;
                }
                let result = crate::core::stream_check::check_provider_reachability(
                    &http_client,
                    &stored,
                    &config,
                )
                .await;
                results.push((stored.provider.id.clone(), result));
            }
            Ok(json!(results))
        }
        "model_test_provider" => {
            let stored = web_resolve_stored_provider(state, &args).await?;
            let config = web_stream_check_config(state).await;
            let response = test_provider_inner(
                state,
                stored,
                &TestProviderQuery {
                    app: None,
                    network: Some(true),
                    timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
                    model: None,
                    stream: Some(true),
                },
            )
            .await?;
            Ok(json!(map_provider_test_to_stream_check_result(
                &response, &config,
            )))
        }
        "model_test_all_providers" => {
            let app = web_arg_app_type(&args)?;
            let proxy_targets_only = args
                .get("proxyTargetsOnly")
                .or_else(|| args.get("proxy_targets_only"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let config = web_stream_check_config(state).await;
            let allowed_ids = if proxy_targets_only {
                Some(web_proxy_target_provider_ids(state, app).await)
            } else {
                None
            };
            let providers = state.providers.read().await.providers.clone();
            let mut results = Vec::new();
            for stored in providers.into_iter().filter(|item| item.app == app) {
                if allowed_ids
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&stored.provider.id))
                {
                    continue;
                }
                let response = test_provider_inner(
                    state,
                    stored.clone(),
                    &TestProviderQuery {
                        app: None,
                        network: Some(true),
                        timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
                        model: None,
                        stream: Some(true),
                    },
                )
                .await
                .unwrap_or_else(|error| {
                    let message = error.message.clone();
                    TestProviderResponse {
                        ok: false,
                        provider_id: stored.provider.id.clone(),
                        app: stored.app,
                        provider_type: stored.provider_type,
                        adapter: "unknown",
                        support: proxy::adapters::AdapterSupport::Planned,
                        endpoint: String::new(),
                        model: String::new(),
                        stream: true,
                        header_names: Vec::new(),
                        network_checked: true,
                        network_status_code: None,
                        network_latency_ms: None,
                        network_stream_completed: None,
                        network_error: Some(message.clone()),
                        message,
                    }
                });
                results.push((
                    stored.provider.id,
                    map_provider_test_to_stream_check_result(&response, &config),
                ));
            }
            Ok(json!(results))
        }
        "fetch_models_for_config" => web_fetch_models_for_config(state, &args).await,
        "get_codex_oauth_models" | "get_antigravity_oauth_models" => Ok(json!([])),
        "update_share_description" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    description: web_optional_string_any(payload, &["description", "value"])
                        .map(Some),
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_for_sale" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    for_sale: web_optional_string_any(payload, &["forSale", "for_sale"]),
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_token_limit" => {
            let payload = web_payload(&args, &["params", "input"]);
            let token_limit = payload
                .get("tokenLimit")
                .or_else(|| payload.get("token_limit"))
                .and_then(Value::as_i64);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    token_limit,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_parallel_limit" => {
            let payload = web_payload(&args, &["params", "input"]);
            let parallel_limit = payload
                .get("parallelLimit")
                .or_else(|| payload.get("parallel_limit"))
                .and_then(Value::as_i64);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    parallel_limit,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_expiration" => {
            let payload = web_payload(&args, &["params", "input"]);
            let expires_at = web_optional_string_any(payload, &["expiresAt", "expires_at"]);
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    expires_at,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_for_sale_official_price_percent" => {
            let payload = web_payload(&args, &["params", "input"]);
            let pricing = web_optional_deserialize::<BTreeMap<String, u16>>(
                payload,
                "forSaleOfficialPricePercentByApp",
            )?
            .or_else(|| {
                web_optional_deserialize::<BTreeMap<String, u16>>(
                    payload,
                    "for_sale_official_price_percent_by_app",
                )
                .ok()
                .flatten()
            });
            let share = web_patch_share_settings(
                state,
                payload,
                ShareSettingsPatch {
                    for_sale_official_price_percent_by_app: pricing,
                    ..ShareSettingsPatch::default()
                },
            )
            .await?;
            Ok(json!(share))
        }
        "update_share_subdomain" => {
            let payload = web_payload(&args, &["params", "input"]);
            let share_id = web_arg_share_id(payload)?;
            let subdomain = web_arg_string_any(payload, &["subdomain"])?;
            let response = update_share_subdomain(
                State(state.clone()),
                headers.clone(),
                Path(share_id),
                Json(UpdateShareSubdomainRequest { subdomain }),
            )
            .await?
            .0;
            Ok(json!(response.share))
        }
        "enable_share" => {
            let share_id = web_arg_share_id(&args)?;
            let _ = resume_share(
                State(state.clone()),
                headers.clone(),
                Path(share_id.clone()),
            )
            .await?;
            let response =
                start_share_tunnel(State(state.clone()), headers.clone(), Path(share_id))
                    .await?
                    .0;
            Ok(json!(response.share))
        }
        "disable_share" => {
            let share_id = web_arg_share_id(&args)?;
            let _ = stop_share_tunnel(
                State(state.clone()),
                headers.clone(),
                Path(share_id.clone()),
            )
            .await?;
            let response = pause_share(State(state.clone()), headers.clone(), Path(share_id))
                .await?
                .0;
            Ok(json!(response.share))
        }
        "import_shares" => {
            let shares: Vec<Share> = web_arg_value_any(&args, &["shares"])?;
            let response = import_shares(
                State(state.clone()),
                headers.clone(),
                Json(ImportSharesRequest { shares }),
            )
            .await?
            .0;
            Ok(json!(response.imported))
        }
        "list_share_binding_history" => {
            let share_id = web_arg_share_id(&args)?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|value| value as usize);
            let history = state
                .shares
                .read()
                .await
                .get(&share_id)
                .map(|share| share.binding_history.clone())
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            let history = match limit {
                Some(limit) => history.into_iter().rev().take(limit).rev().collect(),
                None => history,
            };
            Ok(json!(history))
        }
        "configure_tunnel" => {
            let config: UpdateClientTunnelInput = web_arg_value(&args, "config")?;
            update_client_tunnel(State(state.clone()), headers.clone(), Json(config)).await?;
            Ok(Value::Null)
        }
        "get_claude_common_config_snippet" => {
            let store = state.ui_settings.read().await;
            Ok(ui_settings::common_config_snippet_for_frontend(
                &store, "claude",
            ))
        }
        "set_claude_common_config_snippet" => {
            let snippet = web_arg_string_any(&args, &["snippet", "value"])?;
            let mut store = state.ui_settings.write().await;
            let mut snippets = store
                .value
                .get("commonConfigSnippets")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(map) = snippets.as_object_mut() {
                if snippet.trim().is_empty() {
                    map.remove("claude");
                } else {
                    map.insert("claude".to_string(), json!(snippet));
                }
            }
            store.apply_patch(json!({ "commonConfigSnippets": snippets }));
            state.save_ui_settings().await.map_err(ApiError::internal)?;
            Ok(Value::Null)
        }
        "check_env_conflicts" => Ok(json!([])),
        "delete_env_vars" => Ok(json!({ "backupPath": Value::Null })),
        "restore_env_backup" => Ok(Value::Null),
        "get_auto_launch_status" => Ok(json!(false)),
        "set_auto_launch" => Ok(Value::Null),
        "sync_current_providers_live" => Ok(json!({ "imported": 0, "warnings": [] })),
        "sync_session_usage" => Ok(Value::Null),
        "testUsageScript" => Ok(json!({ "ok": true, "output": "" })),
        "queryProviderUsage" => Ok(json!({ "logs": [], "summary": Value::Null })),
        "get_usage_data_sources" => Ok(json!(["server"])),
        "check_provider_limits" => Ok(json!({ "ok": true, "withinLimits": true })),
        "get_subscription_quota" | "get_coding_plan_quota" | "get_balance" => Ok(Value::Null),
        "get_default_cost_multiplier" => Ok(json!(1.0)),
        "set_default_cost_multiplier" => Ok(Value::Null),
        "read_live_provider_settings" => Ok(json!({})),
        "test_api_endpoints" => Ok(json!([])),
        "get_custom_endpoints" => Ok(json!([])),
        "add_custom_endpoint" | "remove_custom_endpoint" | "update_endpoint_last_used" => {
            Ok(Value::Null)
        }
        "remove_provider_from_live_config" => Ok(json!(true)),
        "import_opencode_providers_from_live"
        | "import_openclaw_providers_from_live"
        | "import_hermes_providers_from_live" => Ok(json!([])),
        "get_opencode_live_provider_ids"
        | "get_openclaw_live_provider_ids"
        | "get_hermes_live_provider_ids" => Ok(json!([])),
        "import_claude_desktop_providers_from_claude"
        | "ensure_claude_desktop_official_provider" => Ok(json!(false)),
        "get_claude_desktop_status" => Ok(json!({ "installed": false, "configured": false })),
        "get_claude_desktop_default_routes" => Ok(json!([])),
        "get_claude_code_config_path" => Ok(json!("")),
        "set_app_config_dir_override" => Ok(Value::Null),
        "apply_claude_plugin_config"
        | "apply_claude_onboarding_skip"
        | "clear_claude_onboarding_skip" => Ok(Value::Null),
        "codex_banked_reset_status" => Ok(json!({ "enabled": false })),
        "codex_banked_reset_invite" | "codex_banked_reset_consume" => Err(
            ApiError::not_implemented("codex banked reset is not available on cc-switch-server"),
        ),
        "open_provider_terminal" => Err(ApiError::not_implemented(
            "open_provider_terminal is not available in server web runtime",
        )),
        _ => Err(ApiError::web_invoke_not_wired(format!(
            "desktop invoke command '{command}' is registered but has no dispatcher"
        ))),
    }
}

async fn web_resolve_stored_provider(
    state: &ServerState,
    args: &Value,
) -> Result<StoredProvider, ApiError> {
    let app = web_arg_app_type(args)?;
    let provider_id = web_arg_string_any(args, &["providerId", "provider_id"])?;
    resolve_provider_by_id(state, &provider_id, Some(app)).await
}

async fn web_stream_check_config(
    state: &ServerState,
) -> crate::core::stream_check::StreamCheckConfig {
    let store = state.ui_settings.read().await;
    let value = ui_settings::stream_check_config_for_frontend(&store);
    crate::core::stream_check::stream_check_config_from_value(&value)
}

async fn web_proxy_target_provider_ids(
    state: &ServerState,
    app: AppKind,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;
    let mut ids = HashSet::new();
    let ui_settings = state.ui_settings.read().await.for_frontend();
    if let Some(current) = live_config_import::read_current_provider_id(&ui_settings, app) {
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

fn map_provider_test_to_stream_check_result(
    response: &TestProviderResponse,
    config: &crate::core::stream_check::StreamCheckConfig,
) -> crate::core::stream_check::StreamCheckResult {
    use crate::core::stream_check::{HealthStatus, StreamCheckResult};
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

async fn web_fetch_models_for_config(state: &ServerState, args: &Value) -> Result<Value, ApiError> {
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

async fn web_patch_share_settings(
    state: &ServerState,
    args: &Value,
    patch: ShareSettingsPatch,
) -> Result<Share, ApiError> {
    let share_id = web_arg_share_id(args)?;
    let share = {
        let mut store = state.shares.write().await;
        store
            .apply_settings_patch(&share_id, patch)
            .map_err(map_share_patch_error)?
    };
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "settings_patched");
    Ok(share)
}

async fn web_provider_quota(
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

async fn web_resolve_account_id(
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

async fn web_share_upsert_input(
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

async fn web_share_binding_input(
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

async fn web_update_share_owner_email(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
) -> Result<Share, ApiError> {
    require_session(state, headers).await?;
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let owner_email = web_arg_string_any(value, &["ownerEmail", "owner_email"])?;
    let share = state
        .shares
        .write()
        .await
        .update_owner_email(&share_id, &owner_email)
        .map_err(map_share_patch_error)?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "owner_email_updated");
    Ok(share)
}

async fn web_transfer_share_owner(
    state: &ServerState,
    headers: &HeaderMap,
    args: &Value,
) -> Result<Share, ApiError> {
    require_session(state, headers).await?;
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    let target_email = web_arg_string_any(value, &["targetEmail", "target_email"])?;
    let share = state
        .shares
        .write()
        .await
        .transfer_owner_email(&share_id, &target_email)
        .map_err(map_share_patch_error)?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "owner_transferred");
    Ok(share)
}

async fn web_update_share_acl(state: &ServerState, args: &Value) -> Result<Share, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    let share_id = web_arg_string_any(value, &["shareId", "share_id", "id"])?;
    if let Some(acl_value) = value.get("acl") {
        let acl =
            serde_json::from_value::<ShareAcl>(acl_value.clone()).map_err(ApiError::bad_request)?;
        let share = state
            .shares
            .write()
            .await
            .replace_acl(&share_id, acl)
            .ok_or_else(|| ApiError::not_found("share not found"))?;
        state.save_shares().await.map_err(ApiError::internal)?;
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
        .shares
        .write()
        .await
        .apply_settings_patch(&share_id, patch)
        .map_err(map_share_patch_error)?;
    state.save_shares().await.map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(state, "share.changed", &share, "acl_replaced");
    Ok(share)
}

fn web_client_tunnel_share_status(
    runtime: Option<crate::core::tunnel::TunnelRuntimeStatus>,
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

async fn web_client_tunnel_state(state: &ServerState) -> Value {
    let config = state.config.read().await;
    let runtime = state
        .tunnels
        .status(&crate::core::tunnel::client_tunnel_key())
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

async fn web_share_tunnel_status(state: &ServerState, share_id: &str) -> Result<Value, ApiError> {
    let share = state
        .shares
        .read()
        .await
        .get(share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let runtime_status = state
        .tunnels
        .status(&crate::core::tunnel::share_tunnel_key(share_id))
        .await;
    Ok(json!({
        "shareId": share.id,
        "status": share.status,
        "lastError": share.last_error,
        "runtimeStatus": runtime_status,
        "requiresOwnerLogin": false
    }))
}

async fn web_provider_type_for_binding(
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

fn web_create_backup_request(args: &Value) -> Result<Option<Json<CreateBackupRequest>>, ApiError> {
    if !web_has_payload(args) {
        return Ok(None);
    }
    let value = web_payload(args, &["input", "params"]);
    let request = serde_json::from_value::<CreateBackupRequest>(value.clone())
        .map_err(ApiError::bad_request)?;
    Ok(Some(Json(request)))
}

fn web_client_tunnel_input(args: &Value) -> Result<UpdateClientTunnelInput, ApiError> {
    let value = web_payload(args, &["params", "input", "config"]);
    Ok(UpdateClientTunnelInput {
        tunnel_subdomain: web_optional_string_any(value, &["tunnelSubdomain", "subdomain"]),
        tunnel_status: web_optional_string_any(value, &["tunnelStatus", "status"]),
    })
}

fn web_upstream_proxy_input(args: &Value) -> Result<UpdateUpstreamProxyInput, ApiError> {
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

fn web_arg_share_id(args: &Value) -> Result<String, ApiError> {
    let value = web_payload(args, &["params", "input"]);
    web_arg_string_any(value, &["shareId", "share_id", "id"])
}

fn web_payload<'a>(args: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter().find_map(|key| args.get(*key)).unwrap_or(args)
}

fn web_has_payload(args: &Value) -> bool {
    args.as_object().is_some_and(|object| !object.is_empty())
}

fn web_arg_value_any<T>(args: &Value, keys: &[&str]) -> Result<T, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = web_payload(args, keys).clone();
    serde_json::from_value(value).map_err(ApiError::bad_request)
}

fn web_arg_string_any(args: &Value, keys: &[&str]) -> Result<String, ApiError> {
    web_optional_string_any(args, keys).ok_or_else(|| {
        ApiError::bad_request(format!("{} is required", keys.first().unwrap_or(&"value")))
    })
}

fn web_runtime_auth_required_payload(
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

fn web_runtime_auth_methods(config: &ServerConfig) -> Vec<&'static str> {
    crate::core::web_auth::auth_methods(config).methods
}

fn web_global_proxy_config_json(state: &ServerState) -> Value {
    json!({
        "proxyEnabled": true,
        "listenAddress": state.bind_addr.ip().to_string(),
        "listenPort": state.bind_addr.port(),
        "enableLogging": true,
    })
}

fn web_proxy_status_json(state: &ServerState) -> Value {
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

async fn web_upstream_proxy_status_json(state: &ServerState) -> Value {
    let config = state.config.read().await;
    let url = config.upstream_proxy.url.clone();
    let enabled = url.as_ref().is_some_and(|value| !value.trim().is_empty());
    json!({
        "enabled": enabled,
        "proxyUrl": url,
    })
}

async fn web_test_proxy_url(url: &str) -> Value {
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

async fn web_scan_local_proxies() -> Vec<Value> {
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

async fn web_app_proxy_config_json(state: &ServerState, app: AppKind) -> Value {
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

async fn web_available_providers_for_failover(state: &ServerState, app: AppKind) -> Vec<Provider> {
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

fn web_optional_string_any(args: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn web_optional_u128(args: &Value, keys: &[&str]) -> Option<u128> {
    keys.iter().find_map(|key| {
        args.get(*key).and_then(|value| {
            value
                .as_u64()
                .map(|number| number as u128)
                .or_else(|| value.as_i64().map(|number| number.max(0) as u128))
        })
    })
}

fn web_usage_stats_filter_from_args(args: &Value) -> UsageStatsFilter {
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

fn web_optional_bool(args: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_bool))
}

fn web_optional_i64(args: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        args.get(*key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        })
    })
}

fn web_optional_u64(args: &Value, keys: &[&str]) -> Option<u64> {
    web_optional_i64(args, keys).and_then(|value| (value >= 0).then_some(value as u64))
}

fn web_optional_u32(args: &Value, keys: &[&str]) -> Option<u32> {
    web_optional_i64(args, keys).and_then(|value| u32::try_from(value).ok())
}

fn web_optional_deserialize<T>(args: &Value, key: &str) -> Result<Option<T>, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    args.get(key)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::bad_request)
}

fn web_optional_share_for_sale(args: &Value) -> Option<bool> {
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

fn web_optional_auth_provider_type(args: &Value) -> Result<Option<ProviderType>, ApiError> {
    web_optional_string_any(args, &["providerType", "provider_type", "authProvider"])
        .map(|value| web_parse_auth_provider_type(&value))
        .transpose()
}

fn web_auth_provider_type(args: &Value) -> Result<ProviderType, ApiError> {
    web_optional_auth_provider_type(args)?
        .ok_or_else(|| ApiError::bad_request("authProvider is required"))
}

fn web_parse_auth_provider_type(value: &str) -> Result<ProviderType, ApiError> {
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

fn web_parse_provider_type(value: &str) -> Result<ProviderType, ApiError> {
    serde_json::from_value(Value::String(value.trim().to_string()))
        .map_err(|_| ApiError::bad_request(format!("invalid providerType: {value}")))
}

fn provider_extra_string(provider: &Provider, key: &str) -> Option<String> {
    provider
        .extra
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn provider_record_for_app(
    providers: &[StoredProvider],
    app: AppKind,
) -> BTreeMap<String, Provider> {
    providers
        .iter()
        .filter(|provider| provider.app == app)
        .map(|provider| (provider.provider.id.clone(), provider.provider.clone()))
        .collect()
}

fn managed_auth_provider_label(provider_type: ProviderType) -> &'static str {
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

fn account_is_authenticated(account: &Account) -> bool {
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

fn account_authenticated_at(account: &Account) -> i64 {
    account.quota_refreshed_at.unwrap_or(0)
}

fn map_managed_auth_account(
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

fn managed_auth_is_cli_oauth_flow(oauth_flow_mode: Option<&str>) -> bool {
    matches!(
        oauth_flow_mode
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cli") | Some("browser") | Some("cli_oauth") | Some("clioauth")
    )
}

fn web_managed_auth_redirect_uri(
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

fn map_managed_auth_device_code(
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

fn map_managed_auth_browser_login(
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

async fn web_managed_auth_account_by_id(
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

async fn web_managed_auth_start_login(
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

async fn web_managed_auth_poll_for_account(
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
            let poll_status = {
                let mut store = state.oauth_logins.write().await;
                store.poll_state_by_oauth_state(poll_state, now_ms() as i64)
            };
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

async fn web_managed_auth_remove_account(
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
    delete_account(State(state), headers, Path(account_id)).await?;
    Ok(Value::Null)
}

async fn web_managed_auth_set_default_account(
    state: ServerState,
    headers: HeaderMap,
    args: &Value,
) -> Result<Value, ApiError> {
    require_session(&state, &headers).await?;
    let provider_type = web_auth_provider_type(args)?;
    let account_id = web_arg_string_any(args, &["accountId", "account_id"])?;
    let mut store = state.accounts.write().await;
    let Some(index) = store
        .accounts
        .iter()
        .position(|account| account.id == account_id && account.provider_type == provider_type)
    else {
        return Err(ApiError::not_found("account not found"));
    };
    let account = store.accounts.remove(index);
    let insert_at = store
        .accounts
        .iter()
        .position(|item| item.provider_type == provider_type)
        .unwrap_or(store.accounts.len());
    store.accounts.insert(insert_at, account);
    drop(store);
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Value::Null)
}

async fn web_managed_auth_logout(
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
        delete_account(State(state.clone()), headers.clone(), Path(account_id)).await?;
    }
    Ok(Value::Null)
}

fn web_arg_app_type(args: &Value) -> Result<AppKind, ApiError> {
    let app = web_arg_string_any(args, &["appType", "app", "app_type"])?;
    parse_app_kind(&app)
}

fn web_arg_app(args: &Value) -> Result<AppKind, ApiError> {
    web_arg_string(args, "app")
        .or_else(|_| web_arg_string(args, "appType"))
        .and_then(|value| parse_app_kind(&value))
}

fn web_arg_string(args: &Value, key: &str) -> Result<String, ApiError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::bad_request(format!("{key} is required")))
}

fn web_arg_value<T>(args: &Value, key: &str) -> Result<T, ApiError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| ApiError::bad_request(format!("{key} is required")))?;
    serde_json::from_value(value).map_err(ApiError::bad_request)
}

fn web_runtime_support_label(support: WebRuntimeCommandSupport) -> &'static str {
    match support {
        WebRuntimeCommandSupport::Native => "native",
        WebRuntimeCommandSupport::Shim => "shim",
        WebRuntimeCommandSupport::Excluded => "excluded",
    }
}

async fn proxy_claude_messages(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    proxy::forward(state, ProxyRoute::ClaudeMessages, None, headers, body)
        .await
        .map_err(ApiError::proxy)
}

async fn proxy_codex_chat_completions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    proxy::forward(state, ProxyRoute::CodexChatCompletions, None, headers, body)
        .await
        .map_err(ApiError::proxy)
}

async fn proxy_codex_responses(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    proxy::forward(state, ProxyRoute::CodexResponses, None, headers, body)
        .await
        .map_err(ApiError::proxy)
}

async fn proxy_gemini(
    method: Method,
    State(state): State<ServerState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    if method == Method::GET {
        if let Some(response) = gemini_models_response(&state, &headers, &path).await? {
            return Ok(response);
        }
    }
    proxy::forward(state, ProxyRoute::Gemini, Some(path), headers, body)
        .await
        .map_err(ApiError::proxy)
}

async fn gemini_models_response(
    state: &ServerState,
    headers: &HeaderMap,
    path: &str,
) -> Result<Option<Response>, ApiError> {
    let path = path.trim_matches('/');
    if path != "models" && !path.starts_with("models/") {
        return Ok(None);
    }
    let provider_id = headers
        .get("x-cc-provider-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let providers = state.providers.read().await;
    let models = openai_model_list(&providers.providers, Some(AppKind::Gemini), provider_id)
        .into_iter()
        .map(gemini_model_from_openai)
        .collect::<Vec<_>>();
    if path == "models" {
        return Ok(Some(Json(GeminiModelsResponse { models }).into_response()));
    }
    let requested = path.trim_start_matches("models/").trim();
    let requested_name = gemini_model_name(requested);
    let model = models
        .into_iter()
        .find(|model| model.name == requested_name || model.name == requested)
        .ok_or_else(|| ApiError::not_found("Gemini model not found"))?;
    Ok(Some(Json(model).into_response()))
}

fn gemini_model_from_openai(model: OpenAiModel) -> GeminiModel {
    let id = model.id.trim_start_matches("models/").to_string();
    GeminiModel {
        name: gemini_model_name(&id),
        version: "001".to_string(),
        display_name: id.clone(),
        description: format!("cc-switch provider model {id}"),
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        supported_generation_methods: vec![
            "generateContent".to_string(),
            "streamGenerateContent".to_string(),
        ],
    }
}

fn gemini_model_name(model_id: &str) -> String {
    if model_id.starts_with("models/") {
        model_id.to_string()
    } else {
        format!("models/{model_id}")
    }
}

fn openai_model_list(
    providers: &[StoredProvider],
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Vec<OpenAiModel> {
    let mut models = BTreeMap::<String, OpenAiModel>::new();
    for provider in providers.iter().filter(|provider| {
        app.is_none_or(|app| provider.app == app)
            && provider_id.is_none_or(|id| provider.provider.id == id)
    }) {
        let owned_by = model_owner(provider);
        for model_id in provider_model_ids(provider) {
            let key = format!("{model_id}\u{0}{owned_by}");
            models.entry(key).or_insert(OpenAiModel {
                id: model_id,
                object: "model",
                owned_by: owned_by.clone(),
            });
        }
    }
    models.into_values().collect()
}

fn model_owner(provider: &StoredProvider) -> String {
    let name = provider.provider.name.trim();
    if name.is_empty() {
        provider.provider.id.clone()
    } else {
        name.to_string()
    }
}

fn provider_model_ids(provider: &StoredProvider) -> Vec<String> {
    let settings = &provider.provider.settings_config;
    let mut models = Vec::new();
    push_model_catalog(
        settings
            .get("modelCatalog")
            .or_else(|| settings.get("model_catalog")),
        &mut models,
    );
    push_models_value(settings.get("models"), &mut models);
    push_model_mapping(
        settings
            .get("modelMapping")
            .or_else(|| settings.get("model_mapping")),
        &mut models,
    );
    for key in [
        "MODEL",
        "OPENAI_MODEL",
        "ANTHROPIC_MODEL",
        "CLAUDE_MODEL",
        "CODEX_MODEL",
        "GEMINI_MODEL",
    ] {
        if let Some(model) = settings_model_string(settings, key) {
            models.push(model);
        }
    }
    dedupe_non_empty(models)
}

fn push_model_catalog(catalog: Option<&Value>, models: &mut Vec<String>) {
    let Some(catalog) = catalog else {
        return;
    };
    push_models_value(catalog.get("models"), models);
}

fn push_models_value(value: Option<&Value>, models: &mut Vec<String>) {
    match value {
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(model) = model_id_from_value(item) {
                    models.push(model);
                }
            }
        }
        Some(value) => {
            if let Some(model) = model_id_from_value(value) {
                models.push(model);
            }
        }
        None => {}
    }
}

fn push_model_mapping(mapping: Option<&Value>, models: &mut Vec<String>) {
    let Some(Value::Object(map)) = mapping else {
        return;
    };
    if let Some(model) = map
        .get("upstreamModel")
        .or_else(|| map.get("upstream_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        models.push(model.to_string());
    }
    for (key, value) in map {
        if matches!(
            key.as_str(),
            "upstreamModel" | "upstream_model" | "rules" | "modelRules" | "model_rules"
        ) {
            continue;
        }
        if !key.trim().is_empty() {
            models.push(key.trim().to_string());
        }
        if let Some(model) = model_id_from_value(value) {
            models.push(model);
        }
    }
    for rules_key in ["rules", "modelRules", "model_rules"] {
        if let Some(Value::Array(rules)) = map.get(rules_key) {
            for rule in rules {
                if let Some(model) = string_field(
                    rule,
                    &["model", "requestModel", "request_model", "id", "name"],
                ) {
                    models.push(model);
                }
            }
        }
    }
}

fn model_id_from_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| string_field(value, &["id", "model", "name"]))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn settings_model_string(settings: &Value, key: &str) -> Option<String> {
    settings
        .pointer(&format!("/env/{key}"))
        .and_then(Value::as_str)
        .or_else(|| settings.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn dedupe_non_empty(values: Vec<String>) -> Vec<String> {
    let mut deduped = BTreeMap::<String, ()>::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            deduped.entry(value.to_string()).or_insert(());
        }
    }
    deduped.into_keys().collect()
}

async fn web_dist_missing() -> impl IntoResponse {
    web_dist_missing_response()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn clamp_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

fn require_share_router_probe(headers: &HeaderMap) -> Result<(), ApiError> {
    if truthy_header(headers, "x-share-router-probe") {
        Ok(())
    } else {
        Err(ApiError::not_found("not found"))
    }
}

fn require_share_router_health_check(headers: &HeaderMap) -> Result<(), ApiError> {
    if truthy_header(headers, "x-share-router-probe")
        || truthy_header(headers, "x-share-router-health-check")
    {
        Ok(())
    } else {
        Err(ApiError::not_found("not found"))
    }
}

fn truthy_header(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

async fn verify_control_request(
    state: &ServerState,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    let installation_id = required_header(headers, "x-ctl-installation-id")?;
    let timestamp_raw = required_header(headers, "x-ctl-timestamp-ms")?;
    let nonce = required_header(headers, "x-ctl-nonce")?;
    let signature_raw = required_header(headers, "x-ctl-signature")?;
    let timestamp_ms = timestamp_raw
        .parse::<i64>()
        .map_err(|_| ApiError::unauthorized("bad control timestamp"))?;
    let now = now_ms() as i64;
    let delta = if now >= timestamp_ms {
        now - timestamp_ms
    } else {
        timestamp_ms - now
    };
    if delta > CONTROL_SIGNATURE_WINDOW_MS {
        return Err(ApiError::unauthorized("stale control request"));
    }
    if nonce.trim().is_empty() {
        return Err(ApiError::unauthorized("missing control nonce"));
    }

    let config = state.config.read().await;
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("router identity is not registered"))?;
    if identity.installation_id != installation_id {
        return Err(ApiError::unauthorized("control installation mismatch"));
    }
    let secret = identity
        .control_secret
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("router control secret is unavailable"))?;
    let provided = BASE64_STANDARD
        .decode(signature_raw)
        .map_err(|_| ApiError::unauthorized("bad control signature"))?;
    let expected = control_signature(path, secret, body, timestamp_ms, nonce)?;
    if !constant_time_eq(&provided, &expected) {
        return Err(ApiError::unauthorized("bad control signature"));
    }
    if !state
        .control_nonces
        .register(installation_id, nonce, now, CONTROL_SIGNATURE_WINDOW_MS)
    {
        return Err(ApiError::unauthorized("replay control request"));
    }
    Ok(())
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::unauthorized(format!("missing {name}")))
}

fn control_signature(
    path: &str,
    secret: &str,
    body: &[u8],
    timestamp_ms: i64,
    nonce: &str,
) -> Result<Vec<u8>, ApiError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::unauthorized("bad control secret"))?;
    mac.update(b"POST\n");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(body);
    mac.update(b"\n");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn parse_app_kind(value: &str) -> Result<AppKind, ApiError> {
    parse_supported_app_kind(value).ok_or_else(|| ApiError::bad_request("invalid appType"))
}

fn parse_supported_app_kind(value: &str) -> Option<AppKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-desktop" => Some(AppKind::Claude),
        "codex" | "omo" | "omo_slim" => Some(AppKind::Codex),
        "gemini" => Some(AppKind::Gemini),
        "opencode" | "openclaw" | "hermes" => None,
        _ => None,
    }
}

fn web_arg_app_for_read(args: &Value) -> Result<Option<AppKind>, ApiError> {
    let app = web_arg_string_any(args, &["appType", "app", "app_type"])?;
    if parse_supported_app_kind(&app).is_none()
        && !matches!(
            app.trim().to_ascii_lowercase().as_str(),
            "opencode" | "openclaw" | "hermes"
        )
    {
        return Err(ApiError::bad_request("invalid appType"));
    }
    Ok(parse_supported_app_kind(&app))
}

fn web_arg_common_config_app_type(args: &Value) -> Result<&'static str, ApiError> {
    let app = web_arg_string_any(args, &["appType", "app", "app_type"])?;
    ui_settings::normalize_common_config_app_type(&app)
        .ok_or_else(|| ApiError::bad_request("invalid appType"))
}

async fn resolve_share_for_internal_request(
    state: &ServerState,
    share_id: Option<&str>,
) -> Result<Share, ApiError> {
    let shares = state.shares.read().await;
    if let Some(share_id) = share_id.map(str::trim).filter(|value| !value.is_empty()) {
        return shares
            .shares
            .iter()
            .find(|share| share.id == share_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("share not found: {share_id}")));
    }
    match shares.shares.as_slice() {
        [share] => Ok(share.clone()),
        [] => Err(ApiError::not_found("share not found")),
        _ => Err(ApiError::bad_request(
            "multiple shares present; router must specify ?shareId=",
        )),
    }
}

fn runtime_response_from_descriptor(
    descriptor: crate::core::router_client::ShareDescriptor,
) -> ShareRouterRuntimeResponse {
    ShareRouterRuntimeResponse {
        share_id: descriptor.share_id,
        queried_at: (now_ms() / 1000) as i64,
        token_limit: Some(descriptor.token_limit),
        tokens_used: Some(descriptor.tokens_used),
        requests_count: Some(descriptor.requests_count),
        share_status: Some(descriptor.share_status),
        support: descriptor.support,
        app_runtimes: descriptor.app_runtimes,
        app_providers: descriptor.app_providers,
        app_availability: descriptor.app_availability,
        model_health: descriptor.model_health,
    }
}

async fn refresh_share_usage_items(
    state: &ServerState,
    share: &Share,
    app: Option<&str>,
    providers: &crate::core::providers::ProviderStore,
) -> Vec<ControlRefreshShareUsageItem> {
    let requested_app = app
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let mut bindings = if share.bindings.is_empty() {
        vec![ShareBinding {
            app: share.app,
            provider_id: share.provider_id.clone(),
            provider_type: share.provider_type,
        }]
    } else {
        share.bindings.clone()
    };
    bindings.sort_by(|left, right| left.app.as_str().cmp(right.app.as_str()));
    let mut items = Vec::new();
    for binding in bindings.into_iter().filter(|binding| {
        requested_app
            .as_deref()
            .is_none_or(|app| binding.app.as_str() == app)
    }) {
        let provider = providers.providers.iter().find(|provider| {
            provider.app == binding.app && provider.provider.id == binding.provider_id
        });
        let Some(provider) = provider.cloned() else {
            items.push(ControlRefreshShareUsageItem {
                app: binding.app.as_str().to_string(),
                provider_id: Some(binding.provider_id),
                provider_name: None,
                auth_provider: None,
                account_id: None,
                refreshed: false,
                error: Some("provider not found".to_string()),
                message: None,
            });
            continue;
        };
        items.push(refresh_share_usage_item(state, binding.app, &provider).await);
    }
    items
}

async fn refresh_share_usage_item(
    state: &ServerState,
    app: AppKind,
    provider: &StoredProvider,
) -> ControlRefreshShareUsageItem {
    let account_id_hint = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    let mut account = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(provider.provider_type, account_id_hint)
            .cloned()
    };
    let provider_id = provider.provider.id.clone();
    let provider_name = Some(provider.provider.name.clone());
    let auth_provider = Some(provider.provider_type_id.clone());
    let Some(mut active_account) = account.take() else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: account_id_hint.map(str::to_string),
            refreshed: false,
            error: Some("account_not_found".to_string()),
            message: None,
        };
    };
    let account_id = active_account.id.clone();
    let now = now_ms() as i64;

    if account_needs_native_refresh(&active_account, now) {
        let Some(_refresh_guard) = state
            .account_refresh_locks
            .try_lock(active_account.provider_type, &active_account.id)
        else {
            return ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some("account_refresh_in_progress".to_string()),
                message: None,
            };
        };
        let latest_account = {
            let accounts = state.accounts.read().await;
            accounts
                .find_for_provider(provider.provider_type, Some(&active_account.id))
                .cloned()
        };
        let Some(latest_account) = latest_account else {
            return ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some("account_not_found".to_string()),
                message: None,
            };
        };
        active_account = latest_account;
        if account_needs_native_refresh(&active_account, now) {
            let http_client = state.http_client().await;
            match execute_native_account_refresh(&http_client, &active_account, now).await {
                Ok(update) => {
                    let updated = {
                        let mut accounts = state.accounts.write().await;
                        accounts.mark_refresh_success(&active_account.id, update)
                    };
                    if let Some(updated) = updated {
                        active_account = updated;
                    }
                    save_accounts_debounced(state);
                }
                Err(error) => {
                    {
                        let mut accounts = state.accounts.write().await;
                        accounts.mark_refresh_failure(&active_account.id, error.message.clone());
                    }
                    save_accounts_debounced(state);
                    return ControlRefreshShareUsageItem {
                        app: app.as_str().to_string(),
                        provider_id: Some(provider_id),
                        provider_name,
                        auth_provider,
                        account_id: Some(account_id),
                        refreshed: false,
                        error: Some(error.message),
                        message: None,
                    };
                }
            }
        }
    }

    let http_client = state.http_client().await;
    match refresh_account_quota(&http_client, &active_account, now, true).await {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let updated = {
                let mut accounts = state.accounts.write().await;
                accounts.mark_refresh_success(&active_account.id, update)
            };
            save_accounts_debounced(state);
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(
                    updated
                        .as_ref()
                        .map(|account| account.id.clone())
                        .unwrap_or(account_id),
                ),
                refreshed: updated.is_some(),
                error: updated.is_none().then(|| "account_not_found".to_string()),
                message: updated.map(|_| message),
            }
        }
        Ok(QuotaRefreshResult::SkippedCooldown { message, .. }) => ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some(message),
            message: None,
        },
        Err(error) => {
            mark_quota_refresh_error(state, &active_account.id, &error).await;
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some(error.message),
                message: None,
            }
        }
    }
}

async fn mark_quota_refresh_error(
    state: &ServerState,
    account_id: &str,
    error: &QuotaRefreshFailure,
) {
    {
        let mut accounts = state.accounts.write().await;
        accounts.mark_refresh_success(
            account_id,
            AccountRefreshUpdate {
                quota_next_refresh_at: error.next_refresh_at,
                last_refresh_error: Some(error.message.clone()),
                ..Default::default()
            },
        );
    }
    save_accounts_debounced(state);
}

fn default_account_login_redirect_uri(state: &ServerState) -> String {
    format!(
        "http://localhost:{}/api/accounts/login/callback",
        state.bind_addr.port()
    )
}

fn redact_oauth_login_finish(mut finish: OAuthLoginFinish) -> OAuthLoginFinish {
    if let Some(request) = finish.token_request.take() {
        finish.token_request = Some(redact_oauth_request(request));
    }
    finish
}

fn oauth_login_api_error(error: OAuthLoginError) -> ApiError {
    match error {
        OAuthLoginError::Unsupported(message) | OAuthLoginError::RequestShape(message) => {
            ApiError::not_implemented(message)
        }
        error @ (OAuthLoginError::MissingCode | OAuthLoginError::StateMismatch) => {
            ApiError::bad_request(error)
        }
        error @ (OAuthLoginError::Expired | OAuthLoginError::AlreadyConsumed) => {
            ApiError::conflict(error.to_string())
        }
        OAuthLoginError::NotFound => ApiError::not_found(error.to_string()),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    config_dir: String,
    web_dist_dir: Option<String>,
    embedded_web_assets: usize,
    unix_ms: u128,
}

type VersionResponse = BuildInfo;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterHealthResponse {
    ok: bool,
    status: String,
    timestamp_ms: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterRequestLogsQuery {
    #[serde(default)]
    share_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterRequestLogsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_id: Option<String>,
    logs: Vec<ShareRequestLogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterRuntimeQuery {
    #[serde(default)]
    share_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterRuntimeResponse {
    share_id: String,
    queried_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens_used: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requests_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_status: Option<String>,
    support: ShareSupport,
    app_runtimes: ShareAppRuntimes,
    app_providers: ShareAppProviders,
    app_availability: ShareAppAvailability,
    model_health: crate::core::model_health::ShareModelHealthSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterModelHealthRequest {
    app_type: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterModelHealthResponse {
    ok: bool,
    success: bool,
    status: String,
    message: String,
    status_code: Option<u16>,
    model_used: String,
    response_time_ms: Option<u64>,
    tested_at: i64,
    retry_count: u32,
    error_category: Option<String>,
    provider_id: String,
    provider_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlApplyShareSettingsInput {
    share_id: String,
    patch: ShareSettingsPatch,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlApplyShareSettingsResponse {
    ok: bool,
    share: crate::core::router_client::ShareDescriptor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRefreshShareUsageInput {
    share_id: String,
    #[serde(default)]
    app: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlRefreshShareUsageItem {
    app: String,
    provider_id: Option<String>,
    provider_name: Option<String>,
    auth_provider: Option<String>,
    account_id: Option<String>,
    refreshed: bool,
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlRefreshShareUsageResponse {
    ok: bool,
    refreshed: Vec<ControlRefreshShareUsageItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupStatusResponse {
    ok: bool,
    needs_setup: bool,
    owner_email: Option<String>,
    router_url: Option<String>,
    client_tunnel_subdomain: Option<String>,
}

impl SetupStatusResponse {
    fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            needs_setup: !config.is_setup_complete(),
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupResponse {
    ok: bool,
    owner_email: Option<String>,
    router_url: Option<String>,
    client_tunnel_subdomain: Option<String>,
    message: &'static str,
}

impl SetupResponse {
    fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            message: "setup complete; use password login to enter cc-switch-server",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest {
    #[serde(default = "default_password_method")]
    method: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    api_token: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangePasswordRequest {
    new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangePasswordResponse {
    ok: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailLoginCodeRequest {
    email: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailLoginVerifyCodeRequest {
    email: String,
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebPasswordRequest {
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSessionRefreshRequest {
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebPasswordChangeRequest {
    current_password: String,
    new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    ok: bool,
    token: String,
    token_type: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiTokenResponse {
    ok: bool,
    api_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthMeResponse {
    ok: bool,
    owner_email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventQuery {
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBackupRequest {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupListResponse {
    ok: bool,
    backups: Vec<crate::core::backup::BackupManifest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupCreateResponse {
    ok: bool,
    backup: crate::core::backup::BackupManifest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupRestoreResponse {
    ok: bool,
    result: crate::core::backup::BackupRestoreResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSnapshotResponse {
    ok: bool,
    owner_email: Option<String>,
    router_url: Option<String>,
    client_tunnel_subdomain: Option<String>,
    upstream_proxy: UpstreamProxyView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamProxyResponse {
    ok: bool,
    upstream_proxy: UpstreamProxyView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamProxyView {
    enabled: bool,
    url: Option<String>,
    masked_url: Option<String>,
    follow_system_proxy: bool,
}

impl UpstreamProxyView {
    fn from_config(config: &ServerConfig) -> Self {
        let url = config.upstream_proxy.url.clone();
        Self {
            enabled: url.as_deref().is_some_and(|value| !value.trim().is_empty()),
            masked_url: url.as_deref().map(mask_proxy_url),
            url,
            follow_system_proxy: config.upstream_proxy.follow_system_proxy,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterConfigResponse {
    ok: bool,
    router: RouterConfigView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterConfigView {
    url: Option<String>,
    api_base: Option<String>,
    domain: Option<String>,
    region: Option<String>,
    ssh_host: Option<String>,
    ssh_user: Option<String>,
    custom: bool,
    installation_id: Option<String>,
    public_key: Option<String>,
    control_secret_present: bool,
    last_register_error: Option<String>,
    last_registered_at_ms: Option<i64>,
}

impl RouterConfigView {
    fn from_config(config: &RouterConfig) -> Self {
        Self {
            url: config.url.clone(),
            api_base: config.api_base.clone(),
            domain: config.domain.clone(),
            region: config.region.clone(),
            ssh_host: config.ssh_host.clone(),
            ssh_user: config.ssh_user.clone(),
            custom: config.custom,
            installation_id: config
                .identity
                .as_ref()
                .map(|identity| identity.installation_id.clone()),
            public_key: config
                .identity
                .as_ref()
                .map(|identity| identity.public_key.clone()),
            control_secret_present: config
                .identity
                .as_ref()
                .and_then(|identity| identity.control_secret.as_ref())
                .is_some(),
            last_register_error: config.last_register_error.clone(),
            last_registered_at_ms: config.last_registered_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterRegisterResponse {
    ok: bool,
    registration: RouterRegisterResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelResponse {
    ok: bool,
    tunnel_subdomain: Option<String>,
    tunnel_status: Option<String>,
    last_heartbeat_ms: Option<u128>,
    runtime_status: Option<crate::core::tunnel::TunnelRuntimeStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelClaimResponse {
    ok: bool,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelLeaseResponse {
    ok: bool,
    status: Option<crate::core::tunnel::TunnelRuntimeStatus>,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterTunnelsResponse {
    ok: bool,
    tunnels: Vec<crate::core::tunnel::TunnelRuntimeStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProvidersQuery {
    #[serde(default)]
    app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListProvidersResponse {
    ok: bool,
    providers: Vec<StoredProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderHealthResponse {
    ok: bool,
    providers: Vec<ProviderHealth>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FailoverResponse {
    ok: bool,
    failover: FailoverSnapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFailoverAppResponse {
    ok: bool,
    app: AppKind,
    config: FailoverAppConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FailoverProviderResetQuery {
    #[serde(default)]
    app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetFailoverProviderResponse {
    ok: bool,
    breaker: crate::core::failover::ProviderBreaker,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProviderRequest {
    app: AppKind,
    provider: Provider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateProviderResponse {
    ok: bool,
    stored: StoredProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProvidersResponse {
    ok: bool,
    providers: Vec<StoredProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportProviderItem {
    app: AppKind,
    provider: Provider,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportProvidersRequest {
    providers: Vec<ImportProviderItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProvidersResponse {
    ok: bool,
    imported: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchProviderModelsRequest {
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    merge: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchedProviderModel {
    id: String,
    upstream_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    raw: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchProviderModelsResponse {
    ok: bool,
    provider_id: String,
    app: AppKind,
    provider_type: ProviderType,
    url: String,
    merged: bool,
    merged_count: usize,
    models: Vec<FetchedProviderModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<StoredProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListUniversalProvidersResponse {
    ok: bool,
    providers: BTreeMap<String, UniversalProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UniversalProviderPresetsResponse {
    ok: bool,
    presets: Vec<UniversalProviderPreset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportUniversalProvidersResponse {
    ok: bool,
    providers: Vec<UniversalProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportUniversalProvidersRequest {
    providers: Vec<UniversalProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportUniversalProvidersResponse {
    ok: bool,
    imported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetUniversalProviderResponse {
    ok: bool,
    provider: Option<UniversalProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertUniversalProviderRequest {
    provider: UniversalProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpsertUniversalProviderResponse {
    ok: bool,
    provider: UniversalProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncUniversalProviderResponse {
    ok: bool,
    result: UniversalProviderSyncResult,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestProviderQuery {
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    network: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestProvidersRequest {
    #[serde(default)]
    provider_ids: Option<Vec<String>>,
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    network: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestProvidersResponse {
    ok: bool,
    results: Vec<TestProviderResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TestProviderResponse {
    ok: bool,
    provider_id: String,
    app: AppKind,
    provider_type: crate::core::provider::ProviderType,
    adapter: &'static str,
    support: proxy::adapters::AdapterSupport,
    endpoint: String,
    model: String,
    stream: bool,
    header_names: Vec<String>,
    network_checked: bool,
    network_status_code: Option<u16>,
    network_latency_ms: Option<u128>,
    network_stream_completed: Option<bool>,
    network_error: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProviderFromPresetRequest {
    app: AppKind,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPresetsQuery {
    #[serde(default)]
    app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPresetsResponse {
    ok: bool,
    presets: Vec<crate::coverage::PresetSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListAccountsResponse {
    ok: bool,
    accounts: Vec<Account>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpsertAccountResponse {
    ok: bool,
    account: Account,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountCapabilitiesResponse {
    ok: bool,
    capabilities: Vec<crate::core::account_managers::AccountManagerCapability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountImportTemplatesResponse {
    ok: bool,
    templates: Vec<crate::core::account_managers::AccountImportTemplate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartAccountLoginRequest {
    provider_type: crate::core::provider::ProviderType,
    #[serde(default)]
    redirect_uri: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartAccountLoginResponse {
    ok: bool,
    login: OAuthLoginStart,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartCopilotDeviceLoginRequest {
    #[serde(default)]
    github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartCopilotDeviceLoginResponse {
    ok: bool,
    device: crate::core::copilot_device::GitHubDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollCopilotDeviceLoginRequest {
    device_code: String,
    #[serde(default)]
    github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PollCopilotDeviceLoginResponse {
    ok: bool,
    pending: bool,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartKiroDeviceLoginRequest {
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    start_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartKiroDeviceLoginResponse {
    ok: bool,
    device: crate::core::kiro_device::KiroDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollKiroDeviceLoginRequest {
    device_code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PollKiroDeviceLoginResponse {
    ok: bool,
    pending: bool,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountLoginCallbackQuery {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, alias = "error_description")]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinishAccountLoginRequest {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    execute_token_exchange: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FinishAccountLoginResponse {
    ok: bool,
    login: OAuthLoginFinish,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountLoginAccountSummary {
    id: String,
    provider_type: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    has_access_token: bool,
    has_refresh_token: bool,
    scopes: Vec<String>,
}

impl AccountLoginAccountSummary {
    fn from_account(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            provider_type: account.provider_type,
            email: account.email.clone(),
            subscription_level: account.subscription_level.clone(),
            expires_at: account.expires_at,
            has_access_token: account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            has_refresh_token: account
                .refresh_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            scopes: account.scopes.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountQuotaResponse {
    ok: bool,
    quota: Option<AccountQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<Account>,
    refreshed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_refresh_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountQuotaQuery {
    #[serde(default)]
    refresh: Option<bool>,
    #[serde(default)]
    force: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountRefreshPlanResponse {
    ok: bool,
    account_id: String,
    provider_type: crate::core::provider::ProviderType,
    refresh_required: bool,
    server_native_stage: Option<OAuthSupportStage>,
    quota_strategy: Option<OAuthQuotaStrategy>,
    refresh_request: Option<OAuthHttpRequest>,
    profile_request: Option<OAuthHttpRequest>,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResponse {
    ok: bool,
    deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageLogsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    from_ms: Option<u128>,
    #[serde(default)]
    to_ms: Option<u128>,
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    share_id: Option<String>,
    #[serde(default)]
    user_email: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    data_source: Option<String>,
    #[serde(default)]
    is_health_check: Option<bool>,
    #[serde(default)]
    stream_status: Option<String>,
}

impl From<UsageLogsQuery> for UsageLogFilter {
    fn from(query: UsageLogsQuery) -> Self {
        Self {
            limit: query.limit,
            from_ms: query.from_ms,
            to_ms: query.to_ms,
            app: query.app,
            provider_id: query.provider_id,
            share_id: query.share_id,
            user_email: query.user_email,
            session_id: query.session_id,
            data_source: query.data_source,
            is_health_check: query.is_health_check,
            stream_status: query.stream_status,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageStatsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    from_ms: Option<u128>,
    #[serde(default)]
    to_ms: Option<u128>,
    #[serde(default)]
    window_ms: Option<u128>,
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    share_id: Option<String>,
    #[serde(default)]
    user_email: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    data_source: Option<String>,
    #[serde(default)]
    is_health_check: Option<bool>,
    #[serde(default)]
    stream_status: Option<String>,
}

impl From<UsageStatsQuery> for UsageStatsFilter {
    fn from(query: UsageStatsQuery) -> Self {
        Self {
            limit: query.limit,
            from_ms: query.from_ms,
            to_ms: query.to_ms,
            window_ms: query.window_ms,
            app: query.app,
            provider_id: query.provider_id,
            share_id: query.share_id,
            user_email: query.user_email,
            session_id: query.session_id,
            data_source: query.data_source,
            is_health_check: query.is_health_check,
            stream_status: query.stream_status,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageLogsResponse {
    ok: bool,
    logs: Vec<UsageLog>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageLogDetailResponse {
    ok: bool,
    log: UsageLog,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummaryResponse {
    ok: bool,
    summary: UsageRollup,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTrendsResponse {
    ok: bool,
    trends: Vec<UsageTrendPoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageProviderStatsResponse {
    ok: bool,
    providers: Vec<ProviderUsageStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageModelStatsResponse {
    ok: bool,
    models: Vec<ModelUsageStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageBackfillResponse {
    ok: bool,
    updated: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageRouterSyncRetryResponse {
    ok: bool,
    attempted: usize,
    synced: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelPricingListResponse {
    ok: bool,
    models: Vec<ModelPricingEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelPricingUpdateResponse {
    ok: bool,
    model: ModelPricingEntry,
    backfilled: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelPricingDeleteResponse {
    ok: bool,
    deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderLimitsQuery {
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderLimitsResponse {
    ok: bool,
    limits: Vec<ProviderLimitStatusView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderLimitResponse {
    ok: bool,
    limit: ProviderLimitStatusView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderLimitStatusView {
    app: AppKind,
    provider_id: String,
    provider_name: String,
    provider_type: ProviderType,
    daily_usage_usd: f64,
    daily_limit_usd: Option<f64>,
    daily_exceeded: bool,
    monthly_usage_usd: f64,
    monthly_limit_usd: Option<f64>,
    monthly_exceeded: bool,
    account_id: Option<String>,
    account_email: Option<String>,
    account_quota_percent: Option<f64>,
    account_quota_refreshed_at: Option<i64>,
    account_last_refresh_error: Option<String>,
    quota_dispatch_limit_percent: Option<f64>,
    quota_dispatch_exceeded: bool,
    shares: Vec<ShareLimitStatusView>,
    warnings: Vec<String>,
    blocked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareLimitStatusView {
    share_id: String,
    share_name: String,
    status: String,
    enabled: bool,
    token_limit: Option<u64>,
    tokens_used: u64,
    parallel_limit: Option<u32>,
    expires_at: Option<i64>,
    token_exceeded: bool,
    expired: bool,
    blocked: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListSharesResponse {
    ok: bool,
    shares: Vec<Share>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSharesRequest {
    shares: Vec<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportSharesResponse {
    ok: bool,
    imported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpsertShareResponse {
    ok: bool,
    share: Share,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectInfoResponse {
    ok: bool,
    share_id: String,
    direct_url: String,
    subdomain: String,
    router_domain: String,
    snippets: Vec<ShareConnectSnippet>,
    note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareConnectSnippet {
    app: AppKind,
    title: String,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShareSubdomainRequest {
    subdomain: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShareSubdomainResponse {
    ok: bool,
    remote_claimed: bool,
    share: Share,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaceShareAclRequest {
    acl: ShareAcl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShareBindingRequest {
    binding: ShareBinding,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShareMarketGrantRequest {
    market_grant: Option<ShareMarketGrantStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicShareMarket {
    id: String,
    display_name: String,
    email: String,
    subdomain: String,
    public_base_url: Option<String>,
    market_kind: String,
    status: String,
    #[serde(default)]
    scopes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListShareMarketsResponse {
    #[serde(default)]
    ok: bool,
    markets: Vec<PublicShareMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorizeShareMarketRequest {
    market_email: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterStatusResponse {
    ok: bool,
    registered: bool,
    last_error: Option<String>,
    last_heartbeat_ms: Option<u128>,
    pending_request_log_sync: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterDiagnosticsResponse {
    ok: bool,
    router: RouterConfigView,
    registered: bool,
    last_error: Option<String>,
    last_heartbeat_ms: Option<u128>,
    pending_request_log_sync: usize,
    tunnels: Vec<crate::core::tunnel::TunnelRuntimeStatus>,
    share_sync: Vec<ShareSyncDiagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareSyncDiagnostic {
    share_id: String,
    share_name: String,
    status: String,
    enabled: bool,
    router_last_synced_at_ms: Option<u128>,
    router_last_sync_error: Option<String>,
    router_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterBatchSyncResponse {
    ok: bool,
    synced: usize,
    remote_synced: bool,
    message: String,
    shares: Vec<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterShareEditPullResponse {
    ok: bool,
    summary: crate::state::ShareEditSyncSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouterDeleteAllSharesResponse {
    ok: bool,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyCapabilitiesResponse {
    ok: bool,
    capabilities: Vec<proxy::adapters::AdapterCapability>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsQuery {
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default)]
    provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiModelsResponse {
    object: &'static str,
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiModel {
    id: String,
    object: &'static str,
    owned_by: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiModelsResponse {
    models: Vec<GeminiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiModel {
    name: String,
    version: String,
    display_name: String,
    description: String,
    input_token_limit: u32,
    output_token_limit: u32,
    supported_generation_methods: Vec<String>,
}

impl ConfigSnapshotResponse {
    fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            upstream_proxy: UpstreamProxyView::from_config(config),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    ok: bool,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    error_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
    error_type: Option<&'static str>,
    retryable: Option<bool>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            error_type: None,
            retryable: None,
        }
    }

    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error.to_string())
    }

    fn unauthorized(error: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error.into())
    }

    fn forbidden(error: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, error.into())
    }

    fn conflict(error: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, error.into())
    }

    fn not_implemented(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, error.to_string())
    }

    fn feature_disabled(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: error.into(),
            code: Some("cc_switch_feature_disabled"),
            error_type: Some("feature_disabled"),
            retryable: Some(false),
        }
    }

    fn web_invoke_unknown(command: impl Into<String>) -> Self {
        let command = command.into();
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: format!(
                "desktop invoke command '{command}' is not registered in cc-switch-server"
            ),
            code: Some("cc_switch_web_invoke_unknown"),
            error_type: Some("web_invoke_unknown"),
            retryable: Some(false),
        }
    }

    fn web_invoke_not_wired(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: error.into(),
            code: Some("cc_switch_web_invoke_not_wired"),
            error_type: Some("web_invoke_not_wired"),
            retryable: Some(false),
        }
    }

    fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, error.to_string())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!("internal api error: {error}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }

    fn not_found(error: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, error.into())
    }

    fn proxy(error: proxy::ProxyError) -> Self {
        let code = error.error_code();
        let error_type = error.error_type();
        let retryable = error.retryable();
        Self {
            status: error.status,
            message: error.message,
            code: Some(code),
            error_type: Some(error_type),
            retryable: Some(retryable),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                ok: false,
                error: self.message,
                code: self.code,
                error_type: self.error_type,
                status: Some(self.status.as_u16()),
                retryable: self.retryable,
            }),
        )
            .into_response()
    }
}

fn map_email_auth_error(error: EmailAuthError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

fn map_web_auth_error(error: crate::core::web_auth::WebAuthError) -> ApiError {
    let message = error.to_string();
    if message.contains("invalid password")
        || message.contains("not configured")
        || message.contains("not found")
        || message.contains("expired")
        || message.contains("too many")
    {
        ApiError::unauthorized(message)
    } else {
        ApiError::bad_request(message)
    }
}

fn map_share_patch_error(error: crate::core::shares::SharePatchError) -> ApiError {
    match error {
        crate::core::shares::SharePatchError::NotFound => ApiError::not_found("share not found"),
        crate::core::shares::SharePatchError::Invalid(message) => ApiError::bad_request(message),
    }
}

fn map_copilot_device_error(error: CopilotDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

fn map_kiro_device_error(error: KiroDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

async fn require_session(state: &ServerState, headers: &HeaderMap) -> Result<(), ApiError> {
    require_web_admin_session(state, headers).await.map(|_| ())
}

async fn require_web_admin_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<WebAdminPrincipal, ApiError> {
    resolve_web_admin_principal(state, headers)
        .await?
        .ok_or_else(|| ApiError::unauthorized("missing or invalid bearer token"))
}

async fn resolve_web_admin_principal(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<WebAdminPrincipal>, ApiError> {
    if let Some(user_email) = router_web_user_email(headers) {
        return Ok(Some(WebAdminPrincipal {
            user_email,
            role: router_web_role(headers),
        }));
    }

    let Some(token) = bearer_token(headers) else {
        return Ok(None);
    };

    if let Ok(Some(principal)) = state.web_auth.authenticate_access_token(token) {
        return Ok(Some(WebAdminPrincipal {
            user_email: principal.user_email,
            role: principal.role,
        }));
    }

    if state
        .sessions
        .read()
        .await
        .iter()
        .any(|session| session.token == token)
    {
        let config = state.config.read().await;
        return Ok(Some(WebAdminPrincipal {
            user_email: config
                .owner
                .email
                .clone()
                .unwrap_or_else(|| "local-admin@cc-switch.local".to_string()),
            role: "admin".to_string(),
        }));
    }

    let config = state.config.read().await;
    if config.verify_api_token(token) {
        return Ok(Some(WebAdminPrincipal {
            user_email: config
                .owner
                .email
                .clone()
                .unwrap_or_else(|| "local-admin@cc-switch.local".to_string()),
            role: "admin".to_string(),
        }));
    }

    Ok(None)
}

fn router_web_user_email(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cc-switch-web-user-email")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn router_web_role(headers: &HeaderMap) -> String {
    headers
        .get("x-cc-switch-web-role")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("owner")
        .to_string()
}

#[derive(Debug, Clone)]
struct WebAdminPrincipal {
    user_email: String,
    role: String,
}

async fn require_event_session(
    state: &ServerState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<(), ApiError> {
    if resolve_web_admin_principal(state, headers).await?.is_some() {
        return Ok(());
    }
    if let Some(token) = bearer_token(headers).or(query_token) {
        return require_session_token(state, token).await;
    }
    Err(ApiError::unauthorized("missing bearer token"))
}

async fn require_session_token(state: &ServerState, token: &str) -> Result<(), ApiError> {
    if state
        .sessions
        .read()
        .await
        .iter()
        .any(|session| session.token == token)
    {
        Ok(())
    } else {
        Err(ApiError::unauthorized("invalid bearer token"))
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn default_password_method() -> String {
    "password".to_string()
}

fn provider_test_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.filter(|value| *value > 0).unwrap_or(15_000))
}

fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_share_market_grant(
    mut grant: ShareMarketGrantStatus,
) -> Result<ShareMarketGrantStatus, ApiError> {
    let status = grant.status.trim().to_ascii_lowercase();
    if !matches!(status.as_str(), "pending" | "applied" | "error") {
        return Err(ApiError::bad_request(
            "marketGrant.status must be pending, applied, or error",
        ));
    }
    grant.status = status;
    if grant
        .grant_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        grant.grant_id = None;
    }
    if grant
        .last_error
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        grant.last_error = None;
    }
    if grant.updated_at_ms.is_none() {
        grant.updated_at_ms = Some(now_ms());
    }
    Ok(grant)
}

fn fixtures_for_app(
    coverage: &ProviderCoverage,
    app: AppKind,
) -> Vec<crate::coverage::ProviderFixture> {
    match app {
        AppKind::Claude => coverage.fixtures.claude.clone(),
        AppKind::Codex => coverage.fixtures.codex.clone(),
        AppKind::Gemini => coverage.fixtures.gemini.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{to_bytes, Body};
    use axum::http::{HeaderValue, Method, Request};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use super::*;
    use crate::cli::Cli;
    use crate::core::provider::ProviderType;
    use crate::state::ServerStateInner;

    #[test]
    fn bearer_token_accepts_only_bearer_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token-1"),
        );
        assert_eq!(bearer_token(&headers), Some("token-1"));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic token-1"),
        );
        assert_eq!(bearer_token(&headers), None);
    }

    #[test]
    fn provider_test_body_prefers_test_config_model() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "provider".to_string(),
                settings_config: json!({
                    "testConfig": {"model": "test-model"},
                    "modelMapping": {"upstreamModel": "mapped-model"}
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: crate::core::provider::ProviderType::Codex,
            provider_type_id: "codex".to_string(),
        };

        let body = provider_test_body(AppKind::Codex, &stored, None, false);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("test-model")
        );
        assert_eq!(
            value.get("stream").and_then(serde_json::Value::as_bool),
            Some(false)
        );

        let stream_body = provider_test_body(AppKind::Codex, &stored, Some("override-model"), true);
        let stream_value: serde_json::Value = serde_json::from_str(&stream_body).unwrap();
        assert_eq!(
            stream_value
                .get("model")
                .and_then(serde_json::Value::as_str),
            Some("override-model")
        );
        assert_eq!(
            stream_value
                .get("stream")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn gemini_model_response_wraps_openai_model_id() {
        let model = gemini_model_from_openai(OpenAiModel {
            id: "gemini-2.5-pro".to_string(),
            object: "model",
            owned_by: "gemini".to_string(),
        });

        assert_eq!(model.name, "models/gemini-2.5-pro");
        assert!(model
            .supported_generation_methods
            .contains(&"generateContent".to_string()));
        assert!(model
            .supported_generation_methods
            .contains(&"streamGenerateContent".to_string()));
        assert_eq!(
            gemini_model_name("models/gemini-2.5-pro"),
            "models/gemini-2.5-pro"
        );
    }

    #[test]
    fn provider_test_error_redaction_removes_common_secret_shapes() {
        let redacted = redact_provider_test_error(
            r#"{"error":"bad sk-abc1234567890 and ya29.secret-token and Bearer abc.def"}"#,
        );

        assert!(!redacted.contains("sk-abc"));
        assert!(!redacted.contains("ya29.secret"));
        assert!(!redacted.contains("Bearer abc"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn proxy_api_error_response_includes_stable_code_and_type() {
        let response =
            ApiError::proxy(proxy::ProxyError::bad_gateway("connection refused")).into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = json_body(response).await;

        assert_eq!(body["ok"].as_bool(), Some(false));
        assert_eq!(body["code"].as_str(), Some("cc_switch_forward_failed"));
        assert_eq!(body["type"].as_str(), Some("upstream_error"));
        assert_eq!(body["status"].as_u64(), Some(502));
        assert_eq!(body["retryable"].as_bool(), Some(true));
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("connection refused"));
    }

    #[test]
    fn oauth_request_redaction_removes_authorization_codes_and_verifiers() {
        let request = OAuthHttpRequest {
            method: "POST",
            url: "https://api2.cursor.sh/auth/poll?uuid=session&verifier=secret-verifier"
                .to_string(),
            headers: vec![(
                "Authorization".to_string(),
                "Bearer access-token".to_string(),
            )],
            body: json!({
                "code": "auth-code",
                "code_verifier": "secret-code-verifier",
                "client_secret": "secret-client",
                "nested": {"refresh_token": "refresh-token"}
            }),
            body_format: crate::core::oauth_clients::OAuthRequestBodyFormat::Json,
        };

        let redacted = redact_oauth_request(request);
        let serialized = serde_json::to_string(&redacted).unwrap();

        assert!(!serialized.contains("auth-code"));
        assert!(!serialized.contains("secret-code-verifier"));
        assert!(!serialized.contains("secret-client"));
        assert!(!serialized.contains("refresh-token"));
        assert!(!serialized.contains("secret-verifier"));
        assert!(serialized.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn share_router_health_is_hidden_without_probe_header() {
        let state = test_state();
        let app = app_router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/_share-router/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/_share-router/health")
                    .header("X-Share-Router-Probe", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["ok"].as_bool(), Some(true));
        assert_eq!(body["status"].as_str(), Some("healthy"));
    }

    #[tokio::test]
    async fn control_apply_share_settings_rejects_replayed_nonce() {
        let state = test_state();
        state.config.write().await.router.identity = Some(crate::core::config::RouterIdentity {
            installation_id: "inst-ctl".to_string(),
            public_key: "public-key".to_string(),
            private_key: "private-key".to_string(),
            control_secret: Some("control-secret".to_string()),
        });
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "p-ctl".to_string(),
                name: "Control Provider".to_string(),
                settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );
        state.shares.write().await.upsert(test_share_input(
            "share-ctl",
            "p-ctl",
            ProviderType::Codex,
        ));
        let app = app_router(state);
        let body = serde_json::to_vec(&json!({
            "shareId": "share-ctl",
            "patch": {"description": "updated by control"}
        }))
        .unwrap();
        let timestamp_ms = now_ms() as i64;
        let signature = BASE64_STANDARD.encode(
            control_signature(
                APPLY_SHARE_SETTINGS_PATH,
                "control-secret",
                &body,
                timestamp_ms,
                "nonce-ctl",
            )
            .unwrap(),
        );

        let response = app
            .clone()
            .oneshot(control_request(
                APPLY_SHARE_SETTINGS_PATH,
                body.clone(),
                timestamp_ms,
                "nonce-ctl",
                &signature,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(control_request(
                APPLY_SHARE_SETTINGS_PATH,
                body,
                timestamp_ms,
                "nonce-ctl",
                &signature,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = json_body(response).await;
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("replay control request"));
    }

    #[tokio::test]
    async fn control_refresh_share_usage_reports_bound_account_snapshot() {
        let state = test_state();
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "p-refresh".to_string(),
                name: "Refresh Provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(crate::core::provider::ProviderMeta {
                    auth_binding: Some(crate::core::provider::AuthBinding {
                        source: Some("managed_account".to_string()),
                        auth_provider: Some("cursor_oauth".to_string()),
                        account_id: Some("acct-cursor".to_string()),
                    }),
                    provider_type: Some("cursor_oauth".to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
        );
        state.accounts.write().await.upsert(UpsertAccountInput {
            id: Some("acct-cursor".to_string()),
            provider_type: ProviderType::CursorOAuth,
            email: Some("cursor@example.com".to_string()),
            access_token: None,
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: Some(json!({
                "billingOrQuotaSnapshot": {
                    "stripeStatus": {"membershipType": "pro_plus"},
                    "currentPeriodUsage": {
                        "billingCycleEnd": 1774000000000i64,
                        "planUsage": {
                            "limit": 2000.0,
                            "used": 500.0,
                            "totalPercentUsed": 25.0
                        }
                    }
                }
            })),
            subscription_level: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            last_refresh_error: None,
        });
        let share = {
            let mut input =
                test_share_input("share-refresh", "p-refresh", ProviderType::CursorOAuth);
            input.bindings = vec![ShareBinding {
                app: AppKind::Codex,
                provider_id: "p-refresh".to_string(),
                provider_type: ProviderType::CursorOAuth,
            }];
            input
        };
        let share = state.shares.write().await.upsert(share);
        let providers = state.providers.read().await.clone();

        let refreshed = refresh_share_usage_items(&state, &share, Some("codex"), &providers).await;

        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].account_id.as_deref(), Some("acct-cursor"));
        assert!(refreshed[0].refreshed);
        assert!(refreshed[0].error.is_none());
        let account = state
            .accounts
            .read()
            .await
            .find_for_provider(ProviderType::CursorOAuth, Some("acct-cursor"))
            .cloned()
            .unwrap();
        assert_eq!(account.quota_percent, Some(25.0));
    }

    #[tokio::test]
    async fn auth_routes_cover_password_api_token_and_email_paths() {
        let state = test_state();
        let app = app_router(state.clone());

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "password", "password": "password123"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = app
            .clone()
            .oneshot(json_request(Method::GET, "/api/config", json!(null), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/setup",
                json!({
                    "password": "password123",
                    "ownerEmail": "owner@example.com",
                    "routerUrl": "http://127.0.0.1:9",
                    "clientTunnelSubdomain": "ownerabcde"
                }),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "password", "password": "bad"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "password", "password": "password123"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let token = json_body(response).await["token"]
            .as_str()
            .unwrap()
            .to_string();

        let response = app
            .clone()
            .oneshot(json_request(
                Method::GET,
                "/api/auth/me",
                json!(null),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/api-token",
                json!(null),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let api_token = json_body(response).await["apiToken"]
            .as_str()
            .unwrap()
            .to_string();

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "api_token", "apiToken": api_token}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "api_token", "apiToken": "bad"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "email", "email": "owner@example.com"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "email", "email": "OWNER@example.com", "code": "123456"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn share_market_grant_route_updates_snapshot_and_can_clear_status() {
        let state = test_state();
        let app = app_router(state.clone());
        let token = setup_and_login(&app).await;

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/shares",
                json!({
                    "id": "share-grant",
                    "app": "codex",
                    "providerId": "p1",
                    "providerType": "codex",
                    "displayName": "Grant Test"
                }),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/shares/share-grant/market-grant",
                json!({
                    "marketGrant": {
                        "status": "Applied",
                        "grantId": "grant-1",
                        "lastError": ""
                    }
                }),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;

        assert_eq!(body["share"]["marketGrant"]["status"], "applied");
        assert_eq!(body["share"]["marketGrant"]["grantId"], "grant-1");
        assert!(body["share"]["marketGrant"]["lastError"].is_null());
        assert!(body["share"]["marketGrant"]["updatedAtMs"].is_u64());
        assert_eq!(
            body["share"]["runtimeSnapshot"]["marketGrant"]["status"],
            "applied"
        );

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/shares/share-grant/market-grant",
                json!({"marketGrant": {"status": "unknown"}}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/shares/share-grant/market-grant",
                json!({"marketGrant": null}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert!(body["share"]["marketGrant"].is_null());
        assert!(body["share"]["runtimeSnapshot"]["marketGrant"].is_null());
    }

    #[tokio::test]
    async fn provider_network_test_reports_redacted_upstream_4xx_body() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    r#"{"error":"bad sk-abc1234567890 Bearer abc.def"}"#,
                )
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = test_state();
        let app = app_router(state.clone());

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/setup",
                json!({
                    "password": "password123",
                    "ownerEmail": "owner@example.com",
                    "routerUrl": "http://127.0.0.1:9",
                    "clientTunnelSubdomain": "ownerabcde"
                }),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "password", "password": "password123"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let token = json_body(response).await["token"]
            .as_str()
            .unwrap()
            .to_string();

        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-network-test".to_string(),
                name: "Codex Network Test".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret"
                    },
                    "testConfig": {
                        "model": "test-model"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/api/providers/codex-network-test/test?network=true",
                json!(null),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;

        assert_eq!(body["networkChecked"].as_bool(), Some(true));
        assert_eq!(body["networkStatusCode"].as_u64(), Some(401));
        let error = body["networkError"].as_str().unwrap();
        assert!(error.contains("[REDACTED]"));
        assert!(!error.contains("sk-abc"));
        assert!(!error.contains("Bearer abc"));
    }

    #[tokio::test]
    async fn provider_network_test_covers_4xx_5xx_and_empty_bodies() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = Router::new()
            .route(
                "/v1/responses",
                post(|| async {
                    (
                        StatusCode::FORBIDDEN,
                        r#"{"error":"forbidden sk-secret-1234567890"}"#,
                    )
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "") }),
            );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = test_state();
        let app = app_router(state.clone());
        let token = setup_and_login(&app).await;

        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-provider-test".to_string(),
                name: "Codex Provider Test".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/api/providers/codex-provider-test/test?network=true",
                json!(null),
                Some(&token),
            ))
            .await
            .unwrap();
        let body = json_body(response).await;

        assert_eq!(body["networkStatusCode"].as_u64(), Some(403));
        let error = body["networkError"].as_str().unwrap();
        assert!(error.contains("[REDACTED]"));
        assert!(!error.contains("sk-secret"));
    }

    #[tokio::test]
    async fn provider_network_test_timeout_is_configurable_and_redacted() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                (StatusCode::OK, "{}")
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = test_state();
        let app = app_router(state.clone());
        let token = setup_and_login(&app).await;

        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-provider-timeout".to_string(),
                name: "Codex Provider Timeout".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/api/providers/codex-provider-timeout/test?network=true&timeoutMs=25",
                json!(null),
                Some(&token),
            ))
            .await
            .unwrap();
        let body = json_body(response).await;

        assert_eq!(body["networkChecked"].as_bool(), Some(true));
        assert_eq!(body["networkStatusCode"], serde_json::Value::Null);
        let error = body["networkError"].as_str().unwrap();
        assert!(!error.trim().is_empty());
        assert!(!error.contains("sk-local-secret"));
    }

    #[tokio::test]
    async fn non_stream_proxy_preserves_upstream_error_status_body_and_usage() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::CONTENT_TYPE, "text/plain")],
                    "quota_exhausted",
                )
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = test_state();
        let app = app_router(state.clone());
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-proxy-error".to_string(),
                name: "Codex Proxy Error".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/v1/responses",
                json!({"model":"gpt-5.5","input":"ping","stream":false}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = body_text(response).await;
        assert_eq!(body, "quota_exhausted");

        let usage = state.usage.read().await;
        assert_eq!(usage.logs.len(), 1);
        assert_eq!(usage.logs[0].provider_id, "codex-proxy-error");
        assert_eq!(usage.logs[0].status_code, 429);
        assert!(!usage.logs[0].is_streaming);
    }

    #[tokio::test]
    async fn non_stream_proxy_timeout_records_bad_gateway() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_addr = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                (StatusCode::OK, "{}")
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = test_state();
        let app = app_router(state.clone());
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-proxy-timeout".to_string(),
                name: "Codex Proxy Timeout".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret",
                        "UPSTREAM_TIMEOUT_MS": "25"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/v1/responses",
                json!({"model":"gpt-5.5","input":"ping","stream":false}),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let text = body_text(response).await;
        assert!(text.contains("proxy upstream request failed"));
    }

    #[tokio::test]
    async fn stream_proxy_marks_upstream_chunk_error() {
        let upstream_addr = spawn_broken_chunked_upstream().await;
        let state = test_state();
        let app = app_router(state.clone());
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-stream-error".to_string(),
                name: "Codex Stream Error".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                        "OPENAI_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/v1/responses",
                json!({"model":"gpt-5.5","input":"ping","stream":true}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body_text = String::from_utf8_lossy(&body);
        assert!(body_text.contains("response.failed"));
        assert!(body_text.contains("cc_switch_stream_error"));

        for _ in 0..20 {
            let usage = state.usage.read().await;
            if usage
                .logs
                .iter()
                .any(|log| log.stream_status.as_deref() == Some("upstream_error"))
            {
                let log = usage
                    .logs
                    .iter()
                    .find(|log| log.provider_id == "codex-stream-error")
                    .unwrap();
                assert_eq!(log.status_code, 502);
                assert!(log.is_streaming);
                assert!(log.first_token_ms.is_some());
                return;
            }
            drop(usage);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        panic!("stream upstream_error usage log was not recorded");
    }

    #[test]
    fn codex_oauth_schema_fixture_preserves_future_native_fields() {
        let mut store = crate::core::accounts::AccountStore::default();
        let account = store.upsert(UpsertAccountInput {
            id: Some("acct-codex".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("access-token".to_string()),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: vec!["openid".to_string(), "profile".to_string()],
            profile: Some(json!({"plan":"pro"})),
            raw: Some(json!({"source":"mock"})),
            subscription_level: Some("pro".to_string()),
            quota_percent: Some(12.5),
            quota: Some(AccountQuota {
                success: true,
                credential_message: Some("ok".to_string()),
                tiers: vec![crate::core::accounts::AccountQuotaTier {
                    name: "codex".to_string(),
                    utilization: Some(0.125),
                    used: Some(125.0),
                    limit: Some(1000.0),
                    unit: Some("requests".to_string()),
                    resets_at: Some(123456),
                }],
                extra_usage: None,
            }),
            quota_refreshed_at: Some(1000),
            quota_next_refresh_at: Some(2000),
            expires_at: Some(3000),
            last_refresh_error: None,
        });

        assert_eq!(account.provider_type, ProviderType::CodexOAuth);
        assert_eq!(account.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(account.subscription_level.as_deref(), Some("pro"));
        assert_eq!(account.quota_percent, Some(12.5));
        assert_eq!(account.quota.unwrap().tiers[0].name, "codex");
    }

    #[test]
    fn mock_codex_refresh_lock_allows_one_refresh_per_account() {
        #[derive(Default)]
        struct RefreshLocks(std::sync::Mutex<std::collections::HashSet<String>>);

        impl RefreshLocks {
            fn try_lock(&self, account_id: &str) -> bool {
                self.0.lock().unwrap().insert(account_id.to_string())
            }

            fn unlock(&self, account_id: &str) {
                self.0.lock().unwrap().remove(account_id);
            }
        }

        let locks = RefreshLocks::default();
        assert!(locks.try_lock("acct-codex"));
        assert!(!locks.try_lock("acct-codex"));
        assert!(locks.try_lock("acct-other"));
        locks.unlock("acct-codex");
        assert!(locks.try_lock("acct-codex"));

        let capability = crate::core::account_managers::capability_for(ProviderType::CodexOAuth);
        assert_eq!(
            capability.support,
            crate::core::account_managers::AccountManagerSupport::ManualTokenStore
        );
        assert!(capability.supports_refresh);
    }

    #[tokio::test]
    async fn web_runtime_context_reports_setup_and_authenticated_admin() {
        let app = app_router(test_state());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/web-api/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["mode"].as_str(), Some("client-login"));
        assert_eq!(body["status"].as_str(), Some("setup-required"));
        assert_eq!(body["uiAutomation"]["allowed"].as_bool(), Some(false));

        let token = setup_and_login(&app).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/web-api/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["mode"].as_str(), Some("client-login"));
        assert_eq!(body["status"].as_str(), Some("auth-required"));

        let response = app
            .oneshot(json_request(
                Method::GET,
                "/web-api/context",
                Value::Null,
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["mode"].as_str(), Some("local-admin"));
        assert_eq!(body["status"].as_str(), Some("authenticated"));
        assert_eq!(body["apps"].as_array().unwrap().len(), 3);
        assert!(body["commands"].as_array().unwrap().len() > 10);
    }

    #[tokio::test]
    async fn web_runtime_context_treats_invalid_token_as_auth_required() {
        let app = app_router(test_state());
        let _token = setup_and_login(&app).await;

        let response = app
            .oneshot(json_request(
                Method::GET,
                "/web-api/context",
                Value::Null,
                Some("invalid-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["mode"].as_str(), Some("client-login"));
        assert_eq!(body["status"].as_str(), Some("auth-required"));
    }

    #[tokio::test]
    async fn web_invoke_registry_returns_stable_errors() {
        let app = app_router(test_state());
        let token = setup_and_login(&app).await;

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/apply_claude_plugin_config",
                json!({ "official": true }),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/set_window_theme",
                json!({ "theme": "dark" }),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = json_body(response).await;
        assert_eq!(body["code"].as_str(), Some("cc_switch_feature_disabled"));
        assert_eq!(body["type"].as_str(), Some("feature_disabled"));

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/not_a_desktop_command",
                json!({}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = json_body(response).await;
        assert_eq!(body["code"].as_str(), Some("cc_switch_web_invoke_unknown"));
        assert_eq!(body["type"].as_str(), Some("web_invoke_unknown"));
    }

    #[tokio::test]
    async fn web_invoke_get_providers_returns_desktop_record_shape() {
        let state = test_state();
        state.providers.write().await.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-web".to_string(),
                name: "Codex Web".to_string(),
                settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );
        let app = app_router(state);
        let token = setup_and_login(&app).await;

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/get_providers",
                json!({"app": "codex"}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["codex-web"]["name"].as_str(), Some("Codex Web"));

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/get_current_provider",
                json!({"app": "codex"}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await.as_str(), Some("codex-web"));
    }

    fn test_state() -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir = std::env::temp_dir().join(format!("cc-switch-server-http-test-{nanos}"));
        ServerStateInner::load(Cli {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        })
        .unwrap()
    }

    async fn setup_and_login(app: &Router) -> String {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/setup",
                json!({
                    "password": "password123",
                    "ownerEmail": "owner@example.com",
                    "routerUrl": "http://127.0.0.1:9",
                    "clientTunnelSubdomain": "ownerabcde"
                }),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/auth/login",
                json!({"method": "password", "password": "password123"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        json_body(response).await["token"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn spawn_broken_chunked_upstream() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 2048];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\nZZ\r\n",
                )
                .await
                .unwrap();
            let _ = socket.shutdown().await;
        });
        addr
    }

    #[tokio::test]
    async fn web_password_login_authenticates_invoke() {
        let app = app_router(test_state());
        let _ = setup_and_login(&app).await;

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/auth/password/login",
                json!({ "password": "password123" }),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let access_token = body["accessToken"].as_str().unwrap();

        let response = app
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/get_tool_versions",
                json!({ "tools": [] }),
                Some(access_token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn router_identity_headers_authenticate_invoke() {
        let app = app_router(test_state());
        let _ = setup_and_login(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/web-api/invoke/get_tool_versions")
                    .header("content-type", "application/json")
                    .header("x-cc-switch-web-user-email", "owner@example.com")
                    .header("x-cc-switch-web-role", "owner")
                    .body(Body::from(r#"{"tools":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    fn json_request(
        method: Method,
        uri: &str,
        value: serde_json::Value,
        bearer: Option<&str>,
    ) -> Request<Body> {
        let body = if value.is_null() {
            Body::empty()
        } else {
            Body::from(serde_json::to_vec(&value).unwrap())
        };
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::CONTENT_TYPE, "application/json");
        if let Some(token) = bearer {
            builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(body).unwrap()
    }

    fn control_request(
        path: &str,
        body: Vec<u8>,
        timestamp_ms: i64,
        nonce: &str,
        signature: &str,
    ) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .header("x-ctl-installation-id", "inst-ctl")
            .header("x-ctl-timestamp-ms", timestamp_ms.to_string())
            .header("x-ctl-nonce", nonce)
            .header("x-ctl-signature", signature)
            .body(Body::from(body))
            .unwrap()
    }

    fn test_share_input(
        id: &str,
        provider_id: &str,
        provider_type: ProviderType,
    ) -> UpsertShareInput {
        UpsertShareInput {
            id: Some(id.to_string()),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: provider_id.to_string(),
            provider_type,
            display_name: Some(id.to_string()),
            enabled: None,
            status: None,
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
            acl: None,
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            for_sale: None,
            sale_market_kind: None,
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: None,
            description: None,
            bindings: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
        }
    }

    async fn json_body(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn body_text(response: Response) -> String {
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }
}
