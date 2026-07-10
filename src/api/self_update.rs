use std::convert::Infallible;

use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use crate::api::session::{require_event_session, require_session};
use crate::build_info::{build_info, BuildInfo};
use crate::self_update::restart::{
    restart_from_detected_service, rollback_from_backup_and_restart,
};
use crate::self_update::upgrade::{UpgradeLogEntry, UpgradeStatus};
use crate::self_update::version::{
    ensure_binary_writable, fetch_latest_release_meta, rollback_available, BINARY_INSTALL_PATH,
    BINARY_ROLLBACK_PATH,
};
use crate::state::ServerState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AdminVersionResponse {
    #[serde(flatten)]
    pub build: BuildInfo,
    pub binary_path: &'static str,
    pub rollback_path: &'static str,
    pub rollback_available: bool,
    pub uptime_secs: u64,
    pub restart_pending: bool,
    pub upgrade_capable: bool,
    pub service: crate::self_update::version::ServiceStatus,
    pub latest: crate::self_update::version::LatestReleaseMeta,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartUpgradeRequest {
    #[serde(default = "default_restart_after")]
    restart_after: bool,
}

fn default_restart_after() -> bool {
    true
}

pub(in crate::api) fn start_upgrade_request(restart_after: bool) -> StartUpgradeRequest {
    StartUpgradeRequest { restart_after }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpgradeStreamQuery {
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpgradeStatusQuery {
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AdminUpgradeStatusResponse {
    pub task_id: String,
    pub status: &'static str,
    pub restart_pending: bool,
    pub target_commit_id: Option<String>,
    pub logs: Vec<UpgradeLogEntry>,
}

pub(in crate::api) async fn build_admin_version_response(
    state: &ServerState,
) -> AdminVersionResponse {
    let client = state.http_client().await;
    let latest = fetch_latest_release_meta(&client).await;
    let upgrade_capable = ensure_binary_writable().is_ok();
    AdminVersionResponse {
        build: build_info(),
        binary_path: BINARY_INSTALL_PATH,
        rollback_path: BINARY_ROLLBACK_PATH,
        rollback_available: rollback_available(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        restart_pending: state.upgrade.is_restart_pending().await,
        upgrade_capable,
        service: crate::self_update::version::detect_service_status(),
        latest,
    }
}

pub(in crate::api) async fn admin_version(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AdminVersionResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(build_admin_version_response(&state).await))
}

pub(in crate::api) async fn admin_restart(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_session(&state, &headers).await?;
    let script = map_self_update_error(restart_from_detected_service(
        &state.config_dir,
        state.bind_addr,
    ))?;
    state.upgrade.clear_restart_pending().await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "script": script,
    })))
}

pub(in crate::api) async fn admin_rollback(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_session(&state, &headers).await?;
    map_self_update_error(ensure_binary_writable())?;
    let script = map_self_update_error(rollback_from_backup_and_restart(
        &state.config_dir,
        state.bind_addr,
    ))?;
    state.upgrade.clear_restart_pending().await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "script": script,
    })))
}

pub(in crate::api) async fn admin_upgrade_start(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartUpgradeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_session(&state, &headers).await?;
    map_self_update_error(ensure_binary_writable())?;
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-server/0.1 upgrade")
        .build()
        .map_err(|err| ApiError::internal(format!("upgrade client failed: {err}")))?;
    let handle = map_self_update_error(
        state
            .upgrade
            .start(
                client,
                Some("web-admin".to_string()),
                input.restart_after,
                state.bind_addr,
            )
            .await,
    )?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "taskId": handle.task_id,
    })))
}

pub(in crate::api) async fn admin_upgrade_stream(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UpgradeStreamQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_event_session(&state, &headers).await?;

    let handle = state
        .upgrade
        .current()
        .await
        .ok_or_else(|| ApiError::not_found("no upgrade task running"))?;
    if let Some(expected) = query.task_id.as_deref() {
        if expected != handle.task_id {
            return Err(ApiError::not_found("upgrade task id does not match"));
        }
    }

    let history_guard = handle.history.lock().await;
    let receiver = handle.sender.subscribe();
    let history: Vec<UpgradeLogEntry> = history_guard.clone();
    drop(history_guard);
    let status = handle.status.clone();
    let restart_pending = handle.restart_pending.clone();
    let registry = state.upgrade.clone();
    let stream = async_stream::stream! {
        for entry in history {
            yield Ok(sse_event_from_entry(&entry));
        }
        if let Some(event) = emit_done_if_finished(&status, &restart_pending).await {
            yield Ok(event);
            return;
        }
        let mut rx = receiver;
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(entry)) => yield Ok(sse_event_from_entry(&entry)),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    if let Some(event) = emit_done_if_finished(&status, &restart_pending).await {
                        yield Ok(event);
                    }
                    break;
                }
                Err(_) => {}
            }
            registry.refresh_from_disk().await;
            if let Some(event) = emit_done_if_finished(&status, &restart_pending).await {
                while let Ok(entry) = rx.try_recv() {
                    yield Ok(sse_event_from_entry(&entry));
                }
                yield Ok(event);
                break;
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15))))
}

pub(in crate::api) async fn admin_upgrade_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UpgradeStatusQuery>,
) -> Result<Json<AdminUpgradeStatusResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let snapshot = state
        .upgrade
        .status_snapshot()
        .await
        .ok_or_else(|| ApiError::not_found("no upgrade task running"))?;
    if let Some(expected) = query.task_id.as_deref() {
        if expected != snapshot.task_id {
            return Err(ApiError::not_found("upgrade task id does not match"));
        }
    }
    Ok(Json(AdminUpgradeStatusResponse {
        task_id: snapshot.task_id,
        status: upgrade_status_label(snapshot.status),
        restart_pending: snapshot.restart_pending,
        target_commit_id: snapshot.target_commit_id,
        logs: snapshot.logs,
    }))
}

fn upgrade_status_label(status: UpgradeStatus) -> &'static str {
    match status {
        UpgradeStatus::Running => "running",
        UpgradeStatus::Success => "success",
        UpgradeStatus::Failed => "failed",
    }
}

fn map_self_update_error<T>(
    result: Result<T, crate::self_update::version::SelfUpdateError>,
) -> Result<T, ApiError> {
    result.map_err(|error| match error {
        crate::self_update::version::SelfUpdateError::Forbidden(message) => {
            ApiError::forbidden(message)
        }
        crate::self_update::version::SelfUpdateError::Internal(message) => {
            if message.contains("already in progress") {
                ApiError::conflict(message)
            } else {
                ApiError::internal(message)
            }
        }
    })
}

fn sse_event_from_entry(entry: &UpgradeLogEntry) -> Event {
    Event::default()
        .event("log")
        .data(serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string()))
}

async fn emit_done_if_finished(
    status: &std::sync::Arc<tokio::sync::Mutex<UpgradeStatus>>,
    restart_pending: &std::sync::Arc<tokio::sync::Mutex<bool>>,
) -> Option<Event> {
    let current = *status.lock().await;
    if matches!(current, UpgradeStatus::Running) {
        return None;
    }
    let payload = serde_json::json!({
        "status": match current {
            UpgradeStatus::Success => "success",
            UpgradeStatus::Failed => "failed",
            UpgradeStatus::Running => "running",
        },
        "restartPending": *restart_pending.lock().await,
    });
    Some(Event::default().event("done").data(payload.to_string()))
}
