use std::convert::Infallible;
use std::future::IntoFuture;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod web;

pub(in crate::api) mod accounts;
pub(in crate::api) mod backup;
pub(crate) mod control;
pub(in crate::api) mod debug;
pub(crate) mod error;
pub(in crate::api) mod events;
pub(crate) mod invoke;
pub(in crate::api) mod logs;
pub(in crate::api) mod models;
pub(in crate::api) mod provider_health_scheduler;
pub(in crate::api) mod providers;
mod request_audit;
pub(in crate::api) mod router;
pub(in crate::api) mod self_update;
pub(crate) mod session;
pub(in crate::api) mod settings;
pub(in crate::api) mod shares;
pub(in crate::api) mod subscription_quota;
pub(crate) mod terminal;
pub(in crate::api) mod types;
pub(in crate::api) mod usage;

pub(in crate::api) use accounts::*;
pub(in crate::api) use backup::*;
pub(crate) use control::{
    control_abort_client_subdomain_adoption, control_apply_share_settings, control_client_log_tail,
    control_commit_client_subdomain_adoption, control_prepare_client_subdomain_adoption,
    control_refresh_share_usage, share_router_health, share_router_model_health,
    share_router_request_logs, share_router_runtime,
};
pub use control::{
    control_signature, control_signature_for_method, refresh_share_usage_items,
    ControlRefreshShareUsageItem,
};
pub(in crate::api) use debug::*;
pub use error::ApiError;
pub(crate) use error::{
    map_account_write_error, map_codex_active_account_selection_error, map_codex_device_error,
    map_codex_workspace_rebind_error, map_copilot_device_error, map_email_auth_error,
    map_grok_device_error, map_kimi_device_error, map_kiro_device_error, map_share_patch_error,
    map_subscription_binding_error, map_web_auth_error, ErrorResponse, InferenceApiError,
    InferenceSurface,
};
pub(in crate::api) use events::*;
pub(in crate::api) use invoke::dispatch::web_invoke_compat;
pub(in crate::api) use invoke::handlers::*;
pub(in crate::api) use logs::*;
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
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::DefaultBodyLimit;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::Request;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::{Json, Router};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::Digest;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use request_audit::{
    audit_inference_request, is_inference_path, new_transport_request_id, record_ingress_rejection,
};

use crate::api::web::assets as web_assets;
use crate::api::web::coverage::ProviderCoverage;
use crate::api::web::runtime::{self as web_runtime, WebRuntimeCommandSupport};
use crate::build_info::build_info;
use crate::clients::oauth::quota::{refresh_account_quota, QuotaRefreshResult};
use crate::clients::oauth::refresh::{
    execute_oauth_json_request, execute_oauth_token_request, provider_native_refresh_available,
    AccountRefreshFailure,
};

use crate::domain::accounts::cursor_import::{
    cursor_workos_user_id_from_access_token, import_from_local_cursor,
    upsert_input_from_cursor_local_import,
};
use crate::domain::accounts::login::{
    OAuthLoginError, OAuthLoginFinish, OAuthLoginFinishAttempt, OAuthLoginStart, OAuthLoginStatus,
    OAuthSessionPollState,
};
use crate::domain::accounts::managers::{
    manager_for, AccountManager, AccountRefreshFlightFailure, AccountRefreshFlightFailureDetails,
    AccountRefreshFlightStage,
};
use crate::domain::accounts::oauth::{
    build_cursor_profile_request, build_profile_request, build_refresh_request,
    oauth_provider_spec, token_expires_soon, upsert_input_from_login_response,
    upsert_input_from_verified_grok_login_response,
    upsert_input_from_verified_openai_login_response, OAuthAuthorizeFlow, OAuthHttpRequest,
};
use crate::domain::accounts::store::{Account, UpsertAccountInput};
use crate::domain::providers::model::{
    classify_provider_response, AppKind, Provider, ProviderType, ProviderTypeRequest,
    ProviderTypeResponse,
};
use crate::domain::providers::runtime::{RuntimeAuthRef, RuntimeConfigurationState};
use crate::domain::providers::store::{ProviderSortUpdate, StoredProvider};
use crate::domain::settings::config::{
    ServerConfig, UpdateClientTunnelInput, UpdateRouterConfigInput,
};
use crate::domain::settings::ui_settings;
use crate::domain::sharing::shares::{
    Share, ShareAcl, ShareBinding, ShareDeleteTombstone, ShareStore, UpsertShareInput,
};
use crate::proxy::adapters::ProviderAdapter;
use crate::proxy::{self, ProxyRoute};
use crate::state::{ServerEvent, ServerState, Session, ShareInFlightGuard};

pub const APPLY_SHARE_SETTINGS_PATH: &str = "/_ctl/apply_share_settings";
pub const REFRESH_SHARE_USAGE_PATH: &str = "/_ctl/refresh_share_usage";
pub const PREPARE_CLIENT_SUBDOMAIN_ADOPTION_PATH: &str = "/_ctl/client-subdomain-adoption/prepare";
pub const COMMIT_CLIENT_SUBDOMAIN_ADOPTION_PATH: &str = "/_ctl/client-subdomain-adoption/commit";
pub const ABORT_CLIENT_SUBDOMAIN_ADOPTION_PATH: &str = "/_ctl/client-subdomain-adoption/abort";
pub const CLIENT_LOG_TAIL_PATH: &str = "/_ctl/logs/tail";
const HTTP_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(40);
const MANAGED_REFRESH_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(35);

pub async fn serve(state: ServerState) -> anyhow::Result<()> {
    if !state.config.read().await.is_setup_complete() {
        crate::setup::log_setup_required_hints(&state);
    }

    let app = app_router(state.clone());

    let listener = tokio::net::TcpListener::bind(state.bind_addr)
        .await
        .with_context(|| format!("bind {}", state.bind_addr))?;

    provider_health_scheduler::spawn_share_model_health_scheduler(state.clone());
    tracing::info!("cc-switch-server listening on {}", state.bind_addr);
    let mut shutdown = state.subscribe_shutdown();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.clone()))
        .into_future();
    tokio::pin!(server);

    let completed = tokio::select! {
        result = &mut server => Some(result),
        changed = shutdown.changed() => {
            if changed.is_err() {
                tracing::warn!("server shutdown coordinator closed unexpectedly");
            }
            None
        }
    };

    if let Some(result) = completed {
        if state.is_shutting_down()
            && !state
                .drain_managed_account_refreshes(MANAGED_REFRESH_SHUTDOWN_DEADLINE)
                .await
        {
            tracing::error!(
                "managed OAuth refresh owners did not drain before the shutdown deadline"
            );
        }
        return result.context("serve http");
    }

    let (server_result, refreshes_drained) = tokio::join!(
        tokio::time::timeout(HTTP_SHUTDOWN_DEADLINE, &mut server),
        state.drain_managed_account_refreshes(MANAGED_REFRESH_SHUTDOWN_DEADLINE),
    );
    if !refreshes_drained {
        tracing::error!("managed OAuth refresh owners did not drain before the shutdown deadline");
    }
    match server_result {
        Ok(result) => result.context("serve http"),
        Err(_) => {
            tracing::error!(
                deadline_secs = HTTP_SHUTDOWN_DEADLINE.as_secs(),
                "HTTP connections did not drain before the shutdown deadline; forcing close"
            );
            Ok(())
        }
    }
}

async fn shutdown_signal(state: ServerState) {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::warn!(%error, "failed to listen for Ctrl-C");
                        }
                    }
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to listen for SIGTERM");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    state.begin_shutdown();
    state.begin_managed_account_refresh_shutdown();
}

