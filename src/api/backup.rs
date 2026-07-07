use super::*;

pub(in crate::api) async fn list_backups(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BackupListResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let backups =
        crate::infra::backup::list_backups(&state.config_dir).map_err(ApiError::internal)?;
    Ok(Json(BackupListResponse { ok: true, backups }))
}

pub(in crate::api) async fn create_backup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Option<Json<CreateBackupRequest>>,
) -> Result<Json<BackupCreateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let reason = body.and_then(|Json(input)| input.reason);
    let backup = crate::infra::backup::create_backup(
        &state.config_dir,
        &crate::state::backup_targets(&state.config_dir),
        reason,
    )
    .map_err(ApiError::internal)?;
    state.emit_event(
        ServerEvent::new("backup.created", "backup")
            .id(backup.id.clone())
            .message("manual"),
    );
    Ok(Json(BackupCreateResponse { ok: true, backup }))
}

pub(in crate::api) async fn restore_backup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BackupRestoreResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let result = crate::infra::backup::restore_backup(&state.config_dir, &id)
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
