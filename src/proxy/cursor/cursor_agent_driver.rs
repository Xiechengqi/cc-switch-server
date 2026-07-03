use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use async_stream::stream;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::accounts::{Account, AccountStore};
use crate::core::failover::ProviderOutcome;
use crate::core::provider::ProviderType;
use crate::core::providers::StoredProvider;
use crate::core::usage::{TokenUsage, UsageLogContext, UsageModelMetadata};
use crate::proxy::adapters::AdapterRequest;
use crate::state::{ServerState, ShareInFlightGuard};

use super::super::forwarder::{record_provider_outcome, record_share_invocation_result};
use super::super::router::ProxyRoute;
use super::super::usage::{log_usage, update_stream_usage};
use super::super::{setting, ProxyError};
use super::cursor_agent_proto::{
    decode_agent_server_message, decode_exec_server_event, decode_kv_server_event,
    encode_agent_run_request, encode_exec_background_shell_rejected, encode_exec_delete_rejected,
    encode_exec_diagnostics_result, encode_exec_fetch_error, encode_exec_grep_error,
    encode_exec_ls_rejected, encode_exec_mcp_error, encode_exec_mcp_result,
    encode_exec_read_rejected, encode_exec_shell_rejected, encode_exec_write_rejected,
    encode_exec_write_shell_stdin_error, encode_kv_get_blob_result, encode_kv_set_blob_result,
    encode_rich_request_context_response, wrap_connect_frame, AgentRunInput, ConnectFrame,
    ExecServerEvent, InteractionDelta, KvServerEvent,
};
use super::cursor_event_emitter::{
    AgentEvent, AgentSseWriter, CapturedToolCall, ComposerMarkerFilter, MarkerEvent,
};
use super::cursor_h2_client::{CursorH2Stream, DEFAULT_AGENTSERVICE_BASE_URL};
use super::cursor_identity::{
    cursor_account_for_api_key, cursor_account_from_managed_account, cursor_agentservice_headers,
    CursorAccountData,
};
use super::cursor_image::load_images;
use super::cursor_request_builder::{
    build_plan, estimate_input_tokens, validate_tool_result_context, AgentRunPlan,
};
use super::cursor_session::{CursorSession, PendingToolCall, SessionState};
use super::cursor_tool_bridge::{
    bridge_builtin_tool, bridge_grep_tool, bridge_ls_or_glob_tool, bridge_mcp_exec_tool,
    bridge_read_lints_tool, bridge_read_tool, bridge_write_or_edit_tool,
    resolve_shell_mcp_tool_name, BuiltinBridgeKind,
};
use super::cursor_tool_resolver::resolve_tool_call;

const DEFAULT_CURSOR_BACKEND_BASE_URL: &str = "https://api2.cursor.sh";
const EXCHANGE_USER_API_KEY_PATH: &str = "/auth/exchange_user_api_key";

pub struct AgentServiceForwardOptions {
    pub state: ServerState,
    pub route: ProxyRoute,
    pub stored: StoredProvider,
    pub adapter_request: AdapterRequest,
    pub request_context: UsageLogContext,
    pub share_invocation_guard: Option<ShareInFlightGuard>,
}

struct CursorCredential {
    account: CursorAccountData,
    access_token: String,
}

enum ExecHandling {
    Continue,
    ToolCall(CapturedToolCall),
}

enum DriveOutcome {
    Completed(Bytes, TokenUsage),
    Parked(Bytes, TokenUsage),
}

#[derive(Debug, Default)]
struct ExecDedup {
    seen: HashSet<String>,
}

impl ExecDedup {
    fn track(&mut self, event: &ExecServerEvent) -> bool {
        self.seen.insert(event.dedup_key())
    }
}

#[derive(Debug, Deserialize)]
struct CursorApiKeyExchangeResponse {
    #[serde(default, rename = "accessToken", alias = "access_token")]
    access_token: Option<String>,
}

