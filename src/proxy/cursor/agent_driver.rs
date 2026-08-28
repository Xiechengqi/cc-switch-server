#![allow(clippy::items_after_test_module)]

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use async_stream::stream;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use base64::Engine;
use bytes::Bytes;
use futures_util::StreamExt;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cursor_client_contract::DEFAULT_API_KEY_EXCHANGE_URL;
use crate::domain::health::ProviderRequestOutcome as ProviderOutcome;
use crate::domain::providers::model::ProviderType;
use crate::domain::providers::runtime::{
    authoritative_managed_account, managed_account_binding_with_generation,
};
use crate::domain::providers::store::StoredProvider;
use crate::domain::usage::store::{TokenUsage, UsageLogContext, UsageModelMetadata};
use crate::proxy::adapters::AdapterRequest;
use crate::state::{AccountInFlightGuard, ServerState, ShareInFlightGuard};

use super::super::forwarder::{
    managed_credential_accounts_snapshot, mark_managed_account_auth_cooldown_for_stored,
    record_provider_outcome, record_share_invocation_result,
};
use super::super::retry_policy::{AuthRecoveryDecision, AuthRecoveryState};
use super::super::router::ProxyRoute;
use super::super::usage::{log_usage, update_stream_usage};
use super::super::{setting, ProxyError};
use super::agent_endpoint::{
    default_cursor_server_config_url, resolve_cursor_agent_endpoint, CursorAgentEndpointRequest,
    CursorAgentEndpointScope,
};
use super::agent_proto::{
    decode_agent_server_message, decode_exec_server_event, decode_kv_server_event,
    encode_agent_run_request, encode_exec_background_shell_rejected, encode_exec_delete_rejected,
    encode_exec_diagnostics_result, encode_exec_fetch_error, encode_exec_grep_error,
    encode_exec_ls_rejected, encode_exec_mcp_error, encode_exec_mcp_result,
    encode_exec_read_rejected, encode_exec_shell_rejected, encode_exec_write_rejected,
    encode_exec_write_shell_stdin_error, encode_kv_get_blob_result, encode_kv_set_blob_result,
    encode_request_context_response, encode_rich_request_context_response, wrap_connect_frame,
    AgentRunInput, ConnectFrame, ExecServerEvent, InteractionDelta, KvServerEvent, ProtoError,
};
use super::credential_cache::CursorApiKeyCredentialScope;
use super::event_emitter::{
    AgentEvent, AgentSseWriter, CapturedToolCall, ComposerMarkerFilter, MarkerEvent,
};
use super::h2_client::{cursor_transport_diagnostic, CursorH2Stream, CursorH2Timeouts};
use super::identity::{
    cursor_account_for_api_key, cursor_account_from_managed_account, cursor_agentservice_headers,
    CursorAccountData,
};
use super::image::load_images;
use super::profile::CursorProtocolRail;
use super::request_builder::{
    estimate_agent_plan_input_tokens, prepare_response_compaction, prepend_response_context,
    preserve_current_continuation, retry_prompt_after_invalid_tool,
    retry_prompt_after_missing_tool, try_build_plan, validate_request_contract,
    validate_tool_choice_contract, validate_tool_result_context, AgentRunPlan, ExtractedToolChoice,
    InboundProtocol, ToolContinuationKind,
};
use super::response_state::{CursorResponseScope, CursorResponseScopeInput};
use super::session::{
    CursorSession, CursorSessionKey, CursorSessionManager, CursorSessionReference,
    CursorSessionScope, CursorSessionScopeInput, PendingToolCall, SessionState,
};
use super::tool_bridge::{
    bridge_builtin_tool, bridge_grep_tool, bridge_ls_or_glob_tool, bridge_mcp_exec_tool,
    bridge_read_lints_tool, bridge_read_tool, bridge_write_or_edit_tool,
    resolve_shell_mcp_tool_name, BuiltinBridgeKind,
};
use super::tool_resolver::resolve_tool_call;
use crate::domain::accounts::cursor_import::normalize_cursor_access_token;

const MAX_CURSOR_ERROR_BODY_BYTES: usize = 8 * 1024;
const CURSOR_AUTH_FAILURE_COOLDOWN_MS: i64 = 60_000;

pub struct AgentServiceForwardOptions {
    pub state: ServerState,
    pub route: ProxyRoute,
    pub stored: StoredProvider,
    pub adapter_request: AdapterRequest,
    pub request_context: UsageLogContext,
    pub account_in_flight_guard: Option<AccountInFlightGuard>,
    pub share_invocation_guard: Option<ShareInFlightGuard>,
    pub runtime_fingerprint: String,
    pub timeouts: CursorH2Timeouts,
}

enum CursorCredential {
    OAuthCli {
        account: CursorAccountData,
        access_token: String,
        endpoint_principal: String,
    },
    ApiKeySdk {
        account: CursorAccountData,
        access_token: String,
        endpoint_principal: String,
    },
}

impl CursorCredential {
    fn rail(&self) -> CursorProtocolRail {
        match self {
            Self::OAuthCli { .. } => CursorProtocolRail::OAuthCli,
            Self::ApiKeySdk { .. } => CursorProtocolRail::ApiKeySdk,
        }
    }

    fn account(&self) -> &CursorAccountData {
        match self {
            Self::OAuthCli { account, .. } | Self::ApiKeySdk { account, .. } => account,
        }
    }

    fn access_token(&self) -> &str {
        match self {
            Self::OAuthCli { access_token, .. } | Self::ApiKeySdk { access_token, .. } => {
                access_token
            }
        }
    }

    fn endpoint_principal(&self) -> &str {
        match self {
            Self::OAuthCli {
                endpoint_principal, ..
            }
            | Self::ApiKeySdk {
                endpoint_principal, ..
            } => endpoint_principal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorAgentServiceAuthAction {
    RefreshAndReplaySameBinding,
    UseResponse,
}

async fn cursor_agentservice_auth_action(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    rejected_access_token: Option<&str>,
    auth_recovery: &mut AuthRecoveryState,
    status: StatusCode,
    replay_allowed: bool,
) -> CursorAgentServiceAuthAction {
    match auth_recovery.decide(status, stored.provider_type, replay_allowed) {
        Some(AuthRecoveryDecision::RefreshAndReplaySameBinding) => {
            CursorAgentServiceAuthAction::RefreshAndReplaySameBinding
        }
        Some(AuthRecoveryDecision::ReturnUnauthorized) => {
            if auth_recovery.attempted() {
                mark_cursor_agentservice_auth_cooldown(
                    state,
                    stored,
                    runtime_fingerprint,
                    rejected_access_token,
                    "cursor_agentservice_unauthorized_after_refresh",
                )
                .await;
            }
            CursorAgentServiceAuthAction::UseResponse
        }
        None => CursorAgentServiceAuthAction::UseResponse,
    }
}

enum ExecHandling {
    Continue,
    ToolCall(CapturedToolCall),
}

enum DriveOutcome {
    Completed {
        body: Bytes,
        usage: TokenUsage,
        buffered_events: Vec<String>,
    },
    Parked {
        body: Bytes,
        usage: TokenUsage,
        buffered_events: Vec<String>,
        tool_name: String,
    },
}

#[derive(Clone)]
enum SemanticToolConstraint {
    Required,
    Named(String),
    LocalIntent,
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
    #[serde(default, rename = "expiresIn", alias = "expires_in")]
    expires_in: Option<i64>,
    #[serde(default, rename = "expiresAt", alias = "expires_at")]
    expires_at: Option<i64>,
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
        account_in_flight_guard,
        share_invocation_guard,
        runtime_fingerprint,
        timeouts,
    } = options;
    let mut request_context = request_context;
    request_context.usage_estimated = true;
    let started = Instant::now();
    let Some((inbound_protocol, response_format, protocol_label)) =
        super::protocol_for_route(route)
    else {
        return Err(ProxyError {
            status: StatusCode::NOT_IMPLEMENTED,
            message: "Cursor AgentService driver does not support this route yet".to_string(),
        });
    };
    let mut body_value =
        serde_json::from_slice::<Value>(&adapter_request.body).map_err(|error| {
            ProxyError::bad_request(format!("invalid cursor AgentService request JSON: {error}"))
        })?;
    let compact = route == ProxyRoute::CodexResponsesCompact;
    validate_request_contract(inbound_protocol, &body_value, compact)
        .map_err(ProxyError::bad_request)?;
    if compact {
        prepare_response_compaction(&mut body_value).map_err(ProxyError::bad_request)?;
    }
    let preliminary_plan =
        try_build_plan(inbound_protocol, &body_value).map_err(ProxyError::bad_request)?;
    let response_scope = if inbound_protocol == InboundProtocol::OpenAiResponses {
        Some(
            cursor_completed_response_scope(
                &state,
                &stored,
                &runtime_fingerprint,
                &request_context,
                &preliminary_plan.working_directory,
            )
            .await?,
        )
    } else {
        None
    };
    let mut completed_conversation_id = None;
    let mut response_context_prepended = false;
    if preliminary_plan.tool_results.is_empty() {
        if let (Some(scope), Some(previous_response_id)) = (
            response_scope.as_ref(),
            preliminary_plan.previous_response_id.as_deref(),
        ) {
            let previous = match state.cursor_completed_responses.get(
                scope,
                previous_response_id,
                crate::infra::time::now_ms() as i64,
            ) {
                Some(previous) => {
                    metrics::counter!("cursor_response_cache_total", "outcome" => "hit")
                        .increment(1);
                    previous
                }
                None => {
                    metrics::counter!("cursor_response_cache_total", "outcome" => "state_lost")
                        .increment(1);
                    return Err(ProxyError::cursor_response_state_lost(
                        "Cursor completed response state is unavailable, expired, or belongs to another scope",
                    ));
                }
            };
            completed_conversation_id = Some(previous.conversation_id.clone());
            prepend_response_context(&mut body_value, &previous.items)
                .map_err(ProxyError::bad_request)?;
            response_context_prepended = true;
        }
    }
    let mut plan =
        try_build_plan(inbound_protocol, &body_value).map_err(ProxyError::bad_request)?;
    if response_context_prepended {
        preserve_current_continuation(&mut plan, &preliminary_plan);
    }
    validate_tool_result_context(&plan).map_err(|message| {
        ProxyError::bad_request(format!("invalid cursor tool result context: {message}"))
    })?;
    validate_tool_choice_contract(&plan).map_err(ProxyError::bad_request)?;
    validate_cursor_runtime_configuration(&stored)?;
    let rail = CursorProtocolRail::for_provider(stored.provider_type).ok_or_else(|| {
        ProxyError::bad_request("Cursor AgentService driver requires a Cursor provider")
    })?;
    metrics::counter!(
        "cursor_tool_continuation_total",
        "protocol" => protocol_label,
        "kind" => match plan.continuation_kind {
            ToolContinuationKind::None => "none",
            ToolContinuationKind::PureToolResults => "pure_tool_results",
            ToolContinuationKind::MixedToolResults => "mixed_tool_results",
        }
    )
    .increment(1);

    let session_scope =
        cursor_session_scope(&state, &stored, &runtime_fingerprint, &request_context).await?;
    let affinity_conversation_id = cursor_affinity_conversation_id(
        &state,
        &stored,
        &runtime_fingerprint,
        &request_context,
        rail,
        &plan.working_directory,
    )
    .await?;
    let resolved_session = resolve_session_key(
        &state,
        &plan,
        &session_scope,
        rail,
        completed_conversation_id.as_deref(),
        affinity_conversation_id.as_deref(),
    )
    .await?;
    let mut session_key = resolved_session.key.clone();
    let response_model = response_model(&adapter_request, &plan.model_id);
    let input_tokens = estimate_agent_plan_input_tokens(&plan);
    let response_state = (!compact)
        .then(|| {
            response_scope.map(|scope| CursorResponseStateContext {
                scope,
                store: body_value.get("store").and_then(Value::as_bool) != Some(false),
            })
        })
        .flatten();
    let mut auth_recovery = AuthRecoveryState::default();
    let session_open_context = CursorSessionOpenContext {
        state: &state,
        stored: &stored,
        runtime_fingerprint: &runtime_fingerprint,
        plan: &plan,
        request_context: &request_context,
        timeouts,
    };
    let opened = acquire_ready_session(
        &session_open_context,
        &session_key,
        resolved_session.parked.as_ref(),
        &mut auth_recovery,
    )
    .await?;
    let mut session_entry = opened.entry;
    session_key = opened.key;

    let model = usage_model_metadata(&adapter_request);
    let semantic_tool_choice = match (&plan.tool_choice, plan.tool_results.is_empty()) {
        (ExtractedToolChoice::Required, true) => Some(SemanticToolConstraint::Required),
        (ExtractedToolChoice::Named(name), true) => {
            Some(SemanticToolConstraint::Named(name.clone()))
        }
        (_, true) if plan.local_tool_required_by_intent => {
            Some(SemanticToolConstraint::LocalIntent)
        }
        _ => None,
    };
    if adapter_request.stream_requested && semantic_tool_choice.is_none() {
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
            account_in_flight_guard,
            share_invocation_guard,
            response_state,
        )
        .await);
    }