pub fn app_router(state: ServerState) -> Router {
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(readiness))
        .route("/metrics", get(prometheus_metrics))
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
        .route(
            PREPARE_CLIENT_SUBDOMAIN_ADOPTION_PATH,
            post(control_prepare_client_subdomain_adoption),
        )
        .route(
            COMMIT_CLIENT_SUBDOMAIN_ADOPTION_PATH,
            post(control_commit_client_subdomain_adoption),
        )
        .route(
            ABORT_CLIENT_SUBDOMAIN_ADOPTION_PATH,
            post(control_abort_client_subdomain_adoption),
        )
        .route(CLIENT_LOG_TAIL_PATH, get(control_client_log_tail))
        .route("/api/setup/status", get(setup_status))
        .route("/api/setup/check-router", post(setup_check_router))
        .route(
            "/api/setup/suggest-subdomain",
            post(setup_suggest_subdomain),
        )
        .route("/api/setup/check-subdomain", post(setup_check_subdomain))
        .route("/api/setup/validate", post(setup_validate))
        .route("/api/setup/bootstrap", post(setup_bootstrap))
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
        .route("/api/admin/upgrade/status", get(admin_upgrade_status))
        .route("/api/admin/logs/tail", get(admin_logs_tail))
        .route("/api/events", get(events))
        .route("/api/backup", get(list_backups).post(create_backup))
        .route("/api/backups", get(list_backups).post(create_backup))
        .route("/api/backup/:id/restore", post(restore_backup))
        .route("/api/backups/:id/restore", post(restore_backup))
        .route("/api/config", get(config_snapshot))
        .route(
            "/api/provider-bundles",
            get(list_provider_bundles).post(create_provider_bundle),
        )
        .route(
            "/api/provider-bundles/order",
            put(update_provider_bundle_order),
        )
        .route(
            "/api/provider-bundles/:id",
            get(get_provider_bundle)
                .patch(update_provider_bundle)
                .delete(delete_provider_bundle),
        )
        .route(
            "/api/provider-bundles/:id/delete-preview",
            get(provider_bundle_delete_preview),
        )
        .route("/api/providers", get(list_providers).post(create_provider))
        .route(
            "/api/providers/:id",
            get(get_provider)
                .patch(update_provider)
                .delete(delete_provider),
        )
        .route("/api/providers/export", get(export_providers))
        .route("/api/providers/import", post(import_providers))
        .route(
            "/api/providers/account-bindings/migration",
            get(preview_provider_account_binding_migration)
                .post(apply_provider_account_binding_migration),
        )
        .route("/api/providers/health", get(provider_health))
        .route(
            "/api/providers/storage-migration",
            get(provider_storage_migration),
        )
        .route("/api/providers/test", post(test_providers))
        .route("/api/providers/:id/test", post(test_provider))
        .route(
            "/api/providers/:id/fetch-models",
            post(fetch_provider_models),
        )
        .route(
            "/api/providers/:id/delete-preview",
            get(provider_delete_preview),
        )
        .route(
            "/api/providers/:id/adopt-profile",
            post(adopt_provider_profile),
        )
        .route(
            "/api/providers/:id/rebind-custom",
            post(rebind_custom_provider),
        )
        .route(
            "/api/providers/:id/clone-as-custom",
            post(clone_provider_as_custom),
        )
        .route(
            "/api/providers/from-preset",
            post(create_provider_from_preset),
        )
        .route("/api/provider-presets", get(provider_presets))
        .route("/api/provider-registry", get(provider_registry))
        .route("/api/provider-coverage", get(provider_coverage))
        .route("/api/provider-matrix", get(provider_matrix))
        .route("/api/provider-type", post(provider_type))
        .route("/api/accounts", get(list_accounts).post(upsert_account))
        .route(
            "/api/accounts/codex/active",
            post(select_active_codex_oauth_account),
        )
        .route("/api/accounts/capabilities", get(account_capabilities))
        .route(
            "/api/accounts/import-templates",
            get(account_import_templates),
        )
        .route(
            "/api/accounts/claude/credentials/import",
            post(import_claude_credentials),
        )
        .route(
            "/api/accounts/grok/auth-json/import",
            post(import_grok_auth_json),
        )
        .route(
            "/api/accounts/kiro/credentials/import",
            post(import_kiro_credentials_json),
        )
        .route(
            "/api/accounts/kiro/local/import",
            post(import_kiro_local_credentials),
        )
        .route(
            "/api/accounts/kiro/api-key/import",
            post(import_kiro_api_key),
        )
        .route(
            "/api/accounts/cursor/local/import",
            post(import_cursor_local_auth),
        )
        .route("/api/accounts/login/start", post(start_account_login))
        .route("/api/accounts/login/callback", get(account_login_callback))
        .route("/api/accounts/login/finish", post(finish_account_login))
        .route("/api/accounts/login/cancel", post(cancel_account_login))
        .route(
            "/web-api/oauth/claude-cli/callback",
            get(claude_cli_oauth_callback),
        )
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
        .route(
            "/api/accounts/codex/device/cancel",
            post(cancel_codex_device_login),
        )
        .route(
            "/api/accounts/grok/device/start",
            post(start_grok_device_login),
        )
        .route(
            "/api/accounts/grok/device/poll",
            post(poll_grok_device_login),
        )
        .route(
            "/api/accounts/grok/device/cancel",
            post(cancel_grok_device_login),
        )
        .route(
            "/api/accounts/kimi/device/start",
            post(start_kimi_device_login),
        )
        .route(
            "/api/accounts/kimi/device/poll",
            post(poll_kimi_device_login),
        )
        .route(
            "/api/accounts/kimi/device/cancel",
            post(cancel_kimi_device_login),
        )
        .route("/api/accounts/:id", delete(delete_account))
        .route(
            "/api/accounts/:id/delete-preview",
            get(account_delete_preview),
        )
        .route("/api/accounts/:id/refresh", post(refresh_account))
        .route("/api/accounts/:id/refresh-plan", get(account_refresh_plan))
        .route("/api/accounts/:id/quota", get(account_quota))
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
        .route("/api/shares/reuse-candidates", get(share_reuse_candidates))
        .route("/api/shares/export", get(export_shares))
        .route("/api/shares/import", post(import_shares))
        .route("/api/shares/:id", delete(delete_share))
        .route("/api/shares/:id/connect-info", get(share_connect_info))
        .route("/api/shares/:id/subdomain", post(update_share_subdomain))
        .route("/api/shares/:id/binding", post(update_share_binding))
        .route("/api/shares/:id/bindings", post(add_share_binding))
        .route(
            "/api/shares/:id/bindings/remove",
            post(remove_share_binding),
        )
        .route("/api/shares/:id/pause", post(pause_share))
        .route("/api/shares/:id/resume", post(resume_share))
        .route("/api/shares/:id/tunnel/start", post(start_share_tunnel))
        .route("/api/shares/:id/tunnel/stop", post(stop_share_tunnel))
        .route("/api/shares/tunnels/restore", post(restore_share_tunnels))
        .route("/api/shares/:id/reset-usage", post(reset_share_usage))
        .route("/api/shares/:id/acl", post(replace_share_acl))
        .route("/api/token-markets", get(list_token_markets))
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
        .route(
            "/web-api/router/client-tunnel/subdomain-check",
            get(web_client_tunnel_subdomain_check),
        )
        .route("/api/router/tunnels", get(router_tunnels))
        .route("/api/router/heartbeat", post(router_heartbeat))
        .route("/api/router/status", get(router_status))
        .route("/api/router/diagnostics", get(router_diagnostics))
        .route("/api/router/register", post(router_register))
        .route(
            "/api/router/share-edits/pull",
            post(router_pull_share_edits),
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
        .route(
            "/web-api/oauth/openai-cli/callback",
            get(openai_cli_oauth_callback),
        )
        .route("/web-api/context", get(web_runtime_context))
        .route("/web-api/usage/overview", get(usage_overview))
        .route("/web-api/usage/trends", get(usage_trends))
        .route("/web-api/usage/facets", get(usage_facets))
        .route(
            "/web-api/usage/provider-bundles",
            get(usage_provider_bundles),
        )
        .route("/web-api/usage/models", get(usage_models))
        .route("/web-api/usage/shares", get(usage_shares))
        .route("/web-api/usage/requests/:id", get(usage_request_detail))
        .route("/web-api/usage/requests", get(usage_requests))
        .route("/web-api/invoke/*command", post(web_invoke_compat))
        .route(
            "/web-api/terminal/stream",
            get(crate::api::terminal::terminal_stream),
        )
        .route(
            "/web-api/terminal/input",
            post(crate::api::terminal::terminal_input),
        )
        .route(
            "/web-api/terminal/resize",
            post(crate::api::terminal::terminal_resize),
        )
        .route(
            "/web-api/terminal/session/end",
            post(crate::api::terminal::terminal_session_end),
        )
        .route("/web-api/events", get(events))
        .route("/web-api/debug/runtime", get(debug_runtime))
        .route("/web-api/debug/diagnostics", get(debug_diagnostics))
        .route("/web-api/debug/logs/tail", get(debug_logs_tail))
        .route("/web-api/debug/restart", post(debug_restart))
        .route(
            "/web-api/debug/operations/:operation_id",
            get(debug_restart_status),
        )
        .route("/web-api/debug/upgrade", post(debug_upgrade_start))
        .route("/web-api/debug/upgrade/status", get(debug_upgrade_status))
        .route("/web-api/debug/upgrade/stream", get(debug_upgrade_stream))
        .route(
            "/web-api/admin/upgrade/stream",
            get(crate::api::self_update::admin_upgrade_stream),
        )
        .route(
            "/web-api/admin/upgrade/status",
            get(crate::api::self_update::admin_upgrade_status),
        )
        .route(
            "/web-api/admin/logs/tail",
            get(crate::api::logs::admin_logs_tail),
        )
        .merge(inference_router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            verify_router_ingress,
        ))
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