pub async fn forward_agentservice(
    options: AgentServiceForwardOptions,
) -> Result<Response, ProxyError> {
    let AgentServiceForwardOptions {
        state,
        route,
        stored,
        adapter_request,
        request_context,
        share_invocation_guard,
    } = options;
    let started = Instant::now();
    let Some((inbound_protocol, response_format, _protocol_label)) =
        super::protocol_for_route(route)
    else {
        return Err(ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "Cursor AgentService driver does not support this route yet".to_string(),
        });
    };
    let body_value = serde_json::from_slice::<Value>(&adapter_request.body).map_err(|error| {
        ProxyError::bad_request(format!("invalid cursor AgentService request JSON: {error}"))
    })?;
    let plan = build_plan(inbound_protocol, &body_value);
    validate_tool_result_context(&plan).map_err(|message| {
        ProxyError::bad_request(format!("invalid cursor tool result context: {message}"))
    })?;

    let session_key = resolve_session_key(&state, &plan).await?;
    let response_model = response_model(&adapter_request, &plan.model_id);
    let input_tokens = estimate_input_tokens(&plan.user_text);
    let session_entry = acquire_or_open_session(&state, &stored, &plan, &session_key).await?;
    let status = session_status(&session_entry).await?;
    if !status.is_success() {
        record_provider_outcome(
            &state,
            &stored,
            ProviderOutcome::from_status(status.as_u16()),
        )
        .await;
        state
            .cursor_sessions
            .release(session_entry.clone(), SessionState::Closed)
            .await;
        return Err(ProxyError {
            status: StatusCode::BAD_GATEWAY,
            message: format!("Cursor AgentService returned HTTP {}", status.as_u16()),
        });
    }

    let model = usage_model_metadata(&adapter_request);
    if adapter_request.stream_requested {
        return Ok(stream_response(
            state,
            stored,
            session_entry,
            session_key,
            response_format,
            response_model,
            input_tokens,
            request_context,
            started,
            model,
            share_invocation_guard,
        )
        .await);
    }

    let drive = drive_non_stream(
        &state,
        session_entry.clone(),
        &session_key,
        response_format,
        response_model,
        input_tokens,
    )
    .await;
    match drive {
        Ok(outcome) => {
            let (body, usage, final_state) = match outcome {
                DriveOutcome::Completed(body, usage) => (body, usage, SessionState::Closed),
                DriveOutcome::Parked(body, usage) => {
                    (body, usage, SessionState::AwaitingToolResult)
                }
            };
            state
                .cursor_sessions
                .release(session_entry.clone(), final_state)
                .await;
            let status_code = StatusCode::OK.as_u16();
            log_usage(
                &state,
                &stored,
                status_code,
                started.elapsed().as_millis(),
                model,
                usage,
                UsageLogContext {
                    is_streaming: false,
                    ..request_context.clone()
                },
            )
            .await;
            record_share_invocation_result(&state, request_context.share_id.as_deref(), usage)
                .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code))
                .await;
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = StatusCode::OK;
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            Ok(response)
        }
        Err(error) => {
            state
                .cursor_sessions
                .release(session_entry.clone(), SessionState::Closed)
                .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_response(
    state: ServerState,
    stored: StoredProvider,
    session_entry: Arc<tokio::sync::Mutex<CursorSession>>,
    session_key: String,
    response_format: super::cursor_protocol::CursorResponseFormat,
    response_model: String,
    input_tokens: u32,
    request_context: UsageLogContext,
    started: Instant,
    model: UsageModelMetadata,
    share_invocation_guard: Option<ShareInFlightGuard>,
) -> Response {
    let mut writer = AgentSseWriter::new(response_model, response_format, input_tokens);
    state
        .cursor_sessions
        .bind_response_id(writer.message_id(), &session_key)
        .await;
    let request_id = log_usage(
        &state,
        &stored,
        StatusCode::OK.as_u16(),
        started.elapsed().as_millis(),
        model,
        TokenUsage::default(),
        UsageLogContext {
            is_streaming: true,
            stream_status: Some("pending".to_string()),
            ..request_context.clone()
        },
    )
    .await;
    let share_id = request_context.share_id.clone();
    let first_token_ms_shared = Arc::new(AtomicU64::new(0));
    let interrupted_guard = CursorStreamInterruptGuard {
        armed: Arc::new(AtomicBool::new(true)),
        state: state.clone(),
        stored: stored.clone(),
        request_id: request_id.clone(),
        status_code: StatusCode::OK.as_u16(),
        share_id: share_id.clone(),
        started,
        first_token_ms: first_token_ms_shared.clone(),
        session_entry: Some(session_entry.clone()),
    };
    let stream = stream! {
        let interrupted_guard = interrupted_guard;
        let _share_invocation_guard = share_invocation_guard;
        let mut filter = ComposerMarkerFilter::default();
        let mut exec_dedup = ExecDedup::default();
        let mut first_token_ms = None;
        let mut final_status = StatusCode::OK.as_u16();
        let mut final_stream_status = "completed";
        let mut final_session_state = SessionState::Closed;

        for event in writer.start_events() {
            yield Ok::<_, std::io::Error>(Bytes::from(event));
        }

        loop {
            let frame = match next_session_frame(&session_entry).await {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(error) => {
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            };
            if let Err(error) = handle_kv_event(&session_entry, decode_kv_server_event(&frame.payload)).await {
                final_status = error.status.as_u16();
                final_stream_status = "failed";
                for event in writer.error_events(&error.message) {
                    yield Ok::<_, std::io::Error>(Bytes::from(event));
                }
                break;
            }
            match handle_exec_event(
                &state,
                &session_entry,
                &mut exec_dedup,
                decode_exec_server_event(&frame.payload),
            )
            .await
            {
                Ok(ExecHandling::Continue) => {}
                Ok(ExecHandling::ToolCall(tool_call)) => {
                    let events = writer.event(&AgentEvent::ToolCall(tool_call));
                    if first_token_ms.is_none() && !events.is_empty() {
                        let elapsed = started.elapsed().as_millis();
                        first_token_ms = Some(elapsed);
                        first_token_ms_shared.store(elapsed.min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
                        update_stream_usage(
                            &state,
                            &stored,
                            &request_id,
                            StatusCode::OK.as_u16(),
                            elapsed,
                            first_token_ms,
                            TokenUsage::default(),
                            Some("streaming"),
                        )
                        .await;
                    }
                    for event in events {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    final_session_state = SessionState::AwaitingToolResult;
                    break;
                }
                Err(error) => {
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            }
            let mut ended = false;
            for delta in decode_agent_server_message(&frame.payload) {
                let events = match cursor_delta_events(delta, &mut writer, &mut filter) {
                    Ok(CursorDeltaOutcome::Events(events)) => events,
                    Ok(CursorDeltaOutcome::TurnEnded(events)) => {
                        ended = true;
                        events
                    }
                    Err(error) => {
                        final_status = error.status.as_u16();
                        final_stream_status = "failed";
                        writer.error_events(&error.message)
                    }
                };
                if first_token_ms.is_none() && !events.is_empty() {
                    let elapsed = started.elapsed().as_millis();
                    first_token_ms = Some(elapsed);
                    first_token_ms_shared.store(elapsed.min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
                    update_stream_usage(
                        &state,
                        &stored,
                        &request_id,
                        StatusCode::OK.as_u16(),
                        elapsed,
                        first_token_ms,
                        TokenUsage::default(),
                        Some("streaming"),
                    )
                    .await;
                }
                for event in events {
                    yield Ok::<_, std::io::Error>(Bytes::from(event));
                }
                if final_stream_status == "failed" {
                    ended = true;
                }
            }
            if ended {
                break;
            }
        }

        for event in writer.done_events() {
            yield Ok::<_, std::io::Error>(Bytes::from(event));
        }
        let usage = writer_usage(&writer);
        update_stream_usage(
            &state,
            &stored,
            &request_id,
            final_status,
            started.elapsed().as_millis(),
            first_token_ms,
            usage,
            Some(final_stream_status),
        )
        .await;
        record_share_invocation_result(&state, share_id.as_deref(), usage).await;
        let outcome = if final_stream_status == "failed" {
            ProviderOutcome::NetworkFailure
        } else {
            ProviderOutcome::from_status(final_status)
        };
        record_provider_outcome(&state, &stored, outcome).await;
        if final_stream_status == "failed" {
            final_session_state = SessionState::Closed;
        }
        state
            .cursor_sessions
            .release(session_entry.clone(), final_session_state)
            .await;
        interrupted_guard.disarm();
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response
}

async fn drive_non_stream(
    state: &ServerState,
    session_entry: Arc<tokio::sync::Mutex<CursorSession>>,
    session_key: &str,
    response_format: super::cursor_protocol::CursorResponseFormat,
    response_model: String,
    input_tokens: u32,
) -> Result<DriveOutcome, ProxyError> {
    let mut writer = AgentSseWriter::new(response_model, response_format, input_tokens);
    state
        .cursor_sessions
        .bind_response_id(writer.message_id(), session_key)
        .await;
    let _ = writer.start_events();
    let mut filter = ComposerMarkerFilter::default();
    let mut exec_dedup = ExecDedup::default();
    loop {
        let Some(frame) = next_session_frame(&session_entry).await? else {
            break;
        };
        handle_kv_event(&session_entry, decode_kv_server_event(&frame.payload)).await?;
        match handle_exec_event(
            state,
            &session_entry,
            &mut exec_dedup,
            decode_exec_server_event(&frame.payload),
        )
        .await?
        {
            ExecHandling::Continue => {}
            ExecHandling::ToolCall(tool_call) => {
                let _ = writer.event(&AgentEvent::ToolCall(tool_call));
                let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
                    ProxyError::bad_request(format!(
                        "Cursor AgentService JSON response encode failed: {error}"
                    ))
                })?;
                return Ok(DriveOutcome::Parked(
                    Bytes::from(body),
                    writer_usage(&writer),
                ));
            }
        }
        for delta in decode_agent_server_message(&frame.payload) {
            match cursor_delta_events(delta, &mut writer, &mut filter)? {
                CursorDeltaOutcome::Events(_) => {}
                CursorDeltaOutcome::TurnEnded(_) => {
                    let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
                        ProxyError::bad_request(format!(
                            "Cursor AgentService JSON response encode failed: {error}"
                        ))
                    })?;
                    return Ok(DriveOutcome::Completed(
                        Bytes::from(body),
                        writer_usage(&writer),
                    ));
                }
            }
        }
    }
    let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
        ProxyError::bad_request(format!(
            "Cursor AgentService JSON response encode failed: {error}"
        ))
    })?;
    Ok(DriveOutcome::Completed(
        Bytes::from(body),
        writer_usage(&writer),
    ))
}

enum CursorDeltaOutcome {
    Events(Vec<String>),
    TurnEnded(Vec<String>),
}

fn cursor_delta_events(
    delta: InteractionDelta,
    writer: &mut AgentSseWriter,
    filter: &mut ComposerMarkerFilter,
) -> Result<CursorDeltaOutcome, ProxyError> {
    let mut out = Vec::new();
    match delta {
        InteractionDelta::Text(text) => {
            for event in filter.push(&text) {
                match event {
                    MarkerEvent::Text(text) => out.extend(writer.event(&AgentEvent::Text(text))),
                    MarkerEvent::ToolCall(tool_call) => {
                        return Err(ProxyError {
                            status: StatusCode::NOT_IMPLEMENTED,
                            message: format!(
                                "Cursor AgentService emitted marker-only tool call `{}` without Exec/MCP metadata; session resume requires an AgentService MCP event",
                                tool_call.name
                            ),
                        });
                    }
                }
            }
            Ok(CursorDeltaOutcome::Events(out))
        }
        InteractionDelta::Thinking(text) => {
            out.extend(writer.event(&AgentEvent::Thinking(text)));
            Ok(CursorDeltaOutcome::Events(out))
        }
        InteractionDelta::ThinkingComplete => {
            out.extend(writer.event(&AgentEvent::ThinkingComplete));
            Ok(CursorDeltaOutcome::Events(out))
        }
        InteractionDelta::TokenDelta(tokens) => {
            let output = tokens.min(u64::from(u32::MAX)) as u32;
            out.extend(writer.event(&AgentEvent::Usage { input: 0, output }));
            Ok(CursorDeltaOutcome::Events(out))
        }
        InteractionDelta::TurnEnded => {
            for event in filter.flush() {
                match event {
                    MarkerEvent::Text(text) => out.extend(writer.event(&AgentEvent::Text(text))),
                    MarkerEvent::ToolCall(tool_call) => {
                        return Err(ProxyError {
                            status: StatusCode::NOT_IMPLEMENTED,
                            message: format!(
                                "Cursor AgentService emitted marker-only tool call `{}` without Exec/MCP metadata; session resume requires an AgentService MCP event",
                                tool_call.name
                            ),
                        });
                    }
                }
            }
            Ok(CursorDeltaOutcome::TurnEnded(out))
        }
        InteractionDelta::Heartbeat
        | InteractionDelta::ToolCallStarted
        | InteractionDelta::ToolCallCompleted
        | InteractionDelta::KvServerMessage
        | InteractionDelta::Unknown(_) => Ok(CursorDeltaOutcome::Events(out)),
    }
}

async fn handle_kv_event(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    event: Option<KvServerEvent>,
) -> Result<(), ProxyError> {
    match event {
        Some(KvServerEvent::GetBlob {
            kv_id,
            blob_id,
            request_metadata,
            ..
        }) => {
            let key = hex_lower(&blob_id);
            let blob = {
                let session = session_entry.lock().await;
                session
                    .blob_store
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(Bytes::new)
            };
            send_session_frame(
                session_entry,
                encode_kv_get_blob_result(kv_id, &blob, request_metadata.as_deref()),
            )
            .await
        }
        Some(KvServerEvent::SetBlob {
            kv_id,
            blob_id,
            blob_data,
            request_metadata,
            ..
        }) => {
            let key = hex_lower(&blob_id);
            {
                let mut session = session_entry.lock().await;
                session.blob_store.insert(key, blob_data);
            }
            send_session_frame(
                session_entry,
                encode_kv_set_blob_result(kv_id, request_metadata.as_deref()),
            )
            .await
        }
        None => Ok(()),
    }
}

async fn handle_exec_event(
    state: &ServerState,
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    exec_dedup: &mut ExecDedup,
    event: Option<ExecServerEvent>,
) -> Result<ExecHandling, ProxyError> {
    let Some(event) = event else {
        return Ok(ExecHandling::Continue);
    };
    if !exec_dedup.track(&event) {
        return Ok(ExecHandling::Continue);
    }
    let reason = "cc-switch-server Cursor AgentService driver does not execute built-in tools";
    let frame = match event {
        ExecServerEvent::RequestContext {
            exec_msg_id,
            exec_id,
        } => {
            let working_directory = {
                let session = session_entry.lock().await;
                session.working_directory.clone()
            };
            encode_rich_request_context_response(exec_msg_id, &exec_id, &working_directory)
        }
        ExecServerEvent::Read {
            exec_msg_id,
            exec_id,
            path,
            tool_call_id,
            offset,
            limit,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) = bridge_read_tool(&declared, &path, offset, limit) {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    &tool_call_id,
                    args,
                )
                .await;
            }
            encode_exec_read_rejected(exec_msg_id, &exec_id, &path, reason)
        }
        ExecServerEvent::Write {
            exec_msg_id,
            exec_id,
            path,
            file_text,
            stream_content,
            tool_call_id,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) =
                bridge_write_or_edit_tool(&declared, &path, &file_text, &stream_content)
            {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    &tool_call_id,
                    args,
                )
                .await;
            }
            encode_exec_write_rejected(exec_msg_id, &exec_id, &path, reason)
        }
        ExecServerEvent::Delete {
            exec_msg_id,
            exec_id,
            path,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) =
                bridge_builtin_tool(BuiltinBridgeKind::Delete, &declared, &path, "", "")
            {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    args,
                )
                .await;
            }
            encode_exec_delete_rejected(exec_msg_id, &exec_id, &path, reason)
        }
        ExecServerEvent::Ls {
            exec_msg_id,
            exec_id,
            path,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) = bridge_ls_or_glob_tool(&declared, &path) {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    args,
                )
                .await;
            }
            encode_exec_ls_rejected(exec_msg_id, &exec_id, &path, reason)
        }
        ExecServerEvent::Grep {
            exec_msg_id,
            exec_id,
            pattern,
            path,
            glob,
            output_mode,
            case_insensitive,
            head_limit,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) = bridge_grep_tool(
                &declared,
                &pattern,
                &path,
                &glob,
                &output_mode,
                case_insensitive,
                head_limit,
            ) {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    args,
                )
                .await;
            }
            encode_exec_grep_error(exec_msg_id, &exec_id, reason)
        }
        ExecServerEvent::Diagnostics {
            exec_msg_id,
            exec_id,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) = bridge_read_lints_tool(&declared, &[]) {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    args,
                )
                .await;
            }
            encode_exec_diagnostics_result(exec_msg_id, &exec_id)
        }
        ExecServerEvent::Shell {
            exec_msg_id,
            exec_id,
            command,
            working_dir,
        }
        | ExecServerEvent::ShellStream {
            exec_msg_id,
            exec_id,
            command,
            working_dir,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some(name) = resolve_shell_mcp_tool_name(&declared) {
                let mut args_map = serde_json::Map::new();
                args_map.insert("command".to_string(), Value::String(command.clone()));
                if !working_dir.is_empty() {
                    args_map.insert("workdir".to_string(), Value::String(working_dir.clone()));
                }
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    Value::Object(args_map),
                )
                .await;
            }
            encode_exec_shell_rejected(exec_msg_id, &exec_id, &command, &working_dir, reason)
        }
        ExecServerEvent::BackgroundShell {
            exec_msg_id,
            exec_id,
            command,
            working_dir,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some(name) = resolve_shell_mcp_tool_name(&declared) {
                let mut args_map = serde_json::Map::new();
                args_map.insert("command".to_string(), Value::String(command.clone()));
                if !working_dir.is_empty() {
                    args_map.insert("workdir".to_string(), Value::String(working_dir.clone()));
                }
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    Value::Object(args_map),
                )
                .await;
            }
            encode_exec_background_shell_rejected(
                exec_msg_id,
                &exec_id,
                &command,
                &working_dir,
                reason,
            )
        }
        ExecServerEvent::Fetch {
            exec_msg_id,
            exec_id,
            url,
        } => {
            let declared = declared_tool_names(session_entry).await;
            if let Some((name, args)) =
                bridge_builtin_tool(BuiltinBridgeKind::Fetch, &declared, "", &url, "")
            {
                return surface_mcp_tool_call(
                    state,
                    session_entry,
                    exec_msg_id,
                    &exec_id,
                    &name,
                    "",
                    args,
                )
                .await;
            }
            encode_exec_fetch_error(exec_msg_id, &exec_id, &url, reason)
        }
        ExecServerEvent::WriteShellStdin {
            exec_msg_id,
            exec_id,
        } => encode_exec_write_shell_stdin_error(exec_msg_id, &exec_id, reason),
        ExecServerEvent::Mcp {
            exec_msg_id,
            exec_id,
            tool_name,
            tool_call_id,
            args,
        } => {
            let declared = declared_tool_names(session_entry).await;
            let (tool_name, args) = match bridge_mcp_exec_tool(&declared, &tool_name, args.clone())
            {
                Some(remapped) => remapped,
                None => (tool_name, args),
            };
            return surface_mcp_tool_call(
                state,
                session_entry,
                exec_msg_id,
                &exec_id,
                &tool_name,
                &tool_call_id,
                args,
            )
            .await;
        }
    };
    send_session_frame(session_entry, frame).await?;
    Ok(ExecHandling::Continue)
}

