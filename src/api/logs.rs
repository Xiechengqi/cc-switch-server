use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;

use super::error::ApiError;
use crate::api::session::require_session;
use crate::logging::{LogTailAccessError, LogTailResponse};
use crate::state::ServerState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct LogTailQuery {
    #[serde(default)]
    lines: Option<usize>,
}

pub(in crate::api) async fn admin_logs_tail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<LogTailQuery>,
) -> Result<Json<LogTailResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let response = state
        .read_admin_log_tail(query.lines)
        .await
        .map_err(|error| match error {
            LogTailAccessError::Disabled => ApiError::forbidden("log api is disabled"),
        })?;
    Ok(Json(response))
}