fn inference_router(state: ServerState) -> Router<ServerState> {
    Router::new()
        .route("/v1/models", get(proxy_models_or_manifest))
        .route("/models", get(proxy_models_or_manifest))
        .route(
            "/backend-api/codex/models",
            get(proxy_codex_models_manifest),
        )
        .route("/v1/messages", post(proxy_claude_messages))
        .route("/claude/v1/messages", post(proxy_claude_messages))
        .route("/v1/messages/count_tokens", post(proxy_claude_count_tokens))
        .route(
            "/claude/v1/messages/count_tokens",
            post(proxy_claude_count_tokens),
        )
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
        .route(
            "/v1/responses",
            post(proxy_codex_responses).get(proxy_codex_responses_ws),
        )
        .route(
            "/v1/responses/compact",
            post(proxy_codex_responses_compact).get(proxy_codex_responses_ws),
        )
        .route(
            "/v1/v1/responses",
            post(proxy_codex_responses).get(proxy_codex_responses_ws),
        )
        .route(
            "/v1/v1/responses/compact",
            post(proxy_codex_responses_compact).get(proxy_codex_responses_ws),
        )
        .route(
            "/responses",
            post(proxy_codex_responses).get(proxy_codex_responses_ws),
        )
        .route(
            "/responses/compact",
            post(proxy_codex_responses_compact).get(proxy_codex_responses_ws),
        )
        .route(
            "/codex/v1/responses",
            post(proxy_codex_responses).get(proxy_codex_responses_ws),
        )
        .route(
            "/codex/v1/responses/compact",
            post(proxy_codex_responses_compact),
        )
        .route("/backend-api/codex/responses", post(proxy_codex_responses))
        .route(
            "/backend-api/codex/responses/compact",
            post(proxy_codex_responses_compact),
        )
        .route(
            "/v1/responses/input_tokens",
            post(proxy_responses_input_tokens),
        )
        .route(
            "/responses/input_tokens",
            post(proxy_responses_input_tokens),
        )
        .route("/alpha/search", post(proxy_codex_alpha_search))
        .route("/v1/alpha/search", post(proxy_codex_alpha_search))
        .route(
            "/backend-api/codex/alpha/search",
            post(proxy_codex_alpha_search),
        )
        .route(
            "/v1/images/files/:token",
            get(ephemeral_image_file).head(ephemeral_image_file),
        )
        .route(
            "/v1/images/generations",
            post(proxy_images_generations).layer(DefaultBodyLimit::max(
                proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/images/generations",
            post(proxy_images_generations).layer(DefaultBodyLimit::max(
                proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/v1/images/edits",
            post(proxy_images_edits).layer(DefaultBodyLimit::max(
                proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/images/edits",
            post(proxy_images_edits).layer(DefaultBodyLimit::max(
                proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/v1/videos/generations",
            post(proxy_grok_videos_generations)
                .layer(DefaultBodyLimit::max(proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES)),
        )
        .route(
            "/videos/generations",
            post(proxy_grok_videos_generations)
                .layer(DefaultBodyLimit::max(proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES)),
        )
        .route("/v1/videos/:request_id", get(proxy_grok_video_status))
        .route("/videos/:request_id", get(proxy_grok_video_status))
        .route("/v1beta/*path", any(proxy_gemini))
        .route("/gemini/v1/*path", any(proxy_gemini))
        .route("/gemini/v1beta/*path", any(proxy_gemini))
        .layer(middleware::from_fn(require_router_share_ingress))
        .layer(middleware::from_fn_with_state(
            state,
            audit_inference_request,
        ))
}

async fn require_router_share_ingress(request: Request, next: Next) -> Response {
    let Some(context) = request
        .extensions()
        .get::<crate::clients::router::ingress::IngressContext>()
    else {
        return ApiError::unauthorized("inference requires signed Router Share ingress")
            .into_response();
    };
    if context.share_id.is_none() {
        return ApiError::forbidden("inference requires a Router Share binding").into_response();
    }
    next.run(request).await
}

async fn verify_router_ingress(
    State(state): State<ServerState>,
    mut request: Request,
    next: Next,
) -> Response {
    use crate::clients::router::ingress::{INGRESS_CONTEXT_HEADER, INGRESS_SIGNATURE_HEADER};

    let transport_request_id = new_transport_request_id();
    request
        .extensions_mut()
        .insert(transport_request_id.clone());
    let audit_method = request.method().clone();
    let audit_path = request.uri().path().to_string();
    let encoded = request
        .headers()
        .get(INGRESS_CONTEXT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let signature = request
        .headers()
        .get(INGRESS_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    for name in [
        INGRESS_CONTEXT_HEADER,
        INGRESS_SIGNATURE_HEADER,
        "x-cc-switch-share-id",
        "x-cc-switch-share-subdomain",
        "x-cc-switch-share-host",
        "x-cc-switch-user-email",
        "x-cc-switch-user-country",
        "x-cc-switch-user-country-iso3",
        "x-cc-switch-request-id",
        "x-cc-switch-health-check",
        "x-cc-switch-data-source",
        "x-cc-switch-source",
        "x-cc-switch-web-user-email",
        "x-cc-switch-web-role",
        "x-cc-switch-installation-id",
        "x-cc-switch-client-tunnel-subdomain",
        "x-cc-switch-client-tunnel-host",
    ] {
        request.headers_mut().remove(name);
    }

    let context = match (encoded, signature) {
        (None, None) => None,
        (Some(encoded), Some(signature)) => {
            let config = state.config.read().await;
            let Some(identity) = config.registered_router_identity() else {
                return audited_router_ingress_rejection(
                    &state,
                    &audit_method,
                    &audit_path,
                    &transport_request_id,
                    "router_identity_unavailable",
                    None,
                );
            };
            let Some(control_secret) = identity.control_secret.as_deref() else {
                return audited_router_ingress_rejection(
                    &state,
                    &audit_method,
                    &audit_path,
                    &transport_request_id,
                    "control_secret_unavailable",
                    None,
                );
            };
            let router_id = match crate::clients::router::client::tunnel_router_id(&config) {
                Ok(router_id) => router_id,
                Err(_) => {
                    return audited_router_ingress_rejection(
                        &state,
                        &audit_method,
                        &audit_path,
                        &transport_request_id,
                        "router_binding_invalid",
                        None,
                    )
                }
            };
            let now_ms = chrono::Utc::now().timestamp_millis();
            match crate::clients::router::ingress::verify_envelope(
                &encoded,
                &signature,
                control_secret,
                &router_id,
                &identity.installation_id,
                now_ms,
            ) {
                Ok(context) => Some(context),
                Err(error) => {
                    let timing = error.timing();
                    tracing::warn!(
                        error = %error,
                        ingress_error = error.code(),
                        ingress_age_ms = timing.map(|(issued_at_ms, now_ms)| now_ms.saturating_sub(issued_at_ms)),
                        "router ingress context rejected"
                    );
                    return audited_router_ingress_rejection(
                        &state,
                        &audit_method,
                        &audit_path,
                        &transport_request_id,
                        error.code(),
                        timing,
                    );
                }
            }
        }
        _ => {
            return audited_router_ingress_rejection(
                &state,
                &audit_method,
                &audit_path,
                &transport_request_id,
                "headers_incomplete",
                None,
            )
        }
    };

    let Some(context) = context else {
        let mut response = next.run(request).await;
        strip_internal_ingress_response_headers(&mut response);
        return response;
    };
    if context.signature_version == crate::clients::router::ingress::SIGNATURE_VERSION_V2 {
        let method = request.method().as_str().to_string();
        let path_and_query = request
            .uri()
            .path_and_query()
            .map(|target| target.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());
        let body_limit = router_ingress_body_limit(request.uri().path());
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, body_limit).await {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(body_limit, error = %error, "router ingress request body rejected");
                record_ingress_rejection(
                    &state,
                    &audit_method,
                    &audit_path,
                    &transport_request_id,
                    "request_body_too_large",
                    StatusCode::PAYLOAD_TOO_LARGE,
                );
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
        };
        let body_sha256 = crate::clients::router::ingress::body_sha256_hex(&body);
        if let Err(error) = crate::clients::router::ingress::verify_request_binding(
            &context,
            &method,
            &path_and_query,
            &body_sha256,
        ) {
            tracing::warn!(
                error = %error,
                ingress_error = error.code(),
                "router ingress request binding rejected"
            );
            return audited_router_ingress_rejection(
                &state,
                &audit_method,
                &audit_path,
                &transport_request_id,
                error.code(),
                None,
            );
        }
        request = Request::from_parts(parts, Body::from(body));
    }
    match state.accept_router_ingress_request(&context, chrono::Utc::now().timestamp_millis()) {
        crate::clients::router::ingress::IngressReplayDecision::Accepted => {}
        crate::clients::router::ingress::IngressReplayDecision::Replay => {
            return audited_router_ingress_rejection(
                &state,
                &audit_method,
                &audit_path,
                &transport_request_id,
                "replayed_request_id",
                None,
            );
        }
        crate::clients::router::ingress::IngressReplayDecision::Capacity => {
            return audited_router_ingress_rejection(
                &state,
                &audit_method,
                &audit_path,
                &transport_request_id,
                "replay_cache_capacity",
                None,
            );
        }
    }
    request.extensions_mut().insert(context.clone());
    let headers = request.headers_mut();
    for (name, value) in [
        ("x-cc-switch-share-id", context.share_id.as_deref()),
        (
            "x-cc-switch-share-subdomain",
            context.public_host.split('.').next(),
        ),
        (
            "x-cc-switch-share-host",
            context
                .share_id
                .as_ref()
                .map(|_| context.public_host.as_str()),
        ),
        ("x-cc-switch-user-email", context.user_email.as_deref()),
        ("x-cc-switch-user-country", context.user_country.as_deref()),
        ("x-cc-switch-request-id", Some(context.request_id.as_str())),
    ] {
        if let Some(value) = value.and_then(|value| value.parse().ok()) {
            headers.insert(name, value);
        }
    }
    if context.share_id.is_none() {
        for (name, value) in [
            ("x-cc-switch-web-user-email", context.user_email.as_deref()),
            ("x-cc-switch-web-role", context.user_role.as_deref()),
            (
                "x-cc-switch-installation-id",
                Some(context.installation_id.as_str()),
            ),
            (
                "x-cc-switch-client-tunnel-subdomain",
                context.public_host.split('.').next(),
            ),
            (
                "x-cc-switch-client-tunnel-host",
                Some(context.public_host.as_str()),
            ),
        ] {
            if let Some(value) = value.and_then(|value| value.parse().ok()) {
                headers.insert(name, value);
            }
        }
    }
    let mut response = next.run(request).await;
    strip_internal_ingress_response_headers(&mut response);
    response
}

fn audited_router_ingress_rejection(
    state: &ServerState,
    method: &Method,
    path: &str,
    transport_request_id: &request_audit::TransportRequestId,
    code: &'static str,
    timing: Option<(i64, i64)>,
) -> Response {
    if is_inference_path(path) {
        record_ingress_rejection(
            state,
            method,
            path,
            transport_request_id,
            code,
            StatusCode::UNAUTHORIZED,
        );
    }
    router_ingress_rejection(code, timing)
}

fn router_ingress_body_limit(path: &str) -> usize {
    match path {
        "/v1/images/generations" | "/images/generations" | "/v1/images/edits" | "/images/edits" => {
            proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES
        }
        "/v1/videos/generations" | "/videos/generations" => proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES,
        _ => 2 * 1024 * 1024,
    }
}

fn router_ingress_rejection(code: &'static str, timing: Option<(i64, i64)>) -> Response {
    use crate::clients::router::ingress::{
        INTERNAL_INGRESS_AGE_MS_HEADER, INTERNAL_INGRESS_ERROR_HEADER,
        INTERNAL_INGRESS_SERVER_TIME_MS_HEADER,
    };

    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        INTERNAL_INGRESS_ERROR_HEADER,
        HeaderValue::from_static(code),
    );
    if let Some((issued_at_ms, now_ms)) = timing {
        if let Ok(value) = HeaderValue::from_str(&now_ms.saturating_sub(issued_at_ms).to_string()) {
            response
                .headers_mut()
                .insert(INTERNAL_INGRESS_AGE_MS_HEADER, value);
        }
        if let Ok(value) = HeaderValue::from_str(&now_ms.to_string()) {
            response
                .headers_mut()
                .insert(INTERNAL_INGRESS_SERVER_TIME_MS_HEADER, value);
        }
    }
    response
}

fn strip_internal_ingress_response_headers(response: &mut Response) {
    use crate::clients::router::ingress::{
        INTERNAL_INGRESS_AGE_MS_HEADER, INTERNAL_INGRESS_ERROR_HEADER,
        INTERNAL_INGRESS_SERVER_TIME_MS_HEADER,
    };

    for name in [
        INTERNAL_INGRESS_ERROR_HEADER,
        INTERNAL_INGRESS_AGE_MS_HEADER,
        INTERNAL_INGRESS_SERVER_TIME_MS_HEADER,
    ] {
        response.headers_mut().remove(name);
    }
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
            retry_after_seconds: None,
        }),
    )
        .into_response()
}

async fn health(State(state): State<ServerState>) -> Json<HealthResponse> {
    let degraded = state.credential_persistence_degraded();
    Json(HealthResponse {
        ok: true,
        status: if degraded { "degraded" } else { "healthy" },
        credential_persistence_degraded: degraded,
        config_dir: state.config_dir.display().to_string(),
        web_dist_dir: state
            .web_dist_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        embedded_web_assets: web_assets::asset_count(),
        unix_ms: now_ms(),
    })
}

async fn readiness(State(state): State<ServerState>) -> Response {
    let mut reasons = Vec::new();
    if state.credential_persistence_degraded() {
        reasons.push("oauth_credential_persistence_degraded");
    }
    let ready = reasons.is_empty();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadinessResponse {
            ok: ready,
            status: if ready { "ready" } else { "degraded" },
            reasons,
        }),
    )
        .into_response()
}

async fn prometheus_metrics() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        crate::metrics::render(),
    )
}

async fn version(State(state): State<ServerState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        build: build_info(),
        process_id: std::process::id(),
        process_instance_id: state.process_instance_id.clone(),
    })
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
) -> Result<Json<OpenAiModelsResponse>, InferenceApiError> {
    let app = query.app.unwrap_or(AppKind::Codex);
    let surface = inference_surface_for_app(app);
    let request_id = inference_request_id(&headers);
    let (provider_id, _share_guard) = validate_router_share_surface(&state, &headers, app)
        .await
        .map_err(|error| InferenceApiError::proxy(surface, request_id, error))?;
    Ok(proxy_models_for_selection(&state, Some(app), Some(&provider_id)).await)
}