    let mut active_plan = plan.clone();
    let mut semantic_attempt = 1usize;
    const MAX_SEMANTIC_ATTEMPTS: usize = 3;
    let drive = loop {
        let deadline = tokio::time::Instant::from_std(started + timeouts.request);
        let outcome = match tokio::time::timeout_at(
            deadline,
            drive_non_stream(
                &state,
                session_entry.clone(),
                &session_key,
                response_format,
                response_model.clone(),
                input_tokens,
            ),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => break Err(error),
            Err(_) => {
                break Err(ProxyError {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    message: "Cursor request exceeded its absolute request deadline".to_string(),
                })
            }
        };
        let Some(choice) = semantic_tool_choice.as_ref() else {
            break Ok(outcome);
        };
        let rejection = match (&outcome, choice) {
            (DriveOutcome::Completed { .. }, SemanticToolConstraint::LocalIntent) => {
                Some("local_tool_missing".to_string())
            }
            (DriveOutcome::Completed { .. }, _) => Some("required_tool_missing".to_string()),
            (DriveOutcome::Parked { tool_name, .. }, SemanticToolConstraint::Named(required))
                if !tool_names_equal(tool_name, required) =>
            {
                Some(format!(
                    "named_tool_mismatch:{}",
                    sanitize_metric_component(tool_name)
                ))
            }
            (DriveOutcome::Parked { .. }, _) => None,
        };
        let Some(rejection) = rejection else {
            break Ok(outcome);
        };
        state
            .cursor_sessions
            .release(session_entry.clone(), SessionState::Closed)
            .await;
        tracing::warn!(
            rail = rail.label(),
            attempt = semantic_attempt,
            reason = rejection,
            "Cursor tool-constrained attempt was discarded before client commit"
        );
        metrics::counter!(
            "cursor_tool_retry_total",
            "rail" => rail.label(),
            "reason" => if rejection.starts_with("named_tool_mismatch") {
                "named_tool_mismatch"
            } else if rejection == "local_tool_missing" {
                "local_tool_missing"
            } else {
                "required_tool_missing"
            },
            "attempt" => semantic_attempt.to_string()
        )
        .increment(1);
        if semantic_attempt >= MAX_SEMANTIC_ATTEMPTS {
            break Err(ProxyError {
                status: StatusCode::BAD_GATEWAY,
                message: match choice {
                    SemanticToolConstraint::LocalIntent => format!(
                        "Cursor did not invoke a required local project tool after {MAX_SEMANTIC_ATTEMPTS} attempts"
                    ),
                    _ => format!(
                        "Cursor did not satisfy the required tool choice after {MAX_SEMANTIC_ATTEMPTS} attempts"
                    ),
                },
            });
        }
        semantic_attempt += 1;
        active_plan.user_text = match choice {
            SemanticToolConstraint::Required | SemanticToolConstraint::LocalIntent => {
                retry_prompt_after_missing_tool(
                    &plan.user_text,
                    semantic_attempt,
                    MAX_SEMANTIC_ATTEMPTS,
                )
            }
            SemanticToolConstraint::Named(required) => retry_prompt_after_invalid_tool(
                &plan.user_text,
                &format!("the model called a tool other than required tool `{required}`"),
                &plan
                    .tools
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>(),
                semantic_attempt,
                MAX_SEMANTIC_ATTEMPTS,
            ),
        };
        let retry_context = CursorSessionOpenContext {
            state: &state,
            stored: &stored,
            runtime_fingerprint: &runtime_fingerprint,
            plan: &active_plan,
            request_context: &request_context,
            timeouts: match remaining_cursor_timeouts(timeouts, started.elapsed()) {
                Some(timeouts) => timeouts,
                None => {
                    break Err(ProxyError {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        message: "Cursor semantic retry exhausted the request deadline".to_string(),
                    })
                }
            },
        };
        let opened =
            match acquire_ready_session(&retry_context, &session_key, None, &mut auth_recovery)
                .await
            {
                Ok(opened) => opened,
                Err(error) => break Err(error),
            };
        session_entry = opened.entry;
        session_key = opened.key;
    };
    match drive {
        Ok(outcome) => {
            let (mut body, usage, buffered_events, final_state, completed) = match outcome {
                DriveOutcome::Completed {
                    body,
                    usage,
                    buffered_events,
                } => (body, usage, buffered_events, SessionState::Closed, true),
                DriveOutcome::Parked {
                    body,
                    usage,
                    buffered_events,
                    ..
                } => (
                    body,
                    usage,
                    buffered_events,
                    SessionState::AwaitingToolResult,
                    false,
                ),
            };
            if completed {
                let semantic_items = session_entry.lock().await.semantic_items.clone();
                cache_completed_response(
                    &state,
                    response_state.as_ref(),
                    &session_key,
                    &semantic_items,
                    &body,
                );
            }
            if compact && completed {
                body = response_compaction_body(&body)?;
            }
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
                    is_streaming: adapter_request.stream_requested,
                    ..request_context.clone()
                },
            )
            .await;
            record_share_invocation_result(
                &state,
                request_context.share_id.as_deref(),
                request_context.user_email.as_deref(),
                usage,
            )
            .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code))
                .await;
            let buffered_stream_body = adapter_request
                .stream_requested
                .then(|| Bytes::from(buffered_events.concat()));
            let mut response = Response::new(Body::from(buffered_stream_body.unwrap_or(body)));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static(if adapter_request.stream_requested {
                    "text/event-stream"
                } else {
                    "application/json"
                }),
            );
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

async fn ensure_cursor_success_status(
    state: &ServerState,
    stored: &StoredProvider,
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    status: StatusCode,
) -> Result<(), ProxyError> {
    if status.is_success() {
        return Ok(());
    }
    let failure_reason = cursor_http_failure_reason(status);
    let rail = CursorProtocolRail::for_provider(stored.provider_type)
        .map(CursorProtocolRail::label)
        .unwrap_or("unknown");
    record_cursor_agentservice_failure("open_status", failure_reason, rail);
    tracing::warn!(
        upstream_status = status.as_u16(),
        reason = failure_reason,
        rail,
        "Cursor AgentService rejected a request before business output"
    );
    let upstream_error = read_cursor_upstream_error(session_entry).await;
    maybe_mark_cursor_rate_limited(
        state,
        stored,
        status,
        &upstream_error.headers,
        &upstream_error.body,
    )
    .await;
    record_provider_outcome(state, stored, ProviderOutcome::from_status(status.as_u16())).await;
    state
        .cursor_sessions
        .release(session_entry.clone(), SessionState::Closed)
        .await;
    let message = cursor_upstream_error_message(status, upstream_error.message);
    if status == StatusCode::TOO_MANY_REQUESTS {
        let now = crate::infra::time::now_ms() as i64;
        let until = cursor_rate_limit_until(&upstream_error.headers, &upstream_error.body, now);
        let retry_after_seconds = u64::try_from(until.saturating_sub(now))
            .unwrap_or(u64::MAX)
            .saturating_add(999)
            / 1_000;
        return Err(ProxyError::rate_limited(message, retry_after_seconds));
    }
    Err(ProxyError {
        status: match status {
            StatusCode::UNAUTHORIZED => StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_GATEWAY,
        },
        message,
    })
}

fn cursor_http_failure_reason(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "upstream_authentication",
        StatusCode::FORBIDDEN => "upstream_authorization",
        StatusCode::TOO_MANY_REQUESTS => "upstream_rate_limit",
        status if status.is_server_error() => "upstream_server",
        status if status.is_client_error() => "upstream_request_rejected",
        _ => "upstream_http",
    }
}

fn record_cursor_agentservice_failure(
    phase: &'static str,
    reason: &'static str,
    rail: &'static str,
) {
    metrics::counter!(
        "cursor_agentservice_failure_total",
        "phase" => phase,
        "reason" => reason,
        "rail" => rail
    )
    .increment(1);
}

