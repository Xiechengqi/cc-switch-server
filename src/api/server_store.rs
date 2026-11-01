//! Web admin view of the Server SQLite authority and the one-shot request
//! that commits it at the next start (see `ActivationTrigger::Requested`).

use axum::extract::State;
use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::api::error::ApiError;
use crate::repository::server_sqlite::{
    self, ActivationRequest, ActivationRequestStatus, AuthorityState,
};
use crate::state::ServerState;

fn authority_label(authority: Option<AuthorityState>) -> &'static str {
    match authority {
        None => "legacy",
        Some(AuthorityState::ShadowVerified) => "shadow_verified",
        Some(AuthorityState::Prepared) => "prepared",
        Some(AuthorityState::Committed) => "committed",
    }
}

pub(crate) async fn authority_snapshot(state: &ServerState) -> Result<Value, ApiError> {
    let config_dir = state.config_dir.clone();
    let (marker, request, shadow) = tokio::task::spawn_blocking(move || {
        let marker = server_sqlite::read_marker(&config_dir)?;
        let request = server_sqlite::read_activation_request(&config_dir)?;
        let shadow = match marker.as_ref().map(|marker| marker.authority) {
            Some(AuthorityState::ShadowVerified | AuthorityState::Prepared) => Some(
                server_sqlite::shadow_is_current(&config_dir).map_err(|error| format!("{error:#}")),
            ),
            _ => None,
        };
        anyhow::Ok((marker, request, shadow))
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    let counts = json!({
        "providers": state.providers.read().await.providers.len(),
        "accounts": state.accounts.read().await.accounts.len(),
        "shares": state.shares.read().await.shares.len(),
        "usageRecords": state.usage.read().await.logs.len(),
    });
    let authority = marker.as_ref().map(|marker| marker.authority);
    Ok(json!({
        "authority": authority_label(authority),
        "committedAtMs": marker.as_ref().and_then(|marker| marker.committed_at_ms),
        "legacyBackupDir": marker.as_ref().and_then(|marker| marker.legacy_backup_dir.clone()),
        "shadowCurrent": shadow.as_ref().and_then(|result| result.as_ref().ok().copied()),
        "shadowError": shadow.and_then(Result::err),
        // A request left over from before a commit is moot.
        "activationRequest": request.filter(|_| authority != Some(AuthorityState::Committed)),
        "restartStrategy": crate::self_update::restart::detect_restart_strategy().label(),
        "counts": counts,
    }))
}

/// Records the request, then (by default) restarts through the same helper as
/// the admin restart. A failed restart keeps the request for the next start.
pub(crate) async fn request_activation(
    state: &ServerState,
    headers: &HeaderMap,
    restart: bool,
) -> Result<Value, ApiError> {
    let authority = server_sqlite::read_marker(&state.config_dir)
        .map_err(ApiError::internal)?
        .map(|marker| marker.authority);
    if authority == Some(AuthorityState::Committed) {
        return Err(ApiError::conflict(
            "Server SQLite authority is already committed",
        ));
    }
    let requested_by = crate::api::session::resolve_web_admin_principal(state, headers)
        .await?
        .map(|principal| principal.user_email().to_string())
        .unwrap_or_else(|| "web-admin".to_string());
    let request = ActivationRequest {
        status: ActivationRequestStatus::Pending,
        requested_at_ms: crate::infra::time::now_ms() as i64,
        requested_by,
        failed_at_ms: None,
        error: None,
    };
    server_sqlite::write_activation_request(&state.config_dir, &request)
        .map_err(ApiError::internal)?;
    tracing::info!(requested_by = %request.requested_by, restart, "SQLite authority activation requested for the next start");
    let (operation_id, restart_error) = if restart {
        match crate::api::self_update::admin_restart(State(state.clone()), headers.clone()).await {
            Ok(response) => (
                response
                    .0
                    .get("operationId")
                    .cloned()
                    .unwrap_or(Value::Null),
                None,
            ),
            Err(error) => (Value::Null, Some(error.message)),
        }
    } else {
        (Value::Null, None)
    };
    Ok(json!({
        "ok": true,
        "activationRequest": request,
        "restartOperationId": operation_id,
        "restartError": restart_error,
    }))
}

pub(crate) async fn cancel_activation(state: &ServerState) -> Result<Value, ApiError> {
    server_sqlite::clear_activation_request(&state.config_dir).map_err(ApiError::internal)?;
    Ok(json!({"ok": true}))
}