async fn declared_tool_names(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
) -> Vec<String> {
    let session = session_entry.lock().await;
    session.declared_tool_names.clone()
}

async fn acquire_or_open_session(
    state: &ServerState,
    stored: &StoredProvider,
    plan: &AgentRunPlan,
    session_key: &str,
) -> Result<Arc<tokio::sync::Mutex<CursorSession>>, ProxyError> {
    if !plan.tool_results.is_empty() {
        let entry = state
            .cursor_sessions
            .acquire(session_key)
            .await
            .ok_or_else(|| {
                ProxyError::conflict(format!(
                    "Cursor AgentService session `{session_key}` is not parked or has expired"
                ))
            })?;
        resume_tool_results(&entry, &plan.tool_results).await?;
        return Ok(entry);
    }

    let credential = resolve_cursor_credential(state, stored).await?;
    open_agent_stream(state, &credential, stored, plan, session_key).await
}

async fn resume_tool_results(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    tool_results: &[super::cursor_request_builder::ToolResultBlock],
) -> Result<(), ProxyError> {
    let mut session = session_entry.lock().await;
    let has_match = tool_results.iter().any(|result| {
        session
            .pending_tool_calls
            .contains_key(&result.tool_call_id)
    });
    if !has_match {
        return Err(ProxyError::conflict(
            "Cursor AgentService tool_result did not match any parked tool call",
        ));
    }

    for result in tool_results {
        let Some(pending) = session.pending_tool_calls.remove(&result.tool_call_id) else {
            continue;
        };
        let frame = encode_exec_mcp_result(
            pending.exec_msg_id,
            &pending.exec_id,
            &result.content,
            result.is_error,
        );
        let stream = session.stream.as_ref().ok_or_else(|| {
            ProxyError::conflict("Cursor AgentService parked session has no live h2 stream")
        })?;
        stream.send_frame(frame)?;
    }
    Ok(())
}

