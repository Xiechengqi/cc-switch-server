use std::convert::Infallible;

use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use crate::api::session::{require_event_session, require_session, router_web_user_email};
use crate::build_info::{build_info, BuildInfo};
use crate::self_update::restart::{
    restart_from_detected_service, rollback_from_backup_and_restart,
};
use crate::self_update::upgrade::{UpgradeFailureDiagnostic, UpgradeLogEntry, UpgradeStatus};
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
    pub process_id: u32,
    pub process_instance_id: String,
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
    #[serde(default)]
    force: bool,
}

fn default_restart_after() -> bool {
    true
}

pub(in crate::api) fn start_upgrade_request(
    restart_after: bool,
    force: bool,
) -> StartUpgradeRequest {
    StartUpgradeRequest {
        restart_after,
        force,
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<UpgradeFailureDiagnostic>,
    pub restart_after: bool,
    pub updated_at: String,
}

pub(in crate::api) async fn build_admin_runtime_version_response(
    state: &ServerState,
) -> AdminVersionResponse {
    let upgrade_capable = ensure_binary_writable().is_ok();
    AdminVersionResponse {
        build: build_info(),
        binary_path: BINARY_INSTALL_PATH,
        rollback_path: BINARY_ROLLBACK_PATH,
        rollback_available: rollback_available(),
        process_id: std::process::id(),
        process_instance_id: state.process_instance_id.clone(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        restart_pending: state.upgrade.is_restart_pending().await,
        upgrade_capable,
        service: crate::self_update::version::detect_service_status(),
        latest: crate::self_update::version::default_latest_release_meta(),
    }
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
        process_id: std::process::id(),
        process_instance_id: state.process_instance_id.clone(),
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
    let _command = state.market_workspace.commands.lock().await;
    require_session(&state, &headers).await?;
    if state.market_workspace.management_locked() {
        super::market_workspace::tenant(&state, &headers).await?;
        if state.client_update_in_progress().await {
            return Err(ApiError::conflict("Client update in progress"));
        }
    }
    let schedule = map_self_update_error(restart_from_detected_service(
        &state.market_workspace.root,
        state.bind_addr,
    ))?;
    state.upgrade.clear_restart_pending().await;
    if let Some(g) = state.market_workspace.access().1 {
        state
            .market_workspace
            .record_event(&g.lease_id, "tenant_restart")
            .map_err(ApiError::internal)?;
        return Ok(Json(
            serde_json::json!({"ok":true,"operationId":schedule.operation_id}),
        ));
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "script": schedule.command,
        "operationId": schedule.operation_id,
    })))
}

pub(in crate::api) async fn admin_rollback(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _command = state.market_workspace.commands.lock().await;
    require_session(&state, &headers).await?;
    if state.market_workspace.management_locked() {
        super::market_workspace::tenant(&state, &headers).await?;
        if state.client_update_in_progress().await {
            return Err(ApiError::conflict("Client update in progress"));
        }
        map_self_update_error(
            crate::self_update::upgrade::sanity_exec(
                std::path::Path::new(BINARY_ROLLBACK_PATH),
                &state.market_workspace.root,
            )
            .await,
        )?;
        super::market_workspace::tenant(&state, &headers).await?;
    }
    map_self_update_error(ensure_binary_writable())?;
    let script = map_self_update_error(rollback_from_backup_and_restart(
        &state.market_workspace.root,
        state.bind_addr,
    ))?;
    state.upgrade.clear_restart_pending().await;
    if let Some(g) = state.market_workspace.access().1 {
        state
            .market_workspace
            .record_event(&g.lease_id, "tenant_rollback")
            .map_err(ApiError::internal)?;
        return Ok(Json(serde_json::json!({"ok":true})));
    }
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
    if state.market_workspace.management_locked() {
        let _command = state.market_workspace.commands.lock().await;
        let grant = super::market_workspace::tenant(&state, &headers).await?;
        let task = format!("lease-{}-{:032x}", grant.lease_id, rand::random::<u128>());
        let id = start_authorized_upgrade(&state, true, "tenant", false, Some(task), None).await?;
        state
            .market_workspace
            .record_event(&grant.lease_id, "tenant_upgrade")
            .map_err(ApiError::internal)?;
        return Ok(Json(serde_json::json!({"ok":true,"taskId":id})));
    }
    ensure_upgrade_actor_allowed(&state, &headers).await?;
    let actor = if router_web_user_email(&headers).is_some() {
        "router-owner"
    } else {
        "web-admin"
    };
    let task_id = start_upgrade_for_actor(&state, input.restart_after, actor, input.force).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "taskId": task_id,
    })))
}