pub(super) async fn validate_router_share_surface(
    state: &ServerState,
    headers: &HeaderMap,
    app: AppKind,
) -> Result<(String, ShareInFlightGuard), proxy::ProxyError> {
    let share_id = headers
        .get("x-cc-switch-share-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            proxy::ProxyError::unauthorized("inference requires signed Router Share ingress")
        })?;
    let user_email = headers
        .get("x-cc-switch-user-email")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (_, guard) =
        proxy::validate_and_acquire_share_invocation(state, share_id, app, user_email).await?;
    let provider_id = state
        .shares
        .read()
        .await
        .get(share_id)
        .and_then(|share| share.bindings.iter().find(|binding| binding.app == app))
        .map(|binding| binding.provider_id.clone())
        .ok_or_else(|| proxy::ProxyError::conflict("Share binding changed during validation"))?;
    let provider_enabled = state.providers.read().await.providers.iter().any(|stored| {
        stored.app == app
            && stored.provider.id == provider_id
            && crate::domain::providers::bundle::surface_enabled(&stored.provider)
    });
    if !provider_enabled {
        return Err(proxy::ProxyError::not_found(format!(
            "enabled Share provider not found: {provider_id}"
        )));
    }
    Ok((provider_id, guard))
}

fn inference_surface_for_app(app: AppKind) -> InferenceSurface {
    match app {
        AppKind::Claude => InferenceSurface::Anthropic,
        AppKind::Codex => InferenceSurface::OpenAi,
        AppKind::Gemini => InferenceSurface::Gemini,
    }
}

