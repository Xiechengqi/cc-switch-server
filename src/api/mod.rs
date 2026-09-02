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
    share_router_model_health_batch, share_router_model_health_batch_v2, share_router_request_logs,
    share_router_runtime,
};
pub use control::{
    control_signature, control_signature_for_method, refresh_share_usage_items,
    ControlRefreshShareUsageItem,
};
pub(in crate::api) use debug::*;
pub use error::ApiError;
pub(crate) use error::{
    map_account_write_error, map_amazon_q_device_error, map_codebuddy_client_error,
    map_codex_active_account_selection_error, map_codex_device_error,
    map_codex_workspace_rebind_error, map_copilot_device_error, map_email_auth_error,
    map_grok_device_error, map_kimi_device_error, map_kiro_device_error, map_qoder_client_error,
    map_share_patch_error, map_subscription_binding_error, map_trae_client_error,
    map_web_auth_error, ErrorResponse, InferenceApiError, InferenceSurface,
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
use axum::extract::Request;
use axum::extract::State;
use axum::extract::{Query, RawQuery};
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
use zeroize::Zeroizing;

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
use crate::domain::providers::store::{ProviderSortUpdate, ProviderStore, StoredProvider};
use crate::domain::settings::config::{
    ServerConfig, UpdateClientTunnelInput, UpdateRouterConfigInput,
};
use crate::domain::settings::ui_settings;
use crate::domain::sharing::shares::{
    Share, ShareBinding, ShareDeleteTombstone, ShareStore, UpsertShareInput,
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
            "/_share-router/model-health/batch",
            post(share_router_model_health_batch),
        )
        .route(
            "/_share-router/model-health/batch-v2",
            post(share_router_model_health_batch_v2),
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
            "/api/providers/:id/inference-test",
            post(test_provider_inference),
        )
        .route(
            "/api/providers/:id/coding-plan-quota",
            get(get_coding_plan_quota).post(refresh_coding_plan_quota),
        )
        .route(
            "/api/providers/:id/account-usage",
            get(get_provider_account_usage).post(refresh_provider_account_usage),
        )
        .route(
            "/api/providers/:id/cursor-account",
            get(get_cursor_account).post(refresh_cursor_account),
        )
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
            "/api/accounts/amazon-q/device/start",
            post(start_amazon_q_device_login),
        )
        .route(
            "/api/accounts/amazon-q/device/poll",
            post(poll_amazon_q_device_login),
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
        .route(
            "/api/accounts/qoder/device/start",
            post(start_qoder_device_login),
        )
        .route(
            "/api/accounts/qoder/device/poll",
            post(poll_qoder_device_login),
        )
        .route(
            "/api/accounts/qoder/device/cancel",
            post(cancel_qoder_device_login),
        )
        .route(
            "/api/accounts/codebuddy/login/start",
            post(start_codebuddy_login),
        )
        .route(
            "/api/accounts/codebuddy/login/poll",
            post(poll_codebuddy_login),
        )
        .route(
            "/api/accounts/codebuddy/login/cancel",
            post(cancel_codebuddy_login),
        )
        .route("/api/accounts/trae/login/start", post(start_trae_login))
        .route("/api/accounts/trae/login/status", post(trae_login_status))
        .route(
            "/api/accounts/trae/login/complete",
            post(complete_trae_login),
        )
        .route("/api/accounts/trae/login/cancel", post(cancel_trae_login))
        .route(
            "/api/accounts/trae/login/callback",
            get(trae_login_callback),
        )
        .route("/api/accounts/qoder/pat/import", post(import_qoder_pat))
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
    // 启动时定格的本地上限。它只是内存兜底：Router ingress 请求的真正策略闸门是
    // `verify_router_ingress` 里的 `min(本地上限, Router 声明上限)`。
    let limits = state.request_body_limits;
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
            post(proxy_images_generations).layer(DefaultBodyLimit::max(limits.image_bytes)),
        )
        .route(
            "/images/generations",
            post(proxy_images_generations).layer(DefaultBodyLimit::max(limits.image_bytes)),
        )
        .route(
            "/v1/images/edits",
            post(proxy_images_edits).layer(DefaultBodyLimit::max(limits.image_bytes)),
        )
        .route(
            "/images/edits",
            post(proxy_images_edits).layer(DefaultBodyLimit::max(limits.image_bytes)),
        )
        .route(
            "/v1/videos/generations",
            post(proxy_grok_videos_generations).layer(DefaultBodyLimit::max(limits.media_bytes)),
        )
        .route(
            "/videos/generations",
            post(proxy_grok_videos_generations).layer(DefaultBodyLimit::max(limits.media_bytes)),
        )
        .route("/v1/videos/:request_id", get(proxy_grok_video_status))
        .route("/videos/:request_id", get(proxy_grok_video_status))
        .route("/v1beta/*path", any(proxy_gemini))
        .route("/gemini/v1/*path", any(proxy_gemini))
        .route("/gemini/v1beta/*path", any(proxy_gemini))
        // 兜底内存上限：覆盖所有上面未单独放宽的推理路由（`/v1/responses`、
        // `/v1/messages` 等）。路由级 layer 更靠内，因此图片/视频档仍然生效。
        .layer(DefaultBodyLimit::max(limits.default_bytes))
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
    // Router 声明的请求体上限。必须在下面的剥离循环之前读取。
    let router_declared = router_declared_body_limit(request.headers());
    for name in [
        INGRESS_CONTEXT_HEADER,
        INGRESS_SIGNATURE_HEADER,
        crate::clients::router::ingress::INGRESS_BODY_LIMIT_HEADER,
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
        let body_limit = resolve_router_ingress_body_limit(
            &state.request_body_limits,
            request.uri().path(),
            router_declared,
        );
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, body_limit).await {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(
                    body_limit,
                    router_declared = router_declared.unwrap_or(0),
                    error = %error,
                    "router ingress request body rejected"
                );
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
    if context.is_health_check {
        headers.insert("x-cc-switch-health-check", HeaderValue::from_static("1"));
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

/// Router 未声明上限时的兜底档位。等于本特性上线前的硬编码值，
/// 因此旧版 Router 转发过来的请求行为完全不变。
fn legacy_router_ingress_body_limit(path: &str) -> usize {
    match path {
        "/v1/images/generations" | "/images/generations" | "/v1/images/edits" | "/images/edits" => {
            proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES
        }
        "/v1/videos/generations" | "/videos/generations" => proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES,
        _ => proxy::LEGACY_REQUEST_BODY_LIMIT_BYTES,
    }
}

/// 解析 Router 声明的上限。缺失、空、非数字、为 0 都视为"未声明"。
///
/// 该头不参与 ingress 签名，但也无需参与：调用方永远取 `min(本地上限, 声明值)`，
/// 伪造只能把自己的上限压低。Router 侧还会剥离来自公网的同名头。
fn router_declared_body_limit(headers: &axum::http::HeaderMap) -> Option<usize> {
    headers
        .get(crate::clients::router::ingress::INGRESS_BODY_LIMIT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
}

/// 本次 ingress 请求的生效上限。
///
/// - Router 声明了 → `min(本地上限, 声明值)`。Router 调大后 Client 自动跟随，
///   卖家的本地上限仍是不可逾越的天花板。
/// - Router 未声明（旧版 Router）→ 沿用历史硬编码档位，行为不变。
fn resolve_router_ingress_body_limit(
    limits: &crate::domain::settings::config::RequestBodyLimits,
    path: &str,
    declared: Option<usize>,
) -> usize {
    match declared {
        Some(declared) => limits.for_path(path).min(declared),
        None => legacy_router_ingress_body_limit(path),
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
            details: None,
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
        .map_err(|error| InferenceApiError::proxy(surface, request_id.clone(), error))?;
    proxy_models_for_selection(&state, Some(app), Some(&provider_id))
        .await
        .map_err(|error| InferenceApiError::api(surface, request_id, error))
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
) -> Result<Json<OpenAiModelsResponse>, ApiError> {
    let providers = state.providers.read().await.clone();
    let mut data = openai_model_list(&providers.providers, app, provider_id);
    let claude_catalog = resolve_claude_catalog_provider(&providers, app, provider_id)
        .map(|_| crate::clients::oauth::claude_models::static_claude_model_catalog());
    let kimi_catalog = append_kimi_models(state, &providers, app, provider_id, &mut data).await;
    let trae_catalog = append_trae_models(state, &providers, app, provider_id, &mut data).await?;
    let qoder_catalog = append_qoder_models(state, &providers, app, provider_id, &mut data).await?;
    let cursor_catalog =
        append_cursor_api_key_models(state, &providers, app, provider_id, &mut data).await;
    let kiro_provider = resolve_kiro_catalog_provider(&providers, app, provider_id).cloned();
    let kiro_catalog = if let Some(provider) = kiro_provider.as_ref() {
        if let Some((account_id, expected_generation)) =
            kiro_catalog_managed_account_binding(&providers, provider)
        {
            let plan = providers
                .runtime_plan(provider.app, &provider.provider.id)
                .expect("validated Kiro binding has a runtime plan");
            #[cfg(test)]
            let endpoint_override = plan
                .driver_options
                .get("testKiroModelsUrl")
                .and_then(Value::as_str)
                .map(str::to_string);
            #[cfg(not(test))]
            let endpoint_override: Option<String> = None;
            Some(
                state
                    .kiro_model_catalog(
                        provider.app,
                        &provider.provider.id,
                        plan.provider_revision,
                        &plan.runtime_fingerprint,
                        &account_id,
                        expected_generation,
                        endpoint_override.as_deref(),
                        std::time::Duration::from_secs(10),
                    )
                    .await,
            )
        } else {
            Some(
                crate::clients::oauth::kiro_runtime::unavailable_model_catalog(
                    "managed_account_binding_unavailable",
                ),
            )
        }
    } else {
        None
    };
    if let (Some(provider), Some(catalog)) = (kiro_provider.as_ref(), kiro_catalog.as_ref()) {
        // A live bound-account catalog replaces the static fallback for this exact Provider.
        data.clear();
        let owned_by = model_owner(provider);
        for id in catalog.model_ids() {
            if !data.iter().any(|model| model.id == id) {
                data.push(OpenAiModel {
                    id: id.to_string(),
                    object: "model",
                    owned_by: owned_by.clone(),
                    reasoning_efforts: None,
                    input_modalities: Some(vec!["text".to_string(), "image".to_string()]),
                    context_window: None,
                    supports_tools: None,
                });
            }
        }
        data.sort_by(|left, right| left.id.cmp(&right.id));
    }
    let amazon_q_provider =
        resolve_amazon_q_catalog_provider(&providers, app, provider_id).cloned();
    let amazon_q_catalog = if let Some(provider) = amazon_q_provider.as_ref() {
        if let Some((account_id, expected_generation)) =
            amazon_q_catalog_managed_account_binding(&providers, provider)
        {
            let plan = providers
                .runtime_plan(provider.app, &provider.provider.id)
                .expect("validated Amazon Q binding has a runtime plan");
            #[cfg(test)]
            let endpoint_override = plan
                .driver_options
                .get("testAmazonQModelsUrl")
                .and_then(Value::as_str)
                .map(str::to_string);
            #[cfg(not(test))]
            let endpoint_override: Option<String> = None;
            Some(
                state
                    .amazon_q_model_catalog(
                        provider.app,
                        &provider.provider.id,
                        plan.provider_revision,
                        &plan.runtime_fingerprint,
                        &account_id,
                        expected_generation,
                        endpoint_override.as_deref(),
                        std::time::Duration::from_secs(10),
                    )
                    .await,
            )
        } else {
            Some(
                crate::clients::oauth::amazon_q_runtime::unavailable_model_catalog(
                    "managed_account_binding_unavailable",
                ),
            )
        }
    } else {
        None
    };
    if let (Some(provider), Some(catalog)) = (amazon_q_provider.as_ref(), amazon_q_catalog.as_ref())
    {
        data.clear();
        let owned_by = model_owner(provider);
        for descriptor in &catalog.descriptors {
            if !data.iter().any(|model| model.id == descriptor.model_id) {
                data.push(OpenAiModel {
                    id: descriptor.model_id.clone(),
                    object: "model",
                    owned_by: owned_by.clone(),
                    reasoning_efforts: None,
                    input_modalities: Some(if descriptor.supported_input_types.is_empty() {
                        vec!["text".to_string()]
                    } else {
                        descriptor.supported_input_types.clone()
                    }),
                    context_window: descriptor.max_input_tokens,
                    supports_tools: None,
                });
            }
        }
        data.sort_by(|left, right| left.id.cmp(&right.id));
    }
    let grok_provider = resolve_grok_catalog_provider(&providers, app, provider_id).cloned();
    let grok_catalog = if let Some(provider) = grok_provider.as_ref() {
        let execution = proxy::provider_ops::ProviderExecution::from_store_for_operation(
            &providers,
            provider.clone(),
        )
        .map_err(ApiError::proxy)?;
        let timeout = execution.request_timeout();
        Some(fetch_bound_grok_model_catalog(state, &execution, timeout).await?)
    } else {
        None
    };
    if let (Some(provider), Some(catalog)) = (grok_provider.as_ref(), grok_catalog.as_ref()) {
        // A successful bound-account response is authoritative, including an empty catalog.
        data.clear();
        let owned_by = model_owner(provider);
        for id in &catalog.models {
            if !data.iter().any(|model| model.id == *id) {
                data.push(OpenAiModel {
                    id: id.clone(),
                    object: "model",
                    owned_by: owned_by.clone(),
                    reasoning_efforts: None,
                    input_modalities: None,
                    context_window: None,
                    supports_tools: None,
                });
            }
        }
        data.sort_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(Json(OpenAiModelsResponse {
        object: "list",
        data,
        source: grok_catalog
            .as_ref()
            .map(|catalog| catalog.source.to_string())
            .or_else(|| trae_catalog.as_ref().map(|catalog| catalog.source.clone()))
            .or_else(|| {
                qoder_catalog
                    .as_ref()
                    .map(|_| "qoder_live_model_catalog".to_string())
            })
            .or_else(|| {
                kimi_catalog
                    .as_ref()
                    .map(|catalog| catalog.source.to_string())
            })
            .or_else(|| {
                kiro_catalog
                    .as_ref()
                    .map(|catalog| catalog.source.to_string())
            })
            .or_else(|| {
                amazon_q_catalog
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
            .or_else(|| trae_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| qoder_catalog.as_ref().map(|_| false))
            .or_else(|| kimi_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| kiro_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| amazon_q_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| claude_catalog.as_ref().map(|catalog| catalog.stale))
            .or_else(|| cursor_catalog.as_ref().map(|catalog| catalog.stale)),
        fetched_at_ms: grok_catalog
            .and_then(|catalog| catalog.fetched_at_ms)
            .or_else(|| trae_catalog.as_ref().map(|catalog| catalog.fetched_at_ms))
            .or_else(|| qoder_catalog.as_ref().map(|catalog| catalog.fetched_at_ms))
            .or_else(|| {
                kimi_catalog.as_ref().and_then(|catalog| {
                    (catalog.fetched_at_ms > 0).then_some(catalog.fetched_at_ms)
                })
            })
            .or_else(|| kiro_catalog.and_then(|catalog| catalog.fetched_at_ms))
            .or_else(|| amazon_q_catalog.and_then(|catalog| catalog.fetched_at_ms))
            .or_else(|| claude_catalog.map(|catalog| catalog.fetched_at_ms))
            .or_else(|| cursor_catalog.map(|catalog| catalog.fetched_at_ms)),
    }))
}

#[derive(Debug, Clone)]
struct TraeCatalogUse {
    source: String,
    stale: bool,
    fetched_at_ms: i64,
}

async fn append_trae_models(
    state: &ServerState,
    providers: &ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
    data: &mut Vec<OpenAiModel>,
) -> Result<Option<TraeCatalogUse>, ApiError> {
    let Some(provider) = resolve_trae_catalog_provider(providers, app, provider_id) else {
        return Ok(None);
    };
    data.clear();
    let execution = proxy::provider_ops::ProviderExecution::from_store_for_operation(
        providers,
        provider.clone(),
    )
    .map_err(ApiError::proxy)?;
    let timeout_ms = execution.plan.transport_policy.timeout_ms.max(1);
    let fetched =
        crate::api::providers::fetch_provider_models_inner(state, &execution, Some(timeout_ms))
            .await?;
    let owner = model_owner(provider);
    for model in fetched.models {
        let reasoning_efforts = model
            .raw
            .pointer("/capabilities/reasoningEfforts")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty());
        let context_window = model
            .raw
            .pointer("/capabilities/contextWindowMax")
            .or_else(|| model.raw.pointer("/capabilities/contextWindow"))
            .and_then(Value::as_u64);
        let supports_tools = model
            .raw
            .pointer("/capabilities/tools")
            .and_then(Value::as_bool);
        data.push(OpenAiModel {
            id: model.id,
            object: "model",
            owned_by: owner.clone(),
            reasoning_efforts,
            input_modalities: Some(vec!["text".to_string()]),
            context_window,
            supports_tools,
        });
    }
    data.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(Some(TraeCatalogUse {
        source: fetched
            .source
            .unwrap_or_else(|| "trae_live_model_catalog".to_string()),
        stale: fetched.stale.unwrap_or(false),
        fetched_at_ms: fetched.fetched_at_ms.unwrap_or_default(),
    }))
}

#[derive(Debug, Clone, Copy)]
struct QoderCatalogUse {
    fetched_at_ms: i64,
}

async fn append_qoder_models(
    state: &ServerState,
    providers: &ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
    data: &mut Vec<OpenAiModel>,
) -> Result<Option<QoderCatalogUse>, ApiError> {
    let Some(provider) = resolve_qoder_catalog_provider(providers, app, provider_id) else {
        return Ok(None);
    };
    data.clear();
    let plan = providers
        .runtime_plan(provider.app, &provider.provider.id)
        .ok_or_else(|| ApiError::conflict("Qoder model discovery runtime plan is unavailable"))?;
    let (account_id, expected_generation) =
        qoder_catalog_managed_account_binding(providers, provider).ok_or_else(|| {
            ApiError::conflict("Qoder model discovery requires one exact managed Account binding")
        })?;

    let mut recovery_attempted = false;
    let runtime = loop {
        match state
            .prepare_qoder_runtime(
                provider.app,
                &provider.provider.id,
                provider.resource.revision,
                &plan.runtime_fingerprint,
                &account_id,
                expected_generation,
                Duration::from_millis(plan.transport_policy.timeout_ms.max(1)),
            )
            .await
        {
            Ok(runtime) => break runtime,
            Err(error) if error.is_authentication_failure() && !recovery_attempted => {
                recovery_attempted = true;
                let account = state
                    .find_account_for_provider(ProviderType::QoderCosy, &account_id)
                    .await
                    .ok_or_else(|| ApiError::not_found("Qoder managed Account not found"))?;
                let profile =
                    crate::domain::qoder::QoderAccountProfile::parse(account.profile.as_ref())
                        .map_err(ApiError::bad_request)?;
                if profile.credential_rail != crate::domain::qoder::QoderCredentialRail::PatJobToken
                {
                    state
                        .refresh_managed_account_now_for_generation(
                            ProviderType::QoderCosy,
                            &account_id,
                            expected_generation,
                        )
                        .await
                        .map_err(qoder_refresh_api_error)?;
                }
            }
            Err(error) => return Err(qoder_runtime_api_error(error)),
        }
    };

    let owner = model_owner(provider);
    data.extend(
        runtime
            .catalog
            .public_models(runtime.session.session.site)
            .into_iter()
            .map(|model| OpenAiModel {
                id: model.id,
                object: "model",
                owned_by: owner.clone(),
                reasoning_efforts: (!model.route.reasoning_efforts.is_empty())
                    .then_some(model.route.reasoning_efforts),
                input_modalities: Some(model.route.input_modalities),
                context_window: model.route.max_input_tokens,
                supports_tools: Some(model.route.supports_tools),
            }),
    );
    data.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(Some(QoderCatalogUse {
        fetched_at_ms: runtime.catalog.fetched_at_ms,
    }))
}

fn qoder_refresh_api_error(error: crate::state::ManagedAccountRefreshError) -> ApiError {
    match error {
        crate::state::ManagedAccountRefreshError::NotFound => {
            ApiError::not_found("Qoder managed Account not found")
        }
        crate::state::ManagedAccountRefreshError::IdentityChanged { .. }
        | crate::state::ManagedAccountRefreshError::Conflict { .. } => {
            ApiError::conflict("Qoder Account identity changed during model discovery")
        }
        crate::state::ManagedAccountRefreshError::InactiveCodexAccount => {
            ApiError::conflict("Qoder Account is not active")
        }
        crate::state::ManagedAccountRefreshError::CredentialPersistenceDegraded => {
            ApiError::service_unavailable_code(
                "qoder_credential_persistence_degraded",
                "Qoder credentials are waiting for durable persistence",
            )
        }
        crate::state::ManagedAccountRefreshError::Refresh { status_code, .. } => ApiError::new(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            "Qoder same-account refresh failed during model discovery",
        ),
    }
}

fn qoder_runtime_api_error(error: crate::state::QoderRuntimeError) -> ApiError {
    match error {
        crate::state::QoderRuntimeError::NotFound => {
            ApiError::not_found("Qoder managed Account not found")
        }
        crate::state::QoderRuntimeError::IdentityChanged => {
            ApiError::conflict("Qoder Provider or Account identity changed during model discovery")
        }
        crate::state::QoderRuntimeError::CredentialPersistenceDegraded => {
            ApiError::service_unavailable_code(
                "qoder_credential_persistence_degraded",
                "Qoder credentials are waiting for durable persistence",
            )
        }
        crate::state::QoderRuntimeError::InvalidAccount(message) => ApiError::bad_request(message),
        crate::state::QoderRuntimeError::Refresh(error) => qoder_refresh_api_error(error),
        crate::state::QoderRuntimeError::Upstream {
            status_code,
            retryable,
            ..
        } => {
            let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);
            ApiError::new(
                if retryable && status != StatusCode::TOO_MANY_REQUESTS {
                    StatusCode::BAD_GATEWAY
                } else {
                    status
                },
                "Qoder live model discovery failed for the bound Account",
            )
        }
    }
}

async fn append_kimi_models(
    state: &ServerState,
    providers: &ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
    data: &mut Vec<OpenAiModel>,
) -> Option<crate::proxy::kimi_runtime::KimiModelCatalog> {
    let provider = resolve_kimi_catalog_provider(providers, app, provider_id)?;
    data.clear();
    let Some(plan) = providers.runtime_plan(provider.app, &provider.provider.id) else {
        return Some(crate::proxy::kimi_runtime::unavailable_catalog());
    };
    let Some((account_id, expected_generation)) =
        kimi_catalog_managed_account_binding(providers, provider)
    else {
        return Some(crate::proxy::kimi_runtime::unavailable_catalog());
    };
    #[cfg(test)]
    let endpoint_override = provider
        .provider
        .settings_config
        .get("testKimiModelsUrl")
        .and_then(Value::as_str);
    let catalog = state
        .kimi_model_catalog(
            provider.app,
            &provider.provider.id,
            provider.resource.revision,
            &plan.runtime_fingerprint,
            &account_id,
            expected_generation,
            Duration::from_millis(plan.transport_policy.timeout_ms.max(1)),
            #[cfg(test)]
            endpoint_override,
        )
        .await;
    let owner = model_owner(provider);
    data.extend(
        crate::proxy::kimi::catalog_models(&catalog.models)
            .into_iter()
            .map(|id| OpenAiModel {
                id,
                object: "model",
                owned_by: owner.clone(),
                reasoning_efforts: None,
                input_modalities: None,
                context_window: None,
                supports_tools: None,
            }),
    );
    data.sort_by(|left, right| left.id.cmp(&right.id));
    Some(catalog)
}

#[derive(Debug, Clone, Copy)]
struct CursorCatalogUse {
    stale: bool,
    fetched_at_ms: i64,
}

async fn append_cursor_api_key_models(
    state: &ServerState,
    providers: &ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
    data: &mut Vec<OpenAiModel>,
) -> Option<CursorCatalogUse> {
    const CURSOR_MODEL_CACHE_TTL_MS: i64 = 5 * 60 * 1000;
    let mut used_catalog: Option<CursorCatalogUse> = None;
    for provider in providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::CursorApiKey
            && app.is_none_or(|app| provider.app == app)
            && provider_id.is_none_or(|id| provider.provider.id == id)
    }) {
        let Some(runtime_plan) = providers.runtime_plan(provider.app, &provider.provider.id) else {
            tracing::warn!(
                provider_id = %provider.provider.id,
                "Cursor model discovery has no compiled runtime plan; failing closed"
            );
            continue;
        };
        if runtime_plan.provider_revision != provider.resource.revision
            || runtime_plan.driver_id.as_str() != "special.cursor"
            || runtime_plan.configuration_state == RuntimeConfigurationState::NeedsAttention
            || !matches!(
                runtime_plan.auth_ref,
                RuntimeAuthRef::StaticCredential {
                    credential_generation,
                    ..
                } if credential_generation == provider.resource.credential_generation
            )
        {
            tracing::warn!(
                provider_id = %provider.provider.id,
                "Cursor model discovery runtime identity is stale or unsupported; failing closed"
            );
            continue;
        }
        let mut materialized = match providers.materialize_provider_record(provider) {
            Ok(materialized) => materialized,
            Err(error) => {
                tracing::warn!(
                    provider_id = %provider.provider.id,
                    error = %error,
                    "Cursor model discovery could not materialize the exact Provider credential; failing closed"
                );
                continue;
            }
        };
        let api_key = cursor_provider_api_key(&materialized).map(Zeroizing::new);
        crate::domain::providers::credentials::zeroize_materialized_provider(
            &mut materialized.provider,
        );
        let Some(api_key) = api_key else {
            tracing::warn!(
                provider_id = %provider.provider.id,
                "Cursor model discovery exact Provider credential is missing; failing closed"
            );
            continue;
        };
        let key_hash = hex::encode(sha2::Sha256::digest(api_key.as_bytes()));
        let scope = crate::proxy::cursor::credential_cache::CursorModelCatalogScope::derive(
            provider.app.as_str(),
            &provider.provider.id,
            provider.resource.revision,
            provider.resource.credential_generation,
            &runtime_plan.runtime_fingerprint,
            &key_hash,
        );
        let now = crate::infra::time::now_ms() as i64;
        let (catalog, stale) =
            if let Some(catalog) = state.cursor_model_catalogs.fresh(&scope, now).await {
                (Some(catalog), false)
            } else {
                let _flight = state.cursor_model_catalogs.lock(&scope).await;
                if let Some(catalog) = state.cursor_model_catalogs.fresh(&scope, now).await {
                    (Some(catalog), false)
                } else {
                    match crate::clients::oauth::cursor::available_models(
                        &state.http_client().await,
                        api_key.as_str(),
                    )
                    .await
                    {
                        Ok(models) => (
                            Some(
                                state
                                    .cursor_model_catalogs
                                    .insert(scope.clone(), models, now, CURSOR_MODEL_CACHE_TTL_MS)
                                    .await,
                            ),
                            false,
                        ),
                        Err(error) => {
                            let transient = error.retryable;
                            tracing::warn!(
                                provider_id = %provider.provider.id,
                                status_code = error.status_code,
                                transient,
                                error = %error,
                                "Cursor model discovery failed"
                            );
                            if transient {
                                (
                                    state
                                        .cursor_model_catalogs
                                        .last_known_good(&scope, now)
                                        .await,
                                    true,
                                )
                            } else {
                                state.cursor_model_catalogs.invalidate(&scope).await;
                                (None, false)
                            }
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
                    context_window: None,
                    supports_tools: None,
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
    responses_input_tokens_response(&headers, body, state.request_body_limits.default_bytes)
        .map_err(|error| InferenceApiError::api(InferenceSurface::OpenAi, request_id, error))
}

/// Estimates input tokens for `/v1/responses/input_tokens`.
///
/// `decoded_limit` is the default-lane cap from [`ServerState::request_body_limits`], so this
/// endpoint tracks the Router-driven ceiling instead of a private constant. Wire bytes were
/// already bounded by the ingress gate and the route's `DefaultBodyLimit`; this check only
/// backstops post-decompression growth.
fn responses_input_tokens_response(
    headers: &HeaderMap,
    body: Bytes,
    decoded_limit: usize,
) -> Result<Response, ApiError> {
    let body = crate::proxy::decode_request_body_for_proxy_with_limit(headers, body, decoded_limit)
        .map_err(ApiError::proxy)?;
    let request = serde_json::from_slice::<Value>(&body)
        .map_err(|error| ApiError::bad_request(format!("invalid token count JSON: {error}")))?;
    let input_tokens =
        crate::proxy::cursor::request_builder::estimate_responses_input_tokens(&request)
            .map_err(ApiError::bad_request)?;
    let mut response = Json(json!({
        "object": "response.input_tokens",
        "input_tokens": input_tokens,
        "estimated": true,
        "estimation_method": "cursor_canonical_prompt_characters"
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
        let limit = 2 * 1024 * 1024;
        let oversized = json!({"input": "x".repeat(limit)}).to_string();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(oversized.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );

        let error =
            responses_input_tokens_response(&headers, Bytes::from(compressed), limit).unwrap_err();

        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.message.contains("2097152 byte limit"));
    }

    #[tokio::test]
    async fn input_token_limit_follows_the_configured_default_lane() {
        let payload = json!({"input": "x".repeat(4 * 1024)}).to_string();
        let body = Bytes::from(payload);
        let headers = HeaderMap::new();

        // The legacy hardcoded ceiling was 2 MiB; a smaller configured lane must now bind.
        let error = responses_input_tokens_response(&headers, body.clone(), 1024).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.message.contains("1024 byte limit"));

        // ...and a larger configured lane must let the same body through.
        let response =
            responses_input_tokens_response(&headers, body, 8 * 1024 * 1024).expect("within limit");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn input_token_estimate_rejects_invalid_tool_schema() {
        let body = Bytes::from(
            json!({
                "input":"lookup",
                "tools":[{
                    "type":"function",
                    "name":"lookup",
                    "parameters":{"type":"object","properties":{"q":{"type":"wat"}}}
                }]
            })
            .to_string(),
        );
        let error =
            responses_input_tokens_response(&HeaderMap::new(), body, 1024 * 1024).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("invalid_tool_schema"));
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

fn resolve_kimi_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::KimiCode
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn resolve_qoder_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::QoderCosy
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn resolve_trae_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::TraeSolo
            && provider.provider.id == provider_id
            && app.is_none_or(|app| provider.app == app)
    });
    let provider = matches.next()?;
    matches.next().is_none().then_some(provider)
}

fn qoder_catalog_managed_account_binding(
    providers: &crate::domain::providers::store::ProviderStore,
    provider: &StoredProvider,
) -> Option<(String, u64)> {
    let plan = providers.runtime_plan(provider.app, &provider.provider.id)?;
    if plan.provider_revision != provider.resource.revision
        || plan.configuration_state == RuntimeConfigurationState::NeedsAttention
        || plan.driver_id.as_str() != "special.qoder_cosy"
    {
        return None;
    }
    match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::QoderCosy,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => Some((account_id.clone(), *auth_identity_generation)),
        _ => None,
    }
}

fn kimi_catalog_managed_account_binding(
    providers: &crate::domain::providers::store::ProviderStore,
    provider: &StoredProvider,
) -> Option<(String, u64)> {
    let plan = providers.runtime_plan(provider.app, &provider.provider.id)?;
    if plan.provider_revision != provider.resource.revision
        || plan.configuration_state == RuntimeConfigurationState::NeedsAttention
        || plan.driver_id.as_str() != "oauth.kimi_code"
    {
        return None;
    }
    match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::KimiCode,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => Some((account_id.clone(), *auth_identity_generation)),
        _ => None,
    }
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

fn resolve_amazon_q_catalog_provider<'a>(
    providers: &'a crate::domain::providers::store::ProviderStore,
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Option<&'a StoredProvider> {
    let provider_id = provider_id.map(str::trim).filter(|id| !id.is_empty())?;
    let mut matches = providers.providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::AmazonQOAuth
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

fn amazon_q_catalog_managed_account_binding(
    providers: &crate::domain::providers::store::ProviderStore,
    provider: &StoredProvider,
) -> Option<(String, u64)> {
    let plan = providers.runtime_plan(provider.app, &provider.provider.id)?;
    if plan.provider_revision != provider.resource.revision
        || plan.configuration_state == RuntimeConfigurationState::NeedsAttention
        || plan.driver_id.as_str() != "special.amazon_q"
    {
        return None;
    }
    match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::AmazonQOAuth,
            auth_identity_generation,
        } if !account_id.trim().is_empty() => Some((account_id.clone(), *auth_identity_generation)),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod grok_catalog_provider_tests {
    use super::*;
    use crate::clients::oauth::cursor::{
        CursorApiKeyVerifier, CursorPublicApiError, VerifiedCursorApiKey,
    };
    use crate::domain::providers::credentials::CredentialPatch;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta};
    use crate::domain::providers::registry::ProfileId;
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

    #[derive(Debug)]
    struct AcceptCursorApiKeyVerifier;

    #[async_trait::async_trait]
    impl CursorApiKeyVerifier for AcceptCursorApiKeyVerifier {
        async fn verify(
            &self,
            _client: &reqwest::Client,
            _api_key: &str,
        ) -> Result<VerifiedCursorApiKey, CursorPublicApiError> {
            Ok(VerifiedCursorApiKey {
                account_id: "cursor-catalog-fixture".to_string(),
                principal_source: "user_id".to_string(),
                email: Some("cursor-catalog@example.com".to_string()),
                display_name: Some("Cursor Catalog Fixture".to_string()),
                credential_name: Some("Fixture key".to_string()),
                subscription_level: Some("Cursor Pro".to_string()),
                quota: None,
                dashboard_errors: Vec::new(),
                profile: json!({"source": "cursor_catalog_fixture"}),
            })
        }
    }

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

    fn cursor_catalog_test_state(name: &str) -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load_with_cursor_api_key_verifier(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir()
                        .join(format!("cc-switch-server-cursor-catalog-{name}-{nanos}")),
                ),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
            std::sync::Arc::new(AcceptCursorApiKeyVerifier),
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
        configure_test_share_binding(state, share_id, app, provider_id, provider_type).await;
    }

    async fn configure_test_share_binding(
        state: &ServerState,
        share_id: &str,
        app: AppKind,
        provider_id: String,
        provider_type: ProviderType,
    ) {
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
                        token_limit: None,
                        parallel_limit: None,
                        expires_at: None,
                        free_access: Some(false),
                        allow_personal_credits: None,
                        auto_consume_banked_reset: None,
                        banked_reset_expiry_lead_minutes: None,
                        previous_response_cache_enabled: None,
                        grok_media_policy: None,
                        auto_start: Some(true),
                        description: None,
                        enabled_apps: None,
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
            is_health_check: false,
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
    async fn gemini_cursor_api_key_models_use_only_the_exact_encrypted_provider_scope() {
        const PROVIDER_ID: &str = "gemini-cursor-api-key-catalog";
        const SHARE_ID: &str = "share-gemini-cursor-catalog";
        const API_KEY: &str = "cursor-catalog-plaintext-secret";

        let state = cursor_catalog_test_state("gemini-exact-scope");
        configure_test_router(&state).await;
        let stored = state
            .upsert_provider_command(
                AppKind::Gemini,
                Provider {
                    id: PROVIDER_ID.to_string(),
                    name: "Gemini Cursor API Key catalog".to_string(),
                    settings_config: json!({}),
                    category: None,
                    meta: Some(ProviderMeta {
                        provider_type: Some(ProviderType::CursorApiKey.as_str().to_string()),
                        ..Default::default()
                    }),
                    extra: Default::default(),
                },
                Some(ProfileId::parse("gemini.cursor_api_key").unwrap()),
                None,
                Some("gemini-cursor-api-key-catalog-create".to_string()),
                std::collections::BTreeMap::from([(
                    "/settingsConfig/apiKey".to_string(),
                    CredentialPatch::Replace {
                        value: API_KEY.to_string(),
                    },
                )]),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.provider_type, ProviderType::CursorApiKey);
        let committed = state.providers_snapshot().await;
        let committed_provider = committed
            .providers
            .iter()
            .find(|provider| provider.app == AppKind::Gemini && provider.provider.id == PROVIDER_ID)
            .unwrap();
        assert_eq!(
            committed_provider.resource.revision,
            stored.resource.revision
        );
        assert_eq!(
            committed_provider.resource.credential_generation,
            stored.resource.credential_generation
        );
        assert_eq!(
            committed_provider.provider.settings_config["apiKey"],
            crate::domain::providers::credentials::SECRET_KEEP_SENTINEL
        );
        let persisted = std::fs::read_to_string(crate::domain::providers::store::providers_path(
            &state.config_dir,
        ))
        .unwrap();
        assert!(persisted.contains("s2-encrypted-typed-records"));
        assert!(!persisted.contains(API_KEY));

        let account_count_before = state.accounts_snapshot().await.accounts.len();
        let account_snapshot = state
            .cursor_account_snapshot(
                crate::domain::providers::registry::ProviderKey::new(AppKind::Gemini, PROVIDER_ID)
                    .unwrap(),
                true,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            account_snapshot
                .account
                .data
                .as_ref()
                .and_then(|account| account.email.as_deref()),
            Some("cursor-catalog@example.com")
        );
        assert_eq!(
            account_snapshot
                .account
                .data
                .as_ref()
                .and_then(|account| account.subscription_level.as_deref()),
            Some("Cursor Pro")
        );
        assert_eq!(
            state.accounts_snapshot().await.accounts.len(),
            account_count_before,
            "Cursor API-key snapshots must remain Provider-owned"
        );

        configure_test_share_binding(
            &state,
            SHARE_ID,
            AppKind::Gemini,
            PROVIDER_ID.to_string(),
            ProviderType::CursorApiKey,
        )
        .await;

        let runtime_plan = state
            .provider_runtime_plan(AppKind::Gemini, PROVIDER_ID)
            .await
            .unwrap();
        assert_eq!(runtime_plan.driver_id.as_str(), "special.cursor");
        assert_ne!(
            runtime_plan.configuration_state,
            RuntimeConfigurationState::NeedsAttention,
            "{:?}",
            runtime_plan.warnings
        );
        assert!(matches!(
            runtime_plan.auth_ref,
            RuntimeAuthRef::StaticCredential {
                credential_generation,
                ..
            } if credential_generation == stored.resource.credential_generation
        ));
        let key_hash = hex::encode(sha2::Sha256::digest(API_KEY.as_bytes()));
        let exact_scope = crate::proxy::cursor::credential_cache::CursorModelCatalogScope::derive(
            AppKind::Gemini.as_str(),
            PROVIDER_ID,
            stored.resource.revision,
            stored.resource.credential_generation,
            &runtime_plan.runtime_fingerprint,
            &key_hash,
        );
        let materialized = committed
            .materialize_provider_record(committed_provider)
            .unwrap();
        let materialized_api_key = cursor_provider_api_key(&materialized).unwrap();
        assert_eq!(materialized_api_key, API_KEY);
        let committed_scope =
            crate::proxy::cursor::credential_cache::CursorModelCatalogScope::derive(
                committed_provider.app.as_str(),
                &committed_provider.provider.id,
                committed_provider.resource.revision,
                committed_provider.resource.credential_generation,
                &runtime_plan.runtime_fingerprint,
                &hex::encode(sha2::Sha256::digest(materialized_api_key.as_bytes())),
            );
        assert_eq!(exact_scope, committed_scope);
        let distractor_scope =
            crate::proxy::cursor::credential_cache::CursorModelCatalogScope::derive(
                AppKind::Codex.as_str(),
                PROVIDER_ID,
                stored.resource.revision,
                stored.resource.credential_generation,
                &runtime_plan.runtime_fingerprint,
                &key_hash,
            );
        let now = crate::infra::time::now_ms() as i64;
        state
            .cursor_model_catalogs
            .insert(
                exact_scope.clone(),
                vec!["cursor-exact-model".to_string()],
                now,
                60_000,
            )
            .await;
        assert_eq!(
            state
                .cursor_model_catalogs
                .fresh(&exact_scope, now)
                .await
                .unwrap()
                .models,
            ["cursor-exact-model"]
        );
        state
            .cursor_model_catalogs
            .insert(
                distractor_scope,
                vec!["cursor-cross-scope-model".to_string()],
                now,
                60_000,
            )
            .await;

        let upstream_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_proxy = std::sync::Arc::clone(&upstream_requests);
        let proxy_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let proxy_address = proxy_listener.local_addr().unwrap();
        let proxy_server = tokio::spawn(async move {
            axum::serve(
                proxy_listener,
                Router::new().fallback(any(move || {
                    let requests = std::sync::Arc::clone(&requests_for_proxy);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        StatusCode::BAD_GATEWAY
                    }
                })),
            )
            .await
            .unwrap();
        });
        *state.http_client.write().await =
            crate::infra::http::test_outbound_client_builder_with_proxy(&format!(
                "http://{proxy_address}"
            ))
            .unwrap()
            .build()
            .unwrap();

        let app = app_router(state.clone());
        let response = app
            .clone()
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1beta/models")
                        .body(Body::empty())
                        .unwrap(),
                    "gemini-cursor-models",
                    Some(SHARE_ID),
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let names = body["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"models/cursor-exact-model"));
        assert!(!names
            .iter()
            .any(|name| name.contains("cursor-cross-scope-model")));
        assert_eq!(
            upstream_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        state
            .cursor_model_catalogs
            .insert(exact_scope, Vec::new(), now.saturating_add(1), 60_000)
            .await;
        let response = app
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1beta/models")
                        .body(Body::empty())
                        .unwrap(),
                    "gemini-cursor-models-empty",
                    Some(SHARE_ID),
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["models"], json!([]));
        assert_eq!(
            upstream_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        proxy_server.abort();
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

    #[test]
    fn router_declared_body_limit_ignores_absent_and_malformed_headers() {
        let name = axum::http::HeaderName::from_static(
            crate::clients::router::ingress::INGRESS_BODY_LIMIT_HEADER,
        );
        assert_eq!(router_declared_body_limit(&HeaderMap::new()), None);
        for raw in ["", "   ", "abc", "0", "-1", "1.5", "12mb"] {
            let mut headers = HeaderMap::new();
            headers.insert(name.clone(), raw.parse().unwrap());
            assert_eq!(router_declared_body_limit(&headers), None, "{raw:?}");
        }

        let mut headers = HeaderMap::new();
        headers.insert(name, " 10485760 ".parse().unwrap());
        assert_eq!(router_declared_body_limit(&headers), Some(10 * 1024 * 1024));
    }

    #[test]
    fn effective_ingress_body_limit_is_the_minimum_of_local_and_declared() {
        let limits = crate::domain::settings::config::RequestBodyLimits {
            default_bytes: 16 * 1024 * 1024,
            media_bytes: 64 * 1024 * 1024,
            image_bytes: 48 * 1024 * 1024,
        };

        // Router 声明更低 → 跟随 Router。
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/responses", Some(4 * 1024 * 1024)),
            4 * 1024 * 1024
        );
        // Router 声明更高 → 被本地上限封顶，卖家仍握有天花板。
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/responses", Some(512 * 1024 * 1024)),
            16 * 1024 * 1024
        );
        // 按档位取本地上限。
        assert_eq!(
            resolve_router_ingress_body_limit(
                &limits,
                "/v1/videos/generations",
                Some(512 * 1024 * 1024)
            ),
            64 * 1024 * 1024
        );
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/images/edits", Some(512 * 1024 * 1024)),
            48 * 1024 * 1024
        );
        // 旧版 Router 未声明 → 历史硬编码档位，行为不变。
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/responses", None),
            proxy::LEGACY_REQUEST_BODY_LIMIT_BYTES
        );
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/videos/generations", None),
            proxy::MEDIA_REQUEST_BODY_LIMIT_BYTES
        );
        assert_eq!(
            resolve_router_ingress_body_limit(&limits, "/v1/images/generations", None),
            proxy::CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES
        );
    }

    #[tokio::test]
    async fn router_declared_body_limit_replaces_the_legacy_ingress_ceiling() {
        let state = catalog_test_state("declared-body-limit");
        configure_test_router(&state).await;
        let app = app_router(state);
        let declared = 4 * 1024 * 1024;
        let header_name = axum::http::HeaderName::from_static(
            crate::clients::router::ingress::INGRESS_BODY_LIMIT_HEADER,
        );

        // 声明值高于历史 2 MiB 兜底：本地默认档是 Router 的最大档位，
        // 因此 min() 落在声明值上，请求不再被 ingress 闸门拒绝。
        let mut within = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b' '; declared - 1]))
                .unwrap(),
            "declared-within",
            Some("share-declared"),
        )
        .await;
        within
            .headers_mut()
            .insert(header_name.clone(), declared.to_string().parse().unwrap());
        let accepted = app.clone().oneshot(within).await.unwrap();
        assert_ne!(accepted.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // 超过声明值 → 在占用 Share 并发之前就 413。
        let mut oversized = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b' '; declared + 1]))
                .unwrap(),
            "declared-oversized",
            Some("share-declared"),
        )
        .await;
        oversized
            .headers_mut()
            .insert(header_name, declared.to_string().parse().unwrap());
        let rejected = app.clone().oneshot(oversized).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // 同样大小、但 Router 未声明（旧版 Router）→ 沿用 2 MiB 兜底，仍然 413。
        let legacy = router_ingress_request(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![
                    b' ';
                    proxy::LEGACY_REQUEST_BODY_LIMIT_BYTES + 1
                ]))
                .unwrap(),
            "declared-legacy",
            Some("share-declared"),
        )
        .await;
        let rejected_legacy = app.oneshot(legacy).await.unwrap();
        assert_eq!(rejected_legacy.status(), StatusCode::PAYLOAD_TOO_LARGE);
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
        // Inject historical drift through the test-only replacement hook. A
        // production Provider commit correctly rejects disabling the last
        // enabled binding of an active Share; this route still has to fail
        // closed if such an invalid snapshot is loaded or observed.
        let mut providers = state.providers_snapshot().await;
        let provider = providers
            .providers
            .iter_mut()
            .find(|stored| {
                stored.app == AppKind::Codex && stored.provider.id == "share-disabled-provider"
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
        state.replace_provider_store_for_test(providers).await;
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

        let error = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("grok-degraded-model-provider"),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
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

        let error = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("grok-refresh-rejected-provider"),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error.status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        ));
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
            let error = proxy_models_for_selection(&state, Some(AppKind::Codex), Some(provider_id))
                .await
                .unwrap_err();
            assert!(matches!(
                error.status,
                StatusCode::BAD_REQUEST | StatusCode::CONFLICT
            ));
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
        .unwrap()
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
        .unwrap()
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

    #[tokio::test]
    async fn kiro_share_catalog_hides_static_models_while_idc_profile_is_unresolved() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&model_requests);
        let upstream = Router::new().route(
            "/models",
            get(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Json(json!({"models": [{"modelId": "must-not-be-requested"}]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = catalog_test_state("kiro-unresolved-idc-catalog");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "kiro-unresolved-idc-account",
                        "providerType": "kiro_oauth",
                        "accessToken": "rotated-but-unresolved-access",
                        "expiresAt": i64::MAX / 2,
                        "profile": {
                            "authMethod": "idc",
                            "authRegion": "eu-north-1",
                            "runtimeRegion": "eu-central-1",
                            "profileProvenance": "profile_resolution_required"
                        },
                        "raw": {
                            "authMethod": "idc",
                            "profileProvenance": "profile_resolution_required"
                        }
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
                        id: "kiro-unresolved-idc-provider".to_string(),
                        name: "Kiro unresolved IdC provider".to_string(),
                        settings_config: json!({
                            "testKiroModelsUrl": models_url,
                            "models": ["configured-model-must-be-hidden"]
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kiro_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kiro_oauth".to_string()),
                                account_id: Some("kiro-unresolved-idc-account".to_string()),
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
            Some("kiro-unresolved-idc-provider"),
        )
        .await
        .unwrap()
        .0;

        assert!(response.data.is_empty());
        assert_eq!(response.source.as_deref(), Some("kiro_identity_unresolved"));
        assert_eq!(response.stale, Some(false));
        assert_eq!(response.fetched_at_ms, None);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    async fn configure_kimi_catalog_fixture(
        state: &ServerState,
        provider_id: &str,
        models_url: String,
        accounts: &[(&str, &str)],
        bound_account_id: &str,
        bound_refresh_token: Option<&str>,
        token_url: Option<String>,
    ) {
        let accounts = accounts
            .iter()
            .map(|(id, access_token)| ((*id).to_string(), (*access_token).to_string()))
            .collect::<Vec<_>>();
        let bound_account_id_for_store = bound_account_id.to_string();
        let bound_refresh_token = bound_refresh_token.map(str::to_string);
        state
            .mutate_accounts_immediate(move |store| {
                for (id, access_token) in accounts {
                    let is_bound = id == bound_account_id_for_store;
                    store.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "kimi_code",
                            "accessToken": access_token,
                            "refreshToken": is_bound.then(|| bound_refresh_token.clone()).flatten(),
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "userId": "kimi-catalog-user",
                                "kimiDevice": {
                                    "deviceId": if is_bound { "bound-kimi-device" } else { "distractor-kimi-device" },
                                    "deviceName": "fixture",
                                    "deviceModel": "fixture-model",
                                    "osVersion": "fixture-os"
                                }
                            },
                            "raw": is_bound.then(|| token_url.as_ref().map(|url| json!({"testOAuthTokenUrl": url}))).flatten()
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let provider_id_for_store = provider_id.to_string();
        let bound_account_id = bound_account_id.to_string();
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: provider_id_for_store.clone(),
                        name: "Kimi catalog fixture".to_string(),
                        settings_config: json!({
                            "testKimiModelsUrl": models_url,
                            "models": ["configured-model-must-not-survive"],
                            "modelCatalog": [{"id": "configured-catalog-must-not-survive"}]
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("kimi_code".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("kimi_code".to_string()),
                                account_id: Some(bound_account_id),
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
    }

    async fn seed_kimi_stale_catalog(
        state: &ServerState,
        provider_id: &str,
        account_id: &str,
        models: &[&str],
    ) -> crate::proxy::kimi_runtime::KimiModelCatalogScope {
        let (app, provider_revision, runtime_fingerprint) = {
            let providers = state.providers.read().await;
            let stored = providers
                .providers
                .iter()
                .find(|stored| stored.provider.id == provider_id)
                .unwrap();
            let plan = providers.runtime_plan(stored.app, provider_id).unwrap();
            (
                stored.app,
                stored.resource.revision,
                plan.runtime_fingerprint.clone(),
            )
        };
        let account = state
            .find_account_for_provider(ProviderType::KimiCode, account_id)
            .await
            .unwrap();
        let scope = crate::proxy::kimi_runtime::KimiModelCatalogScope::derive(
            app.as_str(),
            provider_id,
            provider_revision,
            &runtime_fingerprint,
            account_id,
            account.auth_identity_generation,
            account.token_refresh_generation,
        );
        let fetched_at_ms = (crate::infra::time::now_ms().min(i64::MAX as u128) as i64)
            .saturating_sub(crate::proxy::kimi_runtime::KIMI_MODEL_CATALOG_TTL_MS)
            .saturating_sub(1);
        state
            .kimi_model_catalogs
            .insert(
                scope.clone(),
                models.iter().map(|model| (*model).to_string()).collect(),
                fetched_at_ms,
            )
            .await;
        scope
    }

    async fn configure_qoder_catalog_fixture(
        state: &ServerState,
        provider_id: &str,
        origin: &str,
        site: crate::domain::qoder::QoderSite,
        accounts: &[(&str, &str)],
        bound_account_id: &str,
    ) {
        let accounts = accounts
            .iter()
            .map(|(id, uid)| ((*id).to_string(), (*uid).to_string()))
            .collect::<Vec<_>>();
        let origin_for_store = origin.to_string();
        state
            .mutate_accounts_immediate(move |store| {
                for (index, (id, uid)) in accounts.into_iter().enumerate() {
                    let rail = match site {
                        crate::domain::qoder::QoderSite::Global => "global_oauth",
                        crate::domain::qoder::QoderSite::Cn => "cn_oauth",
                    };
                    let refresh_mode = match site {
                        crate::domain::qoder::QoderSite::Global => "cosy",
                        crate::domain::qoder::QoderSite::Cn => "qodercn20",
                    };
                    let machine_id = match site {
                        crate::domain::qoder::QoderSite::Global => {
                            format!("{:036x}", index + 1)
                        }
                        crate::domain::qoder::QoderSite::Cn => {
                            format!("00000000-0000-4000-8000-{:012x}", index + 1)
                        }
                    };
                    store.upsert(
                        serde_json::from_value(json!({
                            "id": id,
                            "providerType": "qoder_cosy",
                            "accessToken": format!("qoder-access-{uid}"),
                            "refreshToken": format!("qoder-refresh-{uid}"),
                            "expiresAt": i64::MAX / 2,
                            "profile": {
                                "site": site.as_str(),
                                "credentialRail": rail,
                                "refreshMode": refresh_mode,
                                "uid": uid,
                                "aid": format!("aid-{uid}"),
                                "name": "Qoder Catalog Fixture",
                                "userType": "personal_standard",
                                "machineId": machine_id,
                                "machineType": "5"
                            },
                            "raw": {
                                "qoderSecrets": {"machineToken": format!("machine-token-{uid}")},
                                "testQoderEndpoints": {
                                    "openapiBaseUrl": origin_for_store,
                                    "centerBaseUrl": origin_for_store,
                                    "gatewayBaseUrl": origin_for_store,
                                    "jobGatewayBaseUrl": origin_for_store
                                }
                            }
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let provider_id = provider_id.to_string();
        let bound_account_id = bound_account_id.to_string();
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Codex,
                    Provider {
                        id: provider_id,
                        name: "Qoder catalog fixture".to_string(),
                        settings_config: json!({"models": ["configured-model-must-not-survive"]}),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("qoder_cosy".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("qoder_cosy".to_string()),
                                account_id: Some(bound_account_id),
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
    }

    #[tokio::test]
    async fn kimi_catalog_uses_only_bound_account_and_filters_unreviewed_models() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let observed_for_route = std::sync::Arc::clone(&observed);
        let upstream = Router::new().route(
            "/models",
            get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    observed.lock().unwrap().push((
                        headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("x-msh-device-id")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    ));
                    Json(json!({
                        "data": [
                            {"id": "kimi-for-coding"},
                            {"id": "k3"},
                            {"id": "future-unreviewed-model"}
                        ]
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("kimi-bound-account");
        configure_kimi_catalog_fixture(
            &state,
            "kimi-bound-provider",
            format!("http://{address}/models"),
            &[
                ("kimi-distractor", "distractor-token"),
                ("kimi-bound", "bound-token"),
            ],
            "kimi-bound",
            None,
            None,
        )
        .await;

        let response =
            proxy_models_for_selection(&state, Some(AppKind::Codex), Some("kimi-bound-provider"))
                .await
                .unwrap()
                .0;

        assert_eq!(response.source.as_deref(), Some("coding_v1_models"));
        assert_eq!(response.stale, Some(false));
        assert!(response
            .data
            .iter()
            .any(|model| model.id == "kimi-for-coding"));
        assert!(response.data.iter().any(|model| model.id == "kimi-k3"));
        assert!(!response
            .data
            .iter()
            .any(|model| model.id.contains("future-unreviewed")
                || model.id.contains("configured-model")
                || model.id.contains("configured-catalog")));
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            &[(
                "Bearer bound-token".to_string(),
                "bound-kimi-device".to_string()
            )]
        );
        server.abort();
    }

    #[tokio::test]
    async fn kimi_authoritative_empty_catalog_hides_every_static_alias() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new().route("/models", get(|| async { Json(json!({"data": []})) }));
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("kimi-empty-catalog");
        configure_kimi_catalog_fixture(
            &state,
            "kimi-empty-provider",
            format!("http://{address}/models"),
            &[("kimi-empty-account", "empty-token")],
            "kimi-empty-account",
            None,
            None,
        )
        .await;

        let response =
            proxy_models_for_selection(&state, Some(AppKind::Codex), Some("kimi-empty-provider"))
                .await
                .unwrap()
                .0;
        assert!(response.data.is_empty());
        assert_eq!(response.source.as_deref(), Some("coding_v1_models"));
        assert_eq!(response.stale, Some(false));
        let account = state
            .find_account_for_provider(ProviderType::KimiCode, "kimi-empty-account")
            .await
            .unwrap();
        let projection =
            crate::domain::accounts::capability_evidence::account_capability_projections(
                &account,
                crate::infra::time::now_ms() as i64,
            )
            .pop()
            .unwrap();
        assert_eq!(
            projection.dimensions
                [crate::domain::accounts::capability_evidence::MODEL_CATALOG_DIMENSION]
                .state,
            crate::domain::accounts::capability_evidence::AccountCapabilityState::Supported
        );
        assert_eq!(
            projection.dimensions
                [crate::domain::accounts::capability_evidence::MODEL_ENTITLEMENT_DIMENSION]
                .state,
            crate::domain::accounts::capability_evidence::AccountCapabilityState::Unsupported
        );
        server.abort();
    }

    #[tokio::test]
    async fn kimi_catalog_401_refreshes_and_replays_only_the_bound_account_once() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let model_authorizations_for_route = std::sync::Arc::clone(&model_authorizations);
        let refreshed_access = "e30.eyJ1c2VyX2lkIjoia2ltaS1jYXRhbG9nLXVzZXIifQ.rotated";
        let refreshed_access_for_route = refreshed_access.to_string();
        let upstream = Router::new()
            .route(
                "/token",
                post(move || {
                    let requests = std::sync::Arc::clone(&token_requests_for_route);
                    let access = refreshed_access_for_route.clone();
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({
                            "access_token": access,
                            "refresh_token": "rotated-kimi-refresh",
                            "expires_in": 3600,
                            "token_type": "Bearer"
                        }))
                    }
                }),
            )
            .route(
                "/models",
                get(move |headers: HeaderMap| {
                    let authorizations = std::sync::Arc::clone(&model_authorizations_for_route);
                    async move {
                        let authorization = headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        authorizations.lock().unwrap().push(authorization.clone());
                        if authorization == "Bearer initial-kimi-token" {
                            (StatusCode::UNAUTHORIZED, Json(json!({"error": "expired"})))
                        } else {
                            (StatusCode::OK, Json(json!({"data": [{"id": "k3"}]})))
                        }
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("kimi-same-account-401");
        configure_kimi_catalog_fixture(
            &state,
            "kimi-401-provider",
            format!("http://{address}/models"),
            &[
                ("kimi-401-distractor", "distractor-kimi-token"),
                ("kimi-401-bound", "initial-kimi-token"),
            ],
            "kimi-401-bound",
            Some("initial-kimi-refresh"),
            Some(format!("http://{address}/token")),
        )
        .await;

        let response =
            proxy_models_for_selection(&state, Some(AppKind::Codex), Some("kimi-401-provider"))
                .await
                .unwrap()
                .0;
        assert_eq!(response.source.as_deref(), Some("coding_v1_models"));
        assert!(response.data.iter().any(|model| model.id == "k3"));
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            model_authorizations.lock().unwrap().as_slice(),
            &[
                "Bearer initial-kimi-token".to_string(),
                format!("Bearer {refreshed_access}")
            ]
        );
        let account = state
            .find_account_for_provider(ProviderType::KimiCode, "kimi-401-bound")
            .await
            .unwrap();
        assert_eq!(account.token_refresh_generation, 2);
        assert_eq!(account.access_token.as_deref(), Some(refreshed_access));
        server.abort();
    }

    #[tokio::test]
    async fn kimi_catalog_transient_failure_uses_only_exact_scope_stale() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let upstream = Router::new().route(
            "/models",
            get(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error": "temporary"})),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let models_url = format!("http://{address}/models");

        let state_with_stale = catalog_test_state("kimi-exact-stale");
        configure_kimi_catalog_fixture(
            &state_with_stale,
            "kimi-exact-stale-provider",
            models_url.clone(),
            &[("kimi-exact-stale-account", "stale-token")],
            "kimi-exact-stale-account",
            None,
            None,
        )
        .await;
        seed_kimi_stale_catalog(
            &state_with_stale,
            "kimi-exact-stale-provider",
            "kimi-exact-stale-account",
            &["k3"],
        )
        .await;
        let cached = proxy_models_for_selection(
            &state_with_stale,
            Some(AppKind::Codex),
            Some("kimi-exact-stale-provider"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(cached.source.as_deref(), Some("kimi_models_cache"));
        assert_eq!(cached.stale, Some(true));
        assert!(cached.data.iter().any(|model| model.id == "kimi-k3"));

        let state_without_stale = catalog_test_state("kimi-no-static-fallback");
        configure_kimi_catalog_fixture(
            &state_without_stale,
            "kimi-no-cache-provider",
            models_url,
            &[("kimi-no-cache-account", "no-cache-token")],
            "kimi-no-cache-account",
            None,
            None,
        )
        .await;
        let unavailable = proxy_models_for_selection(
            &state_without_stale,
            Some(AppKind::Codex),
            Some("kimi-no-cache-provider"),
        )
        .await
        .unwrap()
        .0;
        assert!(unavailable.data.is_empty());
        assert_eq!(
            unavailable.source.as_deref(),
            Some("kimi_models_unavailable")
        );
        assert_eq!(unavailable.stale, Some(false));
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn kimi_catalog_protocol_drift_invalidates_stale_instead_of_authorizing_it() {
        for (fixture, payload) in [
            ("malformed", json!({"data": {"id": "k3"}})),
            (
                "unknown-only",
                json!({"data": [{"id": "future-unreviewed-model"}]}),
            ),
        ] {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let upstream_payload = payload.clone();
            let upstream = Router::new().route(
                "/models",
                get(move || {
                    let payload = upstream_payload.clone();
                    async move { Json(payload) }
                }),
            );
            let server = tokio::spawn(async move {
                axum::serve(listener, upstream).await.unwrap();
            });
            let state = catalog_test_state(&format!("kimi-{fixture}-catalog"));
            let provider_id = format!("kimi-{fixture}-provider");
            let account_id = format!("kimi-{fixture}-account");
            configure_kimi_catalog_fixture(
                &state,
                &provider_id,
                format!("http://{address}/models"),
                &[(&account_id, "protocol-drift-token")],
                &account_id,
                None,
                None,
            )
            .await;
            let scope = seed_kimi_stale_catalog(&state, &provider_id, &account_id, &["k3"]).await;

            let response =
                proxy_models_for_selection(&state, Some(AppKind::Codex), Some(&provider_id))
                    .await
                    .unwrap()
                    .0;
            assert!(response.data.is_empty(), "fixture={fixture}");
            assert_eq!(
                response.source.as_deref(),
                Some("kimi_models_unavailable"),
                "fixture={fixture}"
            );
            assert_eq!(response.stale, Some(false), "fixture={fixture}");
            assert!(
                state
                    .kimi_model_catalogs
                    .stale(
                        &scope,
                        crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
                    )
                    .await
                    .is_none(),
                "fixture={fixture}"
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn kimi_catalog_second_401_is_terminal_after_one_same_account_refresh() {
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
                            "access_token": "e30.eyJ1c2VyX2lkIjoia2ltaS1jYXRhbG9nLXVzZXIifQ.still-rejected",
                            "refresh_token": "rotated-once",
                            "expires_in": 3600,
                            "token_type": "Bearer"
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
                        (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({"error": "still unauthorized"})),
                        )
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("kimi-second-401");
        configure_kimi_catalog_fixture(
            &state,
            "kimi-second-401-provider",
            format!("http://{address}/models"),
            &[("kimi-second-401-account", "initial-token")],
            "kimi-second-401-account",
            Some("initial-refresh"),
            Some(format!("http://{address}/token")),
        )
        .await;

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("kimi-second-401-provider"),
        )
        .await
        .unwrap()
        .0;
        assert!(response.data.is_empty());
        assert_eq!(response.source.as_deref(), Some("kimi_models_unavailable"));
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn kimi_initial_refresh_transient_can_only_use_pre_refresh_exact_scope_stale() {
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
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error": "temporary"})),
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
        let state = catalog_test_state("kimi-initial-refresh-stale");
        configure_kimi_catalog_fixture(
            &state,
            "kimi-initial-refresh-provider",
            format!("http://{address}/models"),
            &[("kimi-initial-refresh-account", "expired-token")],
            "kimi-initial-refresh-account",
            Some("refresh-token"),
            Some(format!("http://{address}/token")),
        )
        .await;
        seed_kimi_stale_catalog(
            &state,
            "kimi-initial-refresh-provider",
            "kimi-initial-refresh-account",
            &["k3"],
        )
        .await;
        let mut accounts = state.accounts_snapshot().await;
        accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == "kimi-initial-refresh-account")
            .unwrap()
            .expires_at = Some(1);
        state.replace_account_store_for_test(accounts).await;

        let response = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("kimi-initial-refresh-provider"),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(response.source.as_deref(), Some("kimi_models_cache"));
        assert_eq!(response.stale, Some(true));
        assert!(response.data.iter().any(|model| model.id == "kimi-k3"));
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn qoder_catalog_is_bound_account_authoritative_and_projects_site_capabilities() {
        for site in [
            crate::domain::qoder::QoderSite::Global,
            crate::domain::qoder::QoderSite::Cn,
        ] {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let observed_users = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let observed_users_for_route = std::sync::Arc::clone(&observed_users);
            let upstream = Router::new().route(
                crate::domain::qoder::QODER_MODEL_LIST_PATH,
                get(move |headers: HeaderMap| {
                    let observed_users = std::sync::Arc::clone(&observed_users_for_route);
                    async move {
                        observed_users.lock().unwrap().push(
                            headers
                                .get("cosy-user")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        );
                        let catalog = json!({"chat": [
                            {"key":"gmodel","display_name":"GLM-5.3","enable":true},
                            {"key":"mmodel","display_name":"MiniMax","enable":true},
                            {"key":"future-route","display_name":"Future","enable":true},
                            {"key":"qmodel","display_name":"Hidden","enable":false}
                        ]});
                        Json(json!({
                            "statusCodeValue": 200,
                            "body": catalog.to_string()
                        }))
                    }
                }),
            );
            let server = tokio::spawn(async move {
                axum::serve(listener, upstream).await.unwrap();
            });
            let suffix = site.as_str();
            let state = catalog_test_state(&format!("qoder-{suffix}-catalog"));
            let provider_id = format!("qoder-{suffix}-provider");
            let bound_account_id = format!("qoder-{suffix}-bound");
            let distractor_account_id = format!("qoder-{suffix}-distractor");
            configure_qoder_catalog_fixture(
                &state,
                &provider_id,
                &format!("http://{address}"),
                site,
                &[
                    (&distractor_account_id, "distractor-user"),
                    (&bound_account_id, "bound-user"),
                ],
                &bound_account_id,
            )
            .await;

            let response =
                proxy_models_for_selection(&state, Some(AppKind::Codex), Some(&provider_id))
                    .await
                    .unwrap()
                    .0;
            assert_eq!(response.source.as_deref(), Some("qoder_live_model_catalog"));
            assert_eq!(response.stale, Some(false));
            assert!(response.fetched_at_ms.is_some());
            assert!(!response
                .data
                .iter()
                .any(|model| model.id.contains("configured-model") || model.id == "qwen3.7-plus"));
            let glm = response
                .data
                .iter()
                .find(|model| model.id == "glm-5.3")
                .unwrap();
            assert_eq!(
                glm.reasoning_efforts.as_deref(),
                Some(
                    ["none", "low", "high", "max"]
                        .map(str::to_string)
                        .as_slice()
                )
            );
            assert_eq!(glm.context_window, Some(1_000_000));
            assert_eq!(
                glm.input_modalities.as_deref(),
                Some(["text".to_string()].as_slice())
            );
            assert_eq!(glm.supports_tools, Some(true));
            let minimax_id = match site {
                crate::domain::qoder::QoderSite::Global => "minimax-m3",
                crate::domain::qoder::QoderSite::Cn => "minimax-m2.7",
            };
            let minimax = response
                .data
                .iter()
                .find(|model| model.id == minimax_id)
                .unwrap();
            assert_eq!(
                minimax.context_window,
                Some(match site {
                    crate::domain::qoder::QoderSite::Global => 1_000_000,
                    crate::domain::qoder::QoderSite::Cn => 200_000,
                })
            );
            let future = response
                .data
                .iter()
                .find(|model| model.id == "future-route")
                .unwrap();
            assert_eq!(future.reasoning_efforts, None);
            assert_eq!(future.context_window, None);
            assert_eq!(observed_users.lock().unwrap().as_slice(), ["bound-user"]);
            server.abort();
        }
    }

    #[tokio::test]
    async fn qoder_authoritative_empty_catalog_hides_static_models() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            crate::domain::qoder::QODER_MODEL_LIST_PATH,
            get(|| async {
                Json(json!({
                    "statusCodeValue": 200,
                    "body": json!({"chat": []}).to_string()
                }))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("qoder-empty-catalog");
        configure_qoder_catalog_fixture(
            &state,
            "qoder-empty-provider",
            &format!("http://{address}"),
            crate::domain::qoder::QoderSite::Global,
            &[("qoder-empty-account", "empty-user")],
            "qoder-empty-account",
        )
        .await;

        let response =
            proxy_models_for_selection(&state, Some(AppKind::Codex), Some("qoder-empty-provider"))
                .await
                .unwrap()
                .0;
        assert!(response.data.is_empty());
        assert_eq!(response.source.as_deref(), Some("qoder_live_model_catalog"));
        assert_eq!(response.stale, Some(false));
        server.abort();
    }

    #[tokio::test]
    async fn qoder_catalog_second_401_is_terminal_after_one_same_account_refresh() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let model_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let refresh_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_users = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let model_requests_for_route = std::sync::Arc::clone(&model_requests);
        let refresh_requests_for_route = std::sync::Arc::clone(&refresh_requests);
        let observed_users_for_route = std::sync::Arc::clone(&observed_users);
        let upstream = Router::new()
            .route(
                crate::domain::qoder::QODER_MODEL_LIST_PATH,
                get(move |headers: HeaderMap| {
                    let requests = std::sync::Arc::clone(&model_requests_for_route);
                    let observed_users = std::sync::Arc::clone(&observed_users_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        observed_users.lock().unwrap().push(
                            headers
                                .get("cosy-user")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        );
                        (StatusCode::UNAUTHORIZED, Json(json!({"error": "expired"})))
                    }
                }),
            )
            .route(
                "/api/v1/deviceToken/refresh",
                post(move || {
                    let requests = std::sync::Arc::clone(&refresh_requests_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({
                            "device_token": "qoder-refreshed-access-bound-user",
                            "refresh_token": "qoder-refreshed-refresh-bound-user",
                            "expires_at": chrono::Utc::now().timestamp() + 3_600
                        }))
                    }
                }),
            )
            .route(
                "/api/v1/userinfo",
                get(|| async {
                    Json(json!({
                        "id": "bound-user",
                        "user_id": "bound-user",
                        "account_id": "aid-bound-user",
                        "name": "Qoder Catalog Fixture",
                        "user_type": "personal_standard"
                    }))
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("qoder-second-401");
        configure_qoder_catalog_fixture(
            &state,
            "qoder-second-401-provider",
            &format!("http://{address}"),
            crate::domain::qoder::QoderSite::Global,
            &[
                ("qoder-second-401-distractor", "distractor-user"),
                ("qoder-second-401-bound", "bound-user"),
            ],
            "qoder-second-401-bound",
        )
        .await;

        let error = proxy_models_for_selection(
            &state,
            Some(AppKind::Codex),
            Some("qoder-second-401-provider"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            refresh_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(model_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            observed_users.lock().unwrap().as_slice(),
            ["bound-user", "bound-user"]
        );
        let accounts = state.accounts_snapshot().await;
        assert_eq!(
            accounts
                .accounts
                .iter()
                .find(|account| account.id == "qoder-second-401-bound")
                .unwrap()
                .token_refresh_generation,
            2
        );
        assert_eq!(
            accounts
                .accounts
                .iter()
                .find(|account| account.id == "qoder-second-401-distractor")
                .unwrap()
                .token_refresh_generation,
            1
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