async fn session_status(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
) -> Result<StatusCode, ProxyError> {
    let session = session_entry.lock().await;
    let stream = session
        .stream
        .as_ref()
        .ok_or_else(|| ProxyError::conflict("Cursor AgentService session has no live h2 stream"))?;
    Ok(stream.status())
}

async fn next_session_frame(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
) -> Result<Option<ConnectFrame>, ProxyError> {
    let mut session = session_entry.lock().await;
    let stream = session
        .stream
        .as_mut()
        .ok_or_else(|| ProxyError::conflict("Cursor AgentService session has no live h2 stream"))?;
    stream.next_frame().await
}

async fn send_session_frame(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    frame: Bytes,
) -> Result<(), ProxyError> {
    let session = session_entry.lock().await;
    let stream = session
        .stream
        .as_ref()
        .ok_or_else(|| ProxyError::conflict("Cursor AgentService session has no live h2 stream"))?;
    stream.send_frame(frame)
}

async fn surface_mcp_tool_call(
    state: &ServerState,
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    exec_msg_id: u64,
    exec_id: &str,
    tool_name: &str,
    tool_call_id: &str,
    args: Value,
) -> Result<ExecHandling, ProxyError> {
    let (declared_tools, conversation_id) = {
        let session = session_entry.lock().await;
        (
            session.declared_tools.clone(),
            session.conversation_id.clone(),
        )
    };
    let resolved = match resolve_tool_call(&declared_tools, tool_name, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            let message = format!("{}: {}", error.original_name, error.reason);
            send_session_frame(
                session_entry,
                encode_exec_mcp_error(exec_msg_id, exec_id, &message),
            )
            .await?;
            return Ok(ExecHandling::Continue);
        }
    };

    let client_call_id = if tool_call_id.trim().is_empty() {
        random_call_id()
    } else {
        tool_call_id.to_string()
    };
    {
        let mut session = session_entry.lock().await;
        session.pending_tool_calls.insert(
            client_call_id.clone(),
            PendingToolCall {
                exec_msg_id,
                exec_id: exec_id.to_string(),
                tool_name: resolved.name.clone(),
            },
        );
    }
    state
        .cursor_sessions
        .bind_tool_call_id(&client_call_id, &conversation_id)
        .await;

    let arguments_json = serde_json::to_string(&resolved.args).unwrap_or_else(|_| "{}".to_string());
    Ok(ExecHandling::ToolCall(CapturedToolCall {
        id: client_call_id,
        name: resolved.name,
        arguments_json,
    }))
}