fn inference_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cc-switch-request-id")
        .or_else(|| headers.get("x-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn proxy_models_for_selection(
    state: &ServerState,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Json<OpenAiModelsResponse> {
    let providers = state.providers.read().await.clone();
    let mut data = openai_model_list(&providers.providers, app, provider_id);
    let claude_catalog = resolve_claude_catalog_provider(&providers, app, provider_id)
        .map(|_| crate::clients::oauth::claude_models::static_claude_model_catalog());
    let cursor_catalog =
        append_cursor_api_key_models(state, &providers.providers, app, provider_id, &mut data)
            .await;
    let kiro_provider = resolve_kiro_catalog_provider(&providers, app, provider_id).cloned();
    let kiro_catalog = if let Some(provider) = kiro_provider.as_ref() {
        if let Some((account_id, expected_generation)) =
            kiro_catalog_managed_account_binding(&providers, provider)
        {
            let refresh = state
                .refresh_managed_account_if_needed_for_generation(
                    ProviderType::KiroOAuth,
                    &account_id,
                    expected_generation,
                )
                .await;
            if let Err(error) = refresh {
                tracing::warn!(error = ?error, "Kiro model discovery token refresh failed");
                Some(crate::clients::oauth::kiro_runtime::static_model_catalog(
                    "token_refresh_failed",
                ))
            } else if let Some(account) = state
                .find_account_for_provider(ProviderType::KiroOAuth, &account_id)
                .await
                .filter(|account| account.auth_identity_generation == expected_generation)
            {
                #[cfg(test)]
                let endpoint_override = providers
                    .runtime_plan(provider.app, &provider.provider.id)
                    .and_then(|plan| {
                        plan.driver_options
                            .get("testKiroModelsUrl")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                #[cfg(not(test))]
                let endpoint_override: Option<&str> = None;
                Some(
                    crate::clients::oauth::kiro_runtime::model_catalog(
                        &state.http_client().await,
                        &account,
                        endpoint_override.as_deref(),
                    )
                    .await,
                )
            } else {
                Some(crate::clients::oauth::kiro_runtime::static_model_catalog(
                    "bound_account_unavailable",
                ))
            }
        } else {
            Some(crate::clients::oauth::kiro_runtime::static_model_catalog(
                "managed_account_binding_unavailable",
            ))
        }
    } else {
        None
    };
    if let (Some(provider), Some(catalog)) = (kiro_provider.as_ref(), kiro_catalog.as_ref()) {
        // A live bound-account catalog replaces the static fallback for this exact Provider.
        data.clear();
        let owned_by = model_owner(provider);
        for id in &catalog.models {
            if !data.iter().any(|model| model.id == *id) {
                data.push(OpenAiModel {
                    id: id.clone(),
                    object: "model",
                    owned_by: owned_by.clone(),
                    reasoning_efforts: None,
                    input_modalities: Some(vec!["text".to_string(), "image".to_string()]),
                });
            }
        }
        data.sort_by(|left, right| left.id.cmp(&right.id));
    }
    let grok_provider = resolve_grok_catalog_provider(&providers, app, provider_id).cloned();
    #[cfg(test)]
    let grok_models_test_url = grok_provider.as_ref().and_then(|provider| {
        providers
            .runtime_plan(provider.app, &provider.provider.id)
            .and_then(|plan| {
                plan.driver_options
                    .get("testGrokModelsUrl")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
    });
    let grok_catalog = if let Some(provider) = grok_provider.as_ref() {
        if let Some((account_id, expected_generation)) =
            grok_catalog_managed_account_binding(&providers, provider)
        {
            if state.credential_persistence_degraded() {
                Some(
                    crate::clients::oauth::grok_models::static_grok_model_catalog(
                        "credential_persistence_degraded",
                    ),
                )
            } else {
                let refresh = state
                    .refresh_managed_account_if_needed_for_generation(
                        ProviderType::GrokOAuth,
                        account_id.as_str(),
                        expected_generation,
                    )
                    .await;
                if let Err(error) = refresh {
                    tracing::warn!(error = ?error, "Grok model discovery token refresh failed");
                    Some(
                        crate::clients::oauth::grok_models::static_grok_model_catalog(
                            "token_refresh_failed",
                        ),
                    )
                } else if state.credential_persistence_degraded() {
                    Some(
                        crate::clients::oauth::grok_models::static_grok_model_catalog(
                            "credential_persistence_degraded",
                        ),
                    )
                } else {
                    let account = state
                        .find_account_for_provider(ProviderType::GrokOAuth, account_id.as_str())
                        .await
                        .filter(|account| account.auth_identity_generation == expected_generation);
                    if let Some((account, access_token)) = account.as_ref().and_then(|account| {
                        account
                            .access_token
                            .as_deref()
                            .map(str::trim)
                            .filter(|token| !token.is_empty())
                            .map(|token| (account, token))
                    }) {
                        #[cfg(test)]
                        if let Some(url) = grok_models_test_url.as_deref() {
                            Some(
                                crate::clients::oauth::grok_models::grok_model_catalog_at_test_url(
                                    &state.http_client().await,
                                    &account.id,
                                    access_token,
                                    url,
                                )
                                .await,
                            )
                        } else {
                            Some(
                                crate::clients::oauth::grok_models::grok_model_catalog(
                                    &state.http_client().await,
                                    &account.id,
                                    access_token,
                                )
                                .await,
                            )
                        }
                        #[cfg(not(test))]
                        Some(
                            crate::clients::oauth::grok_models::grok_model_catalog(
                                &state.http_client().await,
                                &account.id,
                                access_token,
                            )
                            .await,
                        )
                    } else {
                        Some(
                            crate::clients::oauth::grok_models::static_grok_model_catalog(
                                "access_token_unavailable",
                            ),
                        )
                    }
                }
            }
        } else {
            Some(
                crate::clients::oauth::grok_models::static_grok_model_catalog(
                    "managed_account_binding_unavailable",
                ),
            )
        }
    } else {
        None
    };
    if let (Some(provider), Some(catalog)) = (grok_provider.as_ref(), grok_catalog.as_ref()) {
        let owned_by = model_owner(provider);
        for id in &catalog.models {
            if !data.iter().any(|model| model.id == *id) {
                data.push(OpenAiModel {
                    id: id.clone(),
                    object: "model",
                    owned_by: owned_by.clone(),
                    reasoning_efforts: None,
                    input_modalities: None,
                });
            }
        }
        data.sort_by(|left, right| left.id.cmp(&right.id));
    }
    Json(OpenAiModelsResponse {
        object: "list",
        data,
        source: grok_catalog
            .as_ref()
            .map(|catalog| catalog.source.to_string())
            .or_else(|| {
                kiro_catalog
                    .as_ref()
                    .map(|catalog| catalog.source.to_string())
            })
            .or_else(|| {
                claude_catalog
                    .as_ref()
                    .map(|catalog| catalog.source.to_string())
            })
            .or_else(|| {
                cursor_catalog
                    .as_ref()
                    .map(|_| "cursor_public_api".to_string())
            }),
        stale: grok_catalog
            .as_ref()
            .map(|catalog| catalog.stale)
            .or_else(|| kiro_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| claude_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| cursor_catalog.as_ref().map(|catalog| catalog.stale)),
        fetched_at_ms: grok_catalog
            .and_then(|catalog| catalog.fetched_at_ms)
            .or_else(|| kiro_catalog.and_then(|catalog| catalog.fetched_at_ms))
            .or_else(|| claude_catalog.map(|catalog| catalog.fetched_at_ms))
            .or_else(|| cursor_catalog.map(|catalog| catalog.fetched_at_ms)),
    })
}

#[derive(Debug, Clone, Copy)]
struct CursorCatalogUse {
    stale: bool,
    fetched_at_ms: i64,
}

async fn append_cursor_api_key_models(
    state: &ServerState,
    providers: &[StoredProvider],
    app: Option<AppKind>,
    provider_id: Option<&str>,
    data: &mut Vec<OpenAiModel>,
) -> Option<CursorCatalogUse> {
    const CURSOR_MODEL_CACHE_TTL_MS: i64 = 5 * 60 * 1000;
    let mut used_catalog: Option<CursorCatalogUse> = None;
    for provider in providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::CursorApiKey
            && app.is_none_or(|app| provider.app == app)
            && provider_id.is_none_or(|id| provider.provider.id == id)
    }) {
        let Some(api_key) = cursor_provider_api_key(provider) else {
            continue;
        };
        let key_hash = hex::encode(sha2::Sha256::digest(api_key.as_bytes()));
        let now = crate::infra::time::now_ms() as i64;
        let (catalog, stale) = if let Some(catalog) =
            state.cursor_model_catalogs.fresh(&key_hash, now).await
        {
            (Some(catalog), false)
        } else {
            let _flight = state.cursor_model_catalogs.lock(&key_hash).await;
            if let Some(catalog) = state.cursor_model_catalogs.fresh(&key_hash, now).await {
                (Some(catalog), false)
            } else {
                match crate::clients::oauth::cursor::available_models(
                    &state.http_client().await,
                    &api_key,
                )
                .await
                {
                    Ok(models) if !models.is_empty() => (
                        Some(
                            state
                                .cursor_model_catalogs
                                .insert(key_hash.clone(), models, now, CURSOR_MODEL_CACHE_TTL_MS)
                                .await,
                        ),
                        false,
                    ),
                    Ok(_) => (
                        state.cursor_model_catalogs.last_known_good(&key_hash).await,
                        true,
                    ),
                    Err(error) => {
                        tracing::warn!(
                            provider_id = %provider.provider.id,
                            status_code = error.status_code,
                            error = %error,
                            "Cursor model discovery failed; using last-known-good or configured models"
                        );
                        (
                            state.cursor_model_catalogs.last_known_good(&key_hash).await,
                            true,
                        )
                    }
                }
            }
        };
        let Some(catalog) = catalog else {
            continue;
        };
        used_catalog = Some(match used_catalog {
            Some(existing) => CursorCatalogUse {
                stale: existing.stale || stale,
                fetched_at_ms: existing.fetched_at_ms.min(catalog.fetched_at_ms),
            },
            None => CursorCatalogUse {
                stale,
                fetched_at_ms: catalog.fetched_at_ms,
            },
        });
        let owner = model_owner(provider);
        let mut occupied = data
            .iter()
            .map(|model| model.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for model in catalog.models {
            for id in cursor_catalog_ids(&model, &mut occupied) {
                data.push(OpenAiModel {
                    id,
                    object: "model",
                    owned_by: owner.clone(),
                    reasoning_efforts: None,
                    input_modalities: None,
                });
            }
        }
    }
    data.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.owned_by.cmp(&right.owned_by))
    });
    used_catalog
}

fn cursor_catalog_ids(
    model: &str,
    occupied: &mut std::collections::BTreeSet<String>,
) -> Vec<String> {
    let model = model.trim();
    if model.is_empty() {
        return Vec::new();
    }
    std::iter::once(model.to_string())
        .chain(crate::proxy::cursor::model::cursor_namespaced_model_ids(
            model,
        ))
        .filter(|id| occupied.insert(id.clone()))
        .collect()
}

fn cursor_provider_api_key(provider: &StoredProvider) -> Option<String> {
    provider
        .provider
        .settings_config
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            [
                "CURSOR_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "API_KEY",
            ]
            .iter()
            .find_map(|key| {
                provider
                    .provider
                    .settings_config
                    .pointer(&format!("/env/{key}"))
                    .or_else(|| provider.provider.settings_config.get(*key))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
        })
}

#[cfg(test)]
mod cursor_provider_api_key_tests {
    use super::*;
    use crate::domain::providers::model::{AuthBinding, ProviderMeta};
    use crate::domain::providers::store::ProviderResourceMetadata;

    fn cursor_provider(settings_config: Value) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "cursor-static".to_string(),
                name: "Cursor API Key".to_string(),
                settings_config,
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some(ProviderType::CursorApiKey.as_str().to_string()),
                    auth_binding: Some(AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some(ProviderType::CursorApiKey.as_str().to_string()),
                        account_id: Some("stale-metadata-account".to_string()),
                        auth_identity_generation: Some(8),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorApiKey,
            provider_type_id: ProviderType::CursorApiKey.as_str().to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    #[test]
    fn cursor_catalog_keeps_non_conflicting_ids_and_namespaces_every_wire_model() {
        let mut occupied = ["cursor".to_string(), "shared-model".to_string()]
            .into_iter()
            .collect();

        let ids = cursor_catalog_ids("shared-model", &mut occupied);

        assert!(!ids.iter().any(|id| id == "shared-model"));
        assert!(ids.iter().any(|id| id == "cursor:shared-model"));
        assert!(ids.iter().any(|id| id == "cursor-agent:shared-model"));
        assert!(ids.iter().any(|id| id == "cursor-plan:shared-model"));
        assert!(ids.iter().any(|id| id == "cursor-ask:shared-model"));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn stale_metadata_binding_does_not_supply_cursor_api_key() {
        let provider = cursor_provider(json!({}));

        assert_eq!(cursor_provider_api_key(&provider), None);
    }

    #[test]
    fn cursor_api_key_comes_from_provider_configuration() {
        let provider = cursor_provider(json!({
            "env": {"CURSOR_API_KEY": " provider-secret "}
        }));

        assert_eq!(
            cursor_provider_api_key(&provider).as_deref(),
            Some("provider-secret")
        );
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsDispatchQuery {
    #[serde(default)]
    app: Option<AppKind>,
    #[serde(default, rename = "client_version")]
    client_version: Option<String>,
}

async fn proxy_models_or_manifest(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ModelsDispatchQuery>,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    if query
        .client_version
        .as_deref()
        .is_some_and(|version| !version.trim().is_empty())
    {
        return proxy::forward_codex_models_manifest(state, headers, query.client_version)
            .await
            .map_err(|error| {
                InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error)
            });
    }

    Ok(
        proxy_models(State(state), headers, Query(ModelsQuery { app: query.app }))
            .await?
            .into_response(),
    )
}

#[derive(Debug, Deserialize)]
struct CodexModelsManifestQuery {
    client_version: Option<String>,
}

async fn proxy_codex_models_manifest(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<CodexModelsManifestQuery>,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_codex_models_manifest(state, headers, query.client_version)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_codex_alpha_search(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_codex_alpha_search(state, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_responses_input_tokens(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    let (_provider_id, _share_guard) =
        validate_router_share_surface(&state, &headers, AppKind::Codex)
            .await
            .map_err(|error| {
                InferenceApiError::proxy(InferenceSurface::OpenAi, request_id.clone(), error)
            })?;
    responses_input_tokens_response(&headers, body)
        .map_err(|error| InferenceApiError::api(InferenceSurface::OpenAi, request_id, error))
}

fn responses_input_tokens_response(headers: &HeaderMap, body: Bytes) -> Result<Response, ApiError> {
    const BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
    let body =
        crate::proxy::decode_request_body_for_proxy_with_limit(headers, body, BODY_LIMIT_BYTES)
            .map_err(ApiError::proxy)?;
    let request = serde_json::from_slice::<Value>(&body)
        .map_err(|error| ApiError::bad_request(format!("invalid token count JSON: {error}")))?;
    let payload = json!({
        "instructions": request.get("instructions"),
        "input": request.get("input"),
        "tools": request.get("tools"),
    });
    let characters = serde_json::to_string(&payload)
        .map_err(ApiError::internal)?
        .chars()
        .count();
    let input_tokens = if characters == 0 {
        0
    } else {
        characters.saturating_add(2) / 3 + 8
    };
    let mut response = Json(json!({
        "input_tokens": input_tokens,
        "estimated": true,
        "estimation_method": "json_characters_div_3_plus_8"
    }))
    .into_response();
    response.headers_mut().insert(
        "x-cc-switch-token-count",
        HeaderValue::from_static("estimated"),
    );
    Ok(response)
}

#[cfg(test)]
mod responses_input_token_tests {
    use std::io::Write;

    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    #[tokio::test]
    async fn compressed_input_token_request_is_bounded_after_decode() {
        let oversized = json!({"input": "x".repeat(2 * 1024 * 1024)}).to_string();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(oversized.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );

        let error = responses_input_tokens_response(&headers, Bytes::from(compressed)).unwrap_err();

        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.message.contains("2097152 byte limit"));
    }
}

fn resolve_grok_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::GrokOAuth
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn resolve_claude_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::ClaudeOAuth
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn grok_catalog_managed_account_binding(
    providers: &crate::domain::providers::store::ProviderStore,
    provider: &StoredProvider,
) -> Option<(String, u64)> {
    let plan = providers.runtime_plan(provider.app, &provider.provider.id)?;
    if plan.provider_revision != provider.resource.revision
        || plan.configuration_state == RuntimeConfigurationState::NeedsAttention
        || plan.driver_id.as_str() != "oauth.grok_responses"
    {
        return None;
    }
    match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::GrokOAuth,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => Some((account_id.clone(), *auth_identity_generation)),
        _ => None,
    }
}

fn resolve_kiro_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::KiroOAuth
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn kiro_catalog_managed_account_binding(
    providers: &crate::domain::providers::store::ProviderStore,
    provider: &StoredProvider,
) -> Option<(String, u64)> {
    let plan = providers.runtime_plan(provider.app, &provider.provider.id)?;
    if plan.provider_revision != provider.resource.revision
        || plan.configuration_state == RuntimeConfigurationState::NeedsAttention
        || plan.driver_id.as_str() != "special.kiro"
    {
        return None;
    }
    match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::KiroOAuth,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => Some((account_id.clone(), *auth_identity_generation)),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod grok_catalog_provider_tests {
    use super::*;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta};
    use crate::domain::providers::store::ProviderStore;
    use crate::domain::settings::config::RouterIdentity;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use tower::ServiceExt;

    const TEST_ROUTER_DOMAIN: &str = "router.test";
    const TEST_INSTALLATION_ID: &str = "inst-api-tests";
    const TEST_CONTROL_SECRET: &str = "api-test-control-secret-0123456789";
    static TEST_INGRESS_REQUEST_SEQUENCE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1);

    fn unique_test_ingress_request_id(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            TEST_INGRESS_REQUEST_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    fn catalog_test_state(name: &str) -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir()
                        .join(format!("cc-switch-server-model-catalog-{name}-{nanos}")),
                ),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap()
    }

    async fn configure_test_router(state: &ServerState) {
        let mut config = state.config_snapshot().await;
        config.router.domain = Some(TEST_ROUTER_DOMAIN.to_string());
        config.router.identity = Some(RouterIdentity {
            installation_id: TEST_INSTALLATION_ID.to_string(),
            public_key: "public-key".to_string(),
            private_key: "private-key".to_string(),
            control_secret: Some(TEST_CONTROL_SECRET.to_string()),
        });
        state.replace_config(config).await.unwrap();
    }

    async fn configure_test_share(state: &ServerState, share_id: &str, app: AppKind) {
        configure_test_share_with_provider_type(state, share_id, app, ProviderType::GrokOAuth)
            .await;
    }

    async fn configure_test_share_with_provider_type(
        state: &ServerState,
        share_id: &str,
        app: AppKind,
        provider_type: ProviderType,
    ) {
        let provider_id = format!("{share_id}-provider");
        let provider_id_for_store = provider_id.clone();
        let provider_name = format!("{share_id} Provider");
        let managed_account_id = format!("{share_id}-account");
        if provider_type == ProviderType::ClaudeOAuth {
            let account_id = managed_account_id.clone();
            state
                .mutate_accounts_immediate(move |accounts| {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": account_id,
                            "providerType": "claude_oauth",
                            "accessToken": "test-access-token",
                            "expiresAt": i64::MAX / 2
                        }))
                        .unwrap(),
                    );
                })
                .await
                .unwrap();
        }
        let auth_binding = (provider_type == ProviderType::ClaudeOAuth).then(|| AuthBinding {
            source: Some("account_store".to_string()),
            auth_provider: Some(provider_type.as_str().to_string()),
            account_id: Some(managed_account_id),
            auth_identity_generation: Some(1),
        });
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    app,
                    Provider {
                        id: provider_id_for_store,
                        name: provider_name,
                        settings_config: json!({}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some(provider_type.as_str().to_string()),
                            auth_binding,
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                )
            })
            .await
            .unwrap();
        state
            .mutate_shares_immediate(|shares| {
                shares
                    .upsert(UpsertShareInput {
                        id: Some(share_id.to_string()),
                        owner_email: Some("owner@example.com".to_string()),
                        app,
                        provider_id: provider_id.clone(),
                        provider_type,
                        display_name: Some(share_id.to_string()),
                        enabled: Some(true),
                        status: Some("active".to_string()),
                        subscription_level: None,
                        account_email: None,
                        quota_percent: None,
                        tunnel_subdomain: Some(share_id.to_string()),
                        acl: None,
                        token_limit: None,
                        parallel_limit: None,
                        expires_at: None,
                        for_sale: Some(false),
                        free_access: Some(false),
                        access_by_app: Default::default(),
                        app_settings: Default::default(),
                        for_sale_official_price_percent_by_app: Default::default(),
                        official_price_percent: None,
                        allow_personal_credits: None,
                        auto_consume_banked_reset: None,
                        banked_reset_expiry_lead_minutes: None,
                        previous_response_cache_enabled: None,
                        auto_start: Some(true),
                        description: None,
                        bindings: vec![ShareBinding {
                            app,
                            provider_id,
                            provider_type,
                        }],
                        runtime_snapshot: None,
                        user_grants: Default::default(),
                    })
                    .unwrap()
            })
            .await
            .unwrap();
    }

    async fn router_ingress_request(
        request: Request,
        request_id: &str,
        share_id: Option<&str>,
    ) -> Request {
        router_ingress_request_at(
            request,
            request_id,
            share_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
    }

    async fn router_ingress_request_at(
        request: Request,
        request_id: &str,
        share_id: Option<&str>,
        issued_at_ms: i64,
    ) -> Request {
        router_ingress_request_with_version_at(
            request,
            &unique_test_ingress_request_id(request_id),
            share_id,
            crate::clients::router::ingress::SIGNATURE_VERSION_V2,
            issued_at_ms,
        )
        .await
    }

    async fn router_ingress_request_with_version_at(
        request: Request,
        request_id: &str,
        share_id: Option<&str>,
        signature_version: u8,
        issued_at_ms: i64,
    ) -> Request {
        let method = request.method().as_str().to_string();
        let path_and_query = request
            .uri()
            .path_and_query()
            .map(|target| target.as_str().to_string())
            .unwrap_or_else(|| "/".to_string());
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let (method, path_and_query, body_sha256) =
            if signature_version == crate::clients::router::ingress::SIGNATURE_VERSION_V2 {
                (
                    method,
                    path_and_query,
                    crate::clients::router::ingress::body_sha256_hex(&body),
                )
            } else {
                (String::new(), String::new(), String::new())
            };
        let context = crate::clients::router::ingress::IngressContext {
            signature_version,
            protocol_epoch: crate::clients::router::ingress::PROTOCOL_EPOCH.to_string(),
            router_id: TEST_ROUTER_DOMAIN.to_string(),
            route_id: share_id
                .map(|share_id| format!("share:{share_id}"))
                .unwrap_or_else(|| format!("client:{TEST_INSTALLATION_ID}")),
            installation_id: TEST_INSTALLATION_ID.to_string(),
            target_lane_id: TEST_INSTALLATION_ID.to_string(),
            public_host: "share--client.router.test".to_string(),
            share_id: share_id.map(str::to_string),
            request_id: request_id.to_string(),
            user_email: Some("owner@example.com".to_string()),
            user_role: share_id.is_none().then(|| "owner".to_string()),
            user_country: Some("JP".to_string()),
            method,
            path_and_query,
            body_sha256,
            issued_at_ms,
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&context).unwrap());
        let mut mac = Hmac::<Sha256>::new_from_slice(TEST_CONTROL_SECRET.as_bytes()).unwrap();
        mac.update(match signature_version {
            crate::clients::router::ingress::SIGNATURE_VERSION_V1 => {
                b"cc-switch-router-ingress-v1\n"
            }
            crate::clients::router::ingress::SIGNATURE_VERSION_V2 => {
                b"cc-switch-router-ingress-v2\n"
            }
            _ => panic!("unsupported test signature version"),
        });
        mac.update(crate::clients::router::ingress::PROTOCOL_EPOCH.as_bytes());
        mac.update(b"\n");
        mac.update(encoded.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let mut request = Request::from_parts(parts, Body::from(body));
        request.headers_mut().insert(
            crate::clients::router::ingress::INGRESS_CONTEXT_HEADER,
            encoded.parse().unwrap(),
        );
        request.headers_mut().insert(
            crate::clients::router::ingress::INGRESS_SIGNATURE_HEADER,
            signature.parse().unwrap(),
        );
        request
    }

    #[tokio::test]
    async fn ingress_freshness_rejections_include_internal_diagnostics() {
        let state = catalog_test_state("ingress-freshness-diagnostics");
        configure_test_router(&state).await;
        let app = app_router(state);
        let now_ms = chrono::Utc::now().timestamp_millis();

        for (issued_at_ms, expected_code) in [
            (
                now_ms - crate::clients::router::ingress::DEFAULT_MAX_CONTEXT_AGE_MS - 1_000,
                "expired",
            ),
            (
                now_ms + crate::clients::router::ingress::DEFAULT_FUTURE_CLOCK_SKEW_MS + 1_000,
                "future_timestamp",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    router_ingress_request_at(
                        axum::http::Request::builder()
                            .uri("/web-api/auth/methods")
                            .body(Body::empty())
                            .unwrap(),
                        "freshness-rejection",
                        None,
                        issued_at_ms,
                    )
                    .await,
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response
                    .headers()
                    .get(crate::clients::router::ingress::INTERNAL_INGRESS_ERROR_HEADER)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_code)
            );
            assert!(response
                .headers()
                .contains_key(crate::clients::router::ingress::INTERNAL_INGRESS_AGE_MS_HEADER));
            assert!(response.headers().contains_key(
                crate::clients::router::ingress::INTERNAL_INGRESS_SERVER_TIME_MS_HEADER
            ));
            assert!(axum::body::to_bytes(response.into_body(), 1)
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn claude_oauth_models_route_uses_versioned_static_catalog() {
        let state = catalog_test_state("claude-oauth-static-models");
        configure_test_router(&state).await;
        configure_test_share_with_provider_type(
            &state,
            "share-claude-models",
            AppKind::Claude,
            ProviderType::ClaudeOAuth,
        )
        .await;
        let app = app_router(state);

        let response = app
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1/models?app=claude")
                        .body(Body::empty())
                        .unwrap(),
                    "claude-oauth-models",
                    Some("share-claude-models"),
                )
                .await,
            )
            .await
            .unwrap();

        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["object"], "list");
        assert_eq!(body["source"], "claude_code_wire_profile");
        assert_eq!(body["stale"], false);
        assert_eq!(
            body["fetchedAtMs"],
            crate::clients::oauth::claude_models::CLAUDE_MODEL_CATALOG_CAPTURED_AT_MS
        );
        let models = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut expected = crate::clients::oauth::claude_models::CLAUDE_MODEL_IDS.to_vec();
        expected.sort_unstable();
        assert_eq!(models, expected);
    }

    #[tokio::test]
    async fn ingress_v2_rejects_request_binding_tampering_and_replay() {
        let state = catalog_test_state("ingress-v2-binding");
        configure_test_router(&state).await;
        let app = app_router(state);

        let mut method_tampered = router_ingress_request(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/auth/methods")
                .body(Body::empty())
                .unwrap(),
            "method-tamper",
            None,
        )
        .await;
        *method_tampered.method_mut() = Method::POST;

        let mut path_tampered = router_ingress_request(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/auth/methods?source=signed")
                .body(Body::empty())
                .unwrap(),
            "path-tamper",
            None,
        )
        .await;
        *path_tampered.uri_mut() = "/web-api/auth/methods?source=changed".parse().unwrap();

        let mut body_tampered = router_ingress_request(
            Request::builder()
                .method(Method::POST)
                .uri("/web-api/auth/methods")
                .body(Body::from("signed"))
                .unwrap(),
            "body-tamper",
            None,
        )
        .await;
        *body_tampered.body_mut() = Body::from("changed");

        for (request, expected_code) in [
            (method_tampered, "method_mismatch"),
            (path_tampered, "path_mismatch"),
            (body_tampered, "body_digest_mismatch"),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response
                    .headers()
                    .get(crate::clients::router::ingress::INTERNAL_INGRESS_ERROR_HEADER)
                    .and_then(|value| value.to_str().ok()),
                Some(expected_code)
            );
        }

        let replay_request_id = "fixed-replay-request-id";
        let first = router_ingress_request_with_version_at(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/auth/methods")
                .body(Body::empty())
                .unwrap(),
            replay_request_id,
            None,
            crate::clients::router::ingress::SIGNATURE_VERSION_V2,
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
        let second = router_ingress_request_with_version_at(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/auth/methods")
                .body(Body::empty())
                .unwrap(),
            replay_request_id,
            None,
            crate::clients::router::ingress::SIGNATURE_VERSION_V2,
            chrono::Utc::now().timestamp_millis(),
        )
        .await;

        let first = app.clone().oneshot(first).await.unwrap();
        assert_ne!(first.status(), StatusCode::UNAUTHORIZED);
        let replay = app.oneshot(second).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            replay
                .headers()
                .get(crate::clients::router::ingress::INTERNAL_INGRESS_ERROR_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("replayed_request_id")
        );
    }

    #[test]
    fn normal_ingress_responses_cannot_spoof_internal_diagnostics() {
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        response.headers_mut().insert(
            crate::clients::router::ingress::INTERNAL_INGRESS_ERROR_HEADER,
            HeaderValue::from_static("expired"),
        );
        response.headers_mut().insert(
            crate::clients::router::ingress::INTERNAL_INGRESS_AGE_MS_HEADER,
            HeaderValue::from_static("31000"),
        );
        response
            .headers_mut()
            .insert("x-application-header", HeaderValue::from_static("kept"));

        strip_internal_ingress_response_headers(&mut response);

        assert!(response
            .headers()
            .get(crate::clients::router::ingress::INTERNAL_INGRESS_ERROR_HEADER)
            .is_none());
        assert_eq!(
            response.headers().get("x-application-header"),
            Some(&HeaderValue::from_static("kept"))
        );
    }

    fn grok_provider(id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::GrokOAuth,
            provider_type_id: ProviderType::GrokOAuth.as_str().to_string(),
            resource: Default::default(),
        }
    }

    #[test]
    fn untargeted_model_catalog_does_not_select_an_arbitrary_grok_account() {
        let providers = ProviderStore {
            providers: vec![grok_provider("grok-first"), grok_provider("grok-second")],
            ..Default::default()
        };

        assert!(resolve_grok_catalog_provider(&providers, None, None).is_none());
    }

    #[test]
    fn model_catalog_requires_an_explicit_route_or_share_binding() {
        let providers = ProviderStore {
            providers: vec![grok_provider("grok-first"), grok_provider("grok-current")],
            ..Default::default()
        };
        let explicit = resolve_grok_catalog_provider(&providers, None, Some("grok-first")).unwrap();
        assert_eq!(explicit.provider.id, "grok-first");

        assert!(resolve_grok_catalog_provider(&providers, Some(AppKind::Codex), None).is_none());
    }

    #[test]
    fn provider_id_without_app_must_identify_one_grok_provider() {
        let mut claude = grok_provider("shared-id");
        claude.app = AppKind::Claude;
        let providers = ProviderStore {
            providers: vec![grok_provider("shared-id"), claude],
            ..Default::default()
        };

        assert!(resolve_grok_catalog_provider(&providers, None, Some("shared-id")).is_none());
        assert_eq!(
            resolve_grok_catalog_provider(&providers, Some(AppKind::Codex), Some("shared-id"),)
                .unwrap()
                .app,
            AppKind::Codex,
        );
    }

    #[tokio::test]
    async fn media_routes_override_the_default_body_limit_but_remain_bounded() {
        let state = catalog_test_state("media-body-limit");
        configure_test_router(&state).await;
        let app = app_router(state);

        let above_default = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/images/edits")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b' '; 2 * 1024 * 1024 + 1]))
                .unwrap(),
            "media-above-default",
            Some("share-media"),
        )
        .await;
        let accepted_by_extractor = app.clone().oneshot(above_default).await.unwrap();
        assert_ne!(
            accepted_by_extractor.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let codex_envelope = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/images/edits")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![
                    b' ';
                    proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES + 1
                ]))
                .unwrap(),
            "media-codex-envelope",
            Some("share-media"),
        )
        .await;
        let accepted_codex_envelope = app.clone().oneshot(codex_envelope).await.unwrap();
        assert_ne!(
            accepted_codex_envelope.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let above_images_envelope = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/images/edits")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![
                    b' ';
                    proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES
                        + 1
                ]))
                .unwrap(),
            "media-above-images-envelope",
            Some("share-media"),
        )
        .await;
        let rejected = app.oneshot(above_images_envelope).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn ephemeral_image_capability_url_supports_get_head_and_rejects_invalid_tokens() {
        let state = catalog_test_state("ephemeral-image-download");
        configure_test_router(&state).await;
        configure_test_share(&state, "share-image", AppKind::Codex).await;
        let image = Bytes::from_static(b"\x89PNG\r\n\x1a\nfixture");
        let handle = state
            .store_image_capability(image.clone(), "image/png".to_string())
            .await
            .unwrap();
        let path = format!("/v1/images/files/{}", handle.token);
        let app = app_router(state);

        for method in [Method::GET, Method::HEAD] {
            let unsigned = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(method.clone())
                        .uri(&path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

            let client_ingress = app
                .clone()
                .oneshot(
                    router_ingress_request(
                        axum::http::Request::builder()
                            .method(method.clone())
                            .uri(&path)
                            .body(Body::empty())
                            .unwrap(),
                        &format!("image-client-{}", method.as_str().to_ascii_lowercase()),
                        None,
                    )
                    .await,
                )
                .await
                .unwrap();
            assert_eq!(client_ingress.status(), StatusCode::FORBIDDEN);
        }

        let get_response = app
            .clone()
            .oneshot(
                router_ingress_request(
                    axum::http::Request::builder()
                        .method(Method::GET)
                        .uri(&path)
                        .body(Body::empty())
                        .unwrap(),
                    "image-get",
                    Some("share-image"),
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::OK);
        assert_eq!(get_response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            get_response.headers()[header::CACHE_CONTROL],
            "private, no-store, max-age=0"
        );
        assert_eq!(get_response.headers()["x-content-type-options"], "nosniff");
        let body = axum::body::to_bytes(get_response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body, image);

        let head_response = app
            .clone()
            .oneshot(
                router_ingress_request(
                    axum::http::Request::builder()
                        .method(Method::HEAD)
                        .uri(&path)
                        .body(Body::empty())
                        .unwrap(),
                    "image-head",
                    Some("share-image"),
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_eq!(head_response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(head_response.headers()[header::CONTENT_LENGTH], "15");
        assert_eq!(
            head_response.headers()[header::CACHE_CONTROL],
            "private, no-store, max-age=0"
        );
        assert_eq!(head_response.headers()["x-content-type-options"], "nosniff");
        assert!(axum::body::to_bytes(head_response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty());

        for invalid_path in [
            "/v1/images/files/not-a-token".to_string(),
            format!("/v1/images/files/{}", "0".repeat(64)),
        ] {
            let response = app
                .clone()
                .oneshot(
                    router_ingress_request(
                        axum::http::Request::builder()
                            .method(Method::GET)
                            .uri(invalid_path)
                            .body(Body::empty())
                            .unwrap(),
                        "image-invalid",
                        Some("share-image"),
                    )
                    .await,
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn inference_requires_signed_share_ingress_and_route_key_paths_are_gone() {
        let state = catalog_test_state("share-ingress-only");
        configure_test_router(&state).await;
        let app = app_router(state);

        let unsigned = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/messages")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

        let client_ingress = app
            .clone()
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/v1/messages")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                    "client-inference",
                    None,
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(client_ingress.status(), StatusCode::FORBIDDEN);

        for (method, uri, body) in [
            (Method::GET, "/v1/models", Body::empty()),
            (Method::POST, "/v1/responses/input_tokens", Body::from("{}")),
            (Method::GET, "/v1beta/models", Body::empty()),
        ] {
            let response = app
                .clone()
                .oneshot(
                    router_ingress_request(
                        Request::builder()
                            .method(method)
                            .uri(uri)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(body)
                            .unwrap(),
                        &format!("missing-share-{uri}"),
                        Some("missing-share"),
                    )
                    .await,
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }

        let legacy_route = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/r/provider/v1/messages")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy_route.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auxiliary_inference_routes_reject_disabled_share_surfaces() {
        let state = catalog_test_state("disabled-share-surface");
        configure_test_router(&state).await;
        configure_test_share(&state, "share-disabled", AppKind::Codex).await;
        state
            .mutate_providers_immediate(|providers| {
                let provider = providers
                    .providers
                    .iter_mut()
                    .find(|stored| {
                        stored.app == AppKind::Codex
                            && stored.provider.id == "share-disabled-provider"
                    })
                    .unwrap();
                provider
                    .provider
                    .extra
                    .insert("bundleId".to_string(), json!("share-disabled-provider"));
                provider
                    .provider
                    .extra
                    .insert("familyId".to_string(), json!("family.grok_oauth"));
                provider
                    .provider
                    .extra
                    .insert("surfaceEnabled".to_string(), json!(false));
                providers
                    .bundle_order
                    .push("share-disabled-provider".to_string());
            })
            .await
            .unwrap();
        let app = app_router(state);

        let response = app
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1/models")
                        .body(Body::empty())
                        .unwrap(),
                    "disabled-share-surface",
                    Some("share-disabled"),
                )
                .await,
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn model_catalog_stops_before_upstream_when_refresh_enters_degraded_mode() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let upstream = Router::new()
            .route(
                "/token",
                post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({
                            "access_token": "rotated-model-access",
                            "refresh_token": "rotated-model-refresh",
                            "expires_in": 3_600
                        }))
                    }
                }),
            )
            .route(
                "/models",
                get(move || {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"data": [{"id": "must-not-be-requested"}]}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("refresh-degraded");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-degraded-model-account",
                        "providerType": "grok_oauth",
                        "accessToken": "expiring-model-access",
                        "refreshToken": "unique-degraded-model-refresh",
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "grok-degraded-model-subject"}
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-degraded-model-provider".to_string(),
                        name: "Grok degraded model provider".to_string(),
                        settings_config: json!({"testGrokModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some("grok-degraded-model-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        state.inject_account_refresh_persist_failures(1);

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("grok-degraded-model-provider"),
        )
        .await
        .0;

        assert_eq!(response.source.as_deref(), Some("static_fallback"));
        assert_eq!(response.stale, Some(true));
        assert!(state.credential_persistence_degraded());
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_stops_before_models_when_token_refresh_fails() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let upstream = Router::new()
            .route(
                "/token",
                post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"error": "invalid_grant"})),
                        )
                    }
                }),
            )
            .route(
                "/models",
                get(move || {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"data": [{"id": "must-not-be-requested"}]}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("refresh-rejected");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-refresh-rejected-account",
                        "providerType": "grok_oauth",
                        "accessToken": "stale-model-access",
                        "refreshToken": "rejected-model-refresh",
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "grok-refresh-rejected-subject"}
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-refresh-rejected-provider".to_string(),
                        name: "Grok refresh rejected provider".to_string(),
                        settings_config: json!({"testGrokModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some("grok-refresh-rejected-account".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("grok-refresh-rejected-provider"),
        )
        .await
        .0;

        assert_eq!(response.source.as_deref(), Some("static_fallback"));
        assert_eq!(response.stale, Some(true));
        assert!(!state.credential_persistence_degraded());
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_rejects_unbound_or_stale_binding_without_upstream_requests() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let upstream = Router::new()
            .route(
                "/token",
                post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"access_token": "must-not-be-requested"}))
                    }
                }),
            )
            .route(
                "/models",
                get(move || {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({"data": [{"id": "must-not-be-requested"}]}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("invalid-managed-binding");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "grok-invalid-binding-account",
                        "providerType": "grok_oauth",
                        "accessToken": "expiring-invalid-binding-access",
                        "refreshToken": "invalid-binding-refresh",
                        "expiresAt": 1,
                        "profile": {
                            "verifiedGrokClaims": {"subject": "invalid-binding-subject"}
                        },
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                for (id, auth_binding) in [
                    ("grok-unbound-model-provider", None),
                    (
                        "grok-stale-model-provider",
                        Some(AuthBinding {
                            source: Some("account_store".to_string()),
                            auth_provider: Some("grok_oauth".to_string()),
                            account_id: Some("grok-invalid-binding-account".to_string()),
                            auth_identity_generation: Some(2),
                        }),
                    ),
                ] {
                    providers.upsert(
                        AppKind::Codex,
                        Provider {
                            id: id.to_string(),
                            name: id.to_string(),
                            settings_config: json!({"testGrokModelsUrl": models_url}),
                            category: None,
                            meta: Some(ProviderMeta {
                                provider_type: Some("grok_oauth".to_string()),
                                auth_binding,
                                ..Default::default()
                            }),
                            extra: Default::default(),
                        },
                    );
                }
            })
            .await
            .unwrap();

        for provider_id in ["grok-unbound-model-provider", "grok-stale-model-provider"] {
            let response =
                proxy_models_for_selection(&state, Some(AppKind::Codex), Some(provider_id))
                    .await
                    .0;
            assert_eq!(response.source.as_deref(), Some("static_fallback"));
            assert_eq!(response.stale, Some(true));
            assert_eq!(response.fetched_at_ms, None);
        }

        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_uses_only_the_runtime_bound_account() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed_authorization = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let observed_for_route = std::sync::Arc::clone(&observed_authorization);
        let upstream = Router::new().route(
            "/models",
            get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    *observed.lock().unwrap() = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    Json(json!({"data": [{"id": "grok-runtime-bound"}]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("runtime-bound-account");
        state
            .mutate_accounts_immediate(|accounts| {
                for (id, access_token) in [
                    ("grok-model-distractor", "distractor-access"),
                    ("grok-model-bound", "runtime-bound-access"),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "grok_oauth",
                            "accessToken": access_token,
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "verifiedGrokClaims": {"subject": format!("{id}-subject")}
                            }
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "grok-runtime-bound-provider".to_string(),
                        name: "Grok runtime-bound provider".to_string(),
                        settings_config: json!({"testGrokModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("grok_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("grok_oauth".to_string()),
                                account_id: Some("grok-model-bound".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("grok-runtime-bound-provider"),
        )
        .await
        .0;

        assert_eq!(response.source.as_deref(), Some("upstream"));
        assert!(response
            .data
            .iter()
            .any(|model| model.id == "grok-runtime-bound"));
        assert_eq!(
            observed_authorization.lock().unwrap().as_str(),
            "Bearer runtime-bound-access"
        );
        server.abort();
    }

    #[tokio::test]
    async fn kiro_share_catalog_replaces_static_fallback_without_account_union() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed_authorization = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let observed_for_route = std::sync::Arc::clone(&observed_authorization);
        let upstream = Router::new().route(
            "/models",
            get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    *observed.lock().unwrap() = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    Json(json!({"models": [{"modelId": "kiro-runtime-only"}]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("kiro-runtime-bound-account");
        state
            .mutate_accounts_immediate(|accounts| {
                for (id, access_token) in [
                    ("kiro-model-distractor", "kiro-distractor-access"),
                    ("kiro-model-bound", "kiro-runtime-bound-access"),
                ] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "kiro_oauth",
                            "accessToken": access_token,
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test",
                                "apiRegion": "us-east-1",
                                "machineId": id,
                                "authMethod": "social"
                            }
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let models_url = format!("http://{address}/models");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: "kiro-runtime-bound-provider".to_string(),
                        name: "Kiro runtime-bound provider".to_string(),
                        settings_config: json!({"testKiroModelsUrl": models_url}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kiro_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kiro_oauth".to_string()),
                                account_id: Some("kiro-model-bound".to_string()),
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("kiro-runtime-bound-provider"),
        )
        .await
        .0;

        assert_eq!(
            response.source.as_deref(),
            Some("kiro_list_available_models")
        );
        assert_eq!(
            response
                .data
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["kiro-runtime-only"]
        );
        assert_eq!(
            observed_authorization.lock().unwrap().as_str(),
            "Bearer kiro-runtime-bound-access"
        );
        server.abort();
    }
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
            "providerContract": provider_contract_context(),
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
        "providerContract": provider_contract_context(),
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
            "embeddedWebAssets": web_assets::asset_count(),
            "enableWebTerminal": config.is_web_terminal_enabled()
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

fn provider_contract_context() -> Value {
    json!({
        "version": web_runtime::PROVIDER_CONTRACT_VERSION,
        "minSupported": web_runtime::PROVIDER_CONTRACT_MIN_SUPPORTED,
        "maxSupported": web_runtime::PROVIDER_CONTRACT_MAX_SUPPORTED,
    })
}

async fn proxy_claude_messages(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward(state, ProxyRoute::ClaudeMessages, None, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::Anthropic, request_id, error))
}

async fn proxy_claude_count_tokens(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward(state, ProxyRoute::ClaudeCountTokens, None, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::Anthropic, request_id, error))
}

async fn proxy_codex_chat_completions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward(state, ProxyRoute::CodexChatCompletions, None, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_codex_responses(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward(state, ProxyRoute::CodexResponses, None, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_codex_responses_compact(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward(
        state,
        ProxyRoute::CodexResponsesCompact,
        None,
        headers,
        body,
    )
    .await
    .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_codex_responses_ws(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_codex_responses_ws(state, headers, ws)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_images_generations(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_images_generations(state, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn ephemeral_image_file(
    State(state): State<ServerState>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    let (_provider_id, _share_guard) =
        validate_router_share_surface(&state, &headers, AppKind::Codex)
            .await
            .map_err(|error| {
                InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error)
            })?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let image = match state.image_capability(token).await {
        Ok(Some(image)) => image,
        Ok(None) => return Ok(StatusCode::NOT_FOUND.into_response()),
        Err(error) => {
            tracing::error!(error = %error, "image capability read failed");
            return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(image.data.clone())
    };
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        image
            .mime_type
            .parse()
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&image.data.len().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("cross-origin"),
    );
    Ok(response)
}

async fn proxy_images_edits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_images_edits(state, headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_grok_videos_generations(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    proxy::forward_grok_media(
        state,
        Method::POST,
        "/videos/generations".to_string(),
        headers,
        body,
    )
    .await
    .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, request_id, error))
}

async fn proxy_grok_video_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Result<Response, InferenceApiError> {
    let ingress_request_id = inference_request_id(&headers);
    proxy::forward_grok_media(
        state,
        Method::GET,
        format!("/videos/{request_id}"),
        headers,
        Bytes::new(),
    )
    .await
    .map_err(|error| InferenceApiError::proxy(InferenceSurface::OpenAi, ingress_request_id, error))
}

async fn proxy_gemini(
    method: Method,
    State(state): State<ServerState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, InferenceApiError> {
    let request_id = inference_request_id(&headers);
    if method == Method::GET {
        if let Some(response) = gemini_models_response(&state, &headers, &path)
            .await
            .map_err(|error| {
                InferenceApiError::proxy(InferenceSurface::Gemini, request_id.clone(), error)
            })?
        {
            return Ok(response);
        }
    }
    proxy::forward(state, ProxyRoute::Gemini, Some(path), headers, body)
        .await
        .map_err(|error| InferenceApiError::proxy(InferenceSurface::Gemini, request_id, error))
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
        OAuthLoginError::PrincipalMismatch => ApiError::forbidden(error.to_string()),
        error @ (OAuthLoginError::MissingCode
        | OAuthLoginError::StateMismatch
        | OAuthLoginError::ProviderMismatch) => ApiError::bad_request(error),
        error @ (OAuthLoginError::Expired
        | OAuthLoginError::AlreadyConsumed
        | OAuthLoginError::Cancelled
        | OAuthLoginError::InvalidTransition) => ApiError::conflict(error.to_string()),
        OAuthLoginError::NotFound => ApiError::not_found(error.to_string()),
    }
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
