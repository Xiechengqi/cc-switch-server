use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod web;

pub(in crate::api) mod accounts;
pub(in crate::api) mod backup;
pub(crate) mod control;
pub(crate) mod error;
pub(in crate::api) mod events;
pub(crate) mod invoke;
pub(in crate::api) mod models;
pub(in crate::api) mod providers;
pub(in crate::api) mod router;
pub(in crate::api) mod self_update;
pub(crate) mod session;
pub(in crate::api) mod settings;
pub(in crate::api) mod shares;
pub(in crate::api) mod subscription_quota;
pub(in crate::api) mod types;
pub(in crate::api) mod usage;

pub(in crate::api) use accounts::*;
pub(in crate::api) use backup::*;
pub(crate) use control::{
    control_apply_share_settings, control_refresh_share_usage, share_router_health,
    share_router_model_health, share_router_request_logs, share_router_runtime,
};
pub use control::{control_signature, refresh_share_usage_items, ControlRefreshShareUsageItem};
pub use error::ApiError;
pub(crate) use error::{
    map_codex_device_error, map_copilot_device_error, map_email_auth_error, map_kiro_device_error,
    map_share_patch_error, map_web_auth_error, ErrorResponse,
};
pub(in crate::api) use events::*;
pub(in crate::api) use invoke::dispatch::web_invoke_compat;
pub(in crate::api) use invoke::handlers::*;
pub(in crate::api) use models::*;
pub(in crate::api) use providers::*;
pub(in crate::api) use router::*;
pub(in crate::api) use self_update::*;
pub(crate) use session::{
    bearer_token, generate_session_token, require_event_session, require_session,
    require_web_admin_session, resolve_web_admin_principal,
};
pub(in crate::api) use settings::*;
pub(in crate::api) use shares::*;
pub(in crate::api) use subscription_quota::*;
pub(in crate::api) use types::*;
pub(in crate::api) use usage::*;

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
use chrono::Datelike;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::api::web::assets as web_assets;
use crate::api::web::coverage::ProviderCoverage;
use crate::api::web::runtime::{self as web_runtime, WebRuntimeCommandSupport};
use crate::build_info::build_info;
use crate::clients::oauth::quota::{refresh_account_quota, QuotaRefreshResult};
use crate::clients::oauth::refresh::{
    account_needs_native_refresh, execute_native_account_refresh, execute_oauth_json_request,
    execute_oauth_token_request, provider_native_refresh_available, AccountRefreshFailure,
};
use crate::domain::accounts::login::{
    OAuthLoginError, OAuthLoginFinish, OAuthLoginStart, OAuthLoginStatus, OAuthSessionPollState,
};
use crate::domain::accounts::managers::{manager_for, AccountManager};
use crate::domain::accounts::oauth::{
    build_profile_request, build_refresh_request, oauth_provider_spec, token_expires_soon,
    upsert_input_from_login_response, OAuthAuthorizeFlow, OAuthHttpRequest,
};
use crate::domain::accounts::store::{
    Account, AccountRefreshUpdate, AccountStore, UpsertAccountInput,
};
use crate::domain::failover::UpdateFailoverAppInput;
use crate::domain::providers::live_import;
use crate::domain::providers::model::{
    classify_provider_response, AppKind, Provider, ProviderType, ProviderTypeRequest,
    ProviderTypeResponse,
};
use crate::domain::providers::store::{ProviderSortUpdate, StoredProvider};
use crate::domain::providers::universal::{
    provider_from_universal, universal_provider_presets, UniversalProvider,
    UniversalProviderSyncResult,
};
use crate::domain::settings::config::{
    ServerConfig, SetupInput, UpdateClientTunnelInput, UpdateRouterConfigInput,
    UpdateUpstreamProxyInput,
};
use crate::domain::settings::ui_settings;
use crate::domain::sharing::shares::{
    Share, ShareAcl, ShareBinding, ShareMarketGrantStatus, ShareStore, ShareUpdateError,
    UpsertShareInput,
};
use crate::domain::usage::pricing::UpdateModelPricingInput;
use crate::domain::usage::store::{UsageLogFilter, UsageStatsFilter, UsageStore};
use crate::proxy::adapters::ProviderAdapter;
use crate::proxy::{self, ProxyRoute};
use crate::state::{ServerEvent, ServerState, Session};

pub const APPLY_SHARE_SETTINGS_PATH: &str = "/_ctl/apply_share_settings";
pub const REFRESH_SHARE_USAGE_PATH: &str = "/_ctl/refresh_share_usage";
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
        .route("/api/admin/version", get(admin_version))
        .route("/api/admin/restart", post(admin_restart))
        .route("/api/admin/rollback", post(admin_rollback))
        .route("/api/admin/upgrade", post(admin_upgrade_start))
        .route("/api/admin/upgrade/stream", get(admin_upgrade_stream))
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
        .route(
            "/api/accounts/codex/device/start",
            post(start_codex_device_login),
        )
        .route(
            "/api/accounts/codex/device/poll",
            post(poll_codex_device_login),
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
        .route("/web-api/auth/password/set", post(web_password_set))
        .route("/web-api/auth/session/refresh", post(web_session_refresh))
        .route("/web-api/context", get(web_runtime_context))
        .route("/web-api/invoke/*command", post(web_invoke_compat))
        .route("/web-api/events", get(events))
        .route(
            "/web-api/admin/upgrade/stream",
            get(crate::api::self_update::admin_upgrade_stream),
        )
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

async fn provider_coverage(State(state): State<ServerState>) -> Json<ProviderCoverage> {
    Json(state.provider_coverage.clone())
}

async fn provider_matrix() -> Json<crate::domain::providers::matrix::ProviderMatrix> {
    Json(crate::domain::providers::matrix::provider_matrix())
}

async fn provider_type(Json(input): Json<ProviderTypeRequest>) -> Json<ProviderTypeResponse> {
    Json(classify_provider_response(input.app, &input.provider))
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
        return Ok(Json(web_runtime_auth_required_payload(&config, contract)));
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

async fn web_dist_missing() -> impl IntoResponse {
    web_dist_missing_response()
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn clamp_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

pub(crate) fn parse_app_kind(value: &str) -> Result<AppKind, ApiError> {
    parse_supported_app_kind(value).ok_or_else(|| ApiError::bad_request("invalid appType"))
}

pub(crate) fn parse_supported_app_kind(value: &str) -> Option<AppKind> {
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

fn provider_test_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.filter(|value| *value > 0).unwrap_or(15_000))
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
) -> Vec<crate::api::web::coverage::ProviderFixture> {
    match app {
        AppKind::Claude => coverage.fixtures.claude.clone(),
        AppKind::Codex => coverage.fixtures.codex.clone(),
        AppKind::Gemini => coverage.fixtures.gemini.clone(),
    }
}