async fn resolve_session_key(
    state: &ServerState,
    plan: &AgentRunPlan,
) -> Result<String, ProxyError> {
    if !plan.tool_results.is_empty() {
        for result in &plan.tool_results {
            if let Some(conversation_id) = state
                .cursor_sessions
                .resolve_tool_call_id(&result.tool_call_id)
                .await
            {
                return Ok(conversation_id);
            }
        }
        if let Some(previous_response_id) = plan.previous_response_id.as_deref() {
            if let Some(conversation_id) = state
                .cursor_sessions
                .resolve_response_id(previous_response_id)
                .await
            {
                return Ok(conversation_id);
            }
        }
        return Err(ProxyError::conflict(
            "Cursor AgentService tool_result has no matching parked session",
        ));
    }

    if let Some(previous_response_id) = plan.previous_response_id.as_deref() {
        if let Some(conversation_id) = state
            .cursor_sessions
            .resolve_response_id(previous_response_id)
            .await
        {
            return Ok(conversation_id);
        }
        if !previous_response_id.trim().is_empty() {
            return Ok(previous_response_id.to_string());
        }
    }

    Ok(random_uuid_like())
}

async fn open_agent_stream(
    state: &ServerState,
    credential: &CursorCredential,
    stored: &StoredProvider,
    plan: &super::cursor_request_builder::AgentRunPlan,
    session_key: &str,
) -> Result<Arc<tokio::sync::Mutex<CursorSession>>, ProxyError> {
    let images = load_images(plan.images.clone()).await?;
    let mut blob_store = HashMap::new();
    let mut input = AgentRunInput {
        model_id: &plan.model_id,
        user_text: &plan.user_text,
        conversation_id: Some(session_key),
        message_id: None,
        tools: plan.tools.clone(),
        system_prompt: plan.system_prompt.as_deref(),
        blob_store: Some(&mut blob_store),
        images,
    };
    let body = encode_agent_run_request(&mut input);
    let stream = CursorH2Stream::open(
        &cursor_agentservice_base_url(stored),
        cursor_agentservice_headers(&credential.account, &credential.access_token),
        wrap_connect_frame(&body),
    )
    .await?;
    Ok(state
        .cursor_sessions
        .open(
            session_key.to_string(),
            stream,
            blob_store,
            plan.tools.clone(),
            plan.working_directory.clone(),
        )
        .await)
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn random_call_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("call_{}", hex_lower(&Bytes::copy_from_slice(&bytes)))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

struct CursorStreamInterruptGuard {
    armed: Arc<AtomicBool>,
    state: ServerState,
    stored: StoredProvider,
    request_id: String,
    status_code: u16,
    share_id: Option<String>,
    started: Instant,
    first_token_ms: Arc<AtomicU64>,
    session_entry: Option<Arc<tokio::sync::Mutex<CursorSession>>>,
}

impl CursorStreamInterruptGuard {
    fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }

    fn first_token_ms(&self) -> Option<u128> {
        match self.first_token_ms.load(Ordering::Relaxed) {
            0 => None,
            value => Some(u128::from(value)),
        }
    }
}