fn tool_names_equal(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

fn sanitize_metric_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(64)
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn remaining_cursor_timeouts(
    original: CursorH2Timeouts,
    elapsed: std::time::Duration,
) -> Option<CursorH2Timeouts> {
    let remaining = original.request.checked_sub(elapsed)?;
    if remaining.is_zero() {
        return None;
    }
    Some(CursorH2Timeouts {
        request: remaining,
        first_frame: original.first_frame.map(|timeout| timeout.min(remaining)),
        inter_frame: original.inter_frame.map(|timeout| timeout.min(remaining)),
    })
}

#[allow(clippy::too_many_arguments)]
async fn stream_response(
    state: ServerState,
    stored: StoredProvider,
    session_entry: Arc<tokio::sync::Mutex<CursorSession>>,
    session_key: CursorSessionKey,
    response_format: super::protocol::CursorResponseFormat,
    response_model: String,
    input_tokens: u32,
    request_context: UsageLogContext,
    started: Instant,
    model: UsageModelMetadata,
    account_in_flight_guard: Option<AccountInFlightGuard>,
    share_invocation_guard: Option<ShareInFlightGuard>,
    response_state: Option<CursorResponseStateContext>,
) -> Response {
    let (rail, custom_tool_names, response_tool_namespaces) = {
        let session = session_entry.lock().await;
        (
            session.rail,
            session.custom_tool_names.clone(),
            session.response_tool_namespaces.clone(),
        )
    };
    let mut writer = AgentSseWriter::new(response_model, response_format, input_tokens)
        .with_custom_tool_names(custom_tool_names)
        .with_response_tool_namespaces(response_tool_namespaces);
    state
        .cursor_sessions
        .bind_response_id(&session_key, &session_entry, writer.message_id())
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
    let user_email = request_context.user_email.clone();
    let first_token_ms_shared = Arc::new(AtomicU64::new(0));
    let interrupted_guard = CursorStreamInterruptGuard {
        armed: Arc::new(AtomicBool::new(true)),
        state: state.clone(),
        stored: stored.clone(),
        request_id: request_id.clone(),
        status_code: StatusCode::OK.as_u16(),
        share_id: share_id.clone(),
        user_email: user_email.clone(),
        started,
        first_token_ms: first_token_ms_shared.clone(),
        session_entry: Some(session_entry.clone()),
        parked_handoff: false,
    };
    let stream = stream! {
        let mut interrupted_guard = interrupted_guard;
        let mut account_in_flight_guard = account_in_flight_guard;
        let mut share_invocation_guard = share_invocation_guard;
        let mut filter = ComposerMarkerFilter::default();
        let mut exec_dedup = ExecDedup::default();
        let mut first_token_ms = None;
        let mut final_status = StatusCode::OK.as_u16();
        let mut final_stream_status = "completed";
        let mut final_session_state = SessionState::Closed;
        let mut session_preparked = false;
        let mut completed_response = false;

        for event in writer.start_events() {
            yield Ok::<_, std::io::Error>(Bytes::from(event));
        }

        loop {
            let frame = match next_session_frame(&session_entry).await {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    let error = cursor_incomplete_response_error(rail);
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
                Err(error) => {
                    record_cursor_agentservice_failure(
                        "stream_read",
                        "transport_or_session",
                        rail.label(),
                    );
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            };
            let kv_event = match decode_kv_server_event(&frame.payload)
                .map_err(|error| cursor_proto_error(rail, error))
            {
                Ok(event) => event,
                Err(error) => {
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            };
            let kv_terminal_candidate = kv_event.is_some();
            if let Err(error) = handle_kv_event(&session_entry, kv_event).await {
                final_status = error.status.as_u16();
                final_stream_status = "failed";
                for event in writer.error_events(&error.message) {
                    yield Ok::<_, std::io::Error>(Bytes::from(event));
                }
                break;
            }
            let exec_event = match decode_exec_server_event(&frame.payload)
                .map_err(|error| cursor_proto_error(rail, error))
            {
                Ok(event) => event,
                Err(error) => {
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            };
            match handle_exec_event(
                &state,
                &session_entry,
                &mut exec_dedup,
                exec_event,
            )
            .await
            {
                Ok(ExecHandling::Continue) => {}
                Ok(ExecHandling::ToolCall(tool_call)) => {
                    if let Err(error) = mark_session_business_output(&session_entry).await {
                        final_status = error.status.as_u16();
                        final_stream_status = "failed";
                        for event in writer.error_events(&error.message) {
                            yield Ok::<_, std::io::Error>(Bytes::from(event));
                        }
                        break;
                    }
                    let events = writer.event(&AgentEvent::ToolCall(tool_call));
                    // Park the h2 stream before the first client-visible tool
                    // event. Claude/Codex may submit the result immediately
                    // and may close this SSE without waiting for its terminal
                    // event. Once parked, this response no longer owns the
                    // session lifecycle.
                    state
                        .cursor_sessions
                        .release(session_entry.clone(), SessionState::AwaitingToolResult)
                        .await;
                    metrics::counter!(
                        "cursor_stream_park_total",
                        "outcome" => "before_publish",
                        "rail" => rail.label()
                    )
                    .increment(1);
                    interrupted_guard.hand_off_parked_session();
                    drop(account_in_flight_guard.take());
                    drop(share_invocation_guard.take());
                    session_preparked = true;
                    if first_token_ms.is_none() && !events.is_empty() {
                        record_cursor_first_output(
                            &state,
                            &stored,
                            &request_id,
                            started,
                            &mut first_token_ms,
                            &first_token_ms_shared,
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
            let deltas = match decode_agent_server_message(&frame.payload)
                .map_err(|error| cursor_proto_error(rail, error))
            {
                Ok(deltas) => deltas,
                Err(error) => {
                    final_status = error.status.as_u16();
                    final_stream_status = "failed";
                    for event in writer.error_events(&error.message) {
                        yield Ok::<_, std::io::Error>(Bytes::from(event));
                    }
                    break;
                }
            };
            let terminal_batch = kv_terminal_candidate
                || deltas
                    .iter()
                    .any(|delta| matches!(delta, InteractionDelta::TurnEnded));
            let mut ended = false;
            for delta in deltas {
                let content_delta = cursor_delta_is_business_output(&delta);
                let had_response_content = writer.has_response_content();
                let (events, valid_output) = match cursor_delta_events(delta, &mut writer, &mut filter) {
                    Ok(CursorDeltaOutcome::Events(events)) => (events, true),
                    Ok(CursorDeltaOutcome::TurnEnded(events)) => {
                        if writer.has_response_content() {
                            ended = true;
                            completed_response = true;
                            (events, true)
                        } else {
                            let error = cursor_empty_response_error(rail);
                            final_status = error.status.as_u16();
                            final_stream_status = "failed";
                            ended = true;
                            (writer.error_events(&error.message), false)
                        }
                    }
                    Err(error) => {
                        final_status = error.status.as_u16();
                        final_stream_status = "failed";
                        (writer.error_events(&error.message), false)
                    }
                };
                let business_output = cursor_events_are_business_output(
                    valid_output,
                    content_delta,
                    had_response_content,
                    writer.has_response_content(),
                );
                if business_output {
                    if let Err(error) = mark_session_business_output(&session_entry).await {
                        final_status = error.status.as_u16();
                        final_stream_status = "failed";
                        ended = true;
                        for event in writer.error_events(&error.message) {
                            yield Ok::<_, std::io::Error>(Bytes::from(event));
                        }
                        break;
                    }
                }
                if should_record_progressive_first_output(
                    first_token_ms.is_some(),
                    business_output,
                    terminal_batch,
                ) {
                    record_cursor_first_output(
                        &state,
                        &stored,
                        &request_id,
                        started,
                        &mut first_token_ms,
                        &first_token_ms_shared,
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
            if !ended {
                match cursor_kv_terminal_events(
                    rail,
                    kv_terminal_candidate,
                    &mut writer,
                    &mut filter,
                ) {
                    Ok(Some(events)) => {
                        if let Err(error) = mark_session_business_output(&session_entry).await {
                            final_status = error.status.as_u16();
                            final_stream_status = "failed";
                            for event in writer.error_events(&error.message) {
                                yield Ok::<_, std::io::Error>(Bytes::from(event));
                            }
                        } else {
                            completed_response = true;
                            for event in events {
                                yield Ok::<_, std::io::Error>(Bytes::from(event));
                            }
                        }
                        ended = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        final_status = error.status.as_u16();
                        final_stream_status = "failed";
                        ended = true;
                        for event in writer.error_events(&error.message) {
                            yield Ok::<_, std::io::Error>(Bytes::from(event));
                        }
                    }
                }
            }
            if ended {
                break;
            }
        }

        let done_events = writer.done_events();
        for event in done_events {
            yield Ok::<_, std::io::Error>(Bytes::from(event));
        }
        let usage = writer_usage(&writer);
        if completed_response && final_stream_status != "failed" {
            let response = serde_json::to_vec(&writer.json_response())
                .map(Bytes::from)
                .unwrap_or_default();
            let semantic_items = session_entry.lock().await.semantic_items.clone();
            cache_completed_response(
                &state,
                response_state.as_ref(),
                &session_key,
                &semantic_items,
                &response,
            );
        }
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
        record_share_invocation_result(&state, share_id.as_deref(), user_email.as_deref(), usage)
            .await;
        let outcome = if final_stream_status == "failed" {
            ProviderOutcome::NetworkFailure
        } else {
            ProviderOutcome::from_status(final_status)
        };
        record_provider_outcome(&state, &stored, outcome).await;
        if final_stream_status == "failed" {
            final_session_state = SessionState::Closed;
        }
        if !session_preparked {
            state
                .cursor_sessions
                .release(session_entry.clone(), final_session_state)
                .await;
        }
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
    session_key: &CursorSessionKey,
    response_format: super::protocol::CursorResponseFormat,
    response_model: String,
    input_tokens: u32,
) -> Result<DriveOutcome, ProxyError> {
    let (rail, custom_tool_names, response_tool_namespaces) = {
        let session = session_entry.lock().await;
        (
            session.rail,
            session.custom_tool_names.clone(),
            session.response_tool_namespaces.clone(),
        )
    };
    let mut writer = AgentSseWriter::new(response_model, response_format, input_tokens)
        .with_custom_tool_names(custom_tool_names)
        .with_response_tool_namespaces(response_tool_namespaces);
    state
        .cursor_sessions
        .bind_response_id(session_key, &session_entry, writer.message_id())
        .await;
    let mut buffered_events = writer.start_events();
    let mut filter = ComposerMarkerFilter::default();
    let mut exec_dedup = ExecDedup::default();
    loop {
        let Some(frame) = next_session_frame(&session_entry)
            .await
            .inspect_err(|_error| {
                record_cursor_agentservice_failure(
                    "stream_read",
                    "transport_or_session",
                    rail.label(),
                );
            })?
        else {
            return Err(cursor_incomplete_response_error(rail));
        };
        let kv_event = decode_kv_server_event(&frame.payload)
            .map_err(|error| cursor_proto_error(rail, error))?;
        let kv_terminal_candidate = kv_event.is_some();
        handle_kv_event(&session_entry, kv_event).await?;
        let exec_event = decode_exec_server_event(&frame.payload)
            .map_err(|error| cursor_proto_error(rail, error))?;
        match handle_exec_event(state, &session_entry, &mut exec_dedup, exec_event).await? {
            ExecHandling::Continue => {}
            ExecHandling::ToolCall(tool_call) => {
                mark_session_business_output(&session_entry).await?;
                let tool_name = tool_call.name.clone();
                buffered_events.extend(writer.event(&AgentEvent::ToolCall(tool_call)));
                buffered_events.extend(writer.done_events());
                let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
                    ProxyError::bad_request(format!(
                        "Cursor AgentService JSON response encode failed: {error}"
                    ))
                })?;
                return Ok(DriveOutcome::Parked {
                    body: Bytes::from(body),
                    usage: writer_usage(&writer),
                    buffered_events,
                    tool_name,
                });
            }
        }
        for delta in decode_agent_server_message(&frame.payload)
            .map_err(|error| cursor_proto_error(rail, error))?
        {
            let content_delta = cursor_delta_is_business_output(&delta);
            let had_response_content = writer.has_response_content();
            let outcome = cursor_delta_events(delta, &mut writer, &mut filter)?;
            let business_output = cursor_events_are_business_output(
                true,
                content_delta,
                had_response_content,
                writer.has_response_content(),
            );
            match outcome {
                CursorDeltaOutcome::Events(events) => {
                    buffered_events.extend(events);
                    if business_output {
                        mark_session_business_output(&session_entry).await?;
                    }
                }
                CursorDeltaOutcome::TurnEnded(events) => {
                    buffered_events.extend(events);
                    if !writer.has_response_content() {
                        return Err(cursor_empty_response_error(rail));
                    }
                    mark_session_business_output(&session_entry).await?;
                    buffered_events.extend(writer.done_events());
                    let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
                        ProxyError::bad_request(format!(
                            "Cursor AgentService JSON response encode failed: {error}"
                        ))
                    })?;
                    return Ok(DriveOutcome::Completed {
                        body: Bytes::from(body),
                        usage: writer_usage(&writer),
                        buffered_events,
                    });
                }
            }
        }
        if let Some(events) =
            cursor_kv_terminal_events(rail, kv_terminal_candidate, &mut writer, &mut filter)?
        {
            buffered_events.extend(events);
            mark_session_business_output(&session_entry).await?;
            buffered_events.extend(writer.done_events());
            let body = serde_json::to_vec(&writer.json_response()).map_err(|error| {
                ProxyError::bad_request(format!(
                    "Cursor AgentService JSON response encode failed: {error}"
                ))
            })?;
            return Ok(DriveOutcome::Completed {
                body: Bytes::from(body),
                usage: writer_usage(&writer),
                buffered_events,
            });
        }
    }
}

fn cursor_delta_is_business_output(delta: &InteractionDelta) -> bool {
    match delta {
        InteractionDelta::Text(text) | InteractionDelta::Thinking(text) => !text.is_empty(),
        InteractionDelta::ThinkingComplete
        | InteractionDelta::TokenDelta(_)
        | InteractionDelta::TurnEnded
        | InteractionDelta::Heartbeat
        | InteractionDelta::ToolCallStarted
        | InteractionDelta::ToolCallCompleted
        | InteractionDelta::KvServerMessage
        | InteractionDelta::Unknown(_) => false,
    }
}

fn cursor_events_are_business_output(
    valid_output: bool,
    content_delta: bool,
    had_response_content: bool,
    has_response_content: bool,
) -> bool {
    valid_output && (content_delta || (!had_response_content && has_response_content))
}

fn should_record_progressive_first_output(
    already_recorded: bool,
    business_output: bool,
    terminal_batch: bool,
) -> bool {
    !already_recorded && business_output && !terminal_batch
}

async fn record_cursor_first_output(
    state: &ServerState,
    stored: &StoredProvider,
    request_id: &str,
    started: Instant,
    first_token_ms: &mut Option<u128>,
    first_token_ms_shared: &AtomicU64,
) {
    if first_token_ms.is_some() {
        return;
    }
    let elapsed = started.elapsed().as_millis();
    *first_token_ms = Some(elapsed);
    first_token_ms_shared.store(encode_optional_millis(elapsed), Ordering::Relaxed);
    update_stream_usage(
        state,
        stored,
        request_id,
        StatusCode::OK.as_u16(),
        elapsed,
        *first_token_ms,
        TokenUsage::default(),
        Some("streaming"),
    )
    .await;
}

fn encode_optional_millis(value: u128) -> u64 {
    value.min(u128::from(u64::MAX - 1)) as u64 + 1
}

fn cursor_proto_error(rail: CursorProtocolRail, error: ProtoError) -> ProxyError {
    record_cursor_agentservice_failure("decode", "wire_protocol", rail.label());
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: format!("Cursor Connect-RPC protobuf decode failed: {error}"),
    }
}

fn cursor_incomplete_response_error(rail: CursorProtocolRail) -> ProxyError {
    record_cursor_agentservice_failure("completion", "incomplete_response", rail.label());
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: format!(
            "Cursor {} response ended before a business completion signal",
            rail.label()
        ),
    }
}

fn cursor_empty_response_error(rail: CursorProtocolRail) -> ProxyError {
    record_cursor_agentservice_failure("completion", "empty_response", rail.label());
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: format!(
            "Cursor {} response completed without text, reasoning, or a tool call",
            rail.label()
        ),
    }
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

fn cursor_kv_terminal_events(
    rail: CursorProtocolRail,
    kv_seen: bool,
    writer: &mut AgentSseWriter,
    filter: &mut ComposerMarkerFilter,
) -> Result<Option<Vec<String>>, ProxyError> {
    if !kv_seen || !rail.accepts_kv_after_text_terminal() {
        return Ok(None);
    }
    let events = flush_cursor_marker_filter(writer, filter)?;
    Ok(writer.has_visible_text().then_some(events))
}

fn flush_cursor_marker_filter(
    writer: &mut AgentSseWriter,
    filter: &mut ComposerMarkerFilter,
) -> Result<Vec<String>, ProxyError> {
    let mut out = Vec::new();
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
    Ok(out)
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
            let (rail, working_directory) = {
                let session = session_entry.lock().await;
                (session.rail, session.working_directory.clone())
            };
            if rail.uses_rich_request_context() {
                encode_rich_request_context_response(exec_msg_id, &exec_id, &working_directory)
            } else {
                encode_request_context_response(exec_msg_id, &exec_id, &[])
            }
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

struct CursorSessionOpenContext<'a> {
    state: &'a ServerState,
    stored: &'a StoredProvider,
    runtime_fingerprint: &'a str,
    plan: &'a AgentRunPlan,
    request_context: &'a UsageLogContext,
    timeouts: CursorH2Timeouts,
}

struct OpenedCursorSession {
    entry: Arc<tokio::sync::Mutex<CursorSession>>,
    access_token: Option<String>,
    key: CursorSessionKey,
}

struct CursorSessionReservationGuard {
    manager: CursorSessionManager,
    entry: Option<Arc<tokio::sync::Mutex<CursorSession>>>,
}

impl CursorSessionReservationGuard {
    fn new(manager: CursorSessionManager, entry: Arc<tokio::sync::Mutex<CursorSession>>) -> Self {
        Self {
            manager,
            entry: Some(entry),
        }
    }

    fn disarm(&mut self) {
        self.entry = None;
    }
}

impl Drop for CursorSessionReservationGuard {
    fn drop(&mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        let manager = self.manager.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                manager.release(entry, SessionState::Closed).await;
            });
        }
    }
}

#[derive(Clone)]
struct CursorResponseStateContext {
    scope: CursorResponseScope,
    store: bool,
}

fn cache_completed_response(
    state: &ServerState,
    context: Option<&CursorResponseStateContext>,
    session_key: &CursorSessionKey,
    semantic_items: &[Value],
    body: &[u8],
) {
    let Some(context) = context.filter(|context| context.store) else {
        return;
    };
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(response_id) = response.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(output) = response.get("output").and_then(Value::as_array) else {
        return;
    };
    let mut items = semantic_items.to_vec();
    items.extend(output.iter().cloned());
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    if !state.cursor_completed_responses.insert(
        context.scope.clone(),
        response_id,
        session_key.conversation_id(),
        items,
        now,
    ) {
        tracing::warn!(
            response_id_len = response_id.len(),
            "Cursor completed response context was not cached because it exceeded a cache boundary"
        );
        metrics::counter!("cursor_response_cache_total", "outcome" => "write_rejected")
            .increment(1);
    } else {
        metrics::counter!("cursor_response_cache_total", "outcome" => "write").increment(1);
    }
}

fn response_compaction_body(body: &[u8]) -> Result<Bytes, ProxyError> {
    let response = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("Cursor compaction response decode failed: {error}"))
    })?;
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_cursor_compaction");
    let summary = response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let summary = if summary.is_empty() {
        "[empty conversation summary]".to_string()
    } else {
        summary
    };
    let object = json!({
        "id": response_id,
        "object": "response.compaction",
        "created_at": chrono::Utc::now().timestamp(),
        "output": [{
            "id": format!("cmp_{}", response_id.trim_start_matches("resp_")),
            "type": "compaction",
            "encrypted_content": summary,
        }],
        "usage": response.get("usage").cloned().unwrap_or_else(|| json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        })),
    });
    serde_json::to_vec(&object)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode compaction response: {error}")))
}

