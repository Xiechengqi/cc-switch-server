mod history;
mod manager;
mod options;
mod protocol;
mod session;

pub use manager::OpsTerminalManager;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::api::error::ApiError;
use crate::api::session::{require_web_admin_session, WebAdminPrincipal};
use crate::state::ServerState;

use self::manager::{AttachError, SessionAccessError};
use self::protocol::{decode_input_payload, ServerMessage};
use self::session::{next_client_id, SessionCommand, TerminalSession};

const TERMINAL_EVENT: &str = "terminal";

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalInputPayload {
    d: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalResizePayload {
    c: u16,
    r: u16,
}

pub(crate) async fn terminal_stream(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_enabled(&state).await?;
    let principal = require_web_admin_session(&state, &headers).await?;
    let session = state
        .terminal
        .attach_or_create(principal.user_email(), &state.config_dir)
        .await
        .map_err(map_attach_error)?;

    let client_id = next_client_id();
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replay_tx, replay_rx) = oneshot::channel();
    session
        .request(SessionCommand::Attach {
            client_id,
            output_tx,
            replay_tx,
        })
        .map_err(|_| ApiError::conflict("terminal session closed before attach"))?;
    let attachment = TerminalAttachment::new(Arc::clone(&session), client_id);
    let replay = replay_rx
        .await
        .map_err(|_| ApiError::conflict("terminal session closed during attach"))?;

    let stream = async_stream::stream! {
        let _attachment = attachment;
        yield Ok(terminal_event(ServerMessage::ReplayBegin));
        for chunk in replay {
            yield Ok(terminal_event(ServerMessage::output_bytes(&chunk)));
        }
        yield Ok(terminal_event(ServerMessage::ReplayEnd));

        while let Some(chunk) = output_rx.recv().await {
            yield Ok(terminal_event(ServerMessage::output_bytes(&chunk)));
        }
        yield Ok(terminal_event(ServerMessage::Exit { c: None }));
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("terminal-keepalive"),
    ))
}

pub(crate) async fn terminal_input(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(payload): Json<TerminalInputPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_enabled(&state).await?;
    let principal = require_web_admin_session(&state, &headers).await?;
    let bytes = decode_input_payload(&payload.d).map_err(ApiError::bad_request)?;
    request_for_principal(&state, &principal, SessionCommand::Input(bytes)).await?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn terminal_resize(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(payload): Json<TerminalResizePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_enabled(&state).await?;
    let principal = require_web_admin_session(&state, &headers).await?;
    request_for_principal(
        &state,
        &principal,
        SessionCommand::Resize {
            cols: payload.c,
            rows: payload.r,
        },
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn terminal_session_end(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_enabled(&state).await?;
    let principal = require_web_admin_session(&state, &headers).await?;
    if let Some(owner) = state.terminal.current_owner().await {
        if owner != principal.user_email() {
            return Err(ApiError::conflict_code(
                "terminal_busy",
                format!("terminal session is owned by {owner}"),
            ));
        }
    }
    state.terminal.end_session().await;
    Ok(Json(json!({ "ok": true })))
}

async fn ensure_enabled(state: &ServerState) -> Result<(), ApiError> {
    let config = state.config.read().await;
    if !config.is_web_terminal_enabled() {
        return Err(ApiError::feature_disabled(
            "web terminal is disabled; set enableWebTerminal=true or unset CC_SWITCH_ENABLE_WEB_TERMINAL=0",
        ));
    }
    Ok(())
}

async fn request_for_principal(
    state: &ServerState,
    principal: &WebAdminPrincipal,
    command: SessionCommand,
) -> Result<(), ApiError> {
    state
        .terminal
        .request_for_owner(principal.user_email(), command)
        .await
        .map_err(map_session_access_error)
}

fn map_attach_error(error: AttachError) -> ApiError {
    match error {
        AttachError::Busy { owner } => ApiError::conflict_code(
            "terminal_busy",
            format!("terminal session is owned by {owner}"),
        ),
        AttachError::Spawn(message) => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to start terminal: {message}"),
        ),
    }
}

fn map_session_access_error(error: SessionAccessError) -> ApiError {
    match error {
        SessionAccessError::NotActive => {
            ApiError::conflict_code("terminal_not_active", "terminal session is not active")
        }
        SessionAccessError::Busy { owner } => ApiError::conflict_code(
            "terminal_busy",
            format!("terminal session is owned by {owner}"),
        ),
        SessionAccessError::Closed => {
            ApiError::conflict_code("terminal_closed", "terminal session is closed")
        }
    }
}

fn terminal_event(message: ServerMessage) -> Event {
    let payload = message
        .to_text()
        .unwrap_or_else(|_| r#"{"t":"err","m":"failed to encode terminal event"}"#.to_string());
    Event::default().event(TERMINAL_EVENT).data(payload)
}

struct TerminalAttachment {
    session: Arc<TerminalSession>,
    client_id: u64,
}

impl TerminalAttachment {
    fn new(session: Arc<TerminalSession>, client_id: u64) -> Self {
        Self { session, client_id }
    }
}

impl Drop for TerminalAttachment {
    fn drop(&mut self) {
        let _ = self.session.request(SessionCommand::Detach {
            client_id: self.client_id,
        });
    }
}