impl Drop for CursorStreamInterruptGuard {
    fn drop(&mut self) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        let state = self.state.clone();
        let stored = self.stored.clone();
        let request_id = self.request_id.clone();
        let status_code = self.status_code;
        let share_id = self.share_id.clone();
        let duration_ms = self.started.elapsed().as_millis();
        let first_token_ms = self.first_token_ms();
        let session_entry = self.session_entry.take();
        tokio::spawn(async move {
            update_stream_usage(
                &state,
                &stored,
                &request_id,
                status_code,
                duration_ms,
                first_token_ms,
                TokenUsage::default(),
                Some("interrupted"),
            )
            .await;
            record_share_invocation_result(&state, share_id.as_deref(), TokenUsage::default())
                .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            if let Some(entry) = session_entry {
                state
                    .cursor_sessions
                    .release(entry, SessionState::Closed)
                    .await;
            }
        });
    }
}

async fn resolve_cursor_credential(
    state: &ServerState,
    stored: &StoredProvider,
) -> Result<CursorCredential, ProxyError> {
    match stored.provider_type {
        ProviderType::CursorOAuth => {
            let accounts = state.accounts.read().await;
            let account_id = stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref());
            let account = accounts
                .find_for_provider(ProviderType::CursorOAuth, account_id)
                .ok_or_else(|| {
                    ProxyError::bad_request("Cursor OAuth managed account is required")
                })?;
            let access_token = account
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request("Cursor OAuth managed account access token is required")
                })?
                .to_string();
            Ok(CursorCredential {
                account: cursor_account_from_managed_account(account),
                access_token,
            })
        }
        ProviderType::CursorApiKey => {
            let accounts = state.accounts.read().await;
            let api_key = cursor_api_key(stored, &accounts)?;
            drop(accounts);
            let access_token = exchange_cursor_api_key(state, stored, &api_key).await?;
            Ok(CursorCredential {
                account: cursor_account_for_api_key(&api_key),
                access_token,
            })
        }
        _ => Err(ProxyError::bad_request(
            "Cursor AgentService driver requires a Cursor provider",
        )),
    }
}