async fn acquire_or_open_session(
    context: &CursorSessionOpenContext<'_>,
    session_key: &CursorSessionKey,
    parked: Option<&CursorSessionReference>,
    auth_recovery: &mut AuthRecoveryState,
) -> Result<OpenedCursorSession, ProxyError> {
    if let Some(parked) = parked {
        let entry = context
            .state
            .cursor_sessions
            .acquire_resolved(parked)
            .await
            .ok_or_else(|| {
                ProxyError::cursor_session_lost(format!(
                    "Cursor session `{}` is unavailable, expired, or bound to a different credential",
                    session_key.conversation_id()
                ))
            })?;
        if let Err(error) = resume_tool_results(&entry, &context.plan.tool_results).await {
            context
                .state
                .cursor_sessions
                .release(entry.clone(), SessionState::Closed)
                .await;
            return Err(error);
        }
        return Ok(OpenedCursorSession {
            entry,
            access_token: None,
            key: session_key.clone(),
        });
    }

    let mut active_session_key = session_key.clone();
    loop {
        let credential = resolve_cursor_credential(
            context.state,
            context.stored,
            context.runtime_fingerprint,
            context.timeouts.request,
        )
        .await?;
        let access_token = credential.access_token().to_string();
        match open_agent_stream(
            context.state,
            &credential,
            context.stored,
            context.runtime_fingerprint,
            context.plan,
            &active_session_key,
            context.timeouts,
        )
        .await
        {
            Ok(entry) => {
                return Ok(OpenedCursorSession {
                    entry,
                    access_token: Some(access_token),
                    key: active_session_key,
                })
            }
            Err(error) => {
                let action = cursor_agentservice_auth_action(
                    context.state,
                    context.stored,
                    context.runtime_fingerprint,
                    Some(&access_token),
                    auth_recovery,
                    error.status,
                    true,
                )
                .await;
                match action {
                    CursorAgentServiceAuthAction::UseResponse => return Err(error),
                    CursorAgentServiceAuthAction::RefreshAndReplaySameBinding => {
                        recover_cursor_unauthorized(
                            context.state,
                            context.stored,
                            context.runtime_fingerprint,
                            Some(&access_token),
                        )
                        .await?;
                        let refreshed_scope = cursor_session_scope(
                            context.state,
                            context.stored,
                            context.runtime_fingerprint,
                            context.request_context,
                        )
                        .await?;
                        active_session_key =
                            rekey_cursor_session(&active_session_key, refreshed_scope);
                    }
                }
            }
        }
    }
}

async fn acquire_ready_session(
    context: &CursorSessionOpenContext<'_>,
    session_key: &CursorSessionKey,
    parked: Option<&CursorSessionReference>,
    auth_recovery: &mut AuthRecoveryState,
) -> Result<OpenedCursorSession, ProxyError> {
    let mut opened = acquire_or_open_session(context, session_key, parked, auth_recovery).await?;
    loop {
        let status = session_status(&opened.entry).await?;
        match cursor_agentservice_auth_action(
            context.state,
            context.stored,
            context.runtime_fingerprint,
            opened.access_token.as_deref(),
            auth_recovery,
            status,
            opened.access_token.is_some(),
        )
        .await
        {
            CursorAgentServiceAuthAction::UseResponse => {
                ensure_cursor_success_status(context.state, context.stored, &opened.entry, status)
                    .await?;
                return Ok(opened);
            }
            CursorAgentServiceAuthAction::RefreshAndReplaySameBinding => {
                context
                    .state
                    .cursor_sessions
                    .release(opened.entry, SessionState::Closed)
                    .await;
                recover_cursor_unauthorized(
                    context.state,
                    context.stored,
                    context.runtime_fingerprint,
                    opened.access_token.as_deref(),
                )
                .await?;
                let refreshed_scope = cursor_session_scope(
                    context.state,
                    context.stored,
                    context.runtime_fingerprint,
                    context.request_context,
                )
                .await?;
                let refreshed_key = rekey_cursor_session(&opened.key, refreshed_scope);
                opened =
                    acquire_or_open_session(context, &refreshed_key, None, auth_recovery).await?;
            }
        }
    }
}

fn rekey_cursor_session(
    current: &CursorSessionKey,
    refreshed_scope: CursorSessionScope,
) -> CursorSessionKey {
    CursorSessionKey::new(refreshed_scope, current.conversation_id().to_string())
}

async fn resume_tool_results(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
    tool_results: &[super::request_builder::ToolResultBlock],
) -> Result<(), ProxyError> {
    let mut session = session_entry.lock().await;
    let mut unique_results = Vec::new();
    let mut seen = HashSet::new();
    for result in tool_results {
        if seen.insert(result.tool_call_id.as_str()) {
            unique_results.push(result);
        }
    }
    if unique_results.iter().any(|result| {
        !session
            .pending_tool_calls
            .contains_key(&result.tool_call_id)
    }) {
        return Err(ProxyError::conflict(
            "Cursor AgentService tool_result set did not exactly match the parked tool calls",
        ));
    }

    for result in &unique_results {
        let pending = session
            .pending_tool_calls
            .get(&result.tool_call_id)
            .expect("validated pending tool result")
            .clone();
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
        session.pending_tool_calls.remove(&result.tool_call_id);
        if !session.semantic_items.is_empty() {
            session.semantic_items.push(if pending.custom {
                json!({
                    "type": "custom_tool_call_output",
                    "call_id": result.tool_call_id,
                    "output": result.content,
                })
            } else {
                json!({
                    "type": "function_call_output",
                    "call_id": result.tool_call_id,
                    "output": result.content,
                })
            });
        }
    }
    if !unique_results.is_empty() {
        let stream = session.stream.as_mut().ok_or_else(|| {
            ProxyError::conflict("Cursor AgentService parked session has no live h2 stream")
        })?;
        stream.rearm_business_output_phase();
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

struct CursorUpstreamError {
    headers: HeaderMap,
    body: Bytes,
    message: Option<String>,
}

async fn read_cursor_upstream_error(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
) -> CursorUpstreamError {
    let (headers, body) = {
        let mut session = session_entry.lock().await;
        let Some(stream) = session.stream.as_mut() else {
            return CursorUpstreamError {
                headers: HeaderMap::new(),
                body: Bytes::new(),
                message: None,
            };
        };
        let headers = stream.headers().clone();
        stream.close_writer();
        let body = if cursor_error_body_is_json_like(&headers) {
            stream
                .read_body_limited(MAX_CURSOR_ERROR_BODY_BYTES)
                .await
                .unwrap_or_else(|_| Bytes::new())
        } else {
            Bytes::new()
        };
        (headers, body)
    };
    let message = cursor_error_message_from_body(&body);
    CursorUpstreamError {
        headers,
        body,
        message,
    }
}

fn cursor_error_body_is_json_like(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let content_type = content_type.to_ascii_lowercase();
    content_type.contains("json")
}

fn cursor_upstream_error_message(status: StatusCode, detail: Option<String>) -> String {
    match detail {
        Some(detail) => format!(
            "Cursor AgentService returned HTTP {}: {detail}",
            status.as_u16()
        ),
        None => format!("Cursor AgentService returned HTTP {}", status.as_u16()),
    }
}

fn cursor_error_message_from_body(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let message = cursor_error_field(&value, &["/error/message", "/message"])
        .or_else(|| cursor_error_field(&value, &["/details/0/message"]))
        .or_else(|| cursor_error_field(&value, &["/error", "/code"]));
    let code = cursor_error_field(&value, &["/error/code", "/code"]);
    let detail = match (code, message) {
        (Some(code), Some(message)) if code != message => Some(format!("{code}: {message}")),
        (_, Some(message)) => Some(message),
        (Some(code), None) => Some(code),
        _ => None,
    }?;
    Some(cursor_transport_diagnostic(&detail))
}

fn cursor_error_field(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                value
                    .as_i64()
                    .map(|number| number.to_string())
                    .or_else(|| value.as_u64().map(|number| number.to_string()))
            })
    })
}

async fn maybe_mark_cursor_rate_limited(
    state: &ServerState,
    stored: &StoredProvider,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) {
    if status != StatusCode::TOO_MANY_REQUESTS || !is_cursor_account_provider(stored.provider_type)
    {
        return;
    }
    let Some((provider_type, account_id, auth_identity_generation)) =
        managed_account_binding_with_generation(stored)
    else {
        return;
    };
    let now = crate::infra::time::now_ms() as i64;
    let until = cursor_rate_limit_until(headers, body, now);
    let detail = cursor_error_message_from_body(body)
        .map(|message| format!("; {message}"))
        .unwrap_or_default();
    let message = format!("cursor upstream returned 429; cooling account until {until}{detail}");
    state
        .mark_account_rate_limited_until_if_current(
            account_id,
            provider_type,
            auth_identity_generation,
            until,
            Some(message),
        )
        .await;
}

fn cursor_rate_limit_until(headers: &HeaderMap, body: &[u8], now_ms: i64) -> i64 {
    let until = super::super::grok::retry_after_until_ms(headers, now_ms)
        .or_else(|| cursor_rate_limit_until_from_body(body, now_ms))
        .unwrap_or_else(|| now_ms.saturating_add(60_000));
    super::super::bounded_upstream_rate_limit_until(now_ms, until)
}

fn is_cursor_account_provider(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::CursorOAuth | ProviderType::CursorApiKey
    )
}

fn cursor_rate_limit_until_from_body(body: &[u8], now_ms: i64) -> Option<i64> {
    let value: Value = serde_json::from_slice(body).ok()?;
    cursor_duration_ms(&value, &["/error/retry_after_ms"], now_ms)
        .or_else(|| cursor_duration_seconds(&value, &["/retryAfterSeconds"], now_ms))
        .or_else(|| cursor_absolute_ms(&value, &["/rateLimited/resetAtMs"]))
        .filter(|until| *until > now_ms)
}

fn cursor_duration_ms(value: &Value, pointers: &[&str], now_ms: i64) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| number_at(value, pointer))
        .filter(|ms| *ms > 0)
        .map(|ms| now_ms.saturating_add(ms))
}

fn cursor_duration_seconds(value: &Value, pointers: &[&str], now_ms: i64) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| number_at(value, pointer))
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)))
}

fn cursor_absolute_ms(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| number_at(value, pointer))
        .map(|value| {
            if value < 10_000_000_000 {
                value.saturating_mul(1000)
            } else {
                value
            }
        })
}

fn number_at(value: &Value, pointer: &str) -> Option<i64> {
    value.pointer(pointer).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str()?.trim().parse().ok())
    })
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

async fn mark_session_business_output(
    session_entry: &Arc<tokio::sync::Mutex<CursorSession>>,
) -> Result<(), ProxyError> {
    let mut session = session_entry.lock().await;
    let stream = session
        .stream
        .as_mut()
        .ok_or_else(|| ProxyError::conflict("Cursor AgentService session has no live h2 stream"))?;
    stream.mark_business_output();
    Ok(())
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
    let (declared_tools, session_key, custom_tool_names, response_tool_namespaces) = {
        let session = session_entry.lock().await;
        (
            session.declared_tools.clone(),
            session.key.clone(),
            session.custom_tool_names.clone(),
            session.response_tool_namespaces.clone(),
        )
    };
    let resolved = match resolve_tool_call(&declared_tools, tool_name, args) {
        Ok(resolved) => resolved,
        Err(error) => {
            metrics::counter!(
                "cursor_tool_resolution_total",
                "outcome" => "rejected",
                "reason" => error.reason_code
            )
            .increment(1);
            let message = format!("{}: {}", error.original_name, error.reason);
            send_session_frame(
                session_entry,
                encode_exec_mcp_error(exec_msg_id, exec_id, &message),
            )
            .await?;
            return Ok(ExecHandling::Continue);
        }
    };
    let replay_rejection = {
        let mut session = session_entry.lock().await;
        let replay = session
            .cold_resume_completed_calls
            .iter()
            .any(|(name, args)| tool_names_equal(name, &resolved.name) && args == &resolved.args);
        if replay {
            session.cold_resume_replay_rejections =
                session.cold_resume_replay_rejections.saturating_add(1);
            Some(session.cold_resume_replay_rejections)
        } else {
            None
        }
    };
    if let Some(rejections) = replay_rejection {
        metrics::counter!("cursor_cold_resume_replay_total", "outcome" => "rejected").increment(1);
        if rejections > 2 {
            return Err(ProxyError {
                status: StatusCode::BAD_GATEWAY,
                message: "Cursor repeatedly attempted to replay an already completed tool call after cold resume".to_string(),
            });
        }
        send_session_frame(
            session_entry,
            encode_exec_mcp_error(
                exec_msg_id,
                exec_id,
                "This exact tool call has already completed and its result is present in the conversation. Continue from that result without repeating the call.",
            ),
        )
        .await?;
        return Ok(ExecHandling::Continue);
    }
    metrics::counter!(
        "cursor_tool_resolution_total",
        "outcome" => "resolved",
        "reason" => "declared_schema_valid"
    )
    .increment(1);

    let client_call_id = if tool_call_id.trim().is_empty() {
        random_call_id()
    } else {
        tool_call_id.to_string()
    };
    let custom = custom_tool_names
        .iter()
        .any(|name| tool_names_equal(name, &resolved.name));
    let response_namespace = response_tool_namespaces
        .iter()
        .find(|wire| tool_names_equal(&wire.internal_name, &resolved.name));
    let arguments_json = serde_json::to_string(&resolved.args).unwrap_or_else(|_| "{}".to_string());
    {
        let mut session = session_entry.lock().await;
        session.pending_tool_calls.insert(
            client_call_id.clone(),
            PendingToolCall {
                exec_msg_id,
                exec_id: exec_id.to_string(),
                tool_name: resolved.name.clone(),
                custom,
            },
        );
        if !session.semantic_items.is_empty() {
            if custom {
                session.semantic_items.push(json!({
                    "type": "custom_tool_call",
                    "call_id": client_call_id.clone(),
                    "name": resolved.name.clone(),
                    "input": resolved
                        .args
                        .get("input")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                }));
            } else {
                let mut item = json!({
                    "type": "function_call",
                    "call_id": client_call_id.clone(),
                    "name": response_namespace
                        .map(|wire| wire.name.as_str())
                        .unwrap_or(&resolved.name),
                    "arguments": arguments_json.clone(),
                });
                if let Some(namespace) = response_namespace.map(|wire| wire.namespace.as_str()) {
                    item["namespace"] = Value::String(namespace.to_string());
                }
                session.semantic_items.push(item);
            }
        }
    }
    state
        .cursor_sessions
        .bind_tool_call_id(&session_key, session_entry, &client_call_id)
        .await;

    Ok(ExecHandling::ToolCall(CapturedToolCall {
        id: client_call_id,
        name: resolved.name,
        arguments_json,
    }))
}

