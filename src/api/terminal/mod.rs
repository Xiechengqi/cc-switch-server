mod history;
mod manager;
mod options;
mod protocol;
mod session;

pub use manager::OpsTerminalManager;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Query, State};
use axum::http::{HeaderMap, Request};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use crate::api::error::ApiError;
use crate::api::session::{
    bearer_token, require_web_admin_session, resolve_web_admin_principal, WebAdminPrincipal,
};
use crate::state::ServerState;

use self::manager::AttachError;
use self::protocol::{decode_client_message, decode_input_payload, ClientMessage, ServerMessage};
use self::session::{next_client_id, SessionCommand};

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalWsQuery {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

pub(crate) async fn terminal_ws(
    State(state): State<ServerState>,
    Query(query): Query<TerminalWsQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    // Feature/auth gates must run before WebSocketUpgrade extraction so disabled
    // nodes return 403 instead of extractor 4xx.
    ensure_enabled(&state).await?;
    let headers = request.headers().clone();
    let principal = authorize_terminal(&state, &headers, &query).await?;
    let owner = principal.user_email().to_string();

    let (mut parts, _body) = request.into_parts();
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &state)
        .await
        .map_err(|_| ApiError::bad_request("expected websocket upgrade"))?;

    Ok(ws
        .on_upgrade(move |socket| async move {
            if let Err(error) = handle_terminal_socket(state, owner, socket).await {
                tracing::debug!(error = %error, "web terminal websocket closed with error");
            }
        })
        .into_response())
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

async fn authorize_terminal(
    state: &ServerState,
    headers: &HeaderMap,
    query: &TerminalWsQuery,
) -> Result<WebAdminPrincipal, ApiError> {
    if let Some(principal) = resolve_web_admin_principal(state, headers).await? {
        return Ok(principal);
    }
    let token = query
        .token
        .as_deref()
        .or(query.access_token.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| bearer_token(headers));
    let Some(token) = token else {
        return Err(ApiError::unauthorized("missing or invalid bearer token"));
    };

    let mut synthetic = HeaderMap::new();
    synthetic.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .map_err(|_| ApiError::unauthorized("invalid bearer token"))?,
    );
    require_web_admin_session(state, &synthetic).await
}

async fn handle_terminal_socket(
    state: ServerState,
    owner: String,
    socket: WebSocket,
) -> Result<(), String> {
    let session = match state
        .terminal
        .attach_or_create(&owner, &state.config_dir)
        .await
    {
        Ok(session) => session,
        Err(AttachError::Busy { owner }) => {
            let (mut sink, _stream) = socket.split();
            let msg = ServerMessage::Error {
                m: format!("terminal busy; owned by {owner}"),
            };
            let _ = sink
                .send(Message::Text(msg.to_text().unwrap_or_default()))
                .await;
            return Err("terminal busy".into());
        }
        Err(AttachError::Spawn(message)) => {
            let (mut sink, _stream) = socket.split();
            let msg = ServerMessage::Error { m: message.clone() };
            let _ = sink
                .send(Message::Text(msg.to_text().unwrap_or_default()))
                .await;
            return Err(message);
        }
    };

    session.cancel_idle_timer();
    let client_id = next_client_id();
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replay_tx, replay_rx) = oneshot::channel();
    session
        .request(SessionCommand::Attach {
            client_id,
            output_tx,
            replay_tx,
        })
        .map_err(|_| "session closed".to_string())?;

    let (mut sink, mut stream) = socket.split();
    let replay = replay_rx.await.unwrap_or_default();
    let begin = ServerMessage::ReplayBegin
        .to_text()
        .map_err(|error| error.to_string())?;
    sink.send(Message::Text(begin))
        .await
        .map_err(|error| error.to_string())?;
    for chunk in replay {
        let text = ServerMessage::output_bytes(&chunk)
            .to_text()
            .map_err(|error| error.to_string())?;
        sink.send(Message::Text(text))
            .await
            .map_err(|error| error.to_string())?;
    }
    let end = ServerMessage::ReplayEnd
        .to_text()
        .map_err(|error| error.to_string())?;
    sink.send(Message::Text(end))
        .await
        .map_err(|error| error.to_string())?;

    loop {
        tokio::select! {
            inbound = stream.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        match decode_client_message(&text) {
                            Ok(ClientMessage::Input { d }) => {
                                let bytes = decode_input_payload(&d)?;
                                let _ = session.request(SessionCommand::Input(bytes));
                            }
                            Ok(ClientMessage::Resize { c, r }) => {
                                let _ = session.request(SessionCommand::Resize { cols: c, rows: r });
                            }
                            Ok(ClientMessage::Ping) => {
                                let text = ServerMessage::Pong
                                    .to_text()
                                    .map_err(|error| error.to_string())?;
                                sink.send(Message::Text(text))
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                            Err(error) => {
                                let text = ServerMessage::Error { m: error }
                                    .to_text()
                                    .map_err(|err| err.to_string())?;
                                sink.send(Message::Text(text))
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        sink.send(Message::Pong(payload))
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.to_string()),
                }
            }
            outbound = output_rx.recv() => {
                match outbound {
                    Some(chunk) => {
                        let text = ServerMessage::output_bytes(&chunk)
                            .to_text()
                            .map_err(|error| error.to_string())?;
                        sink.send(Message::Text(text))
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    None => {
                        let text = ServerMessage::Exit { c: None }
                            .to_text()
                            .map_err(|error| error.to_string())?;
                        let _ = sink.send(Message::Text(text)).await;
                        break;
                    }
                }
            }
        }

        if session.is_dead() {
            let text = ServerMessage::Exit { c: None }
                .to_text()
                .map_err(|error| error.to_string())?;
            let _ = sink.send(Message::Text(text)).await;
            break;
        }
    }

    let _ = session.request(SessionCommand::Detach { client_id });
    Ok(())
}