fn cursor_api_key(stored: &StoredProvider, accounts: &AccountStore) -> Result<String, ProxyError> {
    setting(
        &stored.provider,
        &[
            "CURSOR_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "API_KEY",
        ],
    )
    .or_else(|| {
        let account_id = stored
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.auth_binding.as_ref())
            .and_then(|binding| binding.account_id.as_deref());
        accounts
            .find_for_provider(ProviderType::CursorApiKey, account_id)
            .and_then(account_api_key)
    })
    .ok_or_else(|| ProxyError::bad_request("Cursor API key is required"))
}

fn account_api_key(account: &Account) -> Option<String> {
    account
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn exchange_cursor_api_key(
    state: &ServerState,
    stored: &StoredProvider,
    api_key: &str,
) -> Result<String, ProxyError> {
    let url = format!(
        "{}{}",
        cursor_backend_base_url(stored),
        EXCHANGE_USER_API_KEY_PATH
    );
    let response = state
        .http_client()
        .await
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&json!({}))
        .send()
        .await
        .map_err(ProxyError::bad_gateway)?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ProxyError {
            status,
            message: format!("Cursor API key exchange failed: {body}"),
        });
    }
    let parsed = response
        .json::<CursorApiKeyExchangeResponse>()
        .await
        .map_err(ProxyError::bad_gateway)?;
    parsed
        .access_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProxyError::bad_request("Cursor API key exchange response missing access token")
        })
}