async fn resolve_session_key(
    state: &ServerState,
    plan: &AgentRunPlan,
    scope: &CursorSessionScope,
    rail: CursorProtocolRail,
    completed_conversation_id: Option<&str>,
    affinity_conversation_id: Option<&str>,
) -> Result<ResolvedCursorSession, ProxyError> {
    if !plan.tool_results.is_empty() {
        if plan.continuation_kind == ToolContinuationKind::PureToolResults {
            let mut candidate: Option<CursorSessionReference> = None;
            let mut all_call_ids_resolved = true;
            for result in &plan.tool_results {
                match state
                    .cursor_sessions
                    .resolve_tool_call_id(scope, &result.tool_call_id)
                    .await
                {
                    Some(reference) => {
                        if candidate
                            .as_ref()
                            .is_some_and(|current| current.key() != reference.key())
                        {
                            return Err(ProxyError::conflict(
                                "Cursor tool results resolve to different parked sessions",
                            ));
                        }
                        candidate = Some(reference);
                    }
                    None => all_call_ids_resolved = false,
                }
            }
            if let Some(previous_response_id) = plan.previous_response_id.as_deref() {
                if let Some(reference) = state
                    .cursor_sessions
                    .resolve_response_id(scope, previous_response_id)
                    .await
                {
                    if candidate
                        .as_ref()
                        .is_some_and(|current| current.key() != reference.key())
                    {
                        return Err(ProxyError::conflict(
                            "Cursor response_id and tool call IDs resolve to different sessions",
                        ));
                    }
                    candidate = Some(reference);
                }
            }
            if all_call_ids_resolved {
                if let Some(reference) = candidate {
                    metrics::counter!(
                        "cursor_session_resume_total",
                        "outcome" => "live",
                        "rail" => rail.label()
                    )
                    .increment(1);
                    return Ok(ResolvedCursorSession::parked(reference));
                }
            }
        }
        if plan.cold_resume_ready {
            let prior_conversation_id = close_unusable_continuation_session(
                state,
                scope,
                &plan.tool_results,
                plan.previous_response_id.as_deref(),
            )
            .await?;
            metrics::counter!(
                "cursor_session_resume_total",
                "outcome" => "cold",
                "rail" => rail.label()
            )
            .increment(1);
            return Ok(ResolvedCursorSession::new(CursorSessionKey::new(
                scope.clone(),
                prior_conversation_id
                    .or_else(|| affinity_conversation_id.map(str::to_string))
                    .or_else(|| plan.previous_response_id.clone())
                    .unwrap_or_else(|| new_cursor_conversation_id(rail)),
            )));
        }
        metrics::counter!(
            "cursor_session_resume_total",
            "outcome" => "state_lost",
            "rail" => rail.label()
        )
        .increment(1);
        return Err(ProxyError::cursor_session_lost(
            "Cursor AgentService tool_result has no matching parked session",
        ));
    }

    if let Some(conversation_id) = completed_conversation_id {
        return Ok(ResolvedCursorSession::new(CursorSessionKey::new(
            scope.clone(),
            conversation_id.to_string(),
        )));
    }

    if let Some(previous_response_id) = plan.previous_response_id.as_deref() {
        if let Some(reference) = state
            .cursor_sessions
            .resolve_response_id(scope, previous_response_id)
            .await
        {
            return Ok(ResolvedCursorSession::new(reference.key().clone()));
        }
        if !previous_response_id.trim().is_empty() {
            return Ok(ResolvedCursorSession::new(CursorSessionKey::new(
                scope.clone(),
                previous_response_id.to_string(),
            )));
        }
    }

    if let Some(conversation_id) = affinity_conversation_id {
        return Ok(ResolvedCursorSession::new(CursorSessionKey::new(
            scope.clone(),
            conversation_id.to_string(),
        )));
    }

    Ok(ResolvedCursorSession::new(CursorSessionKey::new(
        scope.clone(),
        new_cursor_conversation_id(rail),
    )))
}

async fn close_unusable_continuation_session(
    state: &ServerState,
    scope: &CursorSessionScope,
    tool_results: &[super::request_builder::ToolResultBlock],
    previous_response_id: Option<&str>,
) -> Result<Option<String>, ProxyError> {
    let mut candidate: Option<CursorSessionReference> = None;
    for result in tool_results {
        let Some(reference) = state
            .cursor_sessions
            .resolve_tool_call_id(scope, &result.tool_call_id)
            .await
        else {
            continue;
        };
        if candidate
            .as_ref()
            .is_some_and(|current| current.key() != reference.key())
        {
            return Err(ProxyError::conflict(
                "Cursor cold-resume results refer to different live sessions",
            ));
        }
        candidate = Some(reference);
    }
    if let Some(response_id) = previous_response_id {
        if let Some(reference) = state
            .cursor_sessions
            .resolve_response_id(scope, response_id)
            .await
        {
            if candidate
                .as_ref()
                .is_some_and(|current| current.key() != reference.key())
            {
                return Err(ProxyError::conflict(
                    "Cursor cold-resume response and tool IDs refer to different sessions",
                ));
            }
            candidate = Some(reference);
        }
    }
    if let Some(reference) = candidate {
        let conversation_id = reference.key().conversation_id().to_string();
        if !state
            .cursor_sessions
            .close_parked_resolved(&reference)
            .await
        {
            let still_live = futures_util::future::join_all(tool_results.iter().map(|result| {
                state
                    .cursor_sessions
                    .resolve_tool_call_id(scope, &result.tool_call_id)
            }))
            .await
            .into_iter()
            .any(|reference| reference.is_some());
            let response_still_live = match previous_response_id {
                Some(response_id) => state
                    .cursor_sessions
                    .resolve_response_id(scope, response_id)
                    .await
                    .is_some(),
                None => false,
            };
            if still_live || response_still_live {
                return Err(ProxyError::cursor_conversation_busy(
                    "Cursor continuation is already being resumed by another request",
                ));
            }
        }
        return Ok(Some(conversation_id));
    }
    Ok(None)
}

fn new_cursor_conversation_id(rail: CursorProtocolRail) -> String {
    let id = random_uuid_like();
    match rail {
        CursorProtocolRail::OAuthCli => id,
        CursorProtocolRail::ApiKeySdk => format!("agent-{id}"),
    }
}

struct ResolvedCursorSession {
    key: CursorSessionKey,
    parked: Option<CursorSessionReference>,
}

impl ResolvedCursorSession {
    fn new(key: CursorSessionKey) -> Self {
        Self { key, parked: None }
    }

    fn parked(reference: CursorSessionReference) -> Self {
        Self {
            key: reference.key().clone(),
            parked: Some(reference),
        }
    }
}

async fn open_agent_stream(
    state: &ServerState,
    credential: &CursorCredential,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    plan: &super::request_builder::AgentRunPlan,
    session_key: &CursorSessionKey,
    timeouts: CursorH2Timeouts,
) -> Result<Arc<tokio::sync::Mutex<CursorSession>>, ProxyError> {
    let images = load_images(plan.images.clone()).await?;
    let mut blob_store = HashMap::new();
    let mut input = AgentRunInput {
        rail: credential.rail(),
        model_id: &plan.model_id,
        user_text: &plan.user_text,
        conversation_id: Some(session_key.conversation_id()),
        message_id: None,
        tools: plan.tools.clone(),
        system_prompt: plan.system_prompt.as_deref(),
        blob_store: Some(&mut blob_store),
        images,
    };
    let body = encode_agent_run_request(&mut input).map_err(|message| {
        ProxyError::bad_request(format!("invalid Cursor AgentService model: {message}"))
    })?;
    let endpoint = cursor_agentservice_url(
        state,
        stored,
        runtime_fingerprint,
        credential,
        timeouts.request,
    )
    .await?;
    let entry = state
        .cursor_sessions
        .reserve(
            session_key.clone(),
            credential.rail(),
            blob_store,
            plan.tools.clone(),
            plan.response_input_items.clone(),
            plan.working_directory.clone(),
        )
        .await
        .map_err(|conflict| {
            let state = match conflict.state {
                SessionState::Running => "running",
                SessionState::AwaitingToolResult => "awaiting_tool_result",
                SessionState::Closed => "closed",
            };
            metrics::counter!("cursor_session_conflict_total", "state" => state).increment(1);
            ProxyError::cursor_conversation_busy(format!(
                "Cursor conversation is busy in state {:?}",
                conflict.state
            ))
        })?;
    {
        let mut session = entry.lock().await;
        session.custom_tool_names = plan.custom_tool_names.clone();
        session.response_tool_namespaces = plan.response_tool_namespaces.clone();
        if !plan.tool_results.is_empty() && plan.cold_resume_ready {
            session.cold_resume_completed_calls = plan
                .completed_tool_calls
                .iter()
                .map(|call| (call.name.clone(), call.arguments.clone()))
                .collect();
        }
    }
    let mut reservation_guard =
        CursorSessionReservationGuard::new(state.cursor_sessions.clone(), entry.clone());
    let stream = match CursorH2Stream::open(
        &endpoint,
        cursor_agentservice_headers(
            credential.rail(),
            credential.account(),
            credential.access_token(),
        ),
        wrap_connect_frame(&body),
        timeouts,
    )
    .await
    {
        Ok(stream) => stream,
        Err(error) => {
            record_cursor_agentservice_failure(
                "open_transport",
                "transport_or_session",
                credential.rail().label(),
            );
            state
                .cursor_sessions
                .release(entry.clone(), SessionState::Closed)
                .await;
            reservation_guard.disarm();
            return Err(error);
        }
    };
    if let Err(conflict) = state.cursor_sessions.attach_stream(&entry, stream).await {
        state
            .cursor_sessions
            .release(entry.clone(), SessionState::Closed)
            .await;
        reservation_guard.disarm();
        return Err(ProxyError::cursor_conversation_busy(format!(
            "Cursor conversation changed state during outbound open: {:?}",
            conflict.state
        )));
    }
    reservation_guard.disarm();
    tracing::debug!(
        cursor_rail = credential.rail().label(),
        cursor_protocol_revision = credential.rail().protocol_revision(),
        provider_id = %stored.provider.id,
        "opened Cursor AgentService stream"
    );
    Ok(entry)
}

async fn cursor_session_scope(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    request_context: &UsageLogContext,
) -> Result<CursorSessionScope, ProxyError> {
    let rail = CursorProtocolRail::for_provider(stored.provider_type).ok_or_else(|| {
        ProxyError::bad_request("Cursor session identity requires a Cursor provider")
    })?;
    let principal = match stored.provider_type {
        ProviderType::CursorOAuth => {
            let accounts = managed_credential_accounts_snapshot(state).await?;
            let account = authoritative_managed_account(stored, &accounts).ok_or_else(|| {
                ProxyError::conflict("Cursor OAuth account identity changed; rebind the Provider")
            })?;
            format!(
                "oauth:{}:{}:{}",
                account.id, account.auth_identity_generation, account.token_refresh_generation
            )
        }
        ProviderType::CursorApiKey => {
            let api_key = cursor_api_key(stored)?;
            let principal = stored
                .resource
                .cursor_verified_identity
                .as_ref()
                .map(|identity| identity.account_id.clone())
                .unwrap_or_else(|| cursor_api_key_hash(&api_key));
            format!(
                "apikey:{}:{}",
                principal, stored.resource.credential_generation
            )
        }
        _ => {
            return Err(ProxyError::bad_request(
                "Cursor session identity requires a Cursor provider",
            ));
        }
    };
    Ok(CursorSessionScope::derive(CursorSessionScopeInput {
        app: stored.app.as_str(),
        provider_id: &stored.provider.id,
        provider_revision: stored.resource.revision,
        runtime_fingerprint,
        rail,
        protocol_revision: rail.protocol_revision(),
        principal: &principal,
        share_id: request_context.share_id.as_deref(),
        user_email: request_context.user_email.as_deref(),
    }))
}

async fn cursor_affinity_conversation_id(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    request_context: &UsageLogContext,
    rail: CursorProtocolRail,
    working_directory: &str,
) -> Result<Option<String>, ProxyError> {
    let Some(session_id) = request_context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
    else {
        return Ok(None);
    };
    let principal = match stored.provider_type {
        ProviderType::CursorOAuth => {
            let accounts = managed_credential_accounts_snapshot(state).await?;
            let account = authoritative_managed_account(stored, &accounts).ok_or_else(|| {
                ProxyError::conflict("Cursor OAuth account identity changed; rebind the Provider")
            })?;
            // Deliberately exclude token_refresh_generation: token refresh is
            // not a conversation identity change. The live stream registry
            // still includes it in CursorSessionScope and remains fenced.
            format!("oauth:{}:{}", account.id, account.auth_identity_generation)
        }
        ProviderType::CursorApiKey => {
            let api_key = cursor_api_key(stored)?;
            let principal = stored
                .resource
                .cursor_verified_identity
                .as_ref()
                .map(|identity| identity.account_id.clone())
                .unwrap_or_else(|| cursor_api_key_hash(&api_key));
            format!(
                "apikey:{}:{}",
                principal, stored.resource.credential_generation
            )
        }
        _ => return Ok(None),
    };
    let provider_revision = stored.resource.revision.to_string();
    let share = request_context
        .share_id
        .as_deref()
        .unwrap_or("<direct-share>");
    let normalized_user = request_context
        .user_email
        .as_deref()
        .map(str::trim)
        .filter(|user| !user.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "<direct-user>".to_string());
    Ok(Some(cursor_affinity_id_from_components(
        rail,
        &[
            ("app", stored.app.as_str()),
            ("provider", stored.provider.id.as_str()),
            ("provider_revision", provider_revision.as_str()),
            ("runtime", runtime_fingerprint),
            ("rail", rail.label()),
            ("principal", principal.as_str()),
            ("share", share),
            ("user", normalized_user.as_str()),
            ("session", session_id),
            ("workspace", working_directory),
        ],
    )))
}

