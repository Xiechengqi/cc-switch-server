use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};

use super::error::ApiError;
use crate::domain::settings::ui_settings::{self, ParsedApiManagementConfig};
use crate::self_update::restart::{read_restart_operation, restart_from_detected_service};
use crate::self_update::upgrade::UpgradeStatus;
use crate::state::ServerState;

const DEFAULT_TOKEN_TTL_HOURS: u64 = 1;
const MAX_TOKEN_TTL_HOURS: u64 = 24;
const RESTART_OPERATION_STALE_SECONDS: i64 = 120;

#[derive(Debug, Clone, Copy)]
enum DebugCapability {
    Diagnostics,
    Logs,
    Restart,
    RestartStatus,
    Upgrade,
    UpgradeStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DebugLogQuery {
    lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DebugUpgradeRequest {
    #[serde(default = "default_true")]
    restart_after: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DebugUpgradeQuery {
    task_id: Option<String>,
}

fn default_true() -> bool {
    true
}

pub(crate) async fn api_management_snapshot(state: &ServerState) -> Value {
    let settings = state.ui_settings.read().await;
    let mut value = ui_settings::api_management_config_for_frontend(&settings);
    drop(settings);
    let config = state.config.read().await;
    if let Value::Object(ref mut map) = value {
        map.insert(
            "tokenConfigured".into(),
            json!(config.auth.debug_token_hash.is_some()),
        );
        map.insert(
            "tokenExpiresAtMs".into(),
            json!(config.auth.debug_token_expires_at_ms),
        );
    }
    value
}

pub(crate) async fn save_api_management(
    state: &ServerState,
    value: Value,
) -> Result<Value, ApiError> {
    let parsed = ui_settings::parse_api_management_config(&value);
    let normalized = json!({
        "logEnabled": parsed.log_enabled,
        "restartEnabled": parsed.restart_enabled,
        "upgradeEnabled": parsed.upgrade_enabled,
        "diagnosticsEnabled": parsed.diagnostics_enabled,
        "logTailLines": parsed.log_tail_lines,
    });
    state
        .apply_ui_settings_patch_immediate(json!({ "apiManagement": normalized }))
        .await
        .map_err(ApiError::internal)?;
    Ok(api_management_snapshot(state).await)
}

pub(crate) async fn generate_debug_token(
    state: &ServerState,
    ttl_hours: Option<u64>,
) -> Result<Value, ApiError> {
    let ttl_hours = ttl_hours
        .unwrap_or(DEFAULT_TOKEN_TTL_HOURS)
        .clamp(1, MAX_TOKEN_TTL_HOURS);
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let expires_at_ms = now_ms.saturating_add((ttl_hours as i64) * 60 * 60 * 1000);
    state
        .set_debug_token(&token, expires_at_ms)
        .await
        .map_err(ApiError::internal)?;
    Ok(json!({
        "token": token,
        "expiresAtMs": expires_at_ms,
        "ttlHours": ttl_hours,
    }))
}

pub(crate) async fn revoke_debug_token(state: &ServerState) -> Result<Value, ApiError> {
    state
        .revoke_debug_token()
        .await
        .map_err(ApiError::internal)?;
    Ok(json!({ "ok": true }))
}

async fn require_debug_capability(
    state: &ServerState,
    headers: &HeaderMap,
    capability: DebugCapability,
) -> Result<ParsedApiManagementConfig, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::unauthorized("debug bearer token is required"))?;
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    if !state.config.read().await.verify_debug_token(token, now_ms) {
        return Err(ApiError::unauthorized("debug token is invalid or expired"));
    }
    let store = state.ui_settings.read().await;
    let policy = ui_settings::parse_api_management_config(
        &ui_settings::api_management_config_for_frontend(&store),
    );
    let enabled = match capability {
        DebugCapability::Diagnostics => policy.diagnostics_enabled,
        DebugCapability::Logs => policy.log_enabled,
        DebugCapability::Restart => policy.restart_enabled,
        DebugCapability::RestartStatus => policy.restart_enabled || policy.diagnostics_enabled,
        DebugCapability::Upgrade => policy.upgrade_enabled,
        DebugCapability::UpgradeStatus => policy.upgrade_enabled || policy.diagnostics_enabled,
    };
    if !enabled {
        return Err(ApiError::forbidden("requested debug API is disabled"));
    }
    Ok(policy)
}

pub(crate) async fn debug_runtime(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::Diagnostics).await?;
    Ok(Json(runtime_snapshot(&state)))
}