async fn ensure_upgrade_actor_allowed(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let Some(router_email) = router_web_user_email(headers) else {
        return Ok(());
    };
    let config = state.config.read().await;
    if !config.upgrade_policy.delegate_upgrade_to_router_owner {
        return Err(ApiError::forbidden(
            "router owner upgrade delegation is disabled",
        ));
    }
    let owner_email = config
        .owner
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if owner_email.is_empty() || router_email != owner_email {
        return Err(ApiError::forbidden(
            "only installation owner can trigger upgrade",
        ));
    }
    Ok(())
}

pub(crate) async fn start_upgrade_for_actor(
    state: &ServerState,
    restart_after: bool,
    actor: &str,
    force: bool,
) -> Result<String, ApiError> {
    let _command = state.market_workspace.commands.lock().await;
    if state.market_workspace.management_locked() {
        return Err(ApiError::forbidden("current rental required"));
    }
    start_authorized_upgrade(state, restart_after, actor, force, None, None).await
}

pub(super) async fn start_authorized_upgrade(
    state: &ServerState,
    restart_after: bool,
    actor: &str,
    force: bool,
    task_id: Option<String>,
    expected_commit: Option<String>,
) -> Result<String, ApiError> {
    if crate::self_update::restart::restart_in_progress(&state.market_workspace.root) {
        return Err(ApiError::conflict("Client restart in progress"));
    }
    map_self_update_error(ensure_binary_writable())?;
    let client = crate::infra::http::self_update_client_builder()
        .user_agent("cc-switch-server/0.1 upgrade")
        .build()
        .map_err(|err| ApiError::internal(format!("upgrade client failed: {err}")))?;
    let result = match task_id {
        Some(id) => {
            state
                .upgrade
                .start_task(
                    client,
                    Some(actor.to_owned()),
                    restart_after,
                    force,
                    state.bind_addr,
                    id,
                    expected_commit,
                )
                .await
        }
        None => {
            state
                .upgrade
                .start(
                    client,
                    Some(actor.to_owned()),
                    restart_after,
                    force,
                    state.bind_addr,
                )
                .await
        }
    };
    let handle = map_self_update_error(result)?;
    Ok(handle.task_id)
}

pub(super) fn workspace_can_view_upgrade(state: &ServerState, task_id: &str) -> bool {
    let (_, grant, live) = state.market_workspace.access();
    if let Some(grant) = grant {
        live && task_id.starts_with(&format!("lease-{}-", grant.lease_id))
    } else {
        !state.market_workspace.management_locked() && !task_id.starts_with("lease-")
    }
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
        .filter(|handle| workspace_can_view_upgrade(&state, &handle.task_id))
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
    let mut shutdown = state.subscribe_shutdown();
    let stream = async_stream::stream! {
        for entry in history {
            if !workspace_can_view_upgrade(&state, &handle.task_id) { return; }
            yield Ok(sse_event_from_entry(&entry));
        }
        if let Some(event) = emit_done_if_finished(&status, &restart_pending).await {
            yield Ok(event);
            return;
        }
        let mut rx = receiver;
        loop {
            let received = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                    continue;
                }
                received = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()) => received,
            };
            if !workspace_can_view_upgrade(&state, &handle.task_id) { break; }
            match received {
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
    upgrade_status_for_task(&state, query.task_id.as_deref()).await
}

pub(super) async fn upgrade_status_for_task(
    state: &ServerState,
    task_id: Option<&str>,
) -> Result<Json<AdminUpgradeStatusResponse>, ApiError> {
    let snapshot = state
        .upgrade
        .status_snapshot()
        .await
        .filter(|snapshot| workspace_can_view_upgrade(state, &snapshot.task_id))
        .ok_or_else(|| ApiError::not_found("no upgrade task running"))?;
    if let Some(expected) = task_id {
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
        failure: snapshot.failure,
        restart_after: snapshot.restart_after,
        updated_at: snapshot.updated_at,
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