fn cursor_affinity_id_from_components(
    rail: CursorProtocolRail,
    components: &[(&str, &str)],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-server:cursor-conversation-affinity:v1\0");
    for (label, value) in components {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let id = uuid_like_from_bytes(bytes);
    match rail {
        CursorProtocolRail::OAuthCli => id,
        CursorProtocolRail::ApiKeySdk => format!("agent-{id}"),
    }
}

async fn cursor_completed_response_scope(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    request_context: &UsageLogContext,
    workspace_id: &str,
) -> Result<CursorResponseScope, ProxyError> {
    let rail = CursorProtocolRail::for_provider(stored.provider_type).ok_or_else(|| {
        ProxyError::bad_request("Cursor response identity requires a Cursor provider")
    })?;
    let credential_identity = match stored.provider_type {
        ProviderType::CursorOAuth => {
            let accounts = managed_credential_accounts_snapshot(state).await?;
            let account = authoritative_managed_account(stored, &accounts).ok_or_else(|| {
                ProxyError::conflict("Cursor OAuth account identity changed; rebind the Provider")
            })?;
            format!("oauth:{}:{}", account.id, account.auth_identity_generation)
        }
        ProviderType::CursorApiKey => {
            let api_key = cursor_api_key(stored)?;
            format!(
                "apikey:{}:{}",
                cursor_api_key_hash(&api_key),
                stored.resource.credential_generation
            )
        }
        _ => {
            return Err(ProxyError::bad_request(
                "Cursor response identity requires a Cursor provider",
            ))
        }
    };
    Ok(CursorResponseScope::derive(CursorResponseScopeInput {
        app: stored.app.as_str(),
        provider_id: &stored.provider.id,
        provider_revision: stored.resource.revision,
        runtime_fingerprint,
        rail: rail.label(),
        protocol_revision: rail.protocol_revision(),
        credential_identity: &credential_identity,
        share_id: request_context.share_id.as_deref(),
        user_email: request_context.user_email.as_deref(),
        workspace_id,
    }))
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid_like_from_bytes(bytes)
}

fn uuid_like_from_bytes(bytes: [u8; 16]) -> String {
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
    user_email: Option<String>,
    started: Instant,
    first_token_ms: Arc<AtomicU64>,
    session_entry: Option<Arc<tokio::sync::Mutex<CursorSession>>>,
    parked_handoff: bool,
}

impl CursorStreamInterruptGuard {
    fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }

    fn hand_off_parked_session(&mut self) {
        self.session_entry = None;
        self.parked_handoff = true;
    }

    fn first_token_ms(&self) -> Option<u128> {
        match self.first_token_ms.load(Ordering::Relaxed) {
            0 => None,
            value => Some(u128::from(value - 1)),
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
        let user_email = self.user_email.clone();
        let duration_ms = self.started.elapsed().as_millis();
        let first_token_ms = self.first_token_ms();
        let session_entry = self.session_entry.take();
        let (stream_status, provider_outcome) =
            cursor_stream_drop_classification(self.parked_handoff, status_code);
        tokio::spawn(async move {
            update_stream_usage(
                &state,
                &stored,
                &request_id,
                status_code,
                duration_ms,
                first_token_ms,
                TokenUsage::default(),
                Some(stream_status),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                TokenUsage::default(),
            )
            .await;
            record_provider_outcome(&state, &stored, provider_outcome).await;
            if let Some(entry) = session_entry {
                state
                    .cursor_sessions
                    .release(entry, SessionState::Closed)
                    .await;
            }
        });
    }
}

fn cursor_stream_drop_classification(
    parked_handoff: bool,
    status_code: u16,
) -> (&'static str, ProviderOutcome) {
    if parked_handoff {
        ("completed", ProviderOutcome::from_status(status_code))
    } else {
        ("interrupted", ProviderOutcome::NetworkFailure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn cursor_endpoint_provider(
        provider_type: ProviderType,
        endpoint_env: Value,
    ) -> StoredProvider {
        StoredProvider {
            app: crate::domain::providers::model::AppKind::Codex,
            provider: crate::domain::providers::model::Provider {
                id: format!("endpoint-{}", provider_type.as_str()),
                name: "Cursor endpoint test".to_string(),
                settings_config: json!({"env": endpoint_env}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    fn cursor_stream_test_state(name: &str) -> ServerState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir().join(format!("cc-switch-cursor-{name}-{nanos}")),
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

    #[test]
    fn cursor_business_output_classifier_excludes_all_control_frames() {
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::Heartbeat
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::ToolCallStarted
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::ToolCallCompleted
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::KvServerMessage
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::Unknown(99)
        ));
        assert!(!cursor_delta_is_business_output(&InteractionDelta::Text(
            String::new()
        )));

        assert!(cursor_delta_is_business_output(&InteractionDelta::Text(
            "answer".to_string()
        )));
        assert!(cursor_delta_is_business_output(
            &InteractionDelta::Thinking("reason".to_string())
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::ThinkingComplete
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::TokenDelta(0)
        ));
        assert!(!cursor_delta_is_business_output(
            &InteractionDelta::TurnEnded
        ));
    }

    #[test]
    fn cursor_business_output_classifier_is_independent_of_deferred_wire_events() {
        assert!(!cursor_events_are_business_output(
            true, false, false, false
        ));
        assert!(!cursor_events_are_business_output(true, false, true, true));
        assert!(cursor_events_are_business_output(true, true, false, true));
        assert!(cursor_events_are_business_output(true, false, false, true));
        assert!(!cursor_events_are_business_output(false, true, false, true));
    }

    #[test]
    fn cursor_first_output_requires_a_progressive_non_terminal_batch() {
        assert!(should_record_progressive_first_output(false, true, false));
        assert!(!should_record_progressive_first_output(true, true, false));
        assert!(!should_record_progressive_first_output(false, false, false));
        assert!(!should_record_progressive_first_output(false, true, true));
    }

    #[test]
    fn cursor_interrupted_first_output_encoding_preserves_zero_milliseconds() {
        assert_eq!(encode_optional_millis(0), 1);
        assert_eq!(u128::from(encode_optional_millis(42) - 1), 42);
        assert_eq!(encode_optional_millis(u128::MAX), u64::MAX);
    }

    #[test]
    fn cursor_business_completion_policy_is_rail_specific() {
        let mut sdk_writer = AgentSseWriter::new(
            "composer-2.5".to_string(),
            super::super::protocol::CursorResponseFormat::OpenAiChatCompletions,
            0,
        );
        sdk_writer.event(&AgentEvent::Text("answer".to_string()));
        assert!(cursor_kv_terminal_events(
            CursorProtocolRail::ApiKeySdk,
            true,
            &mut sdk_writer,
            &mut ComposerMarkerFilter::default(),
        )
        .unwrap()
        .is_none());

        let mut cli_writer = AgentSseWriter::new(
            "composer-2.5".to_string(),
            super::super::protocol::CursorResponseFormat::OpenAiChatCompletions,
            0,
        );
        assert!(cursor_kv_terminal_events(
            CursorProtocolRail::OAuthCli,
            true,
            &mut cli_writer,
            &mut ComposerMarkerFilter::default(),
        )
        .unwrap()
        .is_none());
        cli_writer.event(&AgentEvent::Text("answer".to_string()));
        assert!(cursor_kv_terminal_events(
            CursorProtocolRail::OAuthCli,
            true,
            &mut cli_writer,
            &mut ComposerMarkerFilter::default(),
        )
        .unwrap()
        .is_some());

        assert_eq!(
            cursor_incomplete_response_error(CursorProtocolRail::ApiKeySdk).status,
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            cursor_empty_response_error(CursorProtocolRail::OAuthCli).status,
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn cursor_endpoint_configuration_is_full_url_https_and_rail_scoped() {
        assert_eq!(
            validate_cursor_endpoint("https://cursor.example/rpc/run", "test endpoint").unwrap(),
            "https://cursor.example/rpc/run"
        );
        assert!(validate_cursor_endpoint("http://127.0.0.1:8787/rpc/run", "test endpoint").is_ok());
        for invalid in [
            "http://cursor.example/rpc/run",
            "https://cursor.example/",
            "https://user@cursor.example/rpc/run",
            "https://cursor.example/rpc/run#fragment",
        ] {
            let error = validate_cursor_endpoint(invalid, "test endpoint").unwrap_err();
            assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        }

        let oauth = cursor_endpoint_provider(
            ProviderType::CursorOAuth,
            json!({
                "CURSOR_OAUTH_AGENT_ENDPOINT": "http://127.0.0.1:8787/oauth/run"
            }),
        );
        validate_cursor_runtime_configuration(&oauth).unwrap();

        let api_key = cursor_endpoint_provider(
            ProviderType::CursorApiKey,
            json!({
                "CURSOR_APIKEY_AGENT_ENDPOINT": "http://127.0.0.1:8787/sdk/run",
                "CURSOR_APIKEY_EXCHANGE_ENDPOINT": "http://127.0.0.1:8787/token/exchange"
            }),
        );
        validate_cursor_runtime_configuration(&api_key).unwrap();
        let invalid_exchange = cursor_endpoint_provider(
            ProviderType::CursorApiKey,
            json!({
                "CURSOR_APIKEY_AGENT_ENDPOINT": "http://127.0.0.1:8787/sdk/run",
                "CURSOR_APIKEY_EXCHANGE_ENDPOINT": "not-a-url"
            }),
        );
        assert!(validate_cursor_runtime_configuration(&invalid_exchange).is_err());
    }

    #[test]
    fn cursor_preopen_auth_recovery_rekeys_the_session_without_changing_conversation() {
        let original_scope = CursorSessionScope::fixture("pre-refresh-generation");
        let refreshed_scope = CursorSessionScope::fixture("post-refresh-generation");
        let original = CursorSessionKey::new(original_scope, "conversation-stays-stable");

        let refreshed = rekey_cursor_session(&original, refreshed_scope.clone());

        assert_eq!(refreshed.conversation_id(), "conversation-stays-stable");
        assert_eq!(refreshed.scope(), &refreshed_scope);
        assert_ne!(refreshed.scope(), original.scope());
    }

    #[tokio::test]
    async fn cursor_stream_holds_account_lease_until_response_body_is_dropped() {
        let state = cursor_stream_test_state("stream-account-lease");
        let account_id = "cursor-stream-account";
        let account_in_flight_guard = state
            .account_in_flight
            .try_acquire(ProviderType::CursorOAuth, account_id, 1)
            .unwrap();
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-stream-provider".to_string(),
                name: "Cursor stream provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: ProviderType::CursorOAuth.as_str().to_string(),
            resource: Default::default(),
        };
        let session_entry = Arc::new(tokio::sync::Mutex::new(CursorSession {
            key: CursorSessionKey::new(
                CursorSessionScope::fixture("cursor-stream-share"),
                "cursor-stream-session",
            ),
            rail: CursorProtocolRail::OAuthCli,
            stream: None,
            declared_tool_names: Vec::new(),
            declared_tools: Vec::new(),
            custom_tool_names: Vec::new(),
            response_tool_namespaces: Vec::new(),
            semantic_items: Vec::new(),
            working_directory: "/workspace".to_string(),
            pending_tool_calls: HashMap::new(),
            cold_resume_completed_calls: Vec::new(),
            cold_resume_replay_rejections: 0,
            blob_store: HashMap::new(),
            state: SessionState::Running,
            last_activity: Instant::now(),
        }));
        let response = stream_response(
            state.clone(),
            stored,
            session_entry,
            CursorSessionKey::new(
                CursorSessionScope::fixture("cursor-stream-share"),
                "cursor-stream-session",
            ),
            super::super::protocol::CursorResponseFormat::AnthropicMessages,
            "claude-sonnet-4-6".to_string(),
            1,
            UsageLogContext::default(),
            Instant::now(),
            UsageModelMetadata::default(),
            Some(account_in_flight_guard),
            None,
            None,
        )
        .await;

        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::CursorOAuth, account_id),
            1
        );
        let mut body = response.into_body().into_data_stream();
        let first_chunk = tokio::time::timeout(std::time::Duration::from_secs(1), body.next())
            .await
            .unwrap()
            .expect("Cursor stream should emit its initial SSE frame")
            .unwrap();
        assert!(!first_chunk.is_empty());
        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::CursorOAuth, account_id),
            1
        );

        drop(body);
        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::CursorOAuth, account_id),
            0
        );
    }

    #[tokio::test]
    async fn cursor_agentservice_401_refreshes_once_then_cools_bound_account() {
        let state = cursor_stream_test_state("auth-recovery");
        let account_id = "cursor-auth-recovery-account";
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id.to_string()),
                    provider_type: ProviderType::CursorOAuth,
                    email: Some("cursor-auth-recovery@example.com".to_string()),
                    access_token: Some("cursor-old-access-token".to_string()),
                    refresh_token: Some("cursor-refresh-token".to_string()),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota_percent: None,
                    quota: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: None,
                    rate_limited_until: None,
                    last_refresh_error: None,
                });
            })
            .await
            .unwrap();
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-auth-recovery-provider".to_string(),
                name: "Cursor auth recovery provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some(ProviderType::CursorOAuth.as_str().to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some(ProviderType::CursorOAuth.as_str().to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: ProviderType::CursorOAuth.as_str().to_string(),
            resource: Default::default(),
        };
        let mut auth_recovery = AuthRecoveryState::default();

        assert_eq!(
            cursor_agentservice_auth_action(
                &state,
                &stored,
                "cursor-auth-runtime",
                Some("cursor-old-access-token"),
                &mut auth_recovery,
                StatusCode::UNAUTHORIZED,
                true,
            )
            .await,
            CursorAgentServiceAuthAction::RefreshAndReplaySameBinding
        );
        let after_first = state.find_account_by_id(account_id).await.unwrap();
        assert!(after_first.rate_limited_until.is_none());

        let before_cooldown = crate::infra::time::now_ms() as i64;
        assert_eq!(
            cursor_agentservice_auth_action(
                &state,
                &stored,
                "cursor-auth-runtime",
                Some("cursor-old-access-token"),
                &mut auth_recovery,
                StatusCode::UNAUTHORIZED,
                true,
            )
            .await,
            CursorAgentServiceAuthAction::UseResponse
        );
        let after_second = state.find_account_by_id(account_id).await.unwrap();
        assert!(after_second
            .rate_limited_until
            .is_some_and(|until| until > before_cooldown));
        assert!(after_second.last_refresh_error.is_none());
    }

    #[tokio::test]
    async fn cursor_agentservice_refresh_failure_cools_bound_oauth_account() {
        let state = cursor_stream_test_state("auth-recovery-failure");
        let config_dir = state.config_dir.clone();
        let account_id = "cursor-auth-recovery-failure-account";
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id.to_string()),
                    provider_type: ProviderType::CursorOAuth,
                    email: Some("cursor-auth-failure@example.com".to_string()),
                    access_token: Some("cursor-rejected-access-token".to_string()),
                    refresh_token: None,
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota_percent: None,
                    quota: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1),
                    rate_limited_until: None,
                    last_refresh_error: None,
                });
            })
            .await
            .unwrap();
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-auth-recovery-failure-provider".to_string(),
                name: "Cursor auth recovery failure provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some(ProviderType::CursorOAuth.as_str().to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some(ProviderType::CursorOAuth.as_str().to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: ProviderType::CursorOAuth.as_str().to_string(),
            resource: Default::default(),
        };

        let before_cooldown = crate::infra::time::now_ms() as i64;
        let error = recover_cursor_unauthorized(
            &state,
            &stored,
            "cursor-auth-runtime",
            Some("cursor-rejected-access-token"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        let account = state.find_account_by_id(account_id).await.unwrap();
        assert!(account
            .rate_limited_until
            .is_some_and(|until| until > before_cooldown));

        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn cursor_agentservice_cools_unbound_api_key_after_second_unauthorized() {
        let state = cursor_stream_test_state("apikey-auth-cooldown");
        let config_dir = state.config_dir.clone();
        let api_key = "cursor-unbound-auth-cooldown-key";
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-unbound-auth-cooldown-provider".to_string(),
                name: "Cursor unbound auth cooldown provider".to_string(),
                settings_config: json!({"env": {"CURSOR_API_KEY": api_key}}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some(ProviderType::CursorApiKey.as_str().to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorApiKey,
            provider_type_id: ProviderType::CursorApiKey.as_str().to_string(),
            resource: Default::default(),
        };
        let mut auth_recovery = AuthRecoveryState::default();

        assert_eq!(
            cursor_agentservice_auth_action(
                &state,
                &stored,
                "cursor-api-key-runtime",
                None,
                &mut auth_recovery,
                StatusCode::UNAUTHORIZED,
                true,
            )
            .await,
            CursorAgentServiceAuthAction::RefreshAndReplaySameBinding
        );
        assert_eq!(
            cursor_agentservice_auth_action(
                &state,
                &stored,
                "cursor-api-key-runtime",
                None,
                &mut auth_recovery,
                StatusCode::UNAUTHORIZED,
                true,
            )
            .await,
            CursorAgentServiceAuthAction::UseResponse
        );
        let scope = cursor_api_key_credential_scope(&stored, "cursor-api-key-runtime", api_key);
        let now = crate::infra::time::now_ms() as i64;
        assert!(state
            .cursor_api_key_tokens
            .auth_cooldown_until(&scope, now)
            .await
            .is_some_and(|until| until > now));
        let error = cached_cursor_api_key_token(
            &state,
            &stored,
            "cursor-api-key-runtime",
            api_key,
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);

        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn cursor_api_key_stale_binding_does_not_cool_an_oauth_account_on_429() {
        let state = cursor_stream_test_state("apikey-stale-binding-rate-limit");
        let config_dir = state.config_dir.clone();
        let account_id = "cursor-unrelated-oauth-account";
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id.to_string()),
                    provider_type: ProviderType::CursorOAuth,
                    email: Some("cursor-unrelated@example.com".to_string()),
                    access_token: Some("cursor-unrelated-access".to_string()),
                    refresh_token: Some("cursor-unrelated-refresh".to_string()),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota_percent: None,
                    quota: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: None,
                    rate_limited_until: None,
                    last_refresh_error: None,
                });
            })
            .await
            .unwrap();
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-api-key-stale-binding-provider".to_string(),
                name: "Cursor API key stale binding provider".to_string(),
                settings_config: json!({"env": {"CURSOR_API_KEY": "cursor-api-key"}}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some(ProviderType::CursorApiKey.as_str().to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("legacy".to_string()),
                        auth_provider: Some(ProviderType::CursorOAuth.as_str().to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorApiKey,
            provider_type_id: ProviderType::CursorApiKey.as_str().to_string(),
            resource: Default::default(),
        };

        maybe_mark_cursor_rate_limited(
            &state,
            &stored,
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            br#"{"error":"rate_limited"}"#,
        )
        .await;

        let account = state.find_account_by_id(account_id).await.unwrap();
        assert!(account.rate_limited_until.is_none());
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn cursor_429_does_not_cross_same_id_auth_identity_generation() {
        let state = cursor_stream_test_state("oauth-generation-rate-limit");
        let config_dir = state.config_dir.clone();
        let account_id = "cursor-generation-rate-limit-account";
        let account = state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id.to_string()),
                    provider_type: ProviderType::CursorOAuth,
                    email: Some("cursor-before@example.com".to_string()),
                    access_token: Some("cursor-generation-access".to_string()),
                    refresh_token: Some("cursor-generation-refresh".to_string()),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota_percent: None,
                    quota: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: None,
                    rate_limited_until: None,
                    last_refresh_error: None,
                })
            })
            .await
            .unwrap();
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Claude,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-generation-rate-limit-provider".to_string(),
                name: "Cursor generation rate limit provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some(ProviderType::CursorOAuth.as_str().to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("managed_account".to_string()),
                        auth_provider: Some(ProviderType::CursorOAuth.as_str().to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(account.auth_identity_generation),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: ProviderType::CursorOAuth.as_str().to_string(),
            resource: Default::default(),
        };
        state
            .mutate_accounts(|accounts| {
                accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == account_id)
                    .unwrap()
                    .auth_identity_generation += 1;
            })
            .await;

        maybe_mark_cursor_rate_limited(
            &state,
            &stored,
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            br#"{"error":"rate_limited"}"#,
        )
        .await;

        assert!(state
            .find_account_by_id(account_id)
            .await
            .unwrap()
            .rate_limited_until
            .is_none());
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn cursor_error_message_extracts_known_json_shapes() {
        assert_eq!(
            cursor_error_message_from_body(br#"{"error":"insufficient_quota"}"#).as_deref(),
            Some("insufficient_quota")
        );
        assert_eq!(
            cursor_error_message_from_body(
                br#"{"code":"internal","message":"upstream unavailable"}"#
            )
            .as_deref(),
            Some("internal: upstream unavailable")
        );
        assert_eq!(
            cursor_error_message_from_body(
                br#"{"details":[{"type":"cursor.CursorError","message":"quota exhausted"}]}"#
            )
            .as_deref(),
            Some("quota exhausted")
        );
        let redacted = cursor_error_message_from_body(
            br#"{"message":"request https://private.example/internal/run failed"}"#,
        )
        .unwrap();
        assert!(redacted.contains("[REDACTED_CURSOR_URL]"));
        assert!(!redacted.contains("private.example"));
        assert!(!redacted.contains("/internal/run"));
    }

    #[test]
    fn cursor_http_failures_have_low_cardinality_reasons() {
        assert_eq!(
            cursor_http_failure_reason(StatusCode::UNAUTHORIZED),
            "upstream_authentication"
        );
        assert_eq!(
            cursor_http_failure_reason(StatusCode::TOO_MANY_REQUESTS),
            "upstream_rate_limit"
        );
        assert_eq!(
            cursor_http_failure_reason(StatusCode::BAD_GATEWAY),
            "upstream_server"
        );
        assert_eq!(
            cursor_http_failure_reason(StatusCode::BAD_REQUEST),
            "upstream_request_rejected"
        );
    }

    #[test]
    fn cursor_rate_limit_body_parses_duration_and_absolute_reset() {
        let now = 1_700_000_000_000;
        assert_eq!(
            cursor_rate_limit_until_from_body(br#"{"error":{"retry_after_ms":2500}}"#, now),
            Some(now + 2_500)
        );
        assert_eq!(
            cursor_rate_limit_until_from_body(br#"{"retryAfterSeconds":3}"#, now),
            Some(now + 3_000)
        );
        assert_eq!(
            cursor_rate_limit_until_from_body(
                br#"{"rateLimited":{"resetAtMs":1700000060000}}"#,
                now
            ),
            Some(now + 60_000)
        );

        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("7"));
        assert_eq!(cursor_rate_limit_until(&headers, b"{}", now), now + 7_000);
    }

    #[test]
    fn cursor_error_body_accepts_json_or_missing_content_type_only() {
        let mut headers = HeaderMap::new();
        assert!(cursor_error_body_is_json_like(&headers));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(cursor_error_body_is_json_like(&headers));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/connect+proto"),
        );
        assert!(!cursor_error_body_is_json_like(&headers));
    }

    #[test]
    fn cursor_exchange_expiry_uses_shortest_upstream_evidence() {
        let now = 1_700_000_000_000;
        let expiry = cursor_exchange_expiry("not-a-jwt", Some(now + 600_000), Some(1_200), now);
        assert_eq!(expiry, now + 600_000);
        let expiry = cursor_exchange_expiry("not-a-jwt", Some(now + 600_000), Some(120), now);
        assert_eq!(expiry, now + 120_000);

        assert!(!new_cursor_conversation_id(CursorProtocolRail::OAuthCli).starts_with("agent-"));
        assert!(new_cursor_conversation_id(CursorProtocolRail::ApiKeySdk).starts_with("agent-"));
    }

    #[test]
    fn cursor_exchange_success_payload_failures_are_upstream_errors() {
        let error = parse_cursor_api_key_exchange_response(br#"{}"#, 1_700_000_000_000)
            .expect_err("missing exchange token must fail");
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);

        let error = parse_cursor_api_key_exchange_response(b"not-json", 1_700_000_000_000)
            .expect_err("malformed exchange payload must fail");
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);

        let (token, expires_at) = parse_cursor_api_key_exchange_response(
            br#"{"accessToken":"exchanged-token","expiresIn":120}"#,
            1_700_000_000_000,
        )
        .unwrap();
        assert_eq!(token, "exchanged-token");
        assert_eq!(expires_at, 1_700_000_120_000);
    }

    #[test]
    fn cursor_api_key_hash_never_contains_the_key() {
        let key = "cursor-secret-key";
        let hash = cursor_api_key_hash(key);
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(key));
    }

    #[test]
    fn response_compaction_has_a_dedicated_contract() {
        let body = response_compaction_body(
            &serde_json::to_vec(&json!({
                "id":"resp_summary",
                "object":"response",
                "output":[{
                    "type":"message",
                    "content":[{"type":"output_text","text":"Goal and pending work"}]
                }],
                "usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}
            }))
            .unwrap(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "response.compaction");
        assert_eq!(value["output"][0]["type"], "compaction");
        assert_eq!(
            value["output"][0]["encrypted_content"],
            "Goal and pending work"
        );
        assert_eq!(value["usage"]["total_tokens"], 14);
    }

    #[tokio::test]
    async fn cancelled_outbound_open_releases_the_session_reservation() {
        let manager = CursorSessionManager::default();
        let scope = CursorSessionScope::derive(CursorSessionScopeInput {
            app: "codex",
            provider_id: "cursor-provider",
            provider_revision: 1,
            runtime_fingerprint: "runtime-a",
            rail: CursorProtocolRail::ApiKeySdk,
            protocol_revision: CursorProtocolRail::ApiKeySdk.protocol_revision(),
            principal: "apikey:fixture:1",
            share_id: None,
            user_email: None,
        });
        let key = CursorSessionKey::new(scope, "conversation-a");
        let entry = manager
            .reserve(
                key,
                CursorProtocolRail::ApiKeySdk,
                HashMap::new(),
                Vec::new(),
                Vec::new(),
                String::new(),
            )
            .await
            .unwrap();
        let guard = CursorSessionReservationGuard::new(manager.clone(), entry);
        drop(guard);
        for _ in 0..8 {
            if manager.size().await == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(manager.size().await, 0);
    }

    #[test]
    fn named_tool_comparison_is_case_and_separator_insensitive_only() {
        assert!(tool_names_equal("read_file", "ReadFile"));
        assert!(!tool_names_equal("read_file", "write_file"));
    }

    #[test]
    fn cursor_affinity_is_stable_scoped_and_never_contains_the_client_session() {
        let components = [
            ("provider", "provider-a"),
            ("share", "share-a"),
            ("user", "user@example.com"),
            ("session", "private-cli-session"),
        ];
        let first = cursor_affinity_id_from_components(CursorProtocolRail::OAuthCli, &components);
        let second = cursor_affinity_id_from_components(CursorProtocolRail::OAuthCli, &components);
        assert_eq!(first, second);
        assert!(!first.contains("private-cli-session"));

        let other_share = [
            ("provider", "provider-a"),
            ("share", "share-b"),
            ("user", "user@example.com"),
            ("session", "private-cli-session"),
        ];
        assert_ne!(
            first,
            cursor_affinity_id_from_components(CursorProtocolRail::OAuthCli, &other_share)
        );
        assert!(
            cursor_affinity_id_from_components(CursorProtocolRail::ApiKeySdk, &components)
                .starts_with("agent-")
        );
    }

    #[test]
    fn parked_stream_handoff_is_not_classified_as_a_network_interruption() {
        assert_eq!(
            cursor_stream_drop_classification(true, 200),
            ("completed", ProviderOutcome::Success { status_code: 200 })
        );
        assert_eq!(
            cursor_stream_drop_classification(false, 200),
            ("interrupted", ProviderOutcome::NetworkFailure)
        );
    }
}

async fn resolve_cursor_credential(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    request_timeout: std::time::Duration,
) -> Result<CursorCredential, ProxyError> {
    match stored.provider_type {
        ProviderType::CursorOAuth => {
            let accounts = managed_credential_accounts_snapshot(state).await?;
            let account = authoritative_managed_account(stored, &accounts).ok_or_else(|| {
                ProxyError::conflict("Cursor OAuth account identity changed; rebind the Provider")
            })?;
            let stored_access_token = account
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request("Cursor OAuth managed account access token is required")
                })?;
            let access_token = normalize_cursor_access_token(stored_access_token).to_string();
            let endpoint_principal = format!(
                "{}:{}:{}",
                account.id, account.auth_identity_generation, account.token_refresh_generation
            );
            Ok(CursorCredential::OAuthCli {
                account: cursor_account_from_managed_account(account),
                access_token,
                endpoint_principal,
            })
        }
        ProviderType::CursorApiKey => {
            let api_key = cursor_api_key(stored)?;
            let access_token = cached_cursor_api_key_token(
                state,
                stored,
                runtime_fingerprint,
                &api_key,
                request_timeout,
            )
            .await?;
            let verified_account_id = stored
                .resource
                .cursor_verified_identity
                .as_ref()
                .map(|identity| identity.account_id.as_str());
            let account = cursor_account_for_api_key(&api_key, verified_account_id);
            let endpoint_principal = format!(
                "{}:{}",
                account.account_id, stored.resource.credential_generation
            );
            Ok(CursorCredential::ApiKeySdk {
                account,
                access_token,
                endpoint_principal,
            })
        }
        _ => Err(ProxyError::bad_request(
            "Cursor AgentService driver requires a Cursor provider",
        )),
    }
}