pub(crate) async fn debug_diagnostics(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::Diagnostics).await?;
    let upgrade = state.upgrade.status_snapshot().await;
    let helper_log =
        crate::logging::tail_file_lines(&state.config_dir.join("restart-helper.log"), 50)
            .unwrap_or_default()
            .join("\n");
    Ok(Json(json!({
        "runtime": runtime_snapshot(&state),
        "restartOperation": read_restart_operation(&state.config_dir),
        "restartHelperLog": crate::logging::redact_sensitive_text(&helper_log),
        "upgradeOperation": upgrade,
        "generatedAt": chrono::Utc::now().to_rfc3339(),
    })))
}

pub(crate) async fn debug_logs_tail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<DebugLogQuery>,
) -> Result<Json<crate::logging::LogTailResponse>, ApiError> {
    let policy = require_debug_capability(&state, &headers, DebugCapability::Logs).await?;
    let requested = query.lines.map(|lines| lines.min(policy.log_tail_lines));
    let mut response = state
        .read_admin_log_tail(requested)
        .await
        .map_err(|_| ApiError::forbidden("logging is disabled"))?;
    response.content = crate::logging::redact_sensitive_text(&response.content);
    response.path = "[redacted]".into();
    Ok(Json(response))
}

pub(crate) async fn debug_restart(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::Restart).await?;
    if let Some(operation) = read_restart_operation(&state.config_dir) {
        let updated_at = chrono::DateTime::parse_from_rfc3339(&operation.updated_at)
            .map(|value| value.timestamp())
            .unwrap_or(i64::MIN);
        if operation.status == "running"
            && chrono::Utc::now().timestamp().saturating_sub(updated_at)
                < RESTART_OPERATION_STALE_SECONDS
        {
            return Err(ApiError::conflict("a restart operation is already running"));
        }
    }
    let schedule = restart_from_detected_service(&state.config_dir, state.bind_addr)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    state.upgrade.clear_restart_pending().await;
    Ok(Json(json!({
        "ok": true,
        "operationId": schedule.operation_id,
        "status": "running",
    })))
}

pub(crate) async fn debug_restart_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::RestartStatus).await?;
    let operation = read_restart_operation(&state.config_dir)
        .filter(|operation| operation.operation_id == operation_id)
        .ok_or_else(|| ApiError::not_found("restart operation was not found"))?;
    Ok(Json(json!(operation)))
}

pub(crate) async fn debug_upgrade_start(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<DebugUpgradeRequest>,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::Upgrade).await?;
    let task_id = super::self_update::start_upgrade_for_actor(
        &state,
        input.restart_after,
        "remote-debug",
        false,
    )
    .await?;
    Ok(Json(json!({ "ok": true, "taskId": task_id })))
}

pub(crate) async fn debug_upgrade_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<DebugUpgradeQuery>,
) -> Result<Json<Value>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::UpgradeStatus).await?;
    let snapshot = state
        .upgrade
        .status_snapshot()
        .await
        .ok_or_else(|| ApiError::not_found("no upgrade task is available"))?;
    if query
        .task_id
        .as_deref()
        .is_some_and(|id| id != snapshot.task_id)
    {
        return Err(ApiError::not_found("upgrade task id does not match"));
    }
    Ok(Json(json!(snapshot)))
}

pub(crate) async fn debug_upgrade_stream(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<DebugUpgradeQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_debug_capability(&state, &headers, DebugCapability::UpgradeStatus).await?;
    let initial = state
        .upgrade
        .status_snapshot()
        .await
        .ok_or_else(|| ApiError::not_found("no upgrade task is available"))?;
    if query
        .task_id
        .as_deref()
        .is_some_and(|id| id != initial.task_id)
    {
        return Err(ApiError::not_found("upgrade task id does not match"));
    }
    let task_id = initial.task_id;
    let registry = state.upgrade.clone();
    let stream = async_stream::stream! {
        loop {
            registry.refresh_from_disk().await;
            let Some(snapshot) = registry.status_snapshot().await else { break; };
            if snapshot.task_id != task_id { break; }
            let finished = !matches!(snapshot.status, UpgradeStatus::Running);
            yield Ok(Event::default().event("status").data(
                serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".into())
            ));
            if finished { break; }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

fn runtime_snapshot(state: &ServerState) -> Value {
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("unavailable: {error}"));
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("unavailable: {error}"));
    json!({
        "processId": std::process::id(),
        "parentProcessId": parent_process_id(),
        "processInstanceId": state.process_instance_id,
        "uptimeSecs": state.started_at.elapsed().as_secs(),
        "executable": executable,
        "arguments": std::env::args().collect::<Vec<_>>(),
        "workingDirectory": cwd,
        "configDirectory": state.config_dir.display().to_string(),
        "bindAddress": state.bind_addr.to_string(),
        "restartStrategy": crate::self_update::restart::detect_restart_strategy().label(),
        "service": crate::self_update::version::detect_service_status(),
        "build": crate::build_info::build_info(),
    })
}

fn parent_process_id() -> Option<u32> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