fn cursor_backend_base_url(stored: &StoredProvider) -> String {
    setting(
        &stored.provider,
        &["CURSOR_BACKEND_BASE_URL", "CURSOR_API_BASE_URL"],
    )
    .or_else(|| std::env::var("CC_SWITCH_CURSOR_BACKEND_BASE_URL").ok())
    .or_else(|| std::env::var("CURSOR_BACKEND_BASE_URL").ok())
    .map(|value| value.trim().trim_end_matches('/').to_string())
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| DEFAULT_CURSOR_BACKEND_BASE_URL.to_string())
}

fn cursor_agentservice_base_url(stored: &StoredProvider) -> String {
    setting(
        &stored.provider,
        &[
            "CURSOR_AGENT_SERVICE_BASE_URL",
            "CURSOR_AGENTSERVICE_BASE_URL",
            "CURSOR_AGENT_BASE_URL",
        ],
    )
    .or_else(|| std::env::var("CC_SWITCH_CURSOR_AGENT_SERVICE_BASE_URL").ok())
    .or_else(|| std::env::var("CURSOR_AGENT_SERVICE_BASE_URL").ok())
    .map(|value| value.trim().trim_end_matches('/').to_string())
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| DEFAULT_AGENTSERVICE_BASE_URL.to_string())
}

fn response_model(request: &AdapterRequest, plan_model: &str) -> String {
    request
        .requested_model
        .as_deref()
        .or(request.model.as_deref())
        .or(request.actual_model.as_deref())
        .unwrap_or(plan_model)
        .to_string()
}

fn usage_model_metadata(request: &AdapterRequest) -> UsageModelMetadata {
    UsageModelMetadata {
        model: request.model.clone(),
        requested_model: request.requested_model.clone(),
        actual_model: request.actual_model.clone(),
        actual_model_source: request.actual_model_source.clone(),
        pricing_model: request.pricing_model.clone(),
    }
}

fn writer_usage(writer: &AgentSseWriter) -> TokenUsage {
    let input = u64::from(writer.input_tokens());
    let output = u64::from(writer.output_tokens());
    TokenUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        total_tokens: Some(input.saturating_add(output)),
        ..TokenUsage::default()
    }
}