async fn recover_cursor_unauthorized(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    rejected_access_token: Option<&str>,
) -> Result<(), ProxyError> {
    let result = async {
        match stored.provider_type {
            ProviderType::CursorOAuth => {
                let (provider_type, account_id, expected_generation) =
                    managed_account_binding_with_generation(stored).ok_or_else(|| {
                        ProxyError::bad_request(
                            "Cursor OAuth provider must bind one explicit managed account",
                        )
                    })?;
                state
                    .refresh_managed_account_now_for_generation(
                        provider_type,
                        account_id,
                        expected_generation,
                    )
                    .await
                    .map_err(super::super::forwarder::managed_account_refresh_error_to_proxy_error)
            }
            ProviderType::CursorApiKey => {
                let api_key = cursor_api_key(stored)?;
                let scope = cursor_api_key_credential_scope(stored, runtime_fingerprint, &api_key);
                state.cursor_api_key_tokens.invalidate(&scope).await;
                Ok(())
            }
            _ => Err(ProxyError::bad_request(
                "Cursor unauthorized recovery requires a Cursor provider",
            )),
        }
    }
    .await;
    if result.is_err() {
        mark_cursor_agentservice_auth_cooldown(
            state,
            stored,
            runtime_fingerprint,
            rejected_access_token,
            "cursor_agentservice_refresh_failed",
        )
        .await;
    }
    result
}

async fn mark_cursor_agentservice_auth_cooldown(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    rejected_access_token: Option<&str>,
    source: &'static str,
) {
    if stored.provider_type == ProviderType::CursorOAuth {
        mark_managed_account_auth_cooldown_for_stored(state, stored, rejected_access_token, source)
            .await;
        return;
    }
    let api_key = cursor_api_key(stored).ok();
    let Some(api_key) = api_key else {
        return;
    };
    let scope = cursor_api_key_credential_scope(stored, runtime_fingerprint, &api_key);
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    state
        .cursor_api_key_tokens
        .mark_auth_cooldown(&scope, now.saturating_add(CURSOR_AUTH_FAILURE_COOLDOWN_MS))
        .await;
}

fn cursor_api_key(stored: &StoredProvider) -> Result<String, ProxyError> {
    stored
        .provider
        .settings_config
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
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
        })
        .ok_or_else(|| ProxyError::bad_request("Cursor API key is required"))
}

async fn exchange_cursor_api_key(
    state: &ServerState,
    stored: &StoredProvider,
    api_key: &str,
    request_timeout: std::time::Duration,
) -> Result<(String, i64), ProxyError> {
    let url = cursor_api_key_exchange_url(stored)?;
    let response = state
        .http_client()
        .await
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&json!({}))
        .timeout(request_timeout)
        .send()
        .await
        .map_err(|error| {
            ProxyError::bad_gateway(format!(
                "Cursor API key exchange request failed: {}",
                cursor_transport_diagnostic(&error)
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let headers = response.headers().clone();
        let body = read_reqwest_body_limited(response, MAX_CURSOR_ERROR_BODY_BYTES)
            .await
            .unwrap_or_default();
        let detail = cursor_error_message_from_body(&body)
            .map(|detail| crate::logging::redact_sensitive_text_with_values(&detail, [api_key]))
            .unwrap_or_else(|| "upstream rejected the exchange".to_string());
        let message = format!("Cursor API key exchange failed: {detail}");
        return match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(ProxyError { status, message }),
            StatusCode::TOO_MANY_REQUESTS => {
                let now = crate::infra::time::now_ms() as i64;
                let until = cursor_rate_limit_until(&headers, &body, now);
                let retry_after_seconds = u64::try_from(until.saturating_sub(now))
                    .unwrap_or(u64::MAX)
                    .saturating_add(999)
                    / 1_000;
                Err(ProxyError::rate_limited(message, retry_after_seconds))
            }
            _ => Err(ProxyError::bad_gateway(message)),
        };
    }
    let body = read_reqwest_body_limited(response, MAX_CURSOR_ERROR_BODY_BYTES).await?;
    parse_cursor_api_key_exchange_response(&body, crate::infra::time::now_ms() as i64)
}

fn parse_cursor_api_key_exchange_response(
    body: &[u8],
    now_ms: i64,
) -> Result<(String, i64), ProxyError> {
    let parsed = serde_json::from_slice::<CursorApiKeyExchangeResponse>(body)
        .map_err(ProxyError::bad_gateway)?;
    let access_token = parsed
        .access_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProxyError::bad_gateway("Cursor API key exchange response missing access token")
        })?;
    let expires_at =
        cursor_exchange_expiry(&access_token, parsed.expires_at, parsed.expires_in, now_ms);
    Ok((access_token, expires_at))
}

async fn cached_cursor_api_key_token(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    api_key: &str,
    request_timeout: std::time::Duration,
) -> Result<String, ProxyError> {
    let scope = cursor_api_key_credential_scope(stored, runtime_fingerprint, api_key);
    let now = crate::infra::time::now_ms() as i64;
    ensure_cursor_api_key_auth_cooldown_elapsed(state, &scope, now).await?;
    if let Some(token) = state.cursor_api_key_tokens.get(&scope, now).await {
        return Ok(token);
    }
    let _flight = state.cursor_api_key_tokens.lock(&scope).await;
    let now = crate::infra::time::now_ms() as i64;
    ensure_cursor_api_key_auth_cooldown_elapsed(state, &scope, now).await?;
    if let Some(token) = state.cursor_api_key_tokens.get(&scope, now).await {
        return Ok(token);
    }
    let (token, expires_at_ms) =
        exchange_cursor_api_key(state, stored, api_key, request_timeout).await?;
    state
        .cursor_api_key_tokens
        .insert(scope, token.clone(), expires_at_ms)
        .await;
    Ok(token)
}

async fn ensure_cursor_api_key_auth_cooldown_elapsed(
    state: &ServerState,
    scope: &CursorApiKeyCredentialScope,
    now_ms: i64,
) -> Result<(), ProxyError> {
    let Some(until_ms) = state
        .cursor_api_key_tokens
        .auth_cooldown_until(scope, now_ms)
        .await
    else {
        return Ok(());
    };
    let retry_after_seconds = u64::try_from(until_ms.saturating_sub(now_ms))
        .unwrap_or(u64::MAX)
        .saturating_add(999)
        / 1_000;
    Err(ProxyError::rate_limited(
        "Cursor API key is cooling down after repeated upstream authentication failures",
        retry_after_seconds.max(1),
    ))
}

fn cursor_api_key_credential_scope(
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    api_key: &str,
) -> CursorApiKeyCredentialScope {
    CursorApiKeyCredentialScope::derive(
        stored.app.as_str(),
        &stored.provider.id,
        stored.resource.revision,
        stored.resource.credential_generation,
        runtime_fingerprint,
        &cursor_api_key_hash(api_key),
    )
}

async fn read_reqwest_body_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Bytes, ProxyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(ProxyError::bad_gateway(
            "Cursor upstream response exceeded the configured limit",
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            ProxyError::bad_gateway(format!(
                "Cursor upstream response read failed: {}",
                cursor_transport_diagnostic(&error)
            ))
        })?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(ProxyError::bad_gateway(
                "Cursor upstream response exceeded the configured limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn cursor_api_key_hash(api_key: &str) -> String {
    hex::encode(Sha256::digest(api_key.as_bytes()))
}

fn cursor_exchange_expiry(
    token: &str,
    expires_at: Option<i64>,
    expires_in: Option<i64>,
    now_ms: i64,
) -> i64 {
    let absolute = expires_at.map(normalize_epoch_ms);
    let relative = expires_in
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)));
    let jwt = jwt_expiry_ms(token);
    absolute
        .into_iter()
        .chain(relative)
        .chain(jwt)
        .min()
        .unwrap_or_else(|| now_ms.saturating_add(10 * 60 * 1000))
        .max(now_ms)
}

fn normalize_epoch_ms(value: i64) -> i64 {
    if value < 10_000_000_000 {
        value.saturating_mul(1000)
    } else {
        value
    }
}

fn jwt_expiry_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value.get("exp")?.as_i64().map(normalize_epoch_ms)
}

fn cursor_api_key_exchange_url(stored: &StoredProvider) -> Result<String, ProxyError> {
    configured_cursor_endpoint_with_default(
        stored,
        &["CURSOR_APIKEY_EXCHANGE_ENDPOINT"],
        &["CC_SWITCH_CURSOR_APIKEY_EXCHANGE_ENDPOINT"],
        "Cursor API key exchange endpoint",
        DEFAULT_API_KEY_EXCHANGE_URL,
    )
}

pub(super) fn validate_cursor_runtime_configuration(
    stored: &StoredProvider,
) -> Result<(), ProxyError> {
    let rail = CursorProtocolRail::for_provider(stored.provider_type).ok_or_else(|| {
        cursor_configuration_error("Cursor AgentService driver requires a Cursor provider")
    })?;
    cursor_agentservice_override(stored, rail)?;
    cursor_server_config_url(stored)?;
    if rail == CursorProtocolRail::ApiKeySdk {
        cursor_api_key_exchange_url(stored)?;
    }
    Ok(())
}

async fn cursor_agentservice_url(
    state: &ServerState,
    stored: &StoredProvider,
    runtime_fingerprint: &str,
    credential: &CursorCredential,
    request_timeout: std::time::Duration,
) -> Result<String, ProxyError> {
    let rail = credential.rail();
    if let Some(endpoint) = cursor_agentservice_override(stored, rail)? {
        return Ok(endpoint);
    }
    let scope = CursorAgentEndpointScope::derive(
        stored.app.as_str(),
        &stored.provider.id,
        stored.resource.revision,
        stored.resource.credential_generation,
        runtime_fingerprint,
        rail,
        credential.endpoint_principal(),
        credential.access_token(),
    );
    resolve_cursor_agent_endpoint(
        &state.http_client().await,
        &state.cursor_agent_endpoints,
        CursorAgentEndpointRequest {
            scope,
            access_token: credential.access_token(),
            rail,
            account: credential.account(),
            discovery_url: &cursor_server_config_url(stored)?,
            request_timeout,
        },
    )
    .await
}

fn cursor_agentservice_override(
    stored: &StoredProvider,
    rail: CursorProtocolRail,
) -> Result<Option<String>, ProxyError> {
    match rail {
        CursorProtocolRail::OAuthCli => configured_cursor_endpoint_override(
            stored,
            &["CURSOR_OAUTH_AGENT_ENDPOINT"],
            &["CC_SWITCH_CURSOR_OAUTH_AGENT_ENDPOINT"],
            "Cursor OAuth CLI AgentService endpoint",
        ),
        CursorProtocolRail::ApiKeySdk => configured_cursor_endpoint_override(
            stored,
            &["CURSOR_APIKEY_AGENT_ENDPOINT"],
            &["CC_SWITCH_CURSOR_APIKEY_AGENT_ENDPOINT"],
            "Cursor API key SDK AgentService endpoint",
        ),
    }
}

fn cursor_server_config_url(stored: &StoredProvider) -> Result<String, ProxyError> {
    configured_cursor_endpoint_with_default(
        stored,
        &["CURSOR_SERVER_CONFIG_ENDPOINT"],
        &["CC_SWITCH_CURSOR_SERVER_CONFIG_ENDPOINT"],
        "Cursor ServerConfig discovery endpoint",
        default_cursor_server_config_url(),
    )
}

fn configured_cursor_endpoint_with_default(
    stored: &StoredProvider,
    setting_names: &[&str],
    env_names: &[&str],
    label: &str,
    default: &str,
) -> Result<String, ProxyError> {
    Ok(
        configured_cursor_endpoint_override(stored, setting_names, env_names, label)?
            .unwrap_or_else(|| default.to_string()),
    )
}

fn configured_cursor_endpoint_override(
    stored: &StoredProvider,
    setting_names: &[&str],
    env_names: &[&str],
    label: &str,
) -> Result<Option<String>, ProxyError> {
    #[cfg(test)]
    let provider_override = setting(&stored.provider, setting_names);
    #[cfg(not(test))]
    let provider_override: Option<String> = {
        let _ = (stored, setting_names);
        None
    };
    let value = provider_override.or_else(|| {
        env_names.iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
    });
    value
        .map(|value| validate_cursor_endpoint(&value, label))
        .transpose()
}

fn validate_cursor_endpoint(value: &str, label: &str) -> Result<String, ProxyError> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| cursor_configuration_error(format!("invalid {label}: {error}")))?;
    let test_loopback = cfg!(test)
        && url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
    if url.scheme() != "https" && !test_loopback {
        return Err(cursor_configuration_error(format!(
            "{label} must use HTTPS"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(cursor_configuration_error(format!(
            "{label} must not include userinfo or a fragment"
        )));
    }
    if url.path().is_empty() || url.path() == "/" {
        return Err(cursor_configuration_error(format!(
            "{label} must be a complete endpoint URL including its path"
        )));
    }
    Ok(url.to_string())
}

fn cursor_configuration_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: message.into(),
    }
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
