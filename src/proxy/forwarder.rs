use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::header::{ACCEPT, CONNECTION, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::stream::{self, BoxStream};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::CapacityError;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Error as TungsteniteError;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use crate::domain::accounts::store::AccountStore;
use crate::domain::health::ProviderRequestOutcome as ProviderOutcome;
use crate::domain::providers::current_provider;
use crate::domain::providers::model::{AppKind, CodexImageToolStripPolicy, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::shares::{ShareInvocationRejection, ShareRejectReason, ShareStore};
use crate::domain::usage::store::{TokenUsage, UsageLogContext, UsageModelMetadata};
use crate::infra::time::now_ms as current_time_ms;
use crate::state::{
    AccountInFlightGuard, AccountInFlightSnapshot, CopilotUpstreamAuthError, DeepSeekUpstreamError,
    ManagedAccountRefreshError, ServerState, ShareInFlightGuard,
};

use super::adapters::{self, ProviderAdapter};
use super::claude_oauth::ClaudeBodyRetryStage;
use super::cursor;
use super::deepseek;
use super::kiro;
use super::provider_ops::{ProviderExecution, ProviderOperation};
use super::request_governance::{
    content_encoding_value, decode_request_body_for_proxy, decode_response_body_for_proxy,
    ResponseDecodeResult,
};
use super::router::{
    account_concurrency_for_provider, ensure_provider_account_does_not_need_relogin,
    ensure_provider_account_usage_available, provider_supports_claude_count_tokens,
    select_failover_provider, select_provider, select_provider_for_claude_count_tokens,
    select_provider_for_codex_image_generation, select_provider_with_account_inflight, ProxyRoute,
};
use super::streaming::{ClaudeSseErrorDetector, StreamUsageAccumulator};
use super::usage::{log_usage, update_stream_usage};
use super::{setting, ProxyError};

const CODEX_IMAGES_RESPONSES_MAIN_MODEL: &str = "gpt-5.4-mini";
const CODEX_IMAGES_DEFAULT_TOOL_MODEL: &str = "gpt-image-2";
const MAX_CLAUDE_RETRY_ATTEMPTS: u32 = 3;
const MAX_CLAUDE_RETRY_ELAPSED_MS: u128 = 10_000;
const DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS: i64 = 60_000;

#[derive(Debug, Clone)]
struct ForwardAttemptContext {
    attempt: u32,
    started_at_ms: u128,
    body_retry_stage: Option<ClaudeBodyRetryStage>,
    execution: Option<ProviderExecution>,
    auth_refresh_attempted: bool,
    excluded_provider_ids: BTreeSet<String>,
}

impl Default for ForwardAttemptContext {
    fn default() -> Self {
        Self {
            attempt: 0,
            started_at_ms: current_time_ms(),
            body_retry_stage: None,
            execution: None,
            auth_refresh_attempted: false,
            excluded_provider_ids: BTreeSet::new(),
        }
    }
}

impl ForwardAttemptContext {
    fn retry_allowed(&self) -> bool {
        self.attempt < MAX_CLAUDE_RETRY_ATTEMPTS
            && current_time_ms().saturating_sub(self.started_at_ms) < MAX_CLAUDE_RETRY_ELAPSED_MS
    }

    fn next(
        &self,
        execution: &ProviderExecution,
        body_retry_stage: Option<ClaudeBodyRetryStage>,
    ) -> Self {
        let mut next = self.clone();
        next.attempt = next.attempt.saturating_add(1);
        next.body_retry_stage = body_retry_stage;
        next.execution = Some(execution.clone());
        next
    }

    fn after_auth_refresh(&self, execution: &ProviderExecution) -> Self {
        let mut next = self.next(execution, self.body_retry_stage);
        next.auth_refresh_attempted = true;
        next
    }

    fn after_provider_failover(
        &self,
        failed: &ProviderExecution,
        next_execution: &ProviderExecution,
    ) -> Self {
        let mut next = self.next(next_execution, self.body_retry_stage);
        next.excluded_provider_ids
            .insert(failed.stored.provider.id.clone());
        next.auth_refresh_attempted = false;
        next
    }
}

pub async fn forward(
    state: ServerState,
    route: ProxyRoute,
    gemini_path: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    forward_with_attempt(
        state,
        route,
        gemini_path,
        headers,
        body,
        ForwardAttemptContext::default(),
    )
    .await
}

async fn forward_with_attempt(
    state: ServerState,
    route: ProxyRoute,
    gemini_path: Option<String>,
    headers: HeaderMap,
    body: Bytes,
    mut attempt_context: ForwardAttemptContext,
) -> Result<Response, ProxyError> {
    let raw_body_for_retry = body;
    let retry_gemini_path = gemini_path;
    'attempt: loop {
        let gemini_path = retry_gemini_path.clone();
        let body = decode_request_body_for_proxy(&headers, raw_body_for_retry.clone())?;
        let app = route.app();
        let claude_body_retry_stage = attempt_context.body_retry_stage;
        let mut request_context = request_context_from_headers(&headers);
        request_context.session_id = session_id_from_request(route, &headers, &body);
        let share_invocation_guard = if let Some(share_id) = request_context.share_id.clone() {
            let (share_name, guard) = validate_and_acquire_share_invocation(
                &state,
                &share_id,
                request_context.user_email.as_deref(),
            )
            .await?;
            request_context.share_name = Some(share_name);
            Some(guard)
        } else {
            None
        };
        let accounts_for_selection = state.accounts_snapshot().await;
        let (execution, account_in_flight_guard) =
            if let Some(execution) = attempt_context.execution.clone() {
                execution.ensure_operation_supported(ProviderOperation::Forward)?;
                let snapshot = state.account_in_flight.snapshot();
                let guard = acquire_account_in_flight(
                    &state,
                    &execution.stored,
                    &accounts_for_selection,
                    &snapshot,
                )?;
                (execution, guard)
            } else {
                let shares = state.shares.read().await.clone();
                let providers = state.providers.read().await;
                let ui_settings = state.ui_settings.read().await.for_frontend();
                let configured_provider_id =
                    current_provider::resolve_current_provider_id(&providers, &ui_settings, app);
                if let Some(share_id) = request_context.share_id.as_deref() {
                    let (execution, _share_name) = select_share_execution(
                        &providers,
                        &shares,
                        &accounts_for_selection,
                        app,
                        share_id,
                    )?;
                    if route == ProxyRoute::ClaudeCountTokens
                        && !provider_supports_claude_count_tokens(&execution.stored)
                    {
                        return Err(ProxyError::bad_request(
                            "Claude count_tokens requires a native Anthropic provider",
                        ));
                    }
                    let snapshot = state.account_in_flight.snapshot();
                    let guard = acquire_account_in_flight(
                        &state,
                        &execution.stored,
                        &accounts_for_selection,
                        &snapshot,
                    )?;
                    (execution, guard)
                } else {
                    let mut attempts = 0;
                    loop {
                        let snapshot = state.account_in_flight.snapshot();
                        let execution = if route == ProxyRoute::ClaudeCountTokens {
                            select_provider_for_claude_count_tokens(
                                &providers,
                                &accounts_for_selection,
                                &headers,
                                configured_provider_id.as_deref(),
                                &snapshot,
                            )
                        } else {
                            select_provider_with_account_inflight(
                                &providers,
                                &accounts_for_selection,
                                app,
                                &headers,
                                configured_provider_id.as_deref(),
                                &snapshot,
                            )
                        }?
                        .execution;
                        match try_acquire_account_in_flight(
                            &state,
                            &execution.stored,
                            &accounts_for_selection,
                            &snapshot,
                        ) {
                            AccountInFlightAcquire::Acquired(guard) => {
                                break (execution, Some(guard));
                            }
                            AccountInFlightAcquire::NotManaged => break (execution, None),
                            AccountInFlightAcquire::Saturated => {
                                attempts += 1;
                                if attempts >= 3 {
                                    return Err(account_concurrency_proxy_error(&execution.stored));
                                }
                            }
                        }
                    }
                }
            };
        let stored = execution.runtime_stored_view();
        validate_codex_allowed_client(
            &stored,
            route,
            &headers,
            request_context.share_id.is_some(),
        )?;
        let started = Instant::now();
        if execution.driver_is("special.cursor") && cursor::agentservice_driver_requested(&stored) {
            let adapter_request = adapters::cursor_agentservice_request(
                body,
                &stored,
                route,
                gemini_path.as_deref(),
            )?;
            let mut adapter_request = adapter_request;
            execution.enforce_model_policy(&mut adapter_request)?;
            refresh_execution_managed_account_if_needed(&state, &execution).await?;
            let accounts = state.accounts_snapshot().await;
            execution.materialize_auth(&accounts)?;
            return cursor::forward_agentservice(cursor::AgentServiceForwardOptions {
                state,
                route,
                stored,
                adapter_request,
                request_context,
                share_invocation_guard,
            })
            .await;
        }
        if app == AppKind::Claude && execution.driver_is("special.kiro") {
            return forward_claude_kiro(ClaudeKiroForwardOptions {
                state,
                execution,
                stored,
                headers,
                body,
                request_context,
                share_invocation_guard,
                started,
            })
            .await;
        }
        if app == AppKind::Claude && execution.driver_is("special.deepseek_account") {
            return forward_claude_deepseek(ClaudeDeepSeekForwardOptions {
                state,
                execution,
                stored,
                body,
                request_context,
                share_invocation_guard,
                started,
            })
            .await;
        }
        let adapter = adapters::adapter_for(app, stored.provider_type);
        let codex_oauth_session_id = request_context
            .session_id
            .as_deref()
            .and_then(codex_oauth_upstream_session_id);
        let gemini_path_for_request = gemini_path.clone();
        let copilot_metadata = adapters::CopilotRequestMetadata {
            has_anthropic_beta: headers.contains_key("anthropic-beta"),
            session_id: request_context.session_id.clone(),
        };
        let adapter_request = adapter.transform_request_for_route_with_metadata(
            body,
            &stored,
            route,
            gemini_path_for_request.as_deref(),
            &copilot_metadata,
        )?;
        let mut adapter_request = adapter_request;
        if execution.driver_is("oauth.openai_codex")
            && matches!(
                route,
                ProxyRoute::CodexResponses
                    | ProxyRoute::CodexResponsesCompact
                    | ProxyRoute::CodexChatCompletions
            )
        {
            let compact_request = route == ProxyRoute::CodexResponsesCompact
                || (route == ProxyRoute::CodexResponses
                    && codex_responses_body_has_compaction_trigger(&adapter_request.body));
            if compact_request {
                adapter_request.body =
                    normalize_codex_oauth_compact_body_bytes(&adapter_request.body)?;
                adapter_request.stream_requested = false;
            } else {
                adapter_request.body = normalize_codex_oauth_responses_body_bytes(
                    &adapter_request.body,
                    codex_oauth_session_id.as_deref(),
                    codex_image_tool_strip_policy(&stored),
                )?;
            }
        }
        execution.enforce_model_policy(&mut adapter_request)?;
        let grok_contract = if execution.driver_is("oauth.grok_responses") {
            let cli_profile = grok_cli_profile(app, &stored);
            let tenant_scope = grok_tenant_scope(&request_context, &stored);
            let contract = super::grok::apply_forward_contract(
                &mut adapter_request.body,
                &headers,
                route,
                request_context.session_id.as_deref(),
                tenant_scope.as_deref(),
                cli_profile,
            )?;
            if request_context.session_id.is_none() {
                request_context.session_id = contract.session_id.clone();
            }
            if adapter_request.actual_model.as_deref() != Some(contract.actual_model.as_str()) {
                adapter_request.actual_model_source = Some("grok_model_normalization".to_string());
            }
            adapter_request.model = Some(contract.actual_model.clone());
            adapter_request.actual_model = Some(contract.actual_model.clone());
            Some(contract)
        } else {
            None
        };
        let (mut adapter_request, url, target_headers) =
            if execution.driver_is("oauth.claude_messages") {
                refresh_execution_managed_account_if_needed(&state, &execution).await?;
                let accounts = state.accounts_snapshot().await;
                let prepared = execution.finalize_claude_request(
                    adapter_request,
                    route,
                    &headers,
                    &accounts,
                    claude_body_retry_stage,
                )?;
                if request_context.session_id.is_none() {
                    request_context.session_id = prepared.session_id.clone();
                }
                (
                    prepared.adapter_request,
                    prepared.endpoint,
                    prepared.headers,
                )
            } else {
                execution.enforce_model_policy(&mut adapter_request)?;
                execution.finalize_request(&mut adapter_request)?;
                let mut url = execution.resolve_endpoint(route, gemini_path, &adapter_request)?;
                if execution.driver_is("oauth.grok_responses") {
                    url = super::grok::chat_upstream_url(&url, grok_cli_profile(app, &stored));
                }
                if execution.driver_is("oauth.openai_codex")
                    && route == ProxyRoute::CodexResponses
                    && codex_responses_body_has_compaction_trigger(&adapter_request.body)
                {
                    url = codex_compact_url(&url);
                }
                refresh_execution_managed_account_if_needed(&state, &execution).await?;
                let copilot_upstream_auth = if execution.driver_is("special.copilot") {
                    Some(
                        state
                            .prepare_copilot_upstream_auth(execution.managed_account_id())
                            .await
                            .map_err(copilot_upstream_auth_error_to_proxy_error)?,
                    )
                } else {
                    None
                };
                let accounts = state.accounts_snapshot().await;
                let mut target_headers = adapter.build_headers(app, &stored, &accounts)?;
                target_headers.extend(adapter_request.upstream_headers.iter().cloned());
                if execution.driver_is("oauth.openai_codex") {
                    append_codex_oauth_session_headers(
                        &mut target_headers,
                        codex_oauth_session_id.as_deref(),
                    );
                }
                if let Some(contract) = grok_contract {
                    for (name, value) in contract.headers {
                        replace_or_push_header(&mut target_headers, name, value);
                    }
                }
                if route == ProxyRoute::ClaudeCountTokens {
                    super::claude_oauth::normalize_count_tokens_body(&mut adapter_request.body)?;
                    adapter_request.stream_requested = false;
                    replace_or_push_header(
                        &mut target_headers,
                        "anthropic-beta",
                        "token-counting-2024-11-01".to_string(),
                    );
                }
                if let Some(auth) = copilot_upstream_auth {
                    url = super::join_url(&auth.api_endpoint, "/chat/completions");
                    replace_or_push_header(
                        &mut target_headers,
                        "authorization",
                        format!("Bearer {}", auth.token),
                    );
                }
                if execution.driver_is("oauth.openai_codex") {
                    crate::codex_identity::finalize_headers(&mut target_headers);
                }
                let mut target_headers = owned_headers(target_headers);
                let materialized_auth = execution.materialize_auth(&accounts)?;
                execution.apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())?;
                apply_account_header_overrides(&mut target_headers, &stored, &accounts)?;
                if route == ProxyRoute::ClaudeCountTokens {
                    replace_or_push_owned_header(
                        &mut target_headers,
                        "anthropic-beta".to_string(),
                        "token-counting-2024-11-01".to_string(),
                    );
                }
                (adapter_request, url, target_headers)
            };

        let http_client = forward_http_client(&state, &stored).await?;
        let request = build_upstream_post_request(
            &http_client,
            &url,
            adapter_request.body.clone(),
            &headers,
            &target_headers,
            execution.request_timeout(),
            adapter_request.stream_requested,
        );

        let upstream_result = if adapter_request.stream_requested {
            match execution.stream_first_byte_timeout() {
                Some(timeout) => match tokio::time::timeout(timeout, request.send()).await {
                    Ok(result) => result,
                    Err(_) => {
                        record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure)
                            .await;
                        if let Some(next_attempt) = next_claude_transport_attempt(
                            &state,
                            route,
                            &headers,
                            &request_context,
                            &attempt_context,
                            &execution,
                            "send_timeout",
                        )
                        .await
                        {
                            attempt_context = next_attempt;
                            drop(account_in_flight_guard);
                            drop(share_invocation_guard);
                            continue 'attempt;
                        }
                        return Err(ProxyError {
                            status: StatusCode::GATEWAY_TIMEOUT,
                            message: format!(
                                "proxy upstream streaming first byte timeout after {}ms",
                                timeout.as_millis()
                            ),
                        });
                    }
                },
                None => request.send().await,
            }
        } else {
            request.send().await
        };
        let mut upstream = match upstream_result {
            Ok(upstream) => upstream,
            Err(error) => {
                record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                if route == ProxyRoute::ClaudeCountTokens {
                    crate::metrics::record_claude_count_tokens_outcome("network_error");
                }
                if let Some(next_attempt) = next_claude_transport_attempt(
                    &state,
                    route,
                    &headers,
                    &request_context,
                    &attempt_context,
                    &execution,
                    "send_error",
                )
                .await
                {
                    attempt_context = next_attempt;
                    drop(account_in_flight_guard);
                    drop(share_invocation_guard);
                    continue 'attempt;
                }
                return Err(ProxyError::bad_gateway(error));
            }
        };
        let mut status = upstream.status();
        let mut status_code = status.as_u16();
        let mut response_headers = upstream.headers().clone();
        strip_hop_by_hop_response_headers(&mut response_headers);
        if matches!(
            route,
            ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
        ) && execution.driver_is("oauth.claude_messages")
            && status == StatusCode::UNAUTHORIZED
            && !attempt_context.auth_refresh_attempted
            && attempt_context.retry_allowed()
        {
            if let Some((provider_type, account_id)) = execution.managed_account_target() {
                state
                    .refresh_managed_account_now(provider_type, account_id)
                    .await
                    .map_err(managed_account_refresh_error_to_proxy_error)?;
                if route == ProxyRoute::ClaudeCountTokens {
                    crate::metrics::record_claude_count_tokens_outcome("auth_refresh");
                }
                crate::metrics::record_claude_retry("auth", "unauthorized");
                attempt_context = attempt_context.after_auth_refresh(&execution);
                drop(upstream);
                drop(account_in_flight_guard);
                drop(share_invocation_guard);
                continue 'attempt;
            }
        }
        if matches!(
            route,
            ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
        ) && execution.driver_is("oauth.claude_messages")
            && status == StatusCode::UNAUTHORIZED
            && attempt_context.auth_refresh_attempted
            && !claude_request_is_provider_pinned(&headers, &request_context)
        {
            if let Some(next_attempt) = next_claude_provider_failover(
                &state,
                route,
                &attempt_context,
                &execution,
                "unauthorized_after_refresh",
            )
            .await
            {
                record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code))
                    .await;
                attempt_context = next_attempt;
                drop(upstream);
                drop(account_in_flight_guard);
                drop(share_invocation_guard);
                continue 'attempt;
            }
        }
        if matches!(
            route,
            ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
        ) && status.as_u16() == 529
            && !claude_request_is_provider_pinned(&headers, &request_context)
        {
            if let Some(next_attempt) = next_claude_provider_failover(
                &state,
                route,
                &attempt_context,
                &execution,
                "http_529",
            )
            .await
            {
                record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code))
                    .await;
                attempt_context = next_attempt;
                drop(upstream);
                drop(account_in_flight_guard);
                drop(share_invocation_guard);
                continue 'attempt;
            }
        }
        maybe_update_grok_entitlement(&state, &stored, &response_headers).await;
        maybe_mark_grok_cooldown(&state, &stored, status, &response_headers).await;
        let mut content_type = response_headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut content_encoding = content_encoding_value(&response_headers);

        if codex_image_tool_strip_policy(&stored) == CodexImageToolStripPolicy::OnError
            && execution.driver_is("oauth.openai_codex")
            && matches!(
                route,
                ProxyRoute::CodexResponses | ProxyRoute::CodexChatCompletions
            )
            && !status.is_success()
            && status != StatusCode::TOO_MANY_REQUESTS
        {
            let original_bytes = upstream.bytes().await.map_err(ProxyError::bad_gateway)?;
            let original_decoded =
                decode_response_body_for_proxy(&response_headers, original_bytes);
            if codex_image_tool_rejection_body(&original_decoded.body) {
                if let Some(retry_body) =
                    codex_image_tool_stripped_body_bytes(&adapter_request.body)?
                {
                    let retry_request = build_upstream_post_request(
                        &http_client,
                        &url,
                        retry_body.clone(),
                        &headers,
                        &target_headers,
                        execution.request_timeout(),
                        adapter_request.stream_requested,
                    );
                    match retry_request.send().await {
                        Ok(retry_upstream) => {
                            adapter_request.body = retry_body;
                            upstream = retry_upstream;
                            status = upstream.status();
                            status_code = status.as_u16();
                            response_headers = upstream.headers().clone();
                            strip_hop_by_hop_response_headers(&mut response_headers);
                            maybe_update_grok_entitlement(&state, &stored, &response_headers).await;
                            maybe_mark_grok_cooldown(&state, &stored, status, &response_headers)
                                .await;
                            content_type = response_headers
                                .get(CONTENT_TYPE)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string);
                            content_encoding = content_encoding_value(&response_headers);
                        }
                        Err(_) => {
                            record_provider_outcome(
                                &state,
                                &stored,
                                provider_outcome_from_status(status_code),
                            )
                            .await;
                            log_usage(
                                &state,
                                &stored,
                                status_code,
                                started.elapsed().as_millis(),
                                model_metadata(&adapter_request),
                                TokenUsage::default(),
                                UsageLogContext {
                                    is_streaming: adapter_request.stream_requested,
                                    stream_status: adapter_request
                                        .stream_requested
                                        .then(|| "image_tool_retry_failed".to_string()),
                                    ..request_context
                                },
                            )
                            .await;
                            return Ok(decoded_upstream_response(
                                status,
                                &response_headers,
                                content_type,
                                content_encoding,
                                original_decoded,
                            ));
                        }
                    }
                } else {
                    record_provider_outcome(
                        &state,
                        &stored,
                        provider_outcome_from_status(status_code),
                    )
                    .await;
                    log_usage(
                        &state,
                        &stored,
                        status_code,
                        started.elapsed().as_millis(),
                        model_metadata(&adapter_request),
                        TokenUsage::default(),
                        UsageLogContext {
                            is_streaming: adapter_request.stream_requested,
                            stream_status: adapter_request
                                .stream_requested
                                .then(|| "upstream_error".to_string()),
                            ..request_context
                        },
                    )
                    .await;
                    return Ok(decoded_upstream_response(
                        status,
                        &response_headers,
                        content_type,
                        content_encoding,
                        original_decoded,
                    ));
                }
            } else {
                record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code))
                    .await;
                log_usage(
                    &state,
                    &stored,
                    status_code,
                    started.elapsed().as_millis(),
                    model_metadata(&adapter_request),
                    TokenUsage::default(),
                    UsageLogContext {
                        is_streaming: adapter_request.stream_requested,
                        stream_status: adapter_request
                            .stream_requested
                            .then(|| "upstream_error".to_string()),
                        ..request_context
                    },
                )
                .await;
                return Ok(decoded_upstream_response(
                    status,
                    &response_headers,
                    content_type,
                    content_encoding,
                    original_decoded,
                ));
            }
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            let bytes = match upstream.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    record_provider_outcome(
                        &state,
                        &stored,
                        provider_outcome_from_status(status_code),
                    )
                    .await;
                    if let Some(next_attempt) = next_claude_transport_attempt(
                        &state,
                        route,
                        &headers,
                        &request_context,
                        &attempt_context,
                        &execution,
                        "rate_limit_body_read_error",
                    )
                    .await
                    {
                        attempt_context = next_attempt;
                        drop(account_in_flight_guard);
                        drop(share_invocation_guard);
                        continue 'attempt;
                    }
                    return Err(ProxyError::bad_gateway(error));
                }
            };
            let decoded = decode_response_body_for_proxy(&response_headers, bytes);
            maybe_mark_upstream_rate_limited(
                &state,
                &execution,
                status,
                &response_headers,
                &decoded.body,
            )
            .await;
            if matches!(
                route,
                ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
            ) && !claude_request_is_provider_pinned(&headers, &request_context)
            {
                if let Some(next_attempt) = next_claude_provider_failover(
                    &state,
                    route,
                    &attempt_context,
                    &execution,
                    "http_429",
                )
                .await
                {
                    record_provider_outcome(
                        &state,
                        &stored,
                        provider_outcome_from_status(status_code),
                    )
                    .await;
                    attempt_context = next_attempt;
                    drop(account_in_flight_guard);
                    drop(share_invocation_guard);
                    continue 'attempt;
                }
            }
            if route == ProxyRoute::ClaudeCountTokens {
                crate::metrics::record_claude_count_tokens_outcome("rate_limited");
            } else {
                let usage = adapter.parse_usage(&decoded.body, &stored, route);
                let share_id_for_record = request_context.share_id.clone();
                let user_email_for_record = request_context.user_email.clone();
                log_usage(
                    &state,
                    &stored,
                    status_code,
                    started.elapsed().as_millis(),
                    model_metadata(&adapter_request),
                    usage,
                    UsageLogContext {
                        is_streaming: adapter_request.stream_requested,
                        stream_status: adapter_request
                            .stream_requested
                            .then(|| "rate_limited".to_string()),
                        ..request_context
                    },
                )
                .await;
                record_share_invocation_result(
                    &state,
                    share_id_for_record.as_deref(),
                    user_email_for_record.as_deref(),
                    usage,
                )
                .await;
            }
            record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code))
                .await;
            let mut response = Response::new(Body::from(decoded.body));
            *response.status_mut() = status;
            if let Some(content_type) = content_type {
                if let Ok(value) = HeaderValue::from_str(&content_type) {
                    response.headers_mut().insert(CONTENT_TYPE, value);
                }
            }
            if decoded.preserve_content_encoding {
                if let Some(value) = content_encoding {
                    response.headers_mut().insert(CONTENT_ENCODING, value);
                }
            }
            copy_safe_upstream_response_headers(&response_headers, &mut response);
            return Ok(response);
        }

        if adapter_request.stream_requested {
            let timeouts = StreamTimeoutConfig {
                first_byte: execution.stream_first_byte_timeout(),
                idle: execution.stream_idle_timeout(),
            };
            let mut inner = upstream.bytes_stream().boxed();
            let mut pending_chunk = None;
            let mut sse_error_detector = claude_sse_error_detector_for(&stored, route);
            let mut sse_error_outcome_recorded = false;
            if sse_error_detector.is_some() {
                let mut prelude = Vec::new();
                let mut detected_error = None;
                let first_chunk = loop {
                    let (timeout, kind) = if prelude.is_empty() {
                        (timeouts.first_byte, StreamTimeoutKind::FirstByte)
                    } else {
                        (timeouts.idle, StreamTimeoutKind::Idle)
                    };
                    let next = match timeout {
                        Some(timeout) => {
                            match tokio::time::timeout(timeout, inner.try_next()).await {
                                Ok(result) => result.map_err(StreamReadError::Upstream),
                                Err(_) => Err(StreamReadError::Timeout { kind, timeout }),
                            }
                        }
                        None => inner.try_next().await.map_err(StreamReadError::Upstream),
                    };
                    match next {
                        Ok(Some(chunk)) => {
                            prelude.extend_from_slice(&chunk);
                            detected_error = sse_error_detector
                                .as_mut()
                                .and_then(|detector| detector.push(&chunk));
                            let ready = detected_error.is_some()
                                || sse_error_detector
                                    .as_ref()
                                    .is_some_and(ClaudeSseErrorDetector::prelude_ready)
                                || prelude.len() >= 64 * 1024;
                            if ready {
                                break Ok(Some(Bytes::from(prelude)));
                            }
                        }
                        Ok(None) => break Ok((!prelude.is_empty()).then(|| Bytes::from(prelude))),
                        Err(error) => break Err(error),
                    }
                };
                match first_chunk {
                    Ok(Some(chunk)) => {
                        let sse_error = detected_error;
                        let sse_error_outcome = sse_error
                            .as_ref()
                            .and_then(|error| claude_sse_error_outcome(&error.error_type));
                        if let Some(outcome) = sse_error_outcome {
                            record_provider_outcome(&state, &stored, outcome).await;
                            if let Some(next_attempt) = next_claude_transport_attempt(
                                &state,
                                route,
                                &headers,
                                &request_context,
                                &attempt_context,
                                &execution,
                                "sse_error",
                            )
                            .await
                            {
                                attempt_context = next_attempt;
                                drop(account_in_flight_guard);
                                drop(share_invocation_guard);
                                continue 'attempt;
                            }
                            sse_error_outcome_recorded = true;
                        } else if execution.driver_is("oauth.claude_messages") {
                            if let Some(error) = sse_error {
                                if let Some(next_stage) = claude_body_retry_stage_for_error_message(
                                    error.message.as_deref().unwrap_or(&error.error_type),
                                    claude_body_retry_stage,
                                    &adapter_request.body,
                                ) {
                                    if attempt_context.retry_allowed() {
                                        crate::metrics::record_claude_retry(
                                            next_stage.as_header_value(),
                                            "sse_error",
                                        );
                                        attempt_context =
                                            attempt_context.next(&execution, Some(next_stage));
                                        drop(account_in_flight_guard);
                                        drop(share_invocation_guard);
                                        continue 'attempt;
                                    }
                                }
                            }
                        }
                        pending_chunk = Some(chunk);
                        sse_error_detector = claude_sse_error_detector_for(&stored, route);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure)
                            .await;
                        if let Some(next_attempt) = next_claude_transport_attempt(
                            &state,
                            route,
                            &headers,
                            &request_context,
                            &attempt_context,
                            &execution,
                            "first_event_read",
                        )
                        .await
                        {
                            attempt_context = next_attempt;
                            drop(account_in_flight_guard);
                            drop(share_invocation_guard);
                            continue 'attempt;
                        }
                        return Err(ProxyError {
                            status: StatusCode::from_u16(error.status_code())
                                .unwrap_or(StatusCode::BAD_GATEWAY),
                            message: error.to_string(),
                        });
                    }
                }
            }
            let request_id = log_usage(
                &state,
                &stored,
                status_code,
                started.elapsed().as_millis(),
                model_metadata(&adapter_request),
                TokenUsage::default(),
                UsageLogContext {
                    is_streaming: true,
                    stream_status: Some("pending".to_string()),
                    ..request_context.clone()
                },
            )
            .await;

            let stream_stored = stored.clone();
            let interrupted_update_armed = Arc::new(AtomicBool::new(true));
            let stream_state = StreamForwardState {
                inner,
                stored: stream_stored,
                state: state.clone(),
                route,
                request_id,
                status_code,
                share_id: request_context.share_id.clone(),
                user_email: request_context.user_email.clone(),
                started,
                first_token_ms: None,
                received_any_chunk: false,
                usage: StreamUsageAccumulator::new(adapters::usage_input_semantics_for(
                    &stored, route,
                )),
                codex_completed_output_patcher: CodexCompletedOutputPatcher::new(&stored, route),
                codex_pending_function_call_patcher: CodexPendingFunctionCallPatcher::new(
                    &stored, route,
                ),
                codex_custom_tool_stream_patcher: CodexCustomToolStreamPatcher::default(),
                stream_transform: super::stream_transforms::StreamEventTransformer::new(
                    &stored,
                    route,
                    adapter_request.custom_tool_names.clone(),
                ),
                timeouts,
                pending_chunk,
                sse_error_detector,
                sse_error_outcome_recorded,
                terminal_frame_sent: false,
                interrupted_update_armed,
                _account_in_flight_guard: account_in_flight_guard,
                _share_invocation_guard: share_invocation_guard,
            };
            let stream = stream::try_unfold(stream_state, |mut stream_state| async move {
                if stream_state.terminal_frame_sent {
                    return Ok(None);
                }

                let next_chunk = if let Some(chunk) = stream_state.pending_chunk.take() {
                    Ok(Some(chunk))
                } else {
                    let timeout_kind = stream_state.next_timeout_kind();
                    match stream_state.next_timeout() {
                        Some(timeout) => {
                            match tokio::time::timeout(timeout, stream_state.inner.try_next()).await
                            {
                                Ok(result) => result.map_err(StreamReadError::Upstream),
                                Err(_) => Err(StreamReadError::Timeout {
                                    kind: timeout_kind,
                                    timeout,
                                }),
                            }
                        }
                        None => stream_state
                            .inner
                            .try_next()
                            .await
                            .map_err(StreamReadError::Upstream),
                    }
                };

                match next_chunk {
                    Ok(Some(chunk)) => {
                        stream_state.received_any_chunk = true;
                        let chunk = stream_state.codex_completed_output_patcher.push(chunk);
                        let chunk = stream_state.codex_pending_function_call_patcher.push(chunk);
                        stream_state.usage.push(&chunk);
                        if !stream_state.sse_error_outcome_recorded {
                            let sse_error_outcome = stream_state
                                .sse_error_detector
                                .as_mut()
                                .and_then(|detector| detector.push(&chunk))
                                .and_then(|error| claude_sse_error_outcome(&error.error_type));
                            if let Some(outcome) = sse_error_outcome {
                                record_provider_outcome(
                                    &stream_state.state,
                                    &stream_state.stored,
                                    outcome,
                                )
                                .await;
                                stream_state.sse_error_outcome_recorded = true;
                            }
                        }
                        if stream_state.first_token_ms.is_none() && !chunk.is_empty() {
                            let first_token_ms = stream_state.started.elapsed().as_millis();
                            stream_state.first_token_ms = Some(first_token_ms);
                            update_stream_usage(
                                &stream_state.state,
                                &stream_state.stored,
                                &stream_state.request_id,
                                stream_state.status_code,
                                stream_state.started.elapsed().as_millis(),
                                Some(first_token_ms),
                                Default::default(),
                                Some("streaming"),
                            )
                            .await;
                        }
                        let transformed = match stream_state.stream_transform.push(chunk) {
                            Ok(transformed) => transformed,
                            Err(error) => {
                                return stream_state.terminate_transform_error(error).await
                            }
                        };
                        let transformed = stream_state
                            .codex_custom_tool_stream_patcher
                            .push(transformed);
                        Ok(Some((transformed, stream_state)))
                    }
                    Ok(None) => {
                        let chunk = stream_state.codex_completed_output_patcher.finish();
                        let chunk = stream_state.codex_pending_function_call_patcher.push(chunk);
                        let tail = stream_state.codex_pending_function_call_patcher.finish();
                        let chunk = if tail.is_empty() {
                            chunk
                        } else if chunk.is_empty() {
                            tail
                        } else {
                            let mut joined = chunk.to_vec();
                            joined.extend_from_slice(&tail);
                            Bytes::from(joined)
                        };
                        if !chunk.is_empty() {
                            stream_state.usage.push(&chunk);
                            if stream_state.first_token_ms.is_none() {
                                let first_token_ms = stream_state.started.elapsed().as_millis();
                                stream_state.first_token_ms = Some(first_token_ms);
                                update_stream_usage(
                                    &stream_state.state,
                                    &stream_state.stored,
                                    &stream_state.request_id,
                                    stream_state.status_code,
                                    stream_state.started.elapsed().as_millis(),
                                    Some(first_token_ms),
                                    Default::default(),
                                    Some("streaming"),
                                )
                                .await;
                            }
                            let transformed = match stream_state.stream_transform.push(chunk) {
                                Ok(transformed) => transformed,
                                Err(error) => {
                                    return stream_state.terminate_transform_error(error).await
                                }
                            };
                            let tail = match stream_state.stream_transform.finish() {
                                Ok(tail) => tail,
                                Err(error) => {
                                    return stream_state.terminate_transform_error(error).await
                                }
                            };
                            let transformed = join_bytes(transformed, tail);
                            let transformed = stream_state
                                .codex_custom_tool_stream_patcher
                                .push(transformed);
                            return Ok(Some((transformed, stream_state)));
                        }
                        let transform_tail = match stream_state.stream_transform.finish() {
                            Ok(tail) => tail,
                            Err(error) => {
                                return stream_state.terminate_transform_error(error).await
                            }
                        };
                        let transformed_tail = stream_state
                            .codex_custom_tool_stream_patcher
                            .push(transform_tail);
                        let custom_tail = join_bytes(
                            transformed_tail,
                            stream_state.codex_custom_tool_stream_patcher.finish(),
                        );
                        if !custom_tail.is_empty() {
                            return Ok(Some((custom_tail, stream_state)));
                        }
                        let usage = std::mem::take(&mut stream_state.usage).finish();
                        update_stream_usage(
                            &stream_state.state,
                            &stream_state.stored,
                            &stream_state.request_id,
                            stream_state.status_code,
                            stream_state.started.elapsed().as_millis(),
                            stream_state.first_token_ms,
                            usage,
                            Some("completed"),
                        )
                        .await;
                        record_share_invocation_result(
                            &stream_state.state,
                            stream_state.share_id.as_deref(),
                            stream_state.user_email.as_deref(),
                            usage,
                        )
                        .await;
                        if !stream_state.sse_error_outcome_recorded {
                            record_provider_outcome(
                                &stream_state.state,
                                &stream_state.stored,
                                provider_outcome_from_status(stream_state.status_code),
                            )
                            .await;
                        }
                        stream_state
                            .interrupted_update_armed
                            .store(false, Ordering::Relaxed);
                        Ok(None)
                    }
                    Err(error) => {
                        let usage = std::mem::take(&mut stream_state.usage).finish();
                        let status = error.status_code();
                        let stream_status = error.stream_status();
                        update_stream_usage(
                            &stream_state.state,
                            &stream_state.stored,
                            &stream_state.request_id,
                            status,
                            stream_state.started.elapsed().as_millis(),
                            stream_state.first_token_ms,
                            usage,
                            Some(stream_status),
                        )
                        .await;
                        record_share_invocation_result(
                            &stream_state.state,
                            stream_state.share_id.as_deref(),
                            stream_state.user_email.as_deref(),
                            usage,
                        )
                        .await;
                        record_provider_outcome(
                            &stream_state.state,
                            &stream_state.stored,
                            ProviderOutcome::NetworkFailure,
                        )
                        .await;
                        stream_state
                            .interrupted_update_armed
                            .store(false, Ordering::Relaxed);
                        stream_state.terminal_frame_sent = true;
                        let message = error.to_string();
                        if let Some(frame) =
                            stream_terminal_error_frame(stream_state.route, &message, status)
                        {
                            Ok(Some((frame, stream_state)))
                        } else {
                            Err(std::io::Error::other(message))
                        }
                    }
                }
            });
            let mut response = Response::new(Body::from_stream(stream));
            *response.status_mut() = status;
            if let Some(content_type) = content_type {
                if let Ok(value) = HeaderValue::from_str(&content_type) {
                    response.headers_mut().insert(CONTENT_TYPE, value);
                }
            }
            copy_safe_upstream_response_headers(&response_headers, &mut response);
            return Ok(response);
        }

        let bytes = match upstream.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                if route == ProxyRoute::ClaudeCountTokens {
                    crate::metrics::record_claude_count_tokens_outcome("network_error");
                }
                if let Some(next_attempt) = next_claude_transport_attempt(
                    &state,
                    route,
                    &headers,
                    &request_context,
                    &attempt_context,
                    &execution,
                    "body_read_error",
                )
                .await
                {
                    attempt_context = next_attempt;
                    drop(account_in_flight_guard);
                    drop(share_invocation_guard);
                    continue 'attempt;
                }
                return Err(ProxyError::bad_gateway(error));
            }
        };
        let decoded = decode_response_body_for_proxy(&response_headers, bytes);
        let mut preserve_content_encoding = decoded.preserve_content_encoding;
        let mut bytes = decoded.body;
        let next_body_retry_stage = if route == ProxyRoute::ClaudeMessages
            && execution.driver_is("oauth.claude_messages")
        {
            claude_non_stream_retry_stage(
                status,
                &bytes,
                claude_body_retry_stage,
                &adapter_request.body,
            )
        } else {
            None
        };
        if let Some(next_stage) = next_body_retry_stage {
            if attempt_context.retry_allowed() {
                crate::metrics::record_claude_retry(next_stage.as_header_value(), "http_error");
                attempt_context = attempt_context.next(&execution, Some(next_stage));
                drop(account_in_flight_guard);
                drop(share_invocation_guard);
                continue 'attempt;
            }
        }
        let (rewritten, version_gate_rewritten) =
            maybe_rewrite_claude_cli_version_gate_body(status, &stored, route, bytes);
        bytes = rewritten;
        if version_gate_rewritten {
            preserve_content_encoding = false;
        }
        let usage = if route == ProxyRoute::ClaudeCountTokens {
            TokenUsage::default()
        } else {
            adapter.parse_usage(&bytes, &stored, route)
        };
        let bytes =
            adapter.transform_response_for_request(bytes, &stored, route, &adapter_request)?;
        let share_id_for_record = request_context.share_id.clone();
        if route == ProxyRoute::ClaudeCountTokens {
            crate::metrics::record_claude_count_tokens_outcome(count_tokens_metric_outcome(status));
        } else {
            let user_email_for_record = request_context.user_email.clone();
            log_usage(
                &state,
                &stored,
                status_code,
                started.elapsed().as_millis(),
                model_metadata(&adapter_request),
                usage,
                UsageLogContext {
                    is_streaming: false,
                    ..request_context
                },
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id_for_record.as_deref(),
                user_email_for_record.as_deref(),
                usage,
            )
            .await;
        }
        record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code)).await;

        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = status;
        if let Some(content_type) = content_type {
            if let Ok(value) = HeaderValue::from_str(&content_type) {
                response.headers_mut().insert(CONTENT_TYPE, value);
            }
        }
        if preserve_content_encoding {
            if let Some(value) = content_encoding {
                response.headers_mut().insert(CONTENT_ENCODING, value);
            }
        }
        copy_safe_upstream_response_headers(&response_headers, &mut response);
        return Ok(response);
    }
}

pub async fn forward_codex_responses_ws(
    state: ServerState,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ProxyError> {
    let route = ProxyRoute::CodexResponses;
    let app = route.app();
    let mut request_context = request_context_from_headers(&headers);
    let share_invocation_guard = if let Some(share_id) = request_context.share_id.clone() {
        let (share_name, guard) = validate_and_acquire_share_invocation(
            &state,
            &share_id,
            request_context.user_email.as_deref(),
        )
        .await?;
        request_context.share_name = Some(share_name);
        Some(guard)
    } else {
        None
    };
    let shares = state.shares.read().await.clone();
    let accounts_for_selection = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let ui_settings = state.ui_settings.read().await.for_frontend();
    let configured_provider_id =
        current_provider::resolve_current_provider_id(&providers, &ui_settings, app);
    let execution = if let Some(share_id) = request_context.share_id.as_deref() {
        let (execution, _share_name) =
            select_share_execution(&providers, &shares, &accounts_for_selection, app, share_id)?;
        execution
    } else {
        select_provider(
            &providers,
            &accounts_for_selection,
            app,
            &headers,
            configured_provider_id.as_deref(),
        )?
        .execution
    };
    drop(providers);
    let stored = execution.runtime_stored_view();
    if !execution.driver_is("oauth.openai_codex") && !execution.driver_is("oauth.grok_responses") {
        return Err(ProxyError::bad_request(
            "responses websocket is only available for codex_oauth or grok_oauth providers",
        ));
    }
    if !codex_websocket_enabled(&stored) {
        return Err(ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Codex Responses WebSocket is disabled for this provider; use POST /v1/responses (SSE) until the incident rollback is cleared".to_string(),
        });
    }
    validate_codex_allowed_client(&stored, route, &headers, request_context.share_id.is_some())?;
    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    let accounts = state.accounts_snapshot().await;
    let adapter = adapters::adapter_for(app, stored.provider_type);
    let mut target_headers = adapter.build_headers(app, &stored, &accounts)?;
    let mut session_id = codex_oauth_session_id_from_request(&headers, b"").or_else(|| {
        execution
            .driver_is("oauth.grok_responses")
            .then(super::grok::new_session_id)
    });
    if execution.driver_is("oauth.grok_responses") {
        if let Some(raw) = session_id.as_deref() {
            let tenant_scope = grok_tenant_scope(&request_context, &stored);
            session_id = Some(super::grok::namespace_session_id(
                tenant_scope.as_deref(),
                raw,
            ));
        }
    }
    append_codex_oauth_session_headers(&mut target_headers, session_id.as_deref());
    if execution.driver_is("oauth.openai_codex") {
        crate::codex_identity::finalize_headers(&mut target_headers);
    }
    if execution.driver_is("oauth.grok_responses") {
        if let Some(session_id) = session_id.as_deref() {
            replace_or_push_header(
                &mut target_headers,
                "x-grok-conv-id",
                session_id.to_string(),
            );
        }
    }
    let mut target_headers = owned_headers(target_headers);
    let mut ws_url = if execution.driver_is("oauth.grok_responses") {
        super::grok::websocket_url().to_string()
    } else {
        "wss://chatgpt.com/backend-api/codex/responses".to_string()
    };
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut target_headers, &mut ws_url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut target_headers, &stored, &accounts)?;

    let ws_mode = if execution.driver_is("oauth.grok_responses") {
        ResponsesWebsocketMode::Grok
    } else {
        ResponsesWebsocketMode::Codex
    };
    let websocket_upstream_model = match &execution.plan.model_policy {
        crate::domain::providers::runtime::RuntimeModelPolicy::Single { upstream_model } => {
            Some(upstream_model.clone())
        }
        crate::domain::providers::runtime::RuntimeModelPolicy::Passthrough => None,
    };
    let request_timeout = execution.request_timeout();
    let share_id = request_context.share_id.clone();
    let user_email = request_context.user_email.clone();
    let state_for_share = state.clone();
    let response = ws.on_upgrade(move |socket| async move {
        let _share_invocation_guard = share_invocation_guard;
        if let Err(error) = bridge_responses_websocket(
            socket,
            ResponsesWebsocketBridgeOptions {
                headers: target_headers,
                connect_timeout: request_timeout,
                ws_url,
                mode: ws_mode,
                grok_session_id: session_id,
                single_upstream_model: websocket_upstream_model,
                state: &state_for_share,
                execution: &execution,
            },
        )
        .await
        {
            tracing::warn!(error = %error, "responses websocket bridge failed");
        }
        record_share_invocation_result(
            &state_for_share,
            share_id.as_deref(),
            user_email.as_deref(),
            TokenUsage::default(),
        )
        .await;
    });
    Ok(response)
}

fn codex_websocket_enabled(stored: &StoredProvider) -> bool {
    stored.provider_type != ProviderType::CodexOAuth
        || stored
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_websocket_enabled)
            .unwrap_or(true)
}

pub async fn forward_grok_media(
    state: ServerState,
    method: Method,
    upstream_path: String,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = decode_request_body_for_proxy(&headers, body)?;
    let mut request_context = request_context_from_headers(&headers);
    let share_invocation_guard = if let Some(share_id) = request_context.share_id.clone() {
        let (share_name, guard) = validate_and_acquire_share_invocation(
            &state,
            &share_id,
            request_context.user_email.as_deref(),
        )
        .await?;
        request_context.share_name = Some(share_name);
        Some(guard)
    } else {
        None
    };
    let mut selection_headers = headers.clone();
    if let Some(session_key) = super::grok::sticky_media_session_key(&upstream_path, &body) {
        if selection_headers.get("x-cc-provider-id").is_none() {
            if let Some(binding) = state.grok_media_session_binding(&session_key) {
                if let Ok(value) = HeaderValue::from_str(&binding.provider_id) {
                    selection_headers.insert(HeaderName::from_static("x-cc-provider-id"), value);
                }
            }
        }
    }
    let shares = state.shares.read().await.clone();
    let accounts_for_selection = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let ui_settings = state.ui_settings.read().await.for_frontend();
    let configured_provider_id =
        current_provider::resolve_current_provider_id(&providers, &ui_settings, AppKind::Codex);
    let execution = if let Some(share_id) = request_context.share_id.as_deref() {
        let (execution, _share_name) = select_share_execution(
            &providers,
            &shares,
            &accounts_for_selection,
            AppKind::Codex,
            share_id,
        )?;
        execution
    } else {
        super::router::select_provider_for_type(
            &providers,
            &accounts_for_selection,
            AppKind::Codex,
            &selection_headers,
            configured_provider_id.as_deref(),
            ProviderType::GrokOAuth,
        )?
        .execution
    };
    drop(providers);
    if !execution.driver_is("oauth.grok_responses") {
        return Err(ProxyError::bad_request(
            "Grok media endpoints require a grok_oauth provider",
        ));
    }
    forward_grok_media_with_execution(
        state,
        execution,
        method,
        upstream_path,
        headers,
        body,
        request_context.share_id,
        request_context.user_email,
        share_invocation_guard,
    )
    .await
}

pub async fn forward_images_generations(
    state: ServerState,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = decode_request_body_for_proxy(&headers, body)?;
    let mut request_context = request_context_from_headers(&headers);
    let share_invocation_guard = if let Some(share_id) = request_context.share_id.clone() {
        let (share_name, guard) = validate_and_acquire_share_invocation(
            &state,
            &share_id,
            request_context.user_email.as_deref(),
        )
        .await?;
        request_context.share_name = Some(share_name);
        Some(guard)
    } else {
        None
    };
    let shares = state.shares.read().await.clone();
    let accounts_for_selection = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let ui_settings = state.ui_settings.read().await.for_frontend();
    let configured_provider_id =
        current_provider::resolve_current_provider_id(&providers, &ui_settings, AppKind::Codex);
    let execution = if let Some(share_id) = request_context.share_id.as_deref() {
        let (execution, _share_name) = select_share_execution(
            &providers,
            &shares,
            &accounts_for_selection,
            AppKind::Codex,
            share_id,
        )?;
        execution
    } else {
        select_provider_for_codex_image_generation(
            &providers,
            &accounts_for_selection,
            &headers,
            configured_provider_id.as_deref(),
        )?
        .execution
    };
    drop(providers);

    if execution.driver_is("oauth.grok_responses") {
        forward_grok_media_with_execution(
            state,
            execution,
            Method::POST,
            "/images/generations".to_string(),
            headers,
            body,
            request_context.share_id,
            request_context.user_email,
            share_invocation_guard,
        )
        .await
    } else if execution.driver_is("oauth.openai_codex") {
        forward_codex_images_generations(
            state,
            execution,
            headers,
            body,
            request_context,
            share_invocation_guard,
        )
        .await
    } else {
        Err(ProxyError::bad_request(
            "image generation requires a grok_oauth provider or codex_oauth provider with image generation enabled",
        ))
    }
}

#[allow(clippy::too_many_arguments)] // Media forwarding carries the full request/accounting context.
async fn forward_grok_media_with_execution(
    state: ServerState,
    execution: ProviderExecution,
    method: Method,
    upstream_path: String,
    headers: HeaderMap,
    body: Bytes,
    share_id: Option<String>,
    user_email: Option<String>,
    _share_invocation_guard: Option<ShareInFlightGuard>,
) -> Result<Response, ProxyError> {
    let stored = execution.runtime_stored_view();
    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    let accounts = state.accounts_snapshot().await;
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let mut target_headers = adapter.build_headers(AppKind::Codex, &stored, &accounts)?;
    if let Some(session_id) =
        super::grok::sticky_media_session_key(&upstream_path, &body).or_else(|| {
            optional_header(&headers, "x-grok-conv-id").filter(|value| !value.trim().is_empty())
        })
    {
        replace_or_push_header(&mut target_headers, "x-grok-conv-id", session_id);
    }
    replace_or_push_header(
        &mut target_headers,
        "accept",
        "application/json, text/event-stream".to_string(),
    );
    let mut target_headers = owned_headers(target_headers);
    let (body, content_type) = if upstream_path.contains("/images/edits") {
        (
            super::grok::image_edit_body(&headers, body)?,
            "application/json".to_string(),
        )
    } else {
        (
            body,
            copy_header(&headers, CONTENT_TYPE)
                .map(str::to_string)
                .unwrap_or_else(|| "application/json".to_string()),
        )
    };
    let mut url = super::join_url(&execution.plan.endpoint, &upstream_path);
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut target_headers, &stored, &accounts)?;
    let http_client = forward_http_client(&state, &stored).await?;
    let mut request = http_client
        .request(method.clone(), &url)
        .header(CONTENT_TYPE, content_type);
    for (name, value) in &target_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if method != Method::GET {
        request = request.body(body);
    }
    request = request.timeout(execution.request_timeout());
    let started = Instant::now();
    let upstream = request.send().await.map_err(|error| {
        tokio::spawn({
            let state = state.clone();
            let stored = stored.clone();
            async move {
                record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            }
        });
        ProxyError::bad_gateway(error)
    })?;
    let status = upstream.status();
    let status_code = status.as_u16();
    let mut response_headers = upstream.headers().clone();
    strip_hop_by_hop_response_headers(&mut response_headers);
    maybe_update_grok_entitlement(&state, &stored, &response_headers).await;
    maybe_mark_grok_cooldown(&state, &stored, status, &response_headers).await;
    let content_type = response_headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_encoding = content_encoding_value(&response_headers);
    let bytes = upstream.bytes().await.map_err(ProxyError::bad_gateway)?;
    let decoded = decode_response_body_for_proxy(&response_headers, bytes);
    let response_body = decoded.body;
    maybe_mark_upstream_rate_limited(
        &state,
        &execution,
        status,
        &response_headers,
        &response_body,
    )
    .await;
    if status.is_success() && upstream_path.contains("/videos/generations") {
        if let Some(session_key) = super::grok::video_session_key_from_response(&response_body) {
            state.remember_grok_media_session(
                session_key,
                stored.provider.id.clone(),
                execution.managed_account_id().map(str::to_string),
                24 * 60 * 60 * 1000,
            );
        }
    }
    record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code)).await;
    record_share_invocation_result(
        &state,
        share_id.as_deref(),
        user_email.as_deref(),
        TokenUsage::default(),
    )
    .await;
    let mut response = Response::new(Body::from(response_body));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            response.headers_mut().insert(CONTENT_TYPE, value);
        }
    }
    if decoded.preserve_content_encoding {
        if let Some(value) = content_encoding {
            response.headers_mut().insert(CONTENT_ENCODING, value);
        }
    }
    copy_safe_upstream_response_headers(&response_headers, &mut response);
    tracing::debug!(
        provider_id = stored.provider.id,
        status = status_code,
        elapsed_ms = started.elapsed().as_millis(),
        "grok media request completed"
    );
    Ok(response)
}

async fn forward_codex_images_generations(
    state: ServerState,
    execution: ProviderExecution,
    headers: HeaderMap,
    body: Bytes,
    request_context: UsageLogContext,
    _share_invocation_guard: Option<ShareInFlightGuard>,
) -> Result<Response, ProxyError> {
    let stored = execution.runtime_stored_view();
    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    validate_codex_allowed_client(
        &stored,
        ProxyRoute::CodexResponses,
        &headers,
        request_context.share_id.is_some(),
    )?;
    let prepared = codex_images_generation_request(&body)?;
    let accounts = state.accounts_snapshot().await;
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let mut target_headers = adapter.build_headers(AppKind::Codex, &stored, &accounts)?;
    let session_id = codex_oauth_session_id_from_request(&headers, &body);
    append_codex_oauth_session_headers(&mut target_headers, session_id.as_deref());
    crate::codex_identity::finalize_headers(&mut target_headers);
    let mut target_headers = owned_headers(target_headers);
    let mut adapter_request = adapters::AdapterRequest {
        body: prepared.body.clone(),
        upstream_endpoint: None,
        upstream_headers: Vec::new(),
        model: Some(CODEX_IMAGES_RESPONSES_MAIN_MODEL.to_string()),
        requested_model: Some(prepared.tool_model.clone()),
        actual_model: Some(CODEX_IMAGES_RESPONSES_MAIN_MODEL.to_string()),
        actual_model_source: Some("codex_image_generation_bridge".to_string()),
        stream_requested: true,
        custom_tool_names: Default::default(),
    };
    execution.enforce_model_policy(&mut adapter_request)?;
    let mut url = execution.resolve_endpoint(ProxyRoute::CodexResponses, None, &adapter_request)?;
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut target_headers, &stored, &accounts)?;
    let http_client = forward_http_client(&state, &stored).await?;
    let mut request = http_client
        .post(&url)
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(adapter_request.body.clone())
        .timeout(execution.request_timeout());
    for (name, value) in &target_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let started = Instant::now();
    let upstream = request.send().await.map_err(|error| {
        tokio::spawn({
            let state = state.clone();
            let stored = stored.clone();
            async move {
                record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            }
        });
        ProxyError::bad_gateway(error)
    })?;
    let status = upstream.status();
    let status_code = status.as_u16();
    let mut response_headers = upstream.headers().clone();
    strip_hop_by_hop_response_headers(&mut response_headers);
    let content_type = response_headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_encoding = content_encoding_value(&response_headers);
    let bytes = upstream.bytes().await.map_err(ProxyError::bad_gateway)?;
    let decoded = decode_response_body_for_proxy(&response_headers, bytes);
    if status == StatusCode::TOO_MANY_REQUESTS {
        maybe_mark_upstream_rate_limited(
            &state,
            &execution,
            status,
            &response_headers,
            &decoded.body,
        )
        .await;
    }
    record_provider_outcome(&state, &stored, provider_outcome_from_status(status_code)).await;
    if !status.is_success() {
        record_share_invocation_result(
            &state,
            request_context.share_id.as_deref(),
            request_context.user_email.as_deref(),
            TokenUsage::default(),
        )
        .await;
        let mut response = Response::new(Body::from(decoded.body));
        *response.status_mut() = status;
        if let Some(content_type) = content_type {
            if let Ok(value) = HeaderValue::from_str(&content_type) {
                response.headers_mut().insert(CONTENT_TYPE, value);
            }
        }
        if decoded.preserve_content_encoding {
            if let Some(value) = content_encoding {
                response.headers_mut().insert(CONTENT_ENCODING, value);
            }
        }
        copy_safe_upstream_response_headers(&response_headers, &mut response);
        return Ok(response);
    }
    let image_response = codex_images_response_from_responses_body(
        &decoded.body,
        prepared.response_format.as_deref(),
        prepared.stream,
    )?;
    log_usage(
        &state,
        &stored,
        status_code,
        started.elapsed().as_millis(),
        UsageModelMetadata {
            model: Some(prepared.tool_model.clone()),
            requested_model: Some(prepared.tool_model),
            actual_model: Some(CODEX_IMAGES_RESPONSES_MAIN_MODEL.to_string()),
            actual_model_source: Some("codex_image_generation_bridge".to_string()),
        },
        TokenUsage::default(),
        UsageLogContext {
            is_streaming: prepared.stream,
            stream_status: Some("completed".to_string()),
            ..request_context.clone()
        },
    )
    .await;
    record_share_invocation_result(
        &state,
        request_context.share_id.as_deref(),
        request_context.user_email.as_deref(),
        TokenUsage::default(),
    )
    .await;
    let mut response = Response::new(Body::from(image_response.body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(image_response.content_type),
    );
    copy_safe_upstream_response_headers(&response_headers, &mut response);
    Ok(response)
}

struct CodexImagesPreparedRequest {
    body: Bytes,
    tool_model: String,
    response_format: Option<String>,
    stream: bool,
}

struct CodexImagesResponse {
    body: Bytes,
    content_type: &'static str,
}

#[derive(Clone, Default)]
struct CodexImageResult {
    result: String,
    revised_prompt: Option<String>,
    output_format: Option<String>,
    size: Option<String>,
    background: Option<String>,
    quality: Option<String>,
}

fn codex_images_generation_request(body: &[u8]) -> Result<CodexImagesPreparedRequest, ProxyError> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid OpenAI image generation request JSON: {error}"),
    })?;
    let prompt = value
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or_else(|| ProxyError::bad_request("image generation prompt is required"))?;
    let tool_model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(CODEX_IMAGES_DEFAULT_TOOL_MODEL)
        .to_string();
    let response_format = value
        .get("response_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|format| !format.is_empty())
        .map(str::to_ascii_lowercase);
    let stream = value
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut tool = json!({
        "type": "image_generation",
        "action": "generate",
        "model": tool_model,
    });
    for field in [
        "size",
        "quality",
        "background",
        "output_format",
        "moderation",
    ] {
        if let Some(text) = value
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            tool[field] = Value::String(text.to_string());
        }
    }
    for field in ["output_compression", "partial_images", "n"] {
        if let Some(number) = value.get(field).and_then(Value::as_i64) {
            tool[field] = Value::Number(number.into());
        }
    }
    let request = json!({
        "instructions": "",
        "stream": true,
        "reasoning": {"effort": "medium", "summary": "auto"},
        "parallel_tool_calls": true,
        "include": ["reasoning.encrypted_content"],
        "model": CODEX_IMAGES_RESPONSES_MAIN_MODEL,
        "store": false,
        "tool_choice": {"type": "image_generation"},
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}]
        }],
        "tools": [tool],
    });
    let body = serde_json::to_vec(&request)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode codex image generation request failed: {error}"),
        })?;
    Ok(CodexImagesPreparedRequest {
        body,
        tool_model: request["tools"][0]["model"]
            .as_str()
            .unwrap_or(CODEX_IMAGES_DEFAULT_TOOL_MODEL)
            .to_string(),
        response_format,
        stream,
    })
}

fn codex_images_response_from_responses_body(
    body: &[u8],
    response_format: Option<&str>,
    stream: bool,
) -> Result<CodexImagesResponse, ProxyError> {
    let (results, created_at) = collect_codex_image_results(body);
    if results.is_empty() {
        return Err(ProxyError {
            status: StatusCode::BAD_GATEWAY,
            message: "codex image generation response did not contain image output".to_string(),
        });
    }
    if stream {
        let mut output = String::new();
        for result in results {
            let payload = codex_image_result_payload(&result, response_format);
            output.push_str(&format!(
                "event: image_generation.completed\ndata: {payload}\n\n"
            ));
        }
        output.push_str("data: [DONE]\n\n");
        return Ok(CodexImagesResponse {
            body: Bytes::from(output),
            content_type: "text/event-stream",
        });
    }
    let mut data = Vec::new();
    let mut first_meta = CodexImageResult::default();
    for (index, result) in results.iter().enumerate() {
        if index == 0 {
            first_meta = result.clone();
        }
        data.push(codex_image_result_data(result, response_format));
    }
    let mut response = json!({
        "created": created_at,
        "data": data,
    });
    if let Some(value) = first_meta.background {
        response["background"] = Value::String(value);
    }
    if let Some(value) = first_meta.output_format {
        response["output_format"] = Value::String(value);
    }
    if let Some(value) = first_meta.quality {
        response["quality"] = Value::String(value);
    }
    if let Some(value) = first_meta.size {
        response["size"] = Value::String(value);
    }
    let body = serde_json::to_vec(&response)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode codex image generation response failed: {error}"),
        })?;
    Ok(CodexImagesResponse {
        body,
        content_type: "application/json",
    })
}

fn collect_codex_image_results(body: &[u8]) -> (Vec<CodexImageResult>, i64) {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        return images_from_completed_value(&value, Vec::new());
    }
    let text = String::from_utf8_lossy(body);
    let mut buffer = text.to_string();
    let mut fallback = Vec::new();
    let mut completed = None;
    while let Some((event_end, delimiter_len)) = next_sse_event_boundary(&buffer) {
        let event = buffer[..event_end].to_string();
        buffer.drain(..event_end + delimiter_len);
        collect_codex_image_event(&event, &mut fallback, &mut completed);
    }
    if !buffer.trim().is_empty() {
        collect_codex_image_event(&buffer, &mut fallback, &mut completed);
    }
    if let Some(completed) = completed {
        images_from_completed_value(&completed, fallback)
    } else {
        (fallback, (current_time_ms() / 1000) as i64)
    }
}

fn collect_codex_image_event(
    event: &str,
    fallback: &mut Vec<CodexImageResult>,
    completed: &mut Option<Value>,
) {
    let Some(payload) = first_sse_data_payload(event) else {
        return;
    };
    if payload == "[DONE]" || !payload.starts_with('{') {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            if let Some(result) = codex_image_result_from_item(value.get("item")) {
                fallback.push(result);
            }
        }
        Some("response.completed") => *completed = Some(value),
        _ => {}
    }
}

fn images_from_completed_value(
    value: &Value,
    fallback: Vec<CodexImageResult>,
) -> (Vec<CodexImageResult>, i64) {
    let created_at = value
        .pointer("/response/created_at")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| (current_time_ms() / 1000) as i64);
    let results = value
        .pointer("/response/output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| codex_image_result_from_item(Some(item)))
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .unwrap_or(fallback);
    (results, created_at)
}

fn codex_image_result_from_item(item: Option<&Value>) -> Option<CodexImageResult> {
    let item = item?;
    if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
        return None;
    }
    let result = item
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|result| !result.is_empty())?
        .to_string();
    Some(CodexImageResult {
        result,
        revised_prompt: image_string_field(item, "revised_prompt"),
        output_format: image_string_field(item, "output_format"),
        size: image_string_field(item, "size"),
        background: image_string_field(item, "background"),
        quality: image_string_field(item, "quality"),
    })
}

fn image_string_field(item: &Value, field: &str) -> Option<String> {
    item.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn codex_image_result_payload(result: &CodexImageResult, response_format: Option<&str>) -> Value {
    let mut payload = codex_image_result_data(result, response_format);
    payload["type"] = Value::String("image_generation.completed".to_string());
    payload
}

fn codex_image_result_data(result: &CodexImageResult, response_format: Option<&str>) -> Value {
    let mut data = json!({});
    if response_format
        .map(|format| format.eq_ignore_ascii_case("url"))
        .unwrap_or(false)
    {
        data["url"] = Value::String(format!(
            "data:{};base64,{}",
            codex_image_mime_type(result.output_format.as_deref()),
            result.result
        ));
    } else {
        data["b64_json"] = Value::String(result.result.clone());
    }
    if let Some(value) = result.revised_prompt.clone() {
        data["revised_prompt"] = Value::String(value);
    }
    data
}

fn codex_image_mime_type(output_format: Option<&str>) -> &'static str {
    match output_format
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[derive(Debug, Clone, Copy)]
enum ResponsesWebsocketMode {
    Codex,
    Grok,
}

struct ResponsesWebsocketBridgeOptions<'a> {
    headers: Vec<(String, String)>,
    connect_timeout: Duration,
    ws_url: String,
    mode: ResponsesWebsocketMode,
    grok_session_id: Option<String>,
    single_upstream_model: Option<String>,
    state: &'a ServerState,
    execution: &'a ProviderExecution,
}

async fn bridge_responses_websocket(
    downstream: WebSocket,
    options: ResponsesWebsocketBridgeOptions<'_>,
) -> Result<(), ProxyError> {
    let ResponsesWebsocketBridgeOptions {
        headers,
        connect_timeout,
        ws_url,
        mode,
        grok_session_id,
        single_upstream_model,
        state,
        execution,
    } = options;
    let mut request = ws_url.into_client_request().map_err(|error| {
        ProxyError::bad_gateway(format!("build responses websocket request: {error}"))
    })?;
    for (name, value) in headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };
        request.headers_mut().insert(name, value);
    }
    if matches!(mode, ResponsesWebsocketMode::Codex) {
        request.headers_mut().insert(
            HeaderName::from_static("openai-beta"),
            HeaderValue::from_static("responses_websockets=2026-02-06"),
        );
    }

    let connect = tokio_tungstenite::connect_async(request);
    let connect_result = tokio::time::timeout(connect_timeout, connect)
        .await
        .map_err(|_| ProxyError {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "codex websocket connect timeout".to_string(),
        })?;
    let (upstream, _) = match connect_result {
        Ok(upstream) => upstream,
        Err(error) => return Err(responses_websocket_connect_error(state, execution, error).await),
    };
    let (mut upstream_write, mut upstream_read) = upstream.split();
    let (mut downstream_write, mut downstream_read) = downstream.split();

    let client_to_upstream = async {
        while let Some(message) = downstream_read.next().await {
            let message = message.map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
            let Some(message) = axum_ws_message_to_tungstenite(
                message,
                mode,
                grok_session_id.as_deref(),
                single_upstream_model.as_deref(),
            ) else {
                break;
            };
            upstream_write
                .send(message)
                .await
                .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
        }
        Ok::<(), ProxyError>(())
    };

    let upstream_to_client = async {
        let mut output_patcher = CodexWebsocketOutputPatcher::default();
        while let Some(message) = upstream_read.next().await {
            let mut message = match message {
                Ok(message) => message,
                Err(error) if websocket_message_too_big(&error) => {
                    let body = websocket_message_too_big_error_body();
                    let _ = downstream_write
                        .send(AxumWsMessage::Text(body.clone()))
                        .await;
                    return Err(ProxyError {
                        status: StatusCode::PAYLOAD_TOO_LARGE,
                        message: body,
                    });
                }
                Err(error) if websocket_expected_reset(&error) => return Ok(()),
                Err(error) => return Err(ProxyError::bad_gateway(error.to_string())),
            };
            if matches!(mode, ResponsesWebsocketMode::Codex) {
                output_patcher.patch_message(&mut message);
            }
            let Some(message) = tungstenite_message_to_axum_ws(message) else {
                break;
            };
            downstream_write
                .send(message)
                .await
                .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
        }
        Ok::<(), ProxyError>(())
    };

    tokio::select! {
        result = client_to_upstream => result,
        result = upstream_to_client => result,
    }
}

async fn responses_websocket_connect_error(
    state: &ServerState,
    execution: &ProviderExecution,
    error: TungsteniteError,
) -> ProxyError {
    let Some((status, headers, body)) = responses_websocket_http_error(&error) else {
        return ProxyError::bad_gateway(format!("codex websocket connect: {error}"));
    };
    maybe_mark_upstream_rate_limited(state, execution, status, &headers, &body).await;
    ProxyError {
        status,
        message: format!(
            "responses websocket upstream returned HTTP {}",
            status.as_u16()
        ),
    }
}

fn responses_websocket_http_error(
    error: &TungsteniteError,
) -> Option<(StatusCode, HeaderMap, Vec<u8>)> {
    let TungsteniteError::Http(response) = error else {
        return None;
    };
    let status = StatusCode::from_u16(response.status().as_u16()).ok()?;
    let headers = response.headers().clone();
    let body = response.body().clone().unwrap_or_default();
    Some((status, headers, body))
}

#[derive(Debug, Default)]
struct CodexWebsocketOutputPatcher {
    output_items_by_index: BTreeMap<i64, Value>,
    output_items_fallback: Vec<Value>,
}

impl CodexWebsocketOutputPatcher {
    fn patch_message(&mut self, message: &mut TungsteniteMessage) {
        let text = match message {
            TungsteniteMessage::Text(text) => Some(text.to_string()),
            TungsteniteMessage::Binary(bytes) => {
                std::str::from_utf8(bytes).ok().map(str::to_string)
            }
            _ => None,
        };
        let Some(text) = text else {
            return;
        };
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => self.collect(&value),
            Some("response.completed") => {
                let patched = self.patch_completed(&mut value);
                self.clear();
                if patched {
                    let Ok(text) = serde_json::to_string(&value) else {
                        return;
                    };
                    match message {
                        TungsteniteMessage::Text(value) => *value = text,
                        TungsteniteMessage::Binary(value) => *value = text.into_bytes(),
                        _ => {}
                    }
                }
            }
            Some("response.failed") | Some("response.incomplete") => self.clear(),
            _ => {}
        }
    }

    fn collect(&mut self, value: &Value) {
        let Some(item) = value.get("item").filter(|item| item.is_object()).cloned() else {
            return;
        };
        if let Some(index) = value.get("output_index").and_then(Value::as_i64) {
            self.output_items_by_index.insert(index, item);
        } else {
            self.output_items_fallback.push(item);
        }
    }

    fn patch_completed(&self, value: &mut Value) -> bool {
        if self.output_items_by_index.is_empty() && self.output_items_fallback.is_empty() {
            return false;
        }
        if value
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        {
            return false;
        }
        let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) else {
            return false;
        };
        response.insert(
            "output".to_string(),
            Value::Array(
                self.output_items_by_index
                    .values()
                    .cloned()
                    .chain(self.output_items_fallback.iter().cloned())
                    .collect(),
            ),
        );
        true
    }

    fn clear(&mut self) {
        self.output_items_by_index.clear();
        self.output_items_fallback.clear();
    }
}

fn axum_ws_message_to_tungstenite(
    message: AxumWsMessage,
    mode: ResponsesWebsocketMode,
    grok_session_id: Option<&str>,
    single_upstream_model: Option<&str>,
) -> Option<TungsteniteMessage> {
    match message {
        AxumWsMessage::Text(text) => {
            let text = transform_responses_websocket_request(
                &text,
                mode,
                grok_session_id,
                single_upstream_model,
            )
            .unwrap_or(text);
            Some(TungsteniteMessage::Text(text))
        }
        AxumWsMessage::Binary(bytes) => {
            let transformed = std::str::from_utf8(&bytes).ok().and_then(|text| {
                transform_responses_websocket_request(
                    text,
                    mode,
                    grok_session_id,
                    single_upstream_model,
                )
            });
            Some(TungsteniteMessage::Binary(
                transformed
                    .map(String::into_bytes)
                    .unwrap_or_else(|| bytes.to_vec()),
            ))
        }
        AxumWsMessage::Ping(bytes) => Some(TungsteniteMessage::Ping(bytes.to_vec())),
        AxumWsMessage::Pong(bytes) => Some(TungsteniteMessage::Pong(bytes.to_vec())),
        AxumWsMessage::Close(frame) => Some(TungsteniteMessage::Close(frame.map(|frame| {
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        }))),
    }
}

fn transform_responses_websocket_request(
    text: &str,
    mode: ResponsesWebsocketMode,
    grok_session_id: Option<&str>,
    single_upstream_model: Option<&str>,
) -> Option<String> {
    if !matches!(mode, ResponsesWebsocketMode::Grok) && single_upstream_model.is_none() {
        return None;
    }
    let mut value = serde_json::from_str::<Value>(text).ok()?;
    if let Some(model) = single_upstream_model {
        enforce_responses_websocket_model(&mut value, model);
    }
    if matches!(mode, ResponsesWebsocketMode::Grok) {
        value = super::grok::ws_message_body(value, grok_session_id);
    }
    serde_json::to_string(&value).ok()
}

fn enforce_responses_websocket_model(value: &mut Value, model: &str) {
    let target = if value.get("type").and_then(Value::as_str) == Some("response.create") {
        value.get_mut("response")
    } else if value.get("type").is_none() {
        Some(value)
    } else {
        None
    };
    if let Some(target) = target.and_then(Value::as_object_mut) {
        target.insert("model".to_string(), Value::String(model.to_string()));
    }
}

fn tungstenite_message_to_axum_ws(message: TungsteniteMessage) -> Option<AxumWsMessage> {
    match message {
        TungsteniteMessage::Text(text) => Some(AxumWsMessage::Text(text.to_string())),
        TungsteniteMessage::Binary(bytes) => Some(AxumWsMessage::Binary(bytes)),
        TungsteniteMessage::Ping(bytes) => Some(AxumWsMessage::Ping(bytes)),
        TungsteniteMessage::Pong(bytes) => Some(AxumWsMessage::Pong(bytes)),
        TungsteniteMessage::Close(frame) => {
            if frame
                .as_ref()
                .is_some_and(|frame| frame.code == CloseCode::Size)
            {
                return Some(AxumWsMessage::Text(websocket_message_too_big_error_body()));
            }
            Some(AxumWsMessage::Close(frame.map(|frame| {
                axum::extract::ws::CloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.to_string().into(),
                }
            })))
        }
        TungsteniteMessage::Frame(_) => None,
    }
}

fn websocket_message_too_big(error: &TungsteniteError) -> bool {
    matches!(
        error,
        TungsteniteError::Capacity(CapacityError::MessageTooLong { .. })
    ) || error.to_string().contains("1009")
}

fn websocket_expected_reset(error: &TungsteniteError) -> bool {
    match error {
        TungsteniteError::ConnectionClosed
        | TungsteniteError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => true,
        TungsteniteError::Io(error) => {
            matches!(error.raw_os_error(), Some(54 | 104 | 995 | 10053 | 10054))
        }
        _ => false,
    }
}

fn websocket_message_too_big_error_body() -> String {
    json!({
        "error": {
            "message": "upstream websocket message too big",
            "type": "invalid_request_error",
            "code": "message_too_big"
        }
    })
    .to_string()
}

async fn maybe_mark_upstream_rate_limited(
    state: &ServerState,
    execution: &ProviderExecution,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return;
    }
    let Some((provider_type, requested_account_id)) = execution.managed_account_target() else {
        return;
    };
    let Some(account_id) = state
        .find_account_for_provider(provider_type, requested_account_id)
        .await
        .map(|account| account.id)
    else {
        return;
    };
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let Some(until) = upstream_rate_limit_until(provider_type, status, headers, body, now) else {
        return;
    };
    let message = format!("upstream returned 429; account is rate limited until {until}");
    state
        .mark_account_rate_limited_until(&account_id, until, Some(message))
        .await;
}

fn upstream_rate_limit_until(
    provider_type: ProviderType,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> Option<i64> {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    let specialized_until = match provider_type {
        ProviderType::CodexOAuth => codex_rate_limit_reset_at_ms(body, now),
        ProviderType::GrokOAuth => {
            super::grok::parse_cooldown_until_ms(status, headers, now).map(|(until, _)| until)
        }
        _ => None,
    };
    let until = specialized_until
        .or_else(|| super::grok::retry_after_until_ms(headers, now))
        .unwrap_or_else(|| now.saturating_add(DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS));
    Some(super::bounded_upstream_rate_limit_until(now, until))
}

async fn maybe_mark_grok_cooldown(
    state: &ServerState,
    stored: &StoredProvider,
    status: StatusCode,
    headers: &HeaderMap,
) {
    if stored.provider_type != ProviderType::GrokOAuth || status == StatusCode::TOO_MANY_REQUESTS {
        return;
    }
    let Some(account_id) = managed_account_id(stored).map(str::to_string) else {
        return;
    };
    let now = crate::infra::time::now_ms() as i64;
    let Some((until, message)) = super::grok::parse_cooldown_until_ms(status, headers, now) else {
        return;
    };
    state
        .mark_account_rate_limited_until(&account_id, until, Some(message))
        .await;
}

async fn maybe_update_grok_entitlement(
    state: &ServerState,
    stored: &StoredProvider,
    headers: &HeaderMap,
) {
    if stored.provider_type != ProviderType::GrokOAuth {
        return;
    }
    let Some(account_id) = managed_account_id(stored).map(str::to_string) else {
        return;
    };
    let subscription_level = optional_header(headers, "xai-subscription-tier");
    let entitlement_status = optional_header(headers, "xai-entitlement-status");
    if subscription_level.is_none() && entitlement_status.is_none() {
        return;
    }
    state
        .update_account_entitlement_snapshot(
            &account_id,
            subscription_level,
            entitlement_status,
            crate::infra::time::now_ms() as i64,
        )
        .await;
}

fn codex_rate_limit_reset_at_ms(body: &[u8], now_ms: i64) -> Option<i64> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let seconds = value
        .pointer("/error/resets_in_seconds")
        .or_else(|| value.pointer("/body/error/resets_in_seconds"))
        .or_else(|| value.pointer("/response/error/resets_in_seconds"))
        .and_then(Value::as_i64);
    if let Some(seconds) = seconds.filter(|seconds| *seconds > 0) {
        return Some(now_ms.saturating_add(seconds.saturating_mul(1000)));
    }
    value
        .pointer("/error/resets_at")
        .or_else(|| value.pointer("/body/error/resets_at"))
        .or_else(|| value.pointer("/response/error/resets_at"))
        .and_then(Value::as_i64)
        .map(|value| {
            if value < 10_000_000_000 {
                value.saturating_mul(1000)
            } else {
                value
            }
        })
        .filter(|until| *until > now_ms)
}

struct ClaudeKiroForwardOptions {
    state: ServerState,
    execution: ProviderExecution,
    stored: StoredProvider,
    headers: HeaderMap,
    body: Bytes,
    request_context: UsageLogContext,
    share_invocation_guard: Option<ShareInFlightGuard>,
    started: Instant,
}

struct ClaudeDeepSeekForwardOptions {
    state: ServerState,
    execution: ProviderExecution,
    stored: StoredProvider,
    body: Bytes,
    request_context: UsageLogContext,
    share_invocation_guard: Option<ShareInFlightGuard>,
    started: Instant,
}

async fn forward_claude_deepseek(
    options: ClaudeDeepSeekForwardOptions,
) -> Result<Response, ProxyError> {
    let ClaudeDeepSeekForwardOptions {
        state,
        execution,
        stored,
        body,
        request_context,
        share_invocation_guard,
        started,
    } = options;
    let (body, model_selection) =
        adapters::apply_provider_model_routing(body, &stored, ProxyRoute::ClaudeMessages);
    let mut runtime_request = adapters::AdapterRequest {
        body,
        upstream_endpoint: None,
        upstream_headers: Vec::new(),
        model: model_selection
            .actual_model
            .clone()
            .or_else(|| model_selection.requested_model.clone()),
        requested_model: model_selection.requested_model.clone(),
        actual_model: model_selection.actual_model.clone(),
        actual_model_source: model_selection.actual_model_source.clone(),
        stream_requested: false,
        custom_tool_names: Default::default(),
    };
    execution.enforce_model_policy(&mut runtime_request)?;
    let body = runtime_request.body;
    let request_body: Value = serde_json::from_slice(&body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Claude JSON body: {error}")))?;
    let routed_model = request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("missing model"))?
        .to_string();
    let response_model = runtime_request
        .requested_model
        .clone()
        .unwrap_or_else(|| routed_model.clone());
    let stream_requested = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let prompt = deepseek::build_prompt(&request_body)?;
    let input_tokens = deepseek::estimate_billable_user_input_tokens(&request_body);
    let deepseek_model = deepseek::map_model(&routed_model);
    let model_metadata = routed_model_metadata(
        &response_model,
        &deepseek_model,
        runtime_request.actual_model_source.as_deref(),
        "deepseek_model_normalization",
    );

    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    let accounts = state.accounts_snapshot().await;
    execution.materialize_auth(&accounts)?;
    let upstream = state
        .start_deepseek_chat_completion(execution.managed_account_id(), &deepseek_model, &prompt)
        .await
        .map_err(deepseek_upstream_error_to_proxy_error)?;
    let status = upstream.status();
    let status_code = status.as_u16();

    if !status.is_success() {
        let response_headers = upstream.headers().clone();
        let body = upstream.text().await.unwrap_or_default();
        maybe_mark_upstream_rate_limited(
            &state,
            &execution,
            status,
            &response_headers,
            body.as_bytes(),
        )
        .await;
        record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;
        return Err(ProxyError {
            status: StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            message: format!("DeepSeek upstream returned HTTP {status_code}: {body}"),
        });
    }

    if stream_requested {
        let request_id = log_usage(
            &state,
            &stored,
            status_code,
            started.elapsed().as_millis(),
            model_metadata.clone(),
            TokenUsage {
                input_tokens: Some(u64::from(input_tokens)),
                ..Default::default()
            },
            UsageLogContext {
                is_streaming: true,
                stream_status: Some("pending".to_string()),
                ..request_context.clone()
            },
        )
        .await;
        let share_id = request_context.share_id.clone();
        let user_email = request_context.user_email.clone();
        let sse_stream = deepseek::deepseek_bytes_stream_to_claude_sse(
            upstream.bytes_stream(),
            response_model,
            input_tokens,
        );
        let stream = async_stream::stream! {
            let _share_invocation_guard = share_invocation_guard;
            let mut interrupt_guard = ShareStreamInterruptGuard {
                armed: true,
                state: state.clone(),
                stored: stored.clone(),
                request_id: request_id.clone(),
                status_code,
                share_id: share_id.clone(),
                user_email: user_email.clone(),
                started,
                first_token_ms: None,
                usage: StreamUsageAccumulator::default(),
            };
            let mut first_token_ms = None;
            tokio::pin!(sse_stream);
            while let Some(chunk) = sse_stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        let usage = std::mem::take(&mut interrupt_guard.usage).finish();
                        update_stream_usage(
                            &state,
                            &stored,
                            &request_id,
                            StatusCode::BAD_GATEWAY.as_u16(),
                            started.elapsed().as_millis(),
                            first_token_ms,
                            usage,
                            Some("upstream_error"),
                        )
                        .await;
                        record_share_invocation_result(
                            &state,
                            share_id.as_deref(),
                            user_email.as_deref(),
                            usage,
                        ).await;
                        record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                        interrupt_guard.disarm();
                        yield Err::<Bytes, std::io::Error>(error);
                        return;
                    }
                };
                interrupt_guard.usage.push(&chunk);
                if first_token_ms.is_none() && !chunk.is_empty() {
                    first_token_ms = Some(started.elapsed().as_millis());
                    interrupt_guard.first_token_ms = first_token_ms;
                }
                yield Ok::<Bytes, std::io::Error>(chunk);
            }
            let usage = std::mem::take(&mut interrupt_guard.usage).finish();
            update_stream_usage(
                &state,
                &stored,
                &request_id,
                status_code,
                started.elapsed().as_millis(),
                first_token_ms,
                usage,
                Some("completed"),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                usage,
            ).await;
            record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;
            interrupt_guard.disarm();
        };
        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = StatusCode::OK;
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        return Ok(response);
    }

    let body_text = upstream.text().await.unwrap_or_default();
    let text = deepseek::collect_text_from_sse_body(&body_text);
    let output_tokens = deepseek::estimate_tokens(&text);
    let message =
        deepseek::claude_message_json(&text, &response_model, input_tokens, output_tokens);
    let bytes =
        serde_json::to_vec(&message).map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
    let usage = TokenUsage {
        input_tokens: Some(u64::from(input_tokens)),
        output_tokens: Some(u64::from(output_tokens)),
        ..Default::default()
    };
    let share_id_for_record = request_context.share_id.clone();
    let user_email_for_record = request_context.user_email.clone();
    log_usage(
        &state,
        &stored,
        status_code,
        started.elapsed().as_millis(),
        model_metadata,
        usage,
        request_context,
    )
    .await;
    record_share_invocation_result(
        &state,
        share_id_for_record.as_deref(),
        user_email_for_record.as_deref(),
        usage,
    )
    .await;
    record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;
    drop(share_invocation_guard);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}

fn deepseek_upstream_error_to_proxy_error(error: DeepSeekUpstreamError) -> ProxyError {
    match error {
        DeepSeekUpstreamError::NotFound => {
            ProxyError::not_found("deepseek_account managed account not found")
        }
        DeepSeekUpstreamError::MissingToken => ProxyError {
            status: StatusCode::UNAUTHORIZED,
            message: "deepseek account access token is missing".to_string(),
        },
        DeepSeekUpstreamError::Client(message) => ProxyError::bad_gateway(message),
    }
}

async fn forward_claude_kiro(options: ClaudeKiroForwardOptions) -> Result<Response, ProxyError> {
    let ClaudeKiroForwardOptions {
        state,
        execution,
        stored,
        headers,
        body,
        request_context,
        share_invocation_guard,
        started,
    } = options;
    let (body, model_selection) =
        adapters::apply_provider_model_routing(body, &stored, ProxyRoute::ClaudeMessages);
    let mut runtime_request = adapters::AdapterRequest {
        body,
        upstream_endpoint: None,
        upstream_headers: Vec::new(),
        model: model_selection
            .actual_model
            .clone()
            .or_else(|| model_selection.requested_model.clone()),
        requested_model: model_selection.requested_model.clone(),
        actual_model: model_selection.actual_model.clone(),
        actual_model_source: model_selection.actual_model_source.clone(),
        stream_requested: false,
        custom_tool_names: Default::default(),
    };
    execution.enforce_model_policy(&mut runtime_request)?;
    let body = runtime_request.body;
    let request_body: Value = serde_json::from_slice(&body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Claude JSON body: {error}")))?;
    let routed_model = request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("missing model"))?
        .to_string();
    let response_model = runtime_request
        .requested_model
        .clone()
        .unwrap_or_else(|| routed_model.clone());
    let actual_model = kiro::map_model(&routed_model)
        .ok_or_else(|| ProxyError::bad_request(format!("Kiro OAuth 不支持该模型: {routed_model}")))?
        .to_string();
    let model_metadata = routed_model_metadata(
        &response_model,
        &actual_model,
        runtime_request.actual_model_source.as_deref(),
        "kiro_model_normalization",
    );
    let stream_requested = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    let accounts = state.accounts_snapshot().await;
    execution.materialize_auth(&accounts)?;
    let account = state
        .find_account_for_provider(ProviderType::KiroOAuth, execution.managed_account_id())
        .await
        .ok_or_else(|| ProxyError::not_found("kiro_oauth managed account not found"))?;
    let mut prepared = kiro::prepare_kiro_request(&account, &request_body)?;
    if let Some(base_url) = kiro_api_base_override(&stored) {
        prepared.url = super::join_url(&base_url, "/generateAssistantResponse");
    }

    let http_client = forward_http_client(&state, &stored).await?;
    let mut request = http_client
        .post(&prepared.url)
        .json(&prepared.body)
        .header(ACCEPT, copy_header(&headers, ACCEPT).unwrap_or("*/*"));
    for (name, value) in &prepared.headers {
        request = request.header(*name, value);
    }
    if !stream_requested {
        request = request.timeout(execution.request_timeout());
    }

    let upstream_result = if stream_requested {
        match execution.stream_first_byte_timeout() {
            Some(timeout) => match tokio::time::timeout(timeout, request.send()).await {
                Ok(result) => result,
                Err(_) => {
                    record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                    return Err(ProxyError {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        message: format!(
                            "proxy upstream streaming first byte timeout after {}ms",
                            timeout.as_millis()
                        ),
                    });
                }
            },
            None => request.send().await,
        }
    } else {
        request.send().await
    };
    let upstream = match upstream_result {
        Ok(upstream) => upstream,
        Err(error) => {
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            return Err(ProxyError::bad_gateway(error));
        }
    };
    let status = upstream.status();
    let status_code = status.as_u16();
    let mut response_headers = upstream.headers().clone();
    strip_hop_by_hop_response_headers(&mut response_headers);

    if stream_requested && status.is_success() {
        return forward_claude_kiro_stream(ClaudeKiroStreamOptions {
            state,
            stored,
            upstream,
            response_model,
            model_metadata,
            request_body,
            tool_name_map: prepared.tool_name_map,
            request_context,
            share_invocation_guard,
            started,
            status,
            status_code,
        })
        .await;
    }

    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            return Err(ProxyError::bad_gateway(error));
        }
    };
    let decoded = decode_response_body_for_proxy(&response_headers, bytes);
    let bytes = decoded.body;
    if !status.is_success() {
        maybe_mark_upstream_rate_limited(&state, &execution, status, &response_headers, &bytes)
            .await;
        log_usage(
            &state,
            &stored,
            status_code,
            started.elapsed().as_millis(),
            model_metadata.clone(),
            TokenUsage::default(),
            UsageLogContext {
                is_streaming: stream_requested,
                ..request_context
            },
        )
        .await;
        if kiro::is_client_validation_error(&bytes) {
            tracing::warn!(
                provider_id = %stored.provider.id,
                status_code,
                "Kiro request rejected by terminal client validation; skipping provider outcome accounting"
            );
        } else {
            record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code))
                .await;
        }
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = status;
        return Ok(response);
    }

    let message = match kiro::kiro_event_bytes_to_claude_json(
        &bytes,
        &response_model,
        &prepared.tool_name_map,
        &request_body,
    ) {
        Ok(message) => message,
        Err(error) => {
            let proxy_error = ProxyError::kiro_tool_json(error);
            log_usage(
                &state,
                &stored,
                proxy_error.status.as_u16(),
                started.elapsed().as_millis(),
                model_metadata.clone(),
                TokenUsage::default(),
                UsageLogContext {
                    is_streaming: false,
                    ..request_context
                },
            )
            .await;
            tracing::warn!(
                provider_id = %stored.provider.id,
                error_code = proxy_error.error_code(),
                "Kiro non-stream response contained invalid or incomplete tool JSON"
            );
            return Err(proxy_error);
        }
    };
    let usage = crate::domain::usage::store::usage_from_json(&message);
    let response_bytes = serde_json::to_vec(&message)
        .map(Bytes::from)
        .map_err(ProxyError::bad_gateway)?;
    let share_id_for_record = request_context.share_id.clone();
    let user_email_for_record = request_context.user_email.clone();
    log_usage(
        &state,
        &stored,
        status_code,
        started.elapsed().as_millis(),
        model_metadata,
        usage,
        UsageLogContext {
            is_streaming: false,
            ..request_context
        },
    )
    .await;
    record_share_invocation_result(
        &state,
        share_id_for_record.as_deref(),
        user_email_for_record.as_deref(),
        usage,
    )
    .await;
    record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;

    let mut response = Response::new(Body::from(response_bytes));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    drop(share_invocation_guard);
    Ok(response)
}

struct ClaudeKiroStreamOptions {
    state: ServerState,
    stored: StoredProvider,
    upstream: reqwest::Response,
    response_model: String,
    model_metadata: UsageModelMetadata,
    request_body: Value,
    tool_name_map: std::collections::HashMap<String, String>,
    request_context: UsageLogContext,
    share_invocation_guard: Option<ShareInFlightGuard>,
    started: Instant,
    status: reqwest::StatusCode,
    status_code: u16,
}

async fn forward_claude_kiro_stream(
    options: ClaudeKiroStreamOptions,
) -> Result<Response, ProxyError> {
    let ClaudeKiroStreamOptions {
        state,
        stored,
        upstream,
        response_model,
        model_metadata,
        request_body,
        tool_name_map,
        request_context,
        share_invocation_guard,
        started,
        status,
        status_code,
    } = options;
    let request_id = log_usage(
        &state,
        &stored,
        status_code,
        started.elapsed().as_millis(),
        model_metadata,
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
    let stream = kiro::kiro_event_stream_to_claude_sse(
        upstream.bytes_stream(),
        response_model,
        tool_name_map,
        &request_body,
    );
    let stream = async_stream::stream! {
        let _share_invocation_guard = share_invocation_guard;
        let mut interrupt_guard = ShareStreamInterruptGuard {
            armed: true,
            state: state.clone(),
            stored: stored.clone(),
            request_id: request_id.clone(),
            status_code,
            share_id: share_id.clone(),
            user_email: user_email.clone(),
            started,
            first_token_ms: None,
            usage: StreamUsageAccumulator::default(),
        };
        let mut first_token_ms = None;
        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let usage = std::mem::take(&mut interrupt_guard.usage).finish();
                    update_stream_usage(
                        &state,
                        &stored,
                        &request_id,
                        StatusCode::BAD_GATEWAY.as_u16(),
                        started.elapsed().as_millis(),
                        first_token_ms,
                        usage,
                        Some("upstream_error"),
                    )
                    .await;
                    record_share_invocation_result(
                        &state,
                        share_id.as_deref(),
                        user_email.as_deref(),
                        usage,
                    ).await;
                    record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                    interrupt_guard.disarm();
                    yield Err::<Bytes, std::io::Error>(error);
                    return;
                }
            };
            interrupt_guard.usage.push(&chunk);
            if first_token_ms.is_none() && !chunk.is_empty() {
                let elapsed = started.elapsed().as_millis();
                first_token_ms = Some(elapsed);
                interrupt_guard.first_token_ms = first_token_ms;
                update_stream_usage(
                    &state,
                    &stored,
                    &request_id,
                    status_code,
                    elapsed,
                    first_token_ms,
                    Default::default(),
                    Some("streaming"),
                )
                .await;
            }
            yield Ok::<Bytes, std::io::Error>(chunk);
        }
        let usage = std::mem::take(&mut interrupt_guard.usage).finish();
        update_stream_usage(
            &state,
            &stored,
            &request_id,
            status_code,
            started.elapsed().as_millis(),
            first_token_ms,
            usage,
            Some("completed"),
        )
        .await;
        record_share_invocation_result(
            &state,
            share_id.as_deref(),
            user_email.as_deref(),
            usage,
        ).await;
        record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;
        interrupt_guard.disarm();
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    Ok(response)
}

struct ShareStreamInterruptGuard {
    armed: bool,
    state: ServerState,
    stored: StoredProvider,
    request_id: String,
    status_code: u16,
    share_id: Option<String>,
    user_email: Option<String>,
    started: Instant,
    first_token_ms: Option<u128>,
    usage: StreamUsageAccumulator,
}

impl ShareStreamInterruptGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ShareStreamInterruptGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let state = self.state.clone();
        let stored = self.stored.clone();
        let request_id = self.request_id.clone();
        let status_code = self.status_code;
        let share_id = self.share_id.clone();
        let user_email = self.user_email.clone();
        let usage = std::mem::take(&mut self.usage).finish();
        let duration_ms = self.started.elapsed().as_millis();
        let first_token_ms = self.first_token_ms;
        tokio::spawn(async move {
            update_stream_usage(
                &state,
                &stored,
                &request_id,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                Some("interrupted"),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                usage,
            )
            .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
        });
    }
}

fn routed_model_metadata(
    requested_model: &str,
    actual_model: &str,
    policy_source: Option<&str>,
    fallback_source: &str,
) -> UsageModelMetadata {
    UsageModelMetadata {
        model: Some(requested_model.to_string()),
        requested_model: Some(requested_model.to_string()),
        actual_model: Some(actual_model.to_string()),
        actual_model_source: Some(policy_source.unwrap_or(fallback_source).to_string()),
    }
}

fn kiro_api_base_override(stored: &StoredProvider) -> Option<String> {
    setting(
        &stored.provider,
        &[
            "KIRO_API_BASE_URL",
            "KIRO_BASE_URL",
            "CODEWHISPERER_BASE_URL",
        ],
    )
}

enum AccountInFlightAcquire {
    Acquired(AccountInFlightGuard),
    NotManaged,
    Saturated,
}

fn try_acquire_account_in_flight(
    state: &ServerState,
    stored: &StoredProvider,
    accounts: &crate::domain::accounts::store::AccountStore,
    snapshot: &AccountInFlightSnapshot,
) -> AccountInFlightAcquire {
    let Some(selection) = account_concurrency_for_provider(stored, accounts, snapshot) else {
        return AccountInFlightAcquire::NotManaged;
    };
    match state.account_in_flight.try_acquire(
        selection.provider_type,
        &selection.account_id,
        selection.max_concurrent,
    ) {
        Some(guard) => AccountInFlightAcquire::Acquired(guard),
        None => AccountInFlightAcquire::Saturated,
    }
}

fn acquire_account_in_flight(
    state: &ServerState,
    stored: &StoredProvider,
    accounts: &crate::domain::accounts::store::AccountStore,
    snapshot: &AccountInFlightSnapshot,
) -> Result<Option<AccountInFlightGuard>, ProxyError> {
    match try_acquire_account_in_flight(state, stored, accounts, snapshot) {
        AccountInFlightAcquire::Acquired(guard) => Ok(Some(guard)),
        AccountInFlightAcquire::NotManaged => Ok(None),
        AccountInFlightAcquire::Saturated => Err(account_concurrency_proxy_error(stored)),
    }
}

fn account_concurrency_proxy_error(stored: &StoredProvider) -> ProxyError {
    ProxyError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message: format!(
            "provider {} account concurrency limit has been reached",
            stored.provider.id
        ),
    }
}

async fn validate_and_acquire_share_invocation(
    state: &ServerState,
    share_id: &str,
    user_email: Option<&str>,
) -> Result<(String, ShareInFlightGuard), ProxyError> {
    let validation = state
        .validate_share_invocation(share_id, user_email, crate::infra::time::now_ms() as i64)
        .await;

    let invocation = match validation {
        Ok(invocation) => invocation,
        Err(rejection) => return Err(share_rejection_to_proxy_error(rejection)),
    };

    let guard = state
        .share_in_flight
        .try_acquire_for_user(
            &invocation.share_id,
            invocation.parallel_limit,
            invocation.user_email.as_deref(),
            invocation.user_parallel_limit,
        )
        .map_err(|limit| {
            share_rejection_to_proxy_error(ShareInvocationRejection {
                reason: match limit {
                    crate::state::ShareInFlightAcquireError::ShareLimit => {
                        ShareRejectReason::ParallelLimit
                    }
                    crate::state::ShareInFlightAcquireError::UserLimit => {
                        ShareRejectReason::UserParallelLimit
                    }
                },
                message:
                    "Share parallel limit has been reached. Wait for an in-flight request to finish."
                        .to_string(),
                status_changed: false,
            })
        })?;
    Ok((invocation.share_name, guard))
}

fn share_rejection_to_proxy_error(rejection: ShareInvocationRejection) -> ProxyError {
    let status = match rejection.reason {
        ShareRejectReason::NotFound => StatusCode::NOT_FOUND,
        ShareRejectReason::ParallelLimit | ShareRejectReason::UserParallelLimit => {
            StatusCode::TOO_MANY_REQUESTS
        }
        ShareRejectReason::Inactive
        | ShareRejectReason::Expired
        | ShareRejectReason::Exhausted
        | ShareRejectReason::UserExpired
        | ShareRejectReason::UserExhausted => StatusCode::FORBIDDEN,
    };
    ProxyError {
        status,
        message: rejection.formatted_message(),
    }
}

pub(super) async fn record_share_invocation_result(
    state: &ServerState,
    share_id: Option<&str>,
    user_email: Option<&str>,
    usage: TokenUsage,
) {
    let Some(share_id) = share_id else {
        return;
    };
    state
        .mutate_shares_debounced(|shares| {
            shares.record_user_invocation_result(
                share_id,
                user_email,
                share_usage_tokens(usage),
                crate::infra::time::now_ms() as i64,
            );
        })
        .await;
}

pub(super) async fn record_provider_outcome(
    _state: &ServerState,
    stored: &StoredProvider,
    outcome: ProviderOutcome,
) {
    crate::metrics::record_provider_outcome(stored.app.as_str(), &stored.provider.id, outcome);
}

fn provider_outcome_from_status(status_code: u16) -> ProviderOutcome {
    if status_code == StatusCode::TOO_MANY_REQUESTS.as_u16() {
        ProviderOutcome::RateLimited { status_code }
    } else {
        ProviderOutcome::from_status(status_code)
    }
}

fn claude_non_stream_retry_stage(
    status: StatusCode,
    body: &[u8],
    current_stage: Option<ClaudeBodyRetryStage>,
    request_body: &[u8],
) -> Option<ClaudeBodyRetryStage> {
    if status != StatusCode::BAD_REQUEST {
        return None;
    }
    let message = upstream_error_message(body);
    claude_body_retry_stage_for_error_message(&message, current_stage, request_body)
}

fn maybe_rewrite_claude_cli_version_gate_body(
    status: StatusCode,
    stored: &StoredProvider,
    route: ProxyRoute,
    body: Bytes,
) -> (Bytes, bool) {
    if route != ProxyRoute::ClaudeMessages
        || stored.provider_type != ProviderType::ClaudeOAuth
        || !(status.is_client_error() || status.is_server_error())
    {
        return (body, false);
    }
    let upstream_message = upstream_error_message(&body);
    if !is_claude_cli_version_gate_message(&upstream_message) {
        return (body, false);
    }

    tracing::error!(
        provider_id = %stored.provider.id,
        cli_version = %crate::domain::claude_cli::claude_cli_version(),
        "Claude OAuth upstream rejected the advertised Claude Code CLI version; set CC_SWITCH_CLI_UA_VERSION or CC_SWITCH_CLI_UA to a currently accepted version"
    );
    crate::metrics::record_claude_cli_version_gate();

    let admin_message = claude_cli_version_gate_admin_message();
    let bytes = rewrite_error_message_body(&body, &admin_message)
        .unwrap_or_else(|| Bytes::from(admin_message));
    (bytes, true)
}

fn is_claude_cli_version_gate_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("npm update -g @anthropic-ai/claude-code")
        || message.contains("claude-code@latest")
        || message.contains("please update your claude code")
        || (message.contains("claude code")
            && (message.contains("out of date")
                || message.contains("update")
                || message.contains("upgrade")))
}

fn claude_cli_version_gate_admin_message() -> String {
    format!(
        "Claude OAuth upstream rejected the advertised Claude Code CLI version (currently {}). cc-switch-server admin: bump CC_SWITCH_CLI_UA_VERSION or CC_SWITCH_CLI_UA to a currently accepted Claude Code version.",
        crate::domain::claude_cli::claude_cli_version()
    )
}

fn rewrite_error_message_body(body: &[u8], message: &str) -> Option<Bytes> {
    let mut value = serde_json::from_slice::<Value>(body).ok()?;
    let mut replaced = false;
    if let Some(existing) = value.pointer_mut("/error/message") {
        *existing = Value::String(message.to_string());
        replaced = true;
    }
    if let Some(existing) = value.get_mut("message") {
        *existing = Value::String(message.to_string());
        replaced = true;
    }
    if !replaced {
        value = json!({
            "error": {
                "type": "claude_code_version_gate",
                "message": message,
            }
        });
    }
    serde_json::to_vec(&value).ok().map(Bytes::from)
}

fn upstream_error_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).to_string())
}

fn claude_body_retry_stage_for_error_message(
    message: &str,
    current_stage: Option<ClaudeBodyRetryStage>,
    request_body: &[u8],
) -> Option<ClaudeBodyRetryStage> {
    let message = message.to_ascii_lowercase();
    let web_search_error = message.contains("web_search")
        || message.contains("server_tool_use")
        || message.contains("web_search_tool_result");
    if web_search_error && current_stage != Some(ClaudeBodyRetryStage::WebSearchHistory) {
        return Some(ClaudeBodyRetryStage::WebSearchHistory);
    }

    let signature_error = message.contains("signature")
        || message.contains("thought_signature")
        || message.contains("expected `thinking`")
        || message.contains("expected thinking")
        || message.contains("redacted_thinking");
    if !signature_error {
        return None;
    }

    let tool_signature_error = message.contains("tool_use")
        || message.contains("tool_result")
        || message.contains("functioncall")
        || message.contains("function_call")
        || message.contains("functionresponse")
        || message.contains("function_response");
    match current_stage {
        None => Some(ClaudeBodyRetryStage::Thinking),
        Some(ClaudeBodyRetryStage::Thinking) if tool_signature_error => {
            Some(ClaudeBodyRetryStage::SignatureSensitive)
        }
        Some(ClaudeBodyRetryStage::SignatureSensitive)
            if super::claude_oauth::body_contains_web_search_history_blocks(request_body) =>
        {
            Some(ClaudeBodyRetryStage::WebSearchHistory)
        }
        _ => None,
    }
}

fn claude_sse_error_detector_for(
    _stored: &StoredProvider,
    route: ProxyRoute,
) -> Option<ClaudeSseErrorDetector> {
    (route == ProxyRoute::ClaudeMessages).then(ClaudeSseErrorDetector::default)
}

fn claude_sse_error_outcome(error_type: &str) -> Option<ProviderOutcome> {
    match error_type {
        "rate_limit_error" => Some(ProviderOutcome::RateLimited {
            status_code: StatusCode::TOO_MANY_REQUESTS.as_u16(),
        }),
        "overloaded_error" => Some(ProviderOutcome::Failure { status_code: 529 }),
        "api_error" => Some(ProviderOutcome::Failure { status_code: 500 }),
        _ => None,
    }
}

fn share_usage_tokens(usage: TokenUsage) -> u64 {
    usage
        .total_tokens
        .or_else(|| match (usage.input_tokens, usage.output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            (Some(input), None) => Some(input),
            (None, Some(output)) => Some(output),
            (None, None) => None,
        })
        .unwrap_or(0)
}

fn join_bytes(first: Bytes, second: Bytes) -> Bytes {
    if first.is_empty() {
        return second;
    }
    if second.is_empty() {
        return first;
    }
    let mut joined = Vec::with_capacity(first.len() + second.len());
    joined.extend_from_slice(&first);
    joined.extend_from_slice(&second);
    Bytes::from(joined)
}

async fn refresh_managed_account_if_needed(
    state: &ServerState,
    app: AppKind,
    stored: &StoredProvider,
) -> Result<(), ProxyError> {
    if provider_secret_configured(app, stored) {
        return Ok(());
    }

    state
        .refresh_managed_account_if_needed(stored.provider_type, managed_account_id(stored))
        .await
        .map_err(managed_account_refresh_error_to_proxy_error)
}

async fn refresh_execution_managed_account_if_needed(
    state: &ServerState,
    execution: &ProviderExecution,
) -> Result<(), ProxyError> {
    let Some((provider_type, account_id)) = execution.managed_account_target() else {
        return Ok(());
    };
    state
        .refresh_managed_account_if_needed(provider_type, account_id)
        .await
        .map_err(managed_account_refresh_error_to_proxy_error)
}

async fn next_claude_transport_attempt(
    state: &ServerState,
    route: ProxyRoute,
    headers: &HeaderMap,
    request_context: &UsageLogContext,
    attempt_context: &ForwardAttemptContext,
    failed: &ProviderExecution,
    reason: &'static str,
) -> Option<ForwardAttemptContext> {
    if !matches!(
        route,
        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
    ) || !attempt_context.retry_allowed()
    {
        return None;
    }
    if claude_request_is_provider_pinned(headers, request_context) {
        crate::metrics::record_claude_retry("transport", reason);
        return Some(attempt_context.next(failed, attempt_context.body_retry_stage));
    }
    next_claude_provider_failover(state, route, attempt_context, failed, reason).await
}

async fn next_claude_provider_failover(
    state: &ServerState,
    route: ProxyRoute,
    attempt_context: &ForwardAttemptContext,
    failed: &ProviderExecution,
    reason: &'static str,
) -> Option<ForwardAttemptContext> {
    if !attempt_context.retry_allowed() {
        return None;
    }
    let mut excluded = attempt_context.excluded_provider_ids.clone();
    excluded.insert(failed.stored.provider.id.clone());
    let accounts = state.accounts_snapshot().await;
    let in_flight = state.account_in_flight.snapshot();
    let providers = state.providers.read().await;
    let next =
        select_failover_provider(&providers, &accounts, route, &in_flight, &excluded)?.execution;
    tracing::debug!(
        reason,
        from_provider_id = %failed.stored.provider.id,
        to_provider_id = %next.stored.provider.id,
        "switching Claude request to failover Provider"
    );
    crate::metrics::record_claude_retry("provider", reason);
    Some(attempt_context.after_provider_failover(failed, &next))
}

fn claude_request_is_provider_pinned(
    headers: &HeaderMap,
    request_context: &UsageLogContext,
) -> bool {
    request_context.share_id.is_some()
        || headers
            .get("x-cc-provider-id")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.trim().is_empty())
}

fn managed_account_id(stored: &StoredProvider) -> Option<&str> {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
}

fn grok_cli_profile(app: AppKind, stored: &StoredProvider) -> bool {
    stored.provider_type == ProviderType::GrokOAuth && !provider_secret_configured(app, stored)
}

fn grok_tenant_scope(context: &UsageLogContext, stored: &StoredProvider) -> Option<String> {
    if stored.provider_type != ProviderType::GrokOAuth {
        return None;
    }
    Some(format!(
        "share={}|user={}|provider={}|account={}",
        context.share_id.as_deref().unwrap_or("direct"),
        context.user_email.as_deref().unwrap_or("anonymous"),
        stored.provider.id,
        managed_account_id(stored).unwrap_or("provider-secret")
    ))
}

fn provider_secret_configured(app: AppKind, stored: &StoredProvider) -> bool {
    let provider = &stored.provider;
    match auth_header_app_for(app, stored.provider_type) {
        AppKind::Claude => setting(
            provider,
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "API_KEY"],
        )
        .is_some(),
        AppKind::Codex => super::codex_provider_api_key(provider).is_some(),
        AppKind::Gemini => {
            setting(provider, &["GEMINI_API_KEY", "GOOGLE_API_KEY", "API_KEY"]).is_some()
        }
    }
}

fn validate_codex_allowed_client(
    stored: &StoredProvider,
    route: ProxyRoute,
    headers: &HeaderMap,
    share_request: bool,
) -> Result<(), ProxyError> {
    if share_request {
        return Ok(());
    }
    if stored.provider_type != ProviderType::CodexOAuth
        || !matches!(
            route,
            ProxyRoute::CodexResponses
                | ProxyRoute::CodexResponsesCompact
                | ProxyRoute::CodexChatCompletions
        )
    {
        return Ok(());
    }
    let user_agent = optional_header(headers, "user-agent").unwrap_or_default();
    let originator = optional_header(headers, "originator").unwrap_or_default();
    if originator.trim().is_empty() {
        let ua = user_agent.to_ascii_lowercase();
        if ["curl/", "httpie", "wget/", "postmanruntime"]
            .iter()
            .any(|marker| ua.contains(marker))
        {
            return Err(ProxyError {
                status: StatusCode::FORBIDDEN,
                message: "codex oauth upstream requires an allowed Codex client signature"
                    .to_string(),
            });
        }
        return Ok(());
    }
    let originator = originator.trim().to_ascii_lowercase();
    let allowed = codex_allowed_client_signature(&originator, &user_agent);
    if allowed {
        return Ok(());
    }
    Err(ProxyError {
        status: StatusCode::FORBIDDEN,
        message: "codex oauth client originator and user-agent are not allowed".to_string(),
    })
}

fn codex_allowed_client_signature(originator: &str, user_agent: &str) -> bool {
    let originator = originator.trim().to_ascii_lowercase();
    let user_agent = user_agent.trim();
    if user_agent.is_empty() {
        return false;
    }
    let ua = user_agent.to_ascii_lowercase();
    let engine_shape = codex_official_user_agent_shape(user_agent);
    match originator.as_str() {
        "codex_cli_rs" | "codex_cli" | "codex" => ua.starts_with("codex_cli_rs/") && engine_shape,
        "codex-tui" => ua.starts_with("codex-tui/") && engine_shape,
        _ => false,
    }
}

fn codex_official_user_agent_shape(user_agent: &str) -> bool {
    let Some((prefix, rest)) = user_agent.split_once(' ') else {
        return false;
    };
    if !prefix.contains('/') || prefix.ends_with('/') {
        return false;
    }
    let Some(open) = rest.find('(') else {
        return false;
    };
    let Some(close) = rest[open + 1..].find(')') else {
        return false;
    };
    let inside = &rest[open + 1..open + 1 + close];
    let terminal = rest[open + 1 + close + 1..].trim();
    inside.contains(';') && !terminal.is_empty()
}

fn copilot_managed_account_auth_required(app: AppKind, stored: &StoredProvider) -> bool {
    stored.provider_type == ProviderType::GitHubCopilot && !provider_secret_configured(app, stored)
}

fn auth_header_app_for(app: AppKind, provider_type: ProviderType) -> AppKind {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
            AppKind::Claude
        }
        ProviderType::Codex
        | ProviderType::CodexOAuth
        | ProviderType::OllamaCloud
        | ProviderType::GrokOAuth => AppKind::Codex,
        ProviderType::Gemini | ProviderType::GeminiCli => AppKind::Gemini,
        ProviderType::OpenRouter => {
            if app == AppKind::Gemini {
                AppKind::Codex
            } else {
                app
            }
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => AppKind::Gemini,
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
            if app == AppKind::Codex {
                AppKind::Codex
            } else {
                app
            }
        }
        ProviderType::GitHubCopilot | ProviderType::DeepSeekAccount | ProviderType::KiroOAuth => {
            app
        }
        ProviderType::AwsBedrock => AppKind::Claude,
        ProviderType::Nvidia | ProviderType::DeepSeekApi => {
            if app == AppKind::Gemini {
                AppKind::Codex
            } else {
                app
            }
        }
    }
}

fn managed_account_refresh_error_to_proxy_error(error: ManagedAccountRefreshError) -> ProxyError {
    match error {
        ManagedAccountRefreshError::Conflict { provider_type } => ProxyError::conflict(format!(
            "{} account refresh is already in progress",
            provider_type.as_str()
        )),
        ManagedAccountRefreshError::NotFound => ProxyError::not_found("managed account not found"),
        ManagedAccountRefreshError::Refresh {
            status_code,
            message,
        } => ProxyError {
            status: StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            message: format!("managed account refresh failed: {message}"),
        },
    }
}

fn copilot_upstream_auth_error_to_proxy_error(error: CopilotUpstreamAuthError) -> ProxyError {
    match error {
        CopilotUpstreamAuthError::NotFound => {
            ProxyError::not_found("github_copilot managed account not found")
        }
        CopilotUpstreamAuthError::MissingGitHubToken { account_id } => ProxyError::bad_request(
            format!("github_copilot managed account {account_id} lacks a GitHub token"),
        ),
        CopilotUpstreamAuthError::TokenExchange {
            status_code,
            message,
        } => ProxyError {
            status: StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            message: format!("github_copilot token exchange failed: {message}"),
        },
    }
}

fn replace_or_push_header(
    headers: &mut Vec<(&'static str, String)>,
    name: &'static str,
    value: String,
) {
    if let Some((_, existing)) = headers
        .iter_mut()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
    {
        *existing = value;
        return;
    }
    headers.push((name, value));
}

fn owned_headers(headers: Vec<(&'static str, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

fn replace_or_push_owned_header(headers: &mut Vec<(String, String)>, name: String, value: String) {
    if let Some((_, existing)) = headers
        .iter_mut()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(&name))
    {
        *existing = value;
        return;
    }
    headers.push((name, value));
}

fn apply_account_header_overrides(
    headers: &mut Vec<(String, String)>,
    stored: &StoredProvider,
    accounts: &AccountStore,
) -> Result<(), ProxyError> {
    let Some(account) =
        accounts.find_for_provider(stored.provider_type, managed_account_id(stored))
    else {
        return Ok(());
    };
    for (name, value) in &account.extra_headers {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            ProxyError::bad_request(format!(
                "account {} extra header name is invalid: {name}",
                account.id
            ))
        })?;
        let normalized_name = header_name.as_str();
        if account_header_override_blocked(normalized_name, stored.provider_type) {
            return Err(ProxyError::bad_request(format!(
                "account {} extra header cannot override proxy-controlled header: {normalized_name}",
                account.id
            )));
        }
        HeaderValue::from_str(value).map_err(|_| {
            ProxyError::bad_request(format!(
                "account {} extra header value is invalid for {normalized_name}",
                account.id
            ))
        })?;
        replace_or_push_owned_header(headers, normalized_name.to_string(), value.clone());
    }
    Ok(())
}

fn account_header_override_blocked(name: &str, provider_type: ProviderType) -> bool {
    let normalized = name.to_ascii_lowercase();
    if provider_type == ProviderType::ClaudeOAuth
        && (matches!(
            normalized.as_str(),
            "anthropic-beta"
                | "anthropic-version"
                | "x-app"
                | "sec-fetch-mode"
                | "anthropic-dangerous-direct-browser-access"
                | "x-claude-code-session-id"
        ) || normalized.starts_with("x-stainless-"))
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxy-authorization"
            | "host"
            | "content-length"
            | "content-type"
            | "accept"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "cookie"
            | "set-cookie"
            | "user-agent"
            | "originator"
            | "version"
            | "chatgpt-account-id"
            | "session_id"
            | "x-client-request-id"
            | "x-codex-window-id"
            | "openai-beta"
    )
}

fn build_upstream_post_request(
    http_client: &reqwest::Client,
    url: &str,
    body: Bytes,
    client_headers: &HeaderMap,
    target_headers: &[(String, String)],
    request_timeout: Duration,
    stream_requested: bool,
) -> reqwest::RequestBuilder {
    let mut request = http_client
        .post(url)
        .body(body)
        .header(ACCEPT, copy_header(client_headers, ACCEPT).unwrap_or("*/*"));

    if let Some(content_type) = copy_header(client_headers, CONTENT_TYPE) {
        request = request.header(CONTENT_TYPE, content_type);
    } else {
        request = request.header(CONTENT_TYPE, "application/json");
    }

    for (name, value) in target_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if !stream_requested {
        request = request.timeout(request_timeout);
    }
    request
}

fn decoded_upstream_response(
    status: StatusCode,
    response_headers: &HeaderMap,
    content_type: Option<String>,
    content_encoding: Option<HeaderValue>,
    decoded: ResponseDecodeResult,
) -> Response {
    let mut response = Response::new(Body::from(decoded.body));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            response.headers_mut().insert(CONTENT_TYPE, value);
        }
    }
    if decoded.preserve_content_encoding {
        if let Some(value) = content_encoding {
            response.headers_mut().insert(CONTENT_ENCODING, value);
        }
    }
    copy_safe_upstream_response_headers(response_headers, &mut response);
    response
}

fn codex_oauth_session_id_from_request(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    optional_header(headers, "session_id")
        .or_else(|| optional_header(headers, "x-session-id"))
        .or_else(|| optional_header(headers, "x-codex-session-id"))
        .or_else(|| optional_header(headers, "x-client-request-id"))
        .or_else(|| optional_header(headers, "x-codex-window-id"))
        .or_else(|| codex_oauth_session_id_from_body(body))
        .and_then(|value| codex_oauth_upstream_session_id(&value))
}

fn codex_oauth_session_id_from_body(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    [
        "/metadata/session_id",
        "/metadata/sessionId",
        "/session_id",
        "/sessionId",
    ]
    .into_iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .and_then(codex_oauth_upstream_session_id)
    })
}

fn codex_oauth_upstream_session_id(session_id: &str) -> Option<String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }
    let session_id = session_id
        .strip_prefix("codex_")
        .unwrap_or(session_id)
        .trim();
    let session_id = session_id
        .split_once(':')
        .map(|(id, _)| id)
        .unwrap_or(session_id)
        .trim();
    (!session_id.is_empty()).then(|| session_id.to_string())
}

fn append_codex_oauth_session_headers(
    headers: &mut Vec<(&'static str, String)>,
    session_id: Option<&str>,
) {
    let Some(session_id) = session_id.map(str::trim).filter(|item| !item.is_empty()) else {
        return;
    };
    headers.push(("session_id", session_id.to_string()));
    headers.push(("x-client-request-id", session_id.to_string()));
    headers.push(("x-codex-window-id", format!("{session_id}:0")));
}

const CODEX_OAUTH_UNSUPPORTED_RESPONSES_FIELDS: &[&str] = &[
    "max_output_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "logit_bias",
    "logprobs",
    "top_logprobs",
    "n",
    "stop",
    "response_format",
    "seed",
    "stream_options",
    "user",
];

fn normalize_codex_oauth_responses_body_bytes(
    body: &Bytes,
    prompt_cache_key: Option<&str>,
    image_tool_strip_policy: CodexImageToolStripPolicy,
) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid codex oauth responses body: {error}"),
    })?;
    value = normalize_codex_oauth_responses_body(value, prompt_cache_key, image_tool_strip_policy);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode codex oauth responses body failed: {error}"),
        })
}

fn normalize_codex_oauth_compact_body_bytes(body: &Bytes) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid codex oauth compact body: {error}"),
    })?;
    if let Some(object) = value.as_object_mut() {
        object.remove("stream");
        object.remove("store");
        object.remove("prompt_cache_key");
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode codex oauth compact body failed: {error}"),
        })
}

fn codex_responses_body_has_compaction_trigger(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    value
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction_trigger"))
        })
}

fn codex_compact_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/responses/compact") {
        trimmed.to_string()
    } else if trimmed.ends_with("/responses") {
        format!("{trimmed}/compact")
    } else {
        url.to_string()
    }
}

fn normalize_codex_oauth_responses_body(
    mut body: Value,
    prompt_cache_key: Option<&str>,
    image_tool_strip_policy: CodexImageToolStripPolicy,
) -> Value {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    body["store"] = Value::Bool(false);
    body["stream"] = Value::Bool(true);

    if let (Some(model), Some(effort)) = (
        model.as_deref(),
        body.pointer("/reasoning/effort").and_then(Value::as_str),
    ) {
        let normalized = super::codex_models::normalize_reasoning_effort(model, effort);
        body["reasoning"]["effort"] = Value::String(normalized);
    }

    if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("message") {
                let invalid_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.starts_with("msg"));
                if invalid_id {
                    if let Some(object) = item.as_object_mut() {
                        object.remove("id");
                    }
                }
            }
        }
    }

    if body.get("prompt_cache_key").is_none() {
        if let Some(key) = prompt_cache_key
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            body["prompt_cache_key"] = Value::String(key.to_string());
        }
    }

    match body.get_mut("include") {
        Some(Value::Array(include)) => {
            let required = Value::String("reasoning.encrypted_content".to_string());
            if !include.iter().any(|item| item == &required) {
                include.push(required);
            }
        }
        _ => {
            body["include"] = Value::Array(vec![Value::String(
                "reasoning.encrypted_content".to_string(),
            )]);
        }
    }

    let existing_instructions = body
        .get("instructions")
        .and_then(response_instruction_text_for_codex);
    body["instructions"] = Value::String(crate::proxy::codex_instructions::merged_instructions(
        model.as_deref(),
        existing_instructions.as_deref(),
    ));
    if body.get("tools").is_none() {
        body["tools"] = Value::Array(Vec::new());
    }
    if image_tool_strip_policy == CodexImageToolStripPolicy::Always {
        strip_codex_image_generation_tools(&mut body);
    }
    if body.get("parallel_tool_calls").is_none() {
        body["parallel_tool_calls"] = Value::Bool(false);
    }

    if let Some(obj) = body.as_object_mut() {
        for field in CODEX_OAUTH_UNSUPPORTED_RESPONSES_FIELDS {
            obj.remove(*field);
        }
    }

    body
}

fn codex_image_tool_strip_policy(stored: &StoredProvider) -> CodexImageToolStripPolicy {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.codex_image_tool_strip_policy)
        .unwrap_or(CodexImageToolStripPolicy::Never)
}

fn codex_image_tool_stripped_body_bytes(body: &Bytes) -> Result<Option<Bytes>, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid codex oauth responses body: {error}"),
    })?;
    if !strip_codex_image_generation_tools(&mut value) {
        return Ok(None);
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map(Some)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode codex oauth responses body failed: {error}"),
        })
}

fn strip_codex_image_generation_tools(body: &mut Value) -> bool {
    let mut stripped = false;
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        let before = tools.len();
        tools.retain(|tool| !is_codex_image_generation_tool(tool));
        stripped |= tools.len() != before;
    }
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            if let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) {
                let before = tools.len();
                tools.retain(|tool| !is_codex_image_generation_tool(tool));
                stripped |= tools.len() != before;
            }
        }
    }
    stripped
}

fn is_codex_image_generation_tool(tool: &Value) -> bool {
    matches!(
        tool.get("type").and_then(Value::as_str),
        Some("image_generation") | Some("image_gen")
    ) || tool
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| matches!(name, "image_generation" | "image_gen"))
}

fn codex_image_tool_rejection_body(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    (text.contains("image_generation") || text.contains("image_gen"))
        && [
            "unsupported",
            "not allowed",
            "forbidden",
            "invalid",
            "unknown tool",
            "unrecognized",
            "permission",
        ]
        .iter()
        .any(|marker| text.contains(marker))
}

fn response_instruction_text_for_codex(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    }
}

struct StreamForwardState {
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    stored: StoredProvider,
    state: ServerState,
    route: ProxyRoute,
    request_id: String,
    status_code: u16,
    share_id: Option<String>,
    user_email: Option<String>,
    started: Instant,
    first_token_ms: Option<u128>,
    received_any_chunk: bool,
    usage: StreamUsageAccumulator,
    codex_completed_output_patcher: CodexCompletedOutputPatcher,
    codex_pending_function_call_patcher: CodexPendingFunctionCallPatcher,
    codex_custom_tool_stream_patcher: CodexCustomToolStreamPatcher,
    stream_transform: super::stream_transforms::StreamEventTransformer,
    timeouts: StreamTimeoutConfig,
    pending_chunk: Option<Bytes>,
    sse_error_detector: Option<ClaudeSseErrorDetector>,
    sse_error_outcome_recorded: bool,
    terminal_frame_sent: bool,
    interrupted_update_armed: Arc<AtomicBool>,
    _account_in_flight_guard: Option<AccountInFlightGuard>,
    _share_invocation_guard: Option<ShareInFlightGuard>,
}

impl StreamForwardState {
    fn next_timeout_kind(&self) -> StreamTimeoutKind {
        if self.received_any_chunk {
            StreamTimeoutKind::Idle
        } else {
            StreamTimeoutKind::FirstByte
        }
    }

    fn next_timeout(&self) -> Option<Duration> {
        match self.next_timeout_kind() {
            StreamTimeoutKind::FirstByte => self.timeouts.first_byte,
            StreamTimeoutKind::Idle => self.timeouts.idle,
        }
    }

    async fn terminate_transform_error(
        mut self,
        error: ProxyError,
    ) -> Result<Option<(Bytes, Self)>, std::io::Error> {
        let usage = std::mem::take(&mut self.usage).finish();
        let status = error.status.as_u16();
        update_stream_usage(
            &self.state,
            &self.stored,
            &self.request_id,
            status,
            self.started.elapsed().as_millis(),
            self.first_token_ms,
            usage,
            Some("transform_error"),
        )
        .await;
        record_share_invocation_result(
            &self.state,
            self.share_id.as_deref(),
            self.user_email.as_deref(),
            usage,
        )
        .await;
        record_provider_outcome(
            &self.state,
            &self.stored,
            ProviderOutcome::Failure {
                status_code: status,
            },
        )
        .await;
        self.interrupted_update_armed
            .store(false, Ordering::Relaxed);
        self.terminal_frame_sent = true;
        let message = error.client_message().to_string();
        match stream_terminal_error_frame(self.route, &message, status) {
            Some(frame) => Ok(Some((frame, self))),
            None => Err(std::io::Error::other(message)),
        }
    }
}

#[derive(Debug, Default)]
struct CodexCompletedOutputPatcher {
    enabled: bool,
    buffer: String,
    output_items_by_index: BTreeMap<i64, Value>,
    output_items_fallback: Vec<Value>,
}

impl CodexCompletedOutputPatcher {
    fn new(stored: &StoredProvider, route: ProxyRoute) -> Self {
        Self {
            enabled: stored.provider_type == ProviderType::CodexOAuth
                && matches!(
                    route,
                    ProxyRoute::CodexResponses
                        | ProxyRoute::CodexResponsesCompact
                        | ProxyRoute::CodexChatCompletions
                ),
            ..Self::default()
        }
    }

    fn disabled() -> Self {
        Self::default()
    }

    fn push(&mut self, chunk: Bytes) -> Bytes {
        if !self.enabled {
            return chunk;
        }
        let Ok(text) = std::str::from_utf8(&chunk) else {
            return chunk;
        };
        self.buffer.push_str(text);
        let mut output = String::new();
        while let Some((event_end, delimiter_len)) = next_sse_event_boundary(&self.buffer) {
            let delimiter = self.buffer[event_end..event_end + delimiter_len].to_string();
            let event = self.buffer[..event_end].to_string();
            self.buffer.drain(..event_end + delimiter_len);
            output.push_str(&self.patch_event_block(&event));
            output.push_str(&delimiter);
        }
        Bytes::from(output)
    }

    fn finish(&mut self) -> Bytes {
        if !self.enabled || self.buffer.is_empty() {
            return Bytes::new();
        }
        let event = std::mem::take(&mut self.buffer);
        Bytes::from(self.patch_event_block(&event))
    }

    fn patch_event_block(&mut self, event: &str) -> String {
        let Some(payload) = first_sse_data_payload(event) else {
            return event.to_string();
        };
        if payload == "[DONE]" || !payload.starts_with('{') {
            return event.to_string();
        }
        let Ok(mut value) = serde_json::from_str::<Value>(payload) else {
            return event.to_string();
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                self.collect_output_item_done(&value);
                event.to_string()
            }
            Some("response.completed") => {
                if !self.patch_completed_output(&mut value) {
                    return event.to_string();
                }
                let Ok(payload) = serde_json::to_string(&value) else {
                    return event.to_string();
                };
                replace_first_sse_data_payload(event, &payload)
            }
            _ => event.to_string(),
        }
    }

    fn collect_output_item_done(&mut self, value: &Value) {
        let Some(item) = value.get("item").filter(|item| item.is_object()).cloned() else {
            return;
        };
        if let Some(index) = value.get("output_index").and_then(Value::as_i64) {
            self.output_items_by_index.insert(index, item);
        } else {
            self.output_items_fallback.push(item);
        }
    }

    fn patch_completed_output(&self, value: &mut Value) -> bool {
        if self.output_items_by_index.is_empty() && self.output_items_fallback.is_empty() {
            return false;
        }
        let output_is_present = value
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_some_and(|output| !output.is_empty());
        if output_is_present {
            return false;
        }
        let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) else {
            return false;
        };
        let output = self
            .output_items_by_index
            .values()
            .cloned()
            .chain(self.output_items_fallback.iter().cloned())
            .collect::<Vec<_>>();
        response.insert("output".to_string(), Value::Array(output));
        true
    }
}

#[derive(Debug, Default)]
struct CodexPendingFunctionCallPatcher {
    enabled: bool,
    buffer: String,
    pending: Vec<PendingCodexFunctionCall>,
    aliases: BTreeMap<String, usize>,
    last_pending_key: Option<String>,
}

#[derive(Debug, Default)]
struct PendingCodexFunctionCall {
    call_id: Option<String>,
    arguments: String,
}

impl CodexPendingFunctionCallPatcher {
    fn new(stored: &StoredProvider, route: ProxyRoute) -> Self {
        Self {
            enabled: stored.provider_type == ProviderType::CodexOAuth
                && route == ProxyRoute::ClaudeMessages,
            ..Self::default()
        }
    }

    fn disabled() -> Self {
        Self::default()
    }

    fn push(&mut self, chunk: Bytes) -> Bytes {
        if !self.enabled || chunk.is_empty() {
            return chunk;
        }
        let Ok(text) = std::str::from_utf8(&chunk) else {
            return chunk;
        };
        self.buffer.push_str(text);
        let mut output = String::new();
        while let Some((event_end, delimiter_len)) = next_sse_event_boundary(&self.buffer) {
            let delimiter = self.buffer[event_end..event_end + delimiter_len].to_string();
            let event = self.buffer[..event_end].to_string();
            self.buffer.drain(..event_end + delimiter_len);
            output.push_str(&self.patch_event_block(&event));
            output.push_str(&delimiter);
        }
        Bytes::from(output)
    }

    fn finish(&mut self) -> Bytes {
        if !self.enabled || self.buffer.is_empty() {
            return Bytes::new();
        }
        let event = std::mem::take(&mut self.buffer);
        Bytes::from(self.patch_event_block(&event))
    }

    fn patch_event_block(&mut self, event: &str) -> String {
        let Some(payload) = first_sse_data_payload(event) else {
            return event.to_string();
        };
        if payload == "[DONE]" || !payload.starts_with('{') {
            return event.to_string();
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return event.to_string();
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.added") => self.patch_output_item_added(event, &value),
            Some("response.function_call_arguments.delta") => {
                self.patch_function_call_arguments_delta(event, &value)
            }
            Some("response.output_item.done") => self.patch_output_item_done(event, &value),
            _ => event.to_string(),
        }
    }

    fn patch_output_item_added(&mut self, event: &str, value: &Value) -> String {
        let Some(item) = value.get("item") else {
            return event.to_string();
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return event.to_string();
        }
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        if !name.trim().is_empty() {
            if let Some(index) = self.pending_index_for_event(value, item) {
                self.delete_aliases_for_index(index);
            }
            return event.to_string();
        }
        let pending = PendingCodexFunctionCall {
            call_id: function_call_id(item).map(str::to_string),
            arguments: item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        let index = self.pending.len();
        self.pending.push(pending);
        let key =
            function_call_event_key(value, item).unwrap_or_else(|| format!("pending:{index}"));
        self.aliases.insert(key.clone(), index);
        self.last_pending_key = Some(key.clone());
        if let Some(call_id) = self.pending[index].call_id.clone() {
            self.aliases.insert(format!("call:{call_id}"), index);
        }
        String::new()
    }

    fn patch_function_call_arguments_delta(&mut self, event: &str, value: &Value) -> String {
        let Some(index) = self.pending_index_for_event(value, value) else {
            return event.to_string();
        };
        if let Some(delta) = value.get("delta").and_then(Value::as_str) {
            self.pending[index].arguments.push_str(delta);
        }
        String::new()
    }

    fn patch_output_item_done(&mut self, event: &str, value: &Value) -> String {
        let Some(item) = value.get("item") else {
            return event.to_string();
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return event.to_string();
        }
        let Some(index) = self.pending_index_for_event(value, item) else {
            return event.to_string();
        };
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let Some(name) = name else {
            return String::new();
        };
        let call_id = self.pending[index]
            .call_id
            .clone()
            .or_else(|| function_call_id(item).map(str::to_string))
            .unwrap_or_else(|| "tool".to_string());
        let mut added = value.clone();
        if let Some(object) = added.as_object_mut() {
            object.insert(
                "type".to_string(),
                Value::String("response.output_item.added".to_string()),
            );
            object.insert(
                "item".to_string(),
                json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": ""
                }),
            );
        }
        let mut output = encode_sse_json_event("response.output_item.added", &added);
        if !self.pending[index].arguments.is_empty() {
            let mut delta = json!({
                "type": "response.function_call_arguments.delta",
                "delta": self.pending[index].arguments
            });
            if let Some(output_index) = value.get("output_index").cloned() {
                delta["output_index"] = output_index;
            }
            output.push_str(&encode_sse_json_event(
                "response.function_call_arguments.delta",
                &delta,
            ));
        }
        output.push_str(event);
        self.delete_aliases_for_index(index);
        output
    }

    fn pending_index_for_event(&self, value: &Value, item: &Value) -> Option<usize> {
        let mut keys = Vec::new();
        if let Some(key) = function_call_event_key(value, item) {
            keys.push(key);
        }
        if let Some(call_id) = function_call_id(item) {
            keys.push(format!("call:{call_id}"));
        }
        if keys.is_empty() {
            if let Some(key) = self.last_pending_key.clone() {
                keys.push(key);
            }
        }
        keys.into_iter()
            .find_map(|key| self.aliases.get(&key).copied())
    }

    fn delete_aliases_for_index(&mut self, index: usize) {
        self.aliases.retain(|_, value| *value != index);
        if self
            .last_pending_key
            .as_ref()
            .is_some_and(|key| !self.aliases.contains_key(key))
        {
            self.last_pending_key = None;
        }
    }
}

#[derive(Debug, Default)]
struct CodexCustomToolStreamPatcher {
    buffer: String,
    calls: BTreeMap<i64, PendingCustomToolCall>,
}

#[derive(Debug, Clone)]
struct PendingCustomToolCall {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
}

impl CodexCustomToolStreamPatcher {
    fn push(&mut self, chunk: Bytes) -> Bytes {
        if chunk.is_empty() {
            return chunk;
        }
        let Ok(text) = std::str::from_utf8(&chunk) else {
            return chunk;
        };
        self.buffer.push_str(text);
        let mut output = String::new();
        while let Some((event_end, delimiter_len)) = next_sse_event_boundary(&self.buffer) {
            let delimiter = self.buffer[event_end..event_end + delimiter_len].to_string();
            let event = self.buffer[..event_end].to_string();
            self.buffer.drain(..event_end + delimiter_len);
            output.push_str(&self.patch_event_block(&event));
            output.push_str(&delimiter);
        }
        Bytes::from(output)
    }

    fn finish(&mut self) -> Bytes {
        if self.buffer.is_empty() {
            return Bytes::new();
        }
        let event = std::mem::take(&mut self.buffer);
        Bytes::from(self.patch_event_block(&event))
    }

    fn patch_event_block(&mut self, event: &str) -> String {
        let Some(payload) = first_sse_data_payload(event) else {
            return event.to_string();
        };
        if payload == "[DONE]" || !payload.starts_with('{') {
            return event.to_string();
        }
        let Ok(mut value) = serde_json::from_str::<Value>(payload) else {
            return event.to_string();
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.added") => {
                let bridged = value
                    .pointer("/item/cc_switch_custom_bridge")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !bridged {
                    return event.to_string();
                }
                let index = value
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let Some(item) = value.get_mut("item").and_then(Value::as_object_mut) else {
                    return event.to_string();
                };
                item.remove("cc_switch_custom_bridge");
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_0")
                    .to_string();
                self.calls.insert(
                    index,
                    PendingCustomToolCall {
                        item_id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("ctc_call_0")
                            .to_string(),
                        call_id,
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: String::new(),
                    },
                );
                serde_json::to_string(&value)
                    .map(|payload| replace_first_sse_data_payload(event, &payload))
                    .unwrap_or_else(|_| event.to_string())
            }
            Some("response.function_call_arguments.delta") => {
                let index = value
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let Some(call) = self.calls.get_mut(&index) else {
                    return event.to_string();
                };
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    call.arguments.push_str(delta);
                }
                String::new()
            }
            Some("response.completed") => {
                if self.calls.is_empty() {
                    return event.to_string();
                }
                let mut output = String::new();
                let mut completed_items = Vec::new();
                for (index, call) in &self.calls {
                    let input = custom_tool_input_from_arguments(&call.arguments);
                    output.push_str(&encode_sse_json_event(
                        "response.custom_tool_call_input.done",
                        &json!({
                            "type": "response.custom_tool_call_input.done",
                            "item_id": call.item_id,
                            "output_index": index,
                            "input": input
                        }),
                    ));
                    let item = json!({
                        "id": call.item_id,
                        "type": "custom_tool_call",
                        "status": "completed",
                        "input": input,
                        "call_id": call.call_id,
                        "name": call.name
                    });
                    completed_items.push(item.clone());
                    output.push_str(&encode_sse_json_event(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": index,
                            "item": item
                        }),
                    ));
                }
                if let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) {
                    let response_output = response
                        .entry("output")
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(items) = response_output.as_array_mut() {
                        items.extend(completed_items);
                    }
                }
                if let Ok(payload) = serde_json::to_string(&value) {
                    output.push_str(&replace_first_sse_data_payload(event, &payload));
                } else {
                    output.push_str(event);
                }
                self.calls.clear();
                output
            }
            Some("response.failed") | Some("response.incomplete") => {
                self.calls.clear();
                event.to_string()
            }
            _ => event.to_string(),
        }
    }
}

fn custom_tool_input_from_arguments(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

fn function_call_event_key(value: &Value, item: &Value) -> Option<String> {
    value
        .get("output_index")
        .map(|index| format!("output:{index}"))
        .or_else(|| function_call_id(item).map(|call_id| format!("call:{call_id}")))
}

fn function_call_id(item: &Value) -> Option<&str> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn encode_sse_json_event(event: &str, value: &Value) -> String {
    if let Some(wire) = super::responses_wire::encode_named_sse_event(event, value) {
        return wire;
    }
    format!("event: {event}\ndata: {value}\n\n")
}

fn next_sse_event_boundary(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf <= lf => Some((crlf, 4)),
        (Some(lf), Some(_)) => Some((lf, 2)),
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn first_sse_data_payload(event: &str) -> Option<&str> {
    event.lines().find_map(|line| {
        let line = line.trim_end_matches('\r');
        line.strip_prefix("data:").map(str::trim)
    })
}

fn replace_first_sse_data_payload(event: &str, payload: &str) -> String {
    let mut replaced = false;
    let mut output = String::new();
    for (index, line) in event.split('\n').enumerate() {
        if index > 0 {
            output.push('\n');
        }
        let line_without_cr = line.trim_end_matches('\r');
        if !replaced && line_without_cr.strip_prefix("data:").is_some() {
            output.push_str("data: ");
            output.push_str(payload);
            if line.ends_with('\r') {
                output.push('\r');
            }
            replaced = true;
        } else {
            output.push_str(line);
        }
    }
    output
}

#[derive(Debug, Clone, Copy)]
struct StreamTimeoutConfig {
    first_byte: Option<Duration>,
    idle: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
enum StreamTimeoutKind {
    FirstByte,
    Idle,
}

enum StreamReadError {
    Upstream(reqwest::Error),
    Timeout {
        kind: StreamTimeoutKind,
        timeout: Duration,
    },
}

impl StreamReadError {
    fn status_code(&self) -> u16 {
        match self {
            Self::Upstream(_) => StatusCode::BAD_GATEWAY.as_u16(),
            Self::Timeout { .. } => StatusCode::GATEWAY_TIMEOUT.as_u16(),
        }
    }

    fn stream_status(&self) -> &'static str {
        match self {
            Self::Upstream(_) => "upstream_error",
            Self::Timeout { .. } => "timeout",
        }
    }
}

impl std::fmt::Display for StreamReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upstream(error) => write!(formatter, "upstream stream error: {error}"),
            Self::Timeout { kind, timeout } => write!(
                formatter,
                "upstream stream {} timeout after {}ms",
                kind.label(),
                timeout.as_millis()
            ),
        }
    }
}

impl StreamTimeoutKind {
    fn label(self) -> &'static str {
        match self {
            Self::FirstByte => "first byte",
            Self::Idle => "idle",
        }
    }
}

impl Drop for StreamForwardState {
    fn drop(&mut self) {
        if !self.interrupted_update_armed.load(Ordering::Relaxed) {
            return;
        }
        let state = self.state.clone();
        let stored = self.stored.clone();
        let request_id = self.request_id.clone();
        let status_code = self.status_code;
        let share_id = self.share_id.clone();
        let user_email = self.user_email.clone();
        let usage = std::mem::take(&mut self.usage).finish();
        let duration_ms = self.started.elapsed().as_millis();
        let first_token_ms = self.first_token_ms;
        tokio::spawn(async move {
            update_stream_usage(
                &state,
                &stored,
                &request_id,
                status_code,
                duration_ms,
                first_token_ms,
                usage,
                Some("interrupted"),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                usage,
            )
            .await;
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
        });
    }
}

fn copy_header(headers: &HeaderMap, name: axum::http::header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn optional_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn forward_http_client(
    state: &ServerState,
    _stored: &StoredProvider,
) -> Result<reqwest::Client, ProxyError> {
    Ok(state.http_client().await)
}

fn strip_hop_by_hop_response_headers(headers: &mut HeaderMap) {
    const HOP_BY_HOP_HEADERS: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "trailers",
        "transfer-encoding",
        "upgrade",
    ];

    let connection_listed_headers = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| HeaderName::from_bytes(value.as_bytes()).ok())
        .collect::<Vec<_>>();

    for name in HOP_BY_HOP_HEADERS {
        headers.remove(*name);
    }
    for name in connection_listed_headers {
        headers.remove(name);
    }
}

fn copy_safe_upstream_response_headers(headers: &HeaderMap, response: &mut Response) {
    const EXACT: &[&str] = &["x-request-id", "retry-after", "x-should-retry"];
    const PREFIXES: &[&str] = &[
        "anthropic-ratelimit-",
        "anthropic-priority-",
        "anthropic-fast-",
    ];

    for (name, value) in headers {
        let normalized = name.as_str();
        if EXACT.contains(&normalized)
            || PREFIXES.iter().any(|prefix| normalized.starts_with(prefix))
        {
            response.headers_mut().append(name.clone(), value.clone());
        }
    }
}

fn request_context_from_headers(headers: &HeaderMap) -> UsageLogContext {
    let share_id = optional_header(headers, "x-cc-switch-share-id");
    let data_source = optional_header(headers, "x-cc-switch-data-source")
        .or_else(|| optional_header(headers, "x-cc-switch-source"))
        .or_else(|| share_id.as_ref().map(|_| "direct".to_string()));
    UsageLogContext {
        request_id: optional_header(headers, "x-cc-switch-request-id"),
        share_id,
        user_email: optional_header(headers, "x-cc-switch-user-email")
            .or_else(|| optional_header(headers, "x-user-email")),
        data_source,
        user_country: optional_header(headers, "x-cc-switch-user-country")
            .or_else(|| optional_header(headers, "x-user-country")),
        user_country_iso3: optional_header(headers, "x-cc-switch-user-country-iso3")
            .or_else(|| optional_header(headers, "x-user-country-iso3")),
        is_health_check: optional_header(headers, "x-cc-switch-health-check")
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes")),
        ..UsageLogContext::default()
    }
}

fn session_id_from_request(route: ProxyRoute, headers: &HeaderMap, body: &[u8]) -> Option<String> {
    optional_header(headers, "x-cc-switch-session-id").or_else(|| match route {
        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens => {
            claude_session_id_from_request(headers, body)
        }
        ProxyRoute::CodexChatCompletions
        | ProxyRoute::CodexResponses
        | ProxyRoute::CodexResponsesCompact => codex_oauth_session_id_from_request(headers, body),
        ProxyRoute::Gemini => None,
    })
}

fn claude_session_id_from_request(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    optional_header(headers, "x-claude-code-session-id")
        .or_else(|| optional_header(headers, "claude-code-session-id"))
        .or_else(|| claude_session_id_from_body(body))
}

fn claude_session_id_from_body(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    value
        .pointer("/metadata/user_id")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_session_from_user_id)
        .or_else(|| {
            ["/metadata/session_id", "/metadata/sessionId"]
                .into_iter()
                .find_map(|pointer| {
                    value
                        .pointer(pointer)
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string)
                })
        })
}

fn parse_session_from_user_id(user_id: &str) -> Option<String> {
    let session_id = user_id.split_once("_session_")?.1.trim();
    (!session_id.is_empty()).then(|| session_id.to_string())
}

fn select_share_provider(
    providers: &ProviderStore,
    shares: &ShareStore,
    app: AppKind,
    share_id: &str,
) -> Result<(StoredProvider, Option<String>), ProxyError> {
    let share = shares
        .shares
        .iter()
        .find(|share| share.id == share_id)
        .ok_or_else(|| ProxyError::not_found(format!("share not found: {share_id}")))?;
    if !share.enabled || share.status != "active" {
        return Err(ProxyError::bad_request(format!(
            "share is not active: {share_id}"
        )));
    }
    let provider_id = share
        .bindings
        .iter()
        .find(|binding| binding.app == app)
        .map(|binding| binding.provider_id.as_str())
        .or_else(|| (share.app == app).then_some(share.provider_id.as_str()))
        .ok_or_else(|| {
            ProxyError::not_found(format!(
                "share {share_id} has no {:?} provider binding",
                app
            ))
        })?;
    let stored = providers
        .providers
        .iter()
        .find(|item| item.app == app && item.provider.id == provider_id)
        .cloned()
        .ok_or_else(|| ProxyError::not_found(format!("provider not found: {provider_id}")))?;
    Ok((
        stored,
        share
            .display_name
            .clone()
            .or_else(|| Some(share.id.clone())),
    ))
}

fn select_share_execution(
    providers: &ProviderStore,
    shares: &ShareStore,
    accounts: &AccountStore,
    app: AppKind,
    share_id: &str,
) -> Result<(ProviderExecution, Option<String>), ProxyError> {
    let (stored, share_name) = select_share_provider(providers, shares, app, share_id)?;
    ensure_provider_account_does_not_need_relogin(&stored, accounts)?;
    ensure_provider_account_usage_available(&stored, accounts, current_time_ms())?;
    let execution = ProviderExecution::from_store(providers, stored)?;
    execution.ensure_operation_supported(ProviderOperation::Forward)?;
    Ok((execution, share_name))
}

fn model_metadata(request: &adapters::AdapterRequest) -> UsageModelMetadata {
    UsageModelMetadata {
        model: request.model.clone(),
        requested_model: request.requested_model.clone(),
        actual_model: request.actual_model.clone(),
        actual_model_source: request.actual_model_source.clone(),
    }
}

fn provider_upstream_timeout(stored: &StoredProvider) -> std::time::Duration {
    let timeout_ms = setting(
        &stored.provider,
        &[
            "UPSTREAM_TIMEOUT_MS",
            "PROXY_TIMEOUT_MS",
            "REQUEST_TIMEOUT_MS",
        ],
    )
    .and_then(|value| value.parse::<u64>().ok())
    .filter(|value| *value > 0)
    .unwrap_or(300_000);
    std::time::Duration::from_millis(timeout_ms)
}

fn stream_timeout_config(stored: &StoredProvider) -> StreamTimeoutConfig {
    StreamTimeoutConfig {
        first_byte: stream_first_byte_timeout(stored),
        idle: provider_timeout_setting(
            stored,
            &[
                "STREAM_IDLE_TIMEOUT_MS",
                "UPSTREAM_STREAM_IDLE_TIMEOUT_MS",
                "IDLE_TIMEOUT_MS",
            ],
            300_000,
        ),
    }
}

fn stream_first_byte_timeout(stored: &StoredProvider) -> Option<Duration> {
    provider_timeout_setting(
        stored,
        &[
            "STREAM_FIRST_BYTE_TIMEOUT_MS",
            "UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT_MS",
            "FIRST_BYTE_TIMEOUT_MS",
        ],
        120_000,
    )
}

fn provider_timeout_setting(
    stored: &StoredProvider,
    keys: &[&str],
    default_ms: u64,
) -> Option<Duration> {
    let timeout_ms = setting(&stored.provider, keys)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_ms);
    (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms))
}

fn stream_terminal_error_frame(
    route: ProxyRoute,
    message: &str,
    status_code: u16,
) -> Option<Bytes> {
    match route {
        ProxyRoute::CodexResponses | ProxyRoute::CodexResponsesCompact => {
            Some(Bytes::from(format!(
                "event: response.failed\ndata: {}\n\ndata: [DONE]\n\n",
                json!({
                    "type": "response.failed",
                    "response": {
                        "object": "response",
                        "status": "failed",
                        "error": {
                            "type": "upstream_error",
                            "code": "cc_switch_stream_error",
                            "message": message,
                            "status": status_code,
                        }
                    }
                })
            )))
        }
        ProxyRoute::CodexChatCompletions | ProxyRoute::Gemini => Some(Bytes::from(format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({
                "error": {
                    "message": message,
                    "type": "upstream_error",
                    "code": "cc_switch_stream_error",
                    "status": status_code,
                }
            })
        ))),
        ProxyRoute::ClaudeMessages => Some(Bytes::from(format!(
            "event: error\ndata: {}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
            json!({
                "type": "error",
                "error": {
                    "type": "upstream_error",
                    "message": message,
                    "code": "cc_switch_stream_error",
                    "status": status_code,
                }
            })
        ))),
        ProxyRoute::ClaudeCountTokens => None,
    }
}

fn count_tokens_metric_outcome(status: StatusCode) -> &'static str {
    match status {
        status if status.is_success() => "success",
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "auth_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        status if status.is_client_error() => "client_error",
        _ => "upstream_error",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::domain::providers::model::{
        AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
    };

    #[test]
    fn retry_context_pins_provider_and_tracks_body_stage() {
        let context = ForwardAttemptContext::default();
        assert_eq!(context.attempt, 0);
        assert!(context.execution.is_none());

        let stored = stored_provider(AppKind::Codex, ProviderType::Codex, json!({}), None);
        let mut failover_stored = stored.clone();
        failover_stored.provider.id = "codex-failover".to_string();
        let mut store = ProviderStore {
            providers: vec![stored.clone(), failover_stored.clone()],
            ..ProviderStore::default()
        };
        store
            .rebuild_runtime_index(&AccountStore::default())
            .unwrap();
        let execution = ProviderExecution::from_store(&store, stored).unwrap();
        let failover_execution = ProviderExecution::from_store(&store, failover_stored).unwrap();

        let next = context.next(&execution, Some(ClaudeBodyRetryStage::Thinking));
        assert_eq!(next.attempt, 1);
        assert_eq!(next.body_retry_stage, Some(ClaudeBodyRetryStage::Thinking));
        assert!(!next.auth_refresh_attempted);
        assert_eq!(
            next.execution
                .as_ref()
                .map(|execution| execution.stored.provider.id.as_str()),
            Some("codex-fixture")
        );

        let refreshed = next.after_auth_refresh(&execution);
        assert!(refreshed.auth_refresh_attempted);
        assert_eq!(refreshed.attempt, 2);
        assert_eq!(
            refreshed.body_retry_stage,
            Some(ClaudeBodyRetryStage::Thinking)
        );

        let failed_over = refreshed.after_provider_failover(&execution, &failover_execution);
        assert_eq!(failed_over.attempt, 3);
        assert!(!failed_over.auth_refresh_attempted);
        assert!(failed_over.excluded_provider_ids.contains("codex-fixture"));
        assert_eq!(
            failed_over
                .execution
                .as_ref()
                .map(|execution| execution.stored.provider.id.as_str()),
            Some("codex-failover")
        );
    }

    #[tokio::test]
    async fn concurrent_legacy_claude_forwards_refresh_once_and_use_rotated_token() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let token_bodies_for_route = std::sync::Arc::clone(&token_bodies);
        let token_upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let token_requests = std::sync::Arc::clone(&token_requests_for_route);
                let token_bodies = std::sync::Arc::clone(&token_bodies_for_route);
                async move {
                    token_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    token_bodies.lock().unwrap().push(body);
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    axum::Json(json!({
                        "access_token": "rotated-forward-access-token",
                        "refresh_token": "rotated-forward-refresh-token",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": "legacy-principal"}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(token_listener, token_upstream).await.unwrap();
        });

        let anthropic_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let anthropic_address = anthropic_listener.local_addr().unwrap();
        let upstream_authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let upstream_authorizations_for_route = std::sync::Arc::clone(&upstream_authorizations);
        let anthropic_upstream = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |headers: HeaderMap| {
                let authorizations = std::sync::Arc::clone(&upstream_authorizations_for_route);
                async move {
                    authorizations.lock().unwrap().push(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    axum::Json(json!({
                        "id": "msg-refreshed",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-sonnet-4-6",
                        "content": [{"type": "text", "text": "ok"}],
                        "stop_reason": "end_turn",
                        "usage": {"input_tokens": 2, "output_tokens": 1}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(anthropic_listener, anthropic_upstream)
                .await
                .unwrap();
        });

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let state = crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(std::env::temp_dir().join(format!(
                    "cc-switch-server-legacy-refresh-forward-test-{nanos}"
                ))),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        let token_url = format!("http://{token_address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some("legacy-refresh-account".to_string()),
                    provider_type: ProviderType::ClaudeOAuth,
                    email: Some("legacy-refresh@example.com".to_string()),
                    access_token: Some("expired-forward-access-token".to_string()),
                    refresh_token: Some("original-forward-refresh-token".to_string()),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: Some(json!({"testOAuthTokenUrl": token_url})),
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
        let base_url = format!("http://{anthropic_address}");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Claude,
                    Provider {
                        id: "legacy-refresh-provider".to_string(),
                        name: "Legacy Refresh Provider".to_string(),
                        settings_config: json!({
                            "env": {"ANTHROPIC_BASE_URL": base_url}
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("claude_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("claude_oauth".to_string()),
                                account_id: Some("legacy-refresh-account".to_string()),
                                auth_identity_generation: None,
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_static("legacy-refresh-provider"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let body = Bytes::from_static(
            br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}"#,
        );
        let (first, second) = tokio::join!(
            forward(
                state.clone(),
                ProxyRoute::ClaudeMessages,
                None,
                headers.clone(),
                body.clone(),
            ),
            forward(
                state.clone(),
                ProxyRoute::ClaudeMessages,
                None,
                headers,
                body,
            )
        );

        assert_eq!(first.unwrap().status(), StatusCode::OK);
        assert_eq!(second.unwrap().status(), StatusCode::OK);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            token_bodies.lock().unwrap()[0]["refresh_token"],
            json!("original-forward-refresh-token")
        );
        assert_eq!(
            upstream_authorizations.lock().unwrap().as_slice(),
            [
                "Bearer rotated-forward-access-token",
                "Bearer rotated-forward-access-token"
            ]
        );
        let account = state
            .find_account_by_id("legacy-refresh-account")
            .await
            .unwrap();
        assert_eq!(
            account.access_token.as_deref(),
            Some("rotated-forward-access-token")
        );
        assert_eq!(
            account.refresh_token.as_deref(),
            Some("rotated-forward-refresh-token")
        );
    }

    #[tokio::test]
    async fn claude_messages_and_count_tokens_recover_once_from_unauthorized() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let token_requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    token_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let kind = body["refresh_token"]
                        .as_str()
                        .and_then(|value| value.strip_prefix("refresh-"))
                        .unwrap();
                    axum::Json(json!({
                        "access_token": format!("new-{kind}-access"),
                        "refresh_token": format!("rotated-{kind}-refresh"),
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": format!("principal-{kind}")}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(token_listener, token_upstream).await.unwrap();
        });

        let anthropic_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let anthropic_address = anthropic_listener.local_addr().unwrap();
        let upstream_attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let message_attempts = std::sync::Arc::clone(&upstream_attempts);
        let count_attempts = std::sync::Arc::clone(&upstream_attempts);
        let anthropic_upstream = axum::Router::new()
            .route(
                "/v1/messages",
                axum::routing::post(move |headers: HeaderMap| {
                    let attempts = std::sync::Arc::clone(&message_attempts);
                    async move {
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        attempts
                            .lock()
                            .unwrap()
                            .push(("messages".to_string(), authorization.clone()));
                        if authorization == "Bearer old-messages-access" {
                            Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({
                                        "type": "error",
                                        "error": {"type": "authentication_error"}
                                    })
                                    .to_string(),
                                ))
                                .unwrap()
                        } else {
                            Response::builder()
                                .status(StatusCode::OK)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({
                                        "id": "msg-after-refresh",
                                        "type": "message",
                                        "role": "assistant",
                                        "model": "claude-sonnet-4-6",
                                        "content": [{"type": "text", "text": "ok"}],
                                        "stop_reason": "end_turn",
                                        "usage": {"input_tokens": 2, "output_tokens": 1}
                                    })
                                    .to_string(),
                                ))
                                .unwrap()
                        }
                    }
                }),
            )
            .route(
                "/v1/messages/count_tokens",
                axum::routing::post(move |headers: HeaderMap| {
                    let attempts = std::sync::Arc::clone(&count_attempts);
                    async move {
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        attempts
                            .lock()
                            .unwrap()
                            .push(("count_tokens".to_string(), authorization.clone()));
                        if authorization == "Bearer old-count-access" {
                            Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({
                                        "type": "error",
                                        "error": {"type": "authentication_error"}
                                    })
                                    .to_string(),
                                ))
                                .unwrap()
                        } else {
                            Response::builder()
                                .status(StatusCode::OK)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(json!({"input_tokens": 9}).to_string()))
                                .unwrap()
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(anthropic_listener, anthropic_upstream)
                .await
                .unwrap();
        });

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let state = crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(std::env::temp_dir().join(format!(
                    "cc-switch-server-unauthorized-refresh-test-{nanos}"
                ))),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        let token_url = format!("http://{token_address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                for (kind, access_token) in [
                    ("messages", "old-messages-access"),
                    ("count", "old-count-access"),
                ] {
                    accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                        id: Some(format!("unauthorized-{kind}-account")),
                        provider_type: ProviderType::ClaudeOAuth,
                        email: Some(format!("{kind}@example.com")),
                        access_token: Some(access_token.to_string()),
                        refresh_token: Some(format!("refresh-{kind}")),
                        id_token: None,
                        token_type: Some("Bearer".to_string()),
                        api_key: None,
                        extra_headers: None,
                        scopes: Vec::new(),
                        profile: None,
                        raw: Some(json!({"testOAuthTokenUrl": token_url})),
                        subscription_level: None,
                        entitlement_status: None,
                        quota_percent: None,
                        quota: None,
                        quota_refreshed_at: None,
                        quota_next_refresh_at: None,
                        expires_at: Some(i64::MAX / 2),
                        rate_limited_until: None,
                        last_refresh_error: None,
                    });
                }
            })
            .await
            .unwrap();
        let base_url = format!("http://{anthropic_address}");
        state
            .mutate_providers_immediate(move |providers| {
                for kind in ["messages", "count"] {
                    providers.upsert(
                        AppKind::Claude,
                        Provider {
                            id: format!("unauthorized-{kind}-provider"),
                            name: format!("Unauthorized {kind} Provider"),
                            settings_config: json!({
                                "env": {"ANTHROPIC_BASE_URL": base_url}
                            }),
                            category: None,
                            meta: Some(ProviderMeta {
                                provider_type: Some("claude_oauth".to_string()),
                                auth_binding: Some(AuthBinding {
                                    source: Some("account_store".to_string()),
                                    auth_provider: Some("claude_oauth".to_string()),
                                    account_id: Some(format!("unauthorized-{kind}-account")),
                                    auth_identity_generation: None,
                                }),
                                ..Default::default()
                            }),
                            extra: Default::default(),
                        },
                    );
                }
            })
            .await
            .unwrap();

        for (kind, route) in [
            ("messages", ProxyRoute::ClaudeMessages),
            ("count", ProxyRoute::ClaudeCountTokens),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cc-provider-id",
                HeaderValue::from_str(&format!("unauthorized-{kind}-provider")).unwrap(),
            );
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let response = forward(
                state.clone(),
                route,
                None,
                headers,
                Bytes::from_static(
                    br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}"#,
                ),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            upstream_attempts.lock().unwrap().as_slice(),
            [
                (
                    "messages".to_string(),
                    "Bearer old-messages-access".to_string()
                ),
                (
                    "messages".to_string(),
                    "Bearer new-messages-access".to_string()
                ),
                (
                    "count_tokens".to_string(),
                    "Bearer old-count-access".to_string()
                ),
                (
                    "count_tokens".to_string(),
                    "Bearer new-count-access".to_string()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn claude_unauthorized_after_refresh_fails_over_to_next_provider() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let token_requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    token_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "access_token": "still-rejected-access",
                        "refresh_token": "rotated-rejected-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": "rejected-principal"}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(token_listener, token_upstream).await.unwrap();
        });

        let rejected_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let rejected_address = rejected_listener.local_addr().unwrap();
        let rejected_authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rejected_authorizations_for_route = std::sync::Arc::clone(&rejected_authorizations);
        let rejected_upstream = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |headers: HeaderMap| {
                let authorizations = std::sync::Arc::clone(&rejected_authorizations_for_route);
                async move {
                    authorizations.lock().unwrap().push(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({
                                "type": "error",
                                "error": {"type": "authentication_error"}
                            })
                            .to_string(),
                        ))
                        .unwrap()
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(rejected_listener, rejected_upstream)
                .await
                .unwrap();
        });

        let fallback_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let fallback_address = fallback_listener.local_addr().unwrap();
        let fallback_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fallback_requests_for_route = std::sync::Arc::clone(&fallback_requests);
        let fallback_upstream = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move || {
                let fallback_requests = std::sync::Arc::clone(&fallback_requests_for_route);
                async move {
                    fallback_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "id": "msg-after-auth-failover",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-sonnet-4-6",
                        "content": [{"type": "text", "text": "ok"}],
                        "stop_reason": "end_turn",
                        "usage": {"input_tokens": 2, "output_tokens": 1}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(fallback_listener, fallback_upstream)
                .await
                .unwrap();
        });

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let state = crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir()
                        .join(format!("cc-switch-server-auth-failover-test-{nanos}")),
                ),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        let token_url = format!("http://{token_address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some("auth-failover-account".to_string()),
                    provider_type: ProviderType::ClaudeOAuth,
                    email: Some("auth-failover@example.com".to_string()),
                    access_token: Some("initial-rejected-access".to_string()),
                    refresh_token: Some("initial-rejected-refresh".to_string()),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: Some(json!({"testOAuthTokenUrl": token_url})),
                    subscription_level: None,
                    entitlement_status: None,
                    quota_percent: None,
                    quota: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(i64::MAX / 2),
                    rate_limited_until: None,
                    last_refresh_error: None,
                });
            })
            .await
            .unwrap();
        let rejected_base_url = format!("http://{rejected_address}");
        let fallback_base_url = format!("http://{fallback_address}");
        state
            .mutate_providers_immediate(move |providers| {
                providers.upsert(
                    AppKind::Claude,
                    Provider {
                        id: "auth-failover-oauth".to_string(),
                        name: "Auth Failover OAuth".to_string(),
                        settings_config: json!({
                            "env": {"ANTHROPIC_BASE_URL": rejected_base_url}
                        }),
                        category: None,
                        meta: Some(ProviderMeta {
                            provider_type: Some("claude_oauth".to_string()),
                            auth_binding: Some(AuthBinding {
                                source: Some("account_store".to_string()),
                                auth_provider: Some("claude_oauth".to_string()),
                                account_id: Some("auth-failover-account".to_string()),
                                auth_identity_generation: None,
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
                providers.upsert(
                    AppKind::Claude,
                    Provider {
                        id: "auth-failover-api-key".to_string(),
                        name: "Auth Failover API Key".to_string(),
                        settings_config: json!({
                            "env": {
                                "ANTHROPIC_BASE_URL": fallback_base_url,
                                "ANTHROPIC_API_KEY": "sk-fallback"
                            }
                        }),
                        category: None,
                        meta: None,
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        state
            .apply_ui_settings_patch_immediate(json!({
                "currentProviderClaude": "auth-failover-oauth"
            }))
            .await
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = forward(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            None,
            headers,
            Bytes::from_static(
                br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"ping"}]}"#,
            ),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["id"],
            "msg-after-auth-failover"
        );
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            rejected_authorizations.lock().unwrap().as_slice(),
            [
                "Bearer initial-rejected-access",
                "Bearer still-rejected-access"
            ]
        );
        assert_eq!(
            fallback_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let usage = state.usage_snapshot().await;
        assert_eq!(usage.logs.len(), 1);
        assert_eq!(usage.logs[0].provider_id, "auth-failover-api-key");
    }

    use super::*;

    fn stored_provider(
        app: AppKind,
        provider_type: ProviderType,
        settings_config: Value,
        account_id: Option<&str>,
    ) -> StoredProvider {
        StoredProvider {
            app,
            provider: Provider {
                id: format!("{}-fixture", provider_type.as_str()),
                name: provider_type.as_str().to_string(),
                settings_config,
                category: None,
                meta: account_id.map(|account_id| ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some(provider_type.as_str().to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: None,
                    }),
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    #[test]
    fn share_execution_rejects_explicit_account_usage_block() {
        let provider = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("share-account"),
        );
        let provider_id = provider.provider.id.clone();
        let providers = ProviderStore {
            providers: vec![provider],
            ..ProviderStore::default()
        };
        let share = serde_json::from_value(json!({
            "id": "blocked-share",
            "app": "codex",
            "providerId": provider_id,
            "providerType": "codex_oauth",
            "enabled": true,
            "status": "active"
        }))
        .unwrap();
        let shares = ShareStore {
            shares: vec![share],
            ..ShareStore::default()
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(
            serde_json::from_value(json!({
                "id": "share-account",
                "providerType": "codex_oauth",
                "rateLimitedUntil": current_time_ms() + 60_000
            }))
            .unwrap(),
        );

        let error = select_share_execution(
            &providers,
            &shares,
            &accounts,
            AppKind::Codex,
            "blocked-share",
        )
        .expect_err("share execution must enforce the bound account block");

        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(error.message.contains("rate_limited"));
    }

    #[test]
    fn deepseek_upstream_errors_map_to_proxy_status_codes() {
        assert_eq!(
            deepseek_upstream_error_to_proxy_error(DeepSeekUpstreamError::NotFound).status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            deepseek_upstream_error_to_proxy_error(DeepSeekUpstreamError::MissingToken).status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            deepseek_upstream_error_to_proxy_error(DeepSeekUpstreamError::Client(
                "upstream failed".to_string()
            ))
            .status,
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn direct_secret_configuration_skips_managed_account_refresh_path() {
        let direct = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({"env": {"OPENAI_API_KEY": "sk-direct"}}),
            Some("acct-1"),
        );
        let managed = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("acct-1"),
        );

        assert_eq!(managed_account_id(&direct), Some("acct-1"));
        assert!(provider_secret_configured(AppKind::Codex, &direct));
        assert!(!provider_secret_configured(AppKind::Codex, &managed));
    }

    #[test]
    fn copilot_static_secret_bypasses_request_time_managed_auth() {
        let direct = stored_provider(
            AppKind::Claude,
            ProviderType::GitHubCopilot,
            json!({"env": {"ANTHROPIC_AUTH_TOKEN": "copilot-static"}}),
            Some("acct-1"),
        );
        let managed = stored_provider(
            AppKind::Claude,
            ProviderType::GitHubCopilot,
            json!({}),
            Some("acct-1"),
        );

        assert!(!copilot_managed_account_auth_required(
            AppKind::Claude,
            &direct
        ));
        assert!(copilot_managed_account_auth_required(
            AppKind::Claude,
            &managed
        ));
    }

    #[test]
    fn replace_or_push_header_overwrites_case_insensitively() {
        let mut headers = vec![("Authorization", "Bearer stale".to_string())];
        replace_or_push_header(&mut headers, "authorization", "Bearer fresh".to_string());
        assert_eq!(headers, vec![("Authorization", "Bearer fresh".to_string())]);

        replace_or_push_header(&mut headers, "x-extra", "1".to_string());
        assert_eq!(
            headers,
            vec![
                ("Authorization", "Bearer fresh".to_string()),
                ("x-extra", "1".to_string())
            ]
        );
    }

    #[test]
    fn claude_oauth_account_headers_cannot_override_signed_contract() {
        for name in [
            "anthropic-beta",
            "Anthropic-Version",
            "x-app",
            "sec-fetch-mode",
            "anthropic-dangerous-direct-browser-access",
            "x-claude-code-session-id",
            "x-stainless-runtime",
        ] {
            assert!(account_header_override_blocked(
                name,
                ProviderType::ClaudeOAuth
            ));
        }
        assert!(!account_header_override_blocked(
            "x-provider-feature",
            ProviderType::ClaudeOAuth
        ));
        assert!(!account_header_override_blocked(
            "anthropic-beta",
            ProviderType::ClaudeAuth
        ));
    }

    #[test]
    fn account_header_overrides_merge_custom_headers_and_reject_controlled_names() {
        let mut accounts = AccountStore::default();
        accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
            id: Some("acct-headers".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: None,
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Some(BTreeMap::from([(
                "x-enterprise-sso".to_string(),
                "tenant-a".to_string(),
            )])),
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
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("acct-headers"),
        );
        let mut headers = owned_headers(vec![("authorization", "Bearer access".to_string())]);

        apply_account_header_overrides(&mut headers, &stored, &accounts).unwrap();

        assert!(headers.contains(&("x-enterprise-sso".to_string(), "tenant-a".to_string())));

        accounts.accounts[0]
            .extra_headers
            .insert("authorization".to_string(), "Bearer attacker".to_string());
        let error = apply_account_header_overrides(&mut headers, &stored, &accounts).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("proxy-controlled header"));
    }

    #[test]
    fn cross_protocol_secret_detection_uses_upstream_auth_family() {
        let codex_to_gemini = stored_provider(
            AppKind::Codex,
            ProviderType::GeminiCli,
            json!({"env": {"GEMINI_API_KEY": "gemini-secret"}}),
            None,
        );
        let gemini_to_openrouter = stored_provider(
            AppKind::Gemini,
            ProviderType::OpenRouter,
            json!({"env": {"OPENAI_API_KEY": "openrouter-secret"}}),
            None,
        );

        assert_eq!(
            auth_header_app_for(AppKind::Codex, ProviderType::GeminiCli),
            AppKind::Gemini
        );
        assert_eq!(
            auth_header_app_for(AppKind::Gemini, ProviderType::OpenRouter),
            AppKind::Codex
        );
        assert!(provider_secret_configured(AppKind::Codex, &codex_to_gemini));
        assert!(provider_secret_configured(
            AppKind::Gemini,
            &gemini_to_openrouter
        ));
    }

    #[test]
    fn refresh_failures_keep_oauth_status_and_managed_account_context() {
        let proxy_error =
            managed_account_refresh_error_to_proxy_error(ManagedAccountRefreshError::Refresh {
                status_code: 429,
                message: "rate limited by provider".to_string(),
            });

        assert_eq!(proxy_error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            proxy_error.message,
            "managed account refresh failed: rate limited by provider"
        );
    }

    #[test]
    fn share_rejections_use_legacy_reason_suffix_and_status_mapping() {
        let expired = share_rejection_to_proxy_error(ShareInvocationRejection {
            reason: ShareRejectReason::Expired,
            message: "Share has expired.".to_string(),
            status_changed: true,
        });
        let parallel = share_rejection_to_proxy_error(ShareInvocationRejection {
            reason: ShareRejectReason::ParallelLimit,
            message: "Share parallel limit has been reached.".to_string(),
            status_changed: false,
        });

        assert_eq!(expired.status, StatusCode::FORBIDDEN);
        assert_eq!(expired.message, "Share has expired. [Expired]");
        assert_eq!(parallel.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            parallel.message,
            "Share parallel limit has been reached. [ParallelLimit]"
        );
    }

    #[test]
    fn share_usage_tokens_prefers_total_and_falls_back_to_input_output_sum() {
        assert_eq!(
            share_usage_tokens(TokenUsage {
                total_tokens: Some(12),
                input_tokens: Some(3),
                output_tokens: Some(4),
                ..Default::default()
            }),
            12
        );
        assert_eq!(
            share_usage_tokens(TokenUsage {
                input_tokens: Some(3),
                output_tokens: Some(4),
                ..Default::default()
            }),
            7
        );
    }

    #[test]
    fn claude_sse_errors_map_to_provider_outcomes() {
        assert_eq!(
            claude_sse_error_outcome("rate_limit_error"),
            Some(ProviderOutcome::RateLimited {
                status_code: StatusCode::TOO_MANY_REQUESTS.as_u16()
            })
        );
        assert_eq!(
            claude_sse_error_outcome("overloaded_error"),
            Some(ProviderOutcome::Failure { status_code: 529 })
        );
        assert_eq!(claude_sse_error_outcome("not_interesting"), None);
    }

    #[test]
    fn claude_retry_stage_ladder_handles_signature_and_tool_errors() {
        assert_eq!(
            claude_body_retry_stage_for_error_message(
                "Invalid signature in thinking block",
                None,
                b"{}",
            ),
            Some(ClaudeBodyRetryStage::Thinking)
        );
        assert_eq!(
            claude_body_retry_stage_for_error_message(
                "Invalid signature near tool_use",
                Some(ClaudeBodyRetryStage::Thinking),
                b"{}",
            ),
            Some(ClaudeBodyRetryStage::SignatureSensitive)
        );
        assert_eq!(
            claude_body_retry_stage_for_error_message(
                "Invalid signature",
                Some(ClaudeBodyRetryStage::Thinking),
                b"{}",
            ),
            None
        );
    }

    #[test]
    fn claude_retry_stage_ladder_handles_web_search_errors() {
        assert_eq!(
            claude_body_retry_stage_for_error_message(
                "invalid value: server_tool_use web_search",
                None,
                b"{}",
            ),
            Some(ClaudeBodyRetryStage::WebSearchHistory)
        );
        assert_eq!(
            claude_body_retry_stage_for_error_message(
                "Invalid signature",
                Some(ClaudeBodyRetryStage::SignatureSensitive),
                br#"{"messages":[{"content":[{"type":"web_search_tool_result"}]}]}"#,
            ),
            Some(ClaudeBodyRetryStage::WebSearchHistory)
        );
    }

    #[test]
    fn claude_version_gate_error_is_rewritten_for_admin() {
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::ClaudeOAuth,
            json!({}),
            Some("acct-1"),
        );
        let body = Bytes::from_static(
            br#"{"error":{"type":"invalid_request_error","message":"Please update your Claude Code CLI by running npm update -g @anthropic-ai/claude-code"}}"#,
        );

        let (rewritten, changed) = maybe_rewrite_claude_cli_version_gate_body(
            StatusCode::BAD_REQUEST,
            &stored,
            ProxyRoute::ClaudeMessages,
            body,
        );
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap();

        assert!(changed);
        assert!(message.contains("cc-switch-server admin"));
        assert!(message.contains("CC_SWITCH_CLI_UA_VERSION"));
        assert!(!message.contains("npm update -g"));
    }

    #[test]
    fn normalize_codex_oauth_responses_body_adds_required_chatgpt_fields() {
        let body = json!({
            "model": "gpt-5",
            "input": [{"role": "user", "content": "hi"}]
        });
        let normalized =
            normalize_codex_oauth_responses_body(body, None, CodexImageToolStripPolicy::Never);
        assert_eq!(normalized["store"], json!(false));
        assert_eq!(normalized["stream"], json!(true));
        assert!(normalized["instructions"]
            .as_str()
            .is_some_and(|instructions| !instructions.trim().is_empty()));
        assert_eq!(normalized["tools"], json!([]));
        assert_eq!(normalized["parallel_tool_calls"], json!(false));
        assert!(normalized["include"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "reasoning.encrypted_content"));
    }

    #[test]
    fn normalize_codex_oauth_gates_reasoning_and_strips_invalid_message_ids() {
        let normalized = normalize_codex_oauth_responses_body(
            json!({
                "model": "gpt-5.6-luna",
                "reasoning": {"effort": "ultra"},
                "input": [
                    {"type": "message", "id": "item_bad", "role": "user", "content": []},
                    {"type": "message", "id": "msg_valid", "role": "assistant", "content": []},
                    {"type": "function_call", "id": "item_call", "call_id": "call_1"}
                ]
            }),
            None,
            CodexImageToolStripPolicy::Never,
        );
        assert_eq!(normalized.pointer("/reasoning/effort"), Some(&json!("max")));
        assert!(normalized.pointer("/input/0/id").is_none());
        assert_eq!(normalized.pointer("/input/1/id"), Some(&json!("msg_valid")));
        assert_eq!(normalized.pointer("/input/2/id"), Some(&json!("item_call")));
    }

    #[test]
    fn normalize_codex_oauth_responses_body_strips_image_generation_tools_when_configured() {
        let normalized = normalize_codex_oauth_responses_body(
            json!({
                "model": "gpt-5",
                "tools": [
                    {"type": "image_generation"},
                    {"type": "function", "name": "lookup"}
                ],
                "input": [{
                    "type": "additional_tools",
                    "tools": [
                        {"type": "image_gen"},
                        {"type": "custom", "name": "edit"}
                    ]
                }]
            }),
            None,
            CodexImageToolStripPolicy::Always,
        );
        assert_eq!(normalized.pointer("/tools/0/name"), Some(&json!("lookup")));
        assert_eq!(
            normalized.pointer("/input/0/tools/0/name"),
            Some(&json!("edit"))
        );
    }

    #[test]
    fn codex_image_tool_on_error_helpers_detect_rejection_and_build_retry_body() {
        assert!(codex_image_tool_rejection_body(
            br#"{"error":{"message":"unsupported image_generation tool"}}"#,
        ));
        assert!(!codex_image_tool_rejection_body(
            br#"{"error":{"message":"ordinary upstream failure"}}"#,
        ));

        let retry = codex_image_tool_stripped_body_bytes(&Bytes::from_static(
            br#"{"tools":[{"type":"image_generation"},{"type":"function","name":"lookup"}]}"#,
        ))
        .unwrap()
        .unwrap();
        let value: Value = serde_json::from_slice(&retry).unwrap();
        assert_eq!(value.pointer("/tools/0/name"), Some(&json!("lookup")));
    }

    #[test]
    fn codex_oauth_chat_completions_body_gets_store_false_after_normalize() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({
                "env": {
                    "OPENAI_API_KEY": "oauth-token"
                }
            }),
            None,
        );
        let adapter = adapters::adapter_for(AppKind::Codex, ProviderType::CodexOAuth);
        let request = adapter
            .transform_request_for_route_with_metadata(
                Bytes::from_static(
                    br#"{"model":"gpt-5.5","messages":[{"role":"user","content":"who are you"}],"max_completion_tokens":16}"#,
                ),
                &stored,
                ProxyRoute::CodexChatCompletions,
                None,
                &adapters::CopilotRequestMetadata {
                    has_anthropic_beta: false,
                    session_id: None,
                },
            )
            .unwrap();
        let normalized = normalize_codex_oauth_responses_body_bytes(
            &request.body,
            None,
            CodexImageToolStripPolicy::Never,
        )
        .expect("normalize");
        let value: Value = serde_json::from_slice(&normalized).unwrap();
        assert_eq!(value["store"], json!(false));
    }

    #[test]
    fn normalize_codex_oauth_responses_body_strips_unsupported_fields() {
        let body = json!({
            "model": "gpt-5",
            "input": [],
            "max_output_tokens": 128,
            "temperature": 0.2
        });
        let normalized =
            normalize_codex_oauth_responses_body(body, None, CodexImageToolStripPolicy::Never);
        assert!(normalized.get("max_output_tokens").is_none());
        assert!(normalized.get("temperature").is_none());
    }

    #[test]
    fn normalize_codex_oauth_responses_body_injects_prompt_cache_key() {
        let body = json!({
            "model": "gpt-5",
            "input": []
        });
        let normalized = normalize_codex_oauth_responses_body(
            body,
            Some("session-123"),
            CodexImageToolStripPolicy::Never,
        );
        assert_eq!(normalized["prompt_cache_key"], json!("session-123"));
    }

    #[test]
    fn codex_compact_body_signal_promotes_and_strips_stream_fields() {
        let body = Bytes::from_static(
            br#"{"model":"gpt-5.5","stream":true,"store":true,"prompt_cache_key":"pck","input":[{"type":"message","role":"user"},{"type":"compaction_trigger"}]}"#,
        );
        assert!(codex_responses_body_has_compaction_trigger(&body));
        let normalized = normalize_codex_oauth_compact_body_bytes(&body).unwrap();
        let value: Value = serde_json::from_slice(&normalized).unwrap();
        assert!(value.get("stream").is_none());
        assert!(value.get("store").is_none());
        assert!(value.get("prompt_cache_key").is_none());
        assert_eq!(
            codex_compact_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );
    }

    #[test]
    fn normalize_codex_oauth_responses_body_preserves_existing_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "instructions": "Keep this local policy.",
            "input": []
        });
        let normalized =
            normalize_codex_oauth_responses_body(body, None, CodexImageToolStripPolicy::Never);
        let instructions = normalized["instructions"].as_str().unwrap();
        assert!(instructions.contains("Keep this local policy."));
        assert!(instructions.len() > "Keep this local policy.".len());
    }

    #[test]
    fn codex_pending_function_call_patcher_delays_unnamed_tool_until_done() {
        let mut patcher = CodexPendingFunctionCallPatcher {
            enabled: true,
            ..CodexPendingFunctionCallPatcher::disabled()
        };
        let output = String::from_utf8(
            patcher
                .push(Bytes::from_static(
                    br#"event: response.output_item.added
data: {"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"call_1"}}

event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"q\":\"x\"}"}

event: response.output_item.done
data: {"type":"response.output_item.done","output_index":2,"item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"q\":\"x\"}"}}

"#,
                ))
                .to_vec(),
        )
        .unwrap();
        assert!(output.contains("\"type\":\"response.output_item.added\""));
        assert!(output.contains("\"name\":\"lookup\""));
        assert!(output.contains("response.function_call_arguments.delta"));
        assert!(output.contains("{\\\"q\\\":\\\"x\\\"}"));
        assert!(output.contains("response.output_item.done"));
    }

    #[test]
    fn codex_images_generation_builds_responses_request_and_extracts_fallback_output() {
        let prepared = codex_images_generation_request(
            br#"{"prompt":"draw a cat","model":"gpt-image-2","response_format":"url","size":"1024x1024","stream":false}"#,
        )
        .unwrap();
        let request: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(
            request.get("model").and_then(Value::as_str),
            Some(CODEX_IMAGES_RESPONSES_MAIN_MODEL)
        );
        assert_eq!(
            request.pointer("/tools/0/type").and_then(Value::as_str),
            Some("image_generation")
        );
        assert_eq!(
            request.pointer("/tools/0/size").and_then(Value::as_str),
            Some("1024x1024")
        );

        let response = codex_images_response_from_responses_body(
            br#"data: {"type":"response.output_item.done","item":{"id":"ig_1","type":"image_generation_call","result":"aGVsbG8=","output_format":"png","revised_prompt":"cat"}}

data: {"type":"response.completed","response":{"created_at":1800000000,"output":[]}}

"#,
            Some("url"),
            false,
        )
        .unwrap();
        assert_eq!(response.content_type, "application/json");
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["created"], json!(1_800_000_000));
        assert_eq!(
            value.pointer("/data/0/url").and_then(Value::as_str),
            Some("data:image/png;base64,aGVsbG8=")
        );
        assert_eq!(
            value
                .pointer("/data/0/revised_prompt")
                .and_then(Value::as_str),
            Some("cat")
        );
    }

    #[test]
    fn codex_completed_output_patcher_reconstructs_empty_completed_output() {
        let mut patcher = CodexCompletedOutputPatcher {
            enabled: true,
            ..CodexCompletedOutputPatcher::disabled()
        };
        let chunk = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","output_index":1,"item":{"id":"item-2","type":"message"}}

event: response.output_item.done
data: {"type":"response.output_item.done","output_index":0,"item":{"id":"item-1","type":"reasoning"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp-1","output":[]}}

"#,
        );

        let output = String::from_utf8(patcher.push(chunk).to_vec()).unwrap();
        let completed_payload = output
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.contains("response.completed"))
            .unwrap();
        let completed: Value = serde_json::from_str(completed_payload).unwrap();
        let output = completed["response"]["output"].as_array().unwrap();
        assert_eq!(output[0]["id"], json!("item-1"));
        assert_eq!(output[1]["id"], json!("item-2"));
    }

    #[test]
    fn codex_completed_output_patcher_handles_split_sse_events() {
        let mut patcher = CodexCompletedOutputPatcher {
            enabled: true,
            ..CodexCompletedOutputPatcher::disabled()
        };
        let first = patcher.push(Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"id":"item-1","type":"message"}}

event: response.completed
"#,
        ));
        assert!(String::from_utf8(first.to_vec())
            .unwrap()
            .contains("output_item.done"));

        let second = String::from_utf8(
            patcher
                .push(Bytes::from_static(
                    br#"data: {"type":"response.completed","response":{"id":"resp-1"}}
"#,
                ))
                .to_vec(),
        )
        .unwrap();
        assert!(second.is_empty());

        let tail = String::from_utf8(patcher.finish().to_vec()).unwrap();
        let completed_payload = tail
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|line| line.contains("response.completed"))
            .unwrap();
        let completed: Value = serde_json::from_str(completed_payload).unwrap();
        assert_eq!(
            completed["response"]["output"],
            json!([{"id": "item-1", "type": "message"}])
        );
    }

    #[test]
    fn codex_completed_output_patcher_keeps_nonempty_completed_output() {
        let mut patcher = CodexCompletedOutputPatcher {
            enabled: true,
            ..CodexCompletedOutputPatcher::disabled()
        };
        let output = String::from_utf8(
            patcher
                .push(Bytes::from_static(
                    br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"id":"collected","type":"message"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp-1","output":[{"id":"existing","type":"message"}]}}

"#,
                ))
                .to_vec(),
        )
        .unwrap();

        assert!(output.contains("\"id\":\"existing\""));
        assert!(!output.contains("\"id\":\"collected\",\"type\":\"message\"}]}}"));
    }

    #[test]
    fn codex_rate_limit_reset_parses_seconds_and_absolute_epoch() {
        assert_eq!(
            codex_rate_limit_reset_at_ms(
                br#"{"error":{"resets_in_seconds":12,"message":"slow down"}}"#,
                1_000
            ),
            Some(13_000)
        );
        assert_eq!(
            codex_rate_limit_reset_at_ms(br#"{"error":{"resets_at":20}}"#, 1_000),
            Some(20_000)
        );
        assert_eq!(
            codex_rate_limit_reset_at_ms(br#"{"error":{"resets_at":1}}"#, 1_000),
            None
        );
    }

    #[test]
    fn upstream_rate_limit_cooldown_is_generic_and_bounded() {
        let now = 1_700_000_000_000;
        let headers = HeaderMap::new();
        assert_eq!(
            upstream_rate_limit_until(
                ProviderType::KiroOAuth,
                StatusCode::TOO_MANY_REQUESTS,
                &headers,
                b"{}",
                now,
            ),
            Some(now + DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS)
        );
        assert_eq!(
            upstream_rate_limit_until(
                ProviderType::KiroOAuth,
                StatusCode::OK,
                &headers,
                b"{}",
                now,
            ),
            None
        );

        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("999999999"));
        assert_eq!(
            upstream_rate_limit_until(
                ProviderType::GeminiCli,
                StatusCode::TOO_MANY_REQUESTS,
                &headers,
                b"{}",
                now,
            ),
            Some(now + super::super::MAX_UPSTREAM_RATE_LIMIT_COOLDOWN_MS)
        );

        assert_eq!(
            upstream_rate_limit_until(
                ProviderType::CodexOAuth,
                StatusCode::TOO_MANY_REQUESTS,
                &HeaderMap::new(),
                br#"{"error":{"resets_in_seconds":12}}"#,
                now,
            ),
            Some(now + 12_000)
        );
    }

    #[test]
    fn codex_oauth_client_gate_blocks_generic_tools_with_originator() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("acct-1"),
        );
        let mut headers = HeaderMap::new();
        headers.insert("originator", HeaderValue::from_static("postman"));
        headers.insert("user-agent", HeaderValue::from_static("PostmanRuntime/7"));
        let error =
            validate_codex_allowed_client(&stored, ProxyRoute::CodexResponses, &headers, false)
                .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);

        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("user-agent", HeaderValue::from_static("curl/8.0"));
        let error =
            validate_codex_allowed_client(&stored, ProxyRoute::CodexResponses, &headers, false)
                .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);

        headers.insert(
            "user-agent",
            HeaderValue::from_static(
                "codex_cli_rs/0.144.1 (Ubuntu 22.04.0; x86_64) xterm-256color",
            ),
        );
        validate_codex_allowed_client(&stored, ProxyRoute::CodexResponses, &headers, false)
            .unwrap();
    }

    #[test]
    fn codex_oauth_client_gate_allows_share_requests_without_originator() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("acct-1"),
        );
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("curl/8.0"));
        validate_codex_allowed_client(&stored, ProxyRoute::CodexResponses, &headers, true).unwrap();
    }

    #[test]
    fn codex_oauth_session_headers_strip_internal_prefix_and_build_window_id() {
        assert_eq!(
            codex_oauth_upstream_session_id("codex_736fc774-8efb-4f67-b8ab-771fc2afe205")
                .as_deref(),
            Some("736fc774-8efb-4f67-b8ab-771fc2afe205")
        );
        assert_eq!(
            codex_oauth_session_id_from_body(br#"{"metadata":{"session_id":"codex_session-123"}}"#)
                .as_deref(),
            Some("session-123")
        );

        let mut headers = Vec::new();
        append_codex_oauth_session_headers(&mut headers, Some("session-123"));

        assert!(headers.contains(&("session_id", "session-123".to_string())));
        assert!(headers.contains(&("x-client-request-id", "session-123".to_string())));
        assert!(headers.contains(&("x-codex-window-id", "session-123:0".to_string())));
    }

    #[test]
    fn extracts_session_id_for_claude_and_codex_logs() {
        assert_eq!(
            claude_session_id_from_body(
                br#"{"metadata":{"user_id":"user_john_doe_session_abc123def456"}}"#
            )
            .as_deref(),
            Some("abc123def456")
        );
        assert_eq!(
            claude_session_id_from_body(br#"{"metadata":{"session_id":"my-session-123"}}"#)
                .as_deref(),
            Some("my-session-123")
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("header-session-123"),
        );
        assert_eq!(
            session_id_from_request(
                ProxyRoute::ClaudeMessages,
                &headers,
                br#"{"metadata":{"session_id":"body-session"}}"#,
            )
            .as_deref(),
            Some("header-session-123")
        );

        let mut codex_headers = HeaderMap::new();
        codex_headers.insert(
            "x-session-id",
            HeaderValue::from_static("codex_session-123"),
        );
        assert_eq!(
            session_id_from_request(ProxyRoute::CodexResponses, &codex_headers, b"{}").as_deref(),
            Some("session-123")
        );
        codex_headers.clear();
        codex_headers.insert(
            "x-codex-window-id",
            HeaderValue::from_static("session-456:0"),
        );
        assert_eq!(
            session_id_from_request(ProxyRoute::CodexResponses, &codex_headers, b"{}").as_deref(),
            Some("session-456")
        );
    }

    #[test]
    fn strips_hop_by_hop_response_headers_and_connection_extensions() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, x-hop"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("x-hop", HeaderValue::from_static("remove"));
        headers.insert("x-end-to-end", HeaderValue::from_static("keep"));

        strip_hop_by_hop_response_headers(&mut headers);

        assert!(!headers.contains_key(CONNECTION));
        assert!(!headers.contains_key("transfer-encoding"));
        assert!(!headers.contains_key("keep-alive"));
        assert!(!headers.contains_key("x-hop"));
        assert_eq!(
            headers
                .get("x-end-to-end")
                .and_then(|value| value.to_str().ok()),
            Some("keep")
        );
    }

    #[test]
    fn copies_only_safe_upstream_headers_to_downstream_response() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req_123"));
        headers.insert(
            "anthropic-ratelimit-unified-reset",
            HeaderValue::from_static("2026-07-13T12:00:00Z"),
        );
        headers.insert("retry-after", HeaderValue::from_static("30"));
        headers.insert("set-cookie", HeaderValue::from_static("secret=value"));
        headers.insert("server", HeaderValue::from_static("upstream"));
        let mut response = Response::new(Body::empty());

        copy_safe_upstream_response_headers(&headers, &mut response);

        assert_eq!(response.headers().get("x-request-id").unwrap(), "req_123");
        assert_eq!(response.headers().get("retry-after").unwrap(), "30");
        assert!(response
            .headers()
            .contains_key("anthropic-ratelimit-unified-reset"));
        assert!(!response.headers().contains_key("set-cookie"));
        assert!(!response.headers().contains_key("server"));
    }

    #[test]
    fn stream_timeouts_use_split_defaults_and_can_disable_idle() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({
                "STREAM_FIRST_BYTE_TIMEOUT_MS": "25",
                "STREAM_IDLE_TIMEOUT_MS": "0"
            }),
            None,
        );

        let timeouts = stream_timeout_config(&stored);

        assert_eq!(timeouts.first_byte, Some(Duration::from_millis(25)));
        assert_eq!(timeouts.idle, None);
    }

    #[test]
    fn stream_terminal_error_frames_match_client_protocols() {
        let responses = stream_terminal_error_frame(ProxyRoute::CodexResponses, "boom", 504)
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .unwrap();
        assert!(responses.contains("event: response.failed"));
        assert!(responses.contains("cc_switch_stream_error"));
        assert!(responses.contains("data: [DONE]"));

        let chat = stream_terminal_error_frame(ProxyRoute::CodexChatCompletions, "boom", 502)
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .unwrap();
        assert!(chat.contains("\"error\""));
        assert!(chat.contains("data: [DONE]"));

        let claude = stream_terminal_error_frame(ProxyRoute::ClaudeMessages, "boom", 502)
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .unwrap();
        assert!(claude.contains("event: error"));
        assert!(claude.contains("event: message_stop"));
    }

    #[test]
    fn websocket_message_too_big_maps_to_structured_error() {
        let error = TungsteniteError::Capacity(CapacityError::MessageTooLong {
            size: 128,
            max_size: 64,
        });
        assert!(websocket_message_too_big(&error));
        let body = websocket_message_too_big_error_body();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value.pointer("/error/code").and_then(Value::as_str),
            Some("message_too_big")
        );

        let message =
            TungsteniteMessage::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: CloseCode::Size,
                reason: "message too big".into(),
            }));
        match tungstenite_message_to_axum_ws(message) {
            Some(AxumWsMessage::Text(text)) => {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    value.pointer("/error/code").and_then(Value::as_str),
                    Some("message_too_big")
                );
            }
            other => panic!("unexpected websocket message: {other:?}"),
        }
    }

    #[test]
    fn websocket_handshake_http_error_preserves_rate_limit_evidence() {
        let response = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(429)
            .header("retry-after", "30")
            .body(Some(br#"{"error":"rate_limited"}"#.to_vec()))
            .unwrap();
        let error = TungsteniteError::Http(response);

        let (status, headers, body) =
            responses_websocket_http_error(&error).expect("HTTP handshake error");

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(headers.get("retry-after").unwrap(), "30");
        assert_eq!(body, br#"{"error":"rate_limited"}"#);
    }

    #[test]
    fn grok_websocket_single_model_matches_http_routing_policy() {
        for requested in [Some("gpt-5.5"), Some("grok-4.3"), Some("grok"), None] {
            let request = match requested {
                Some(model) => json!({
                    "type": "response.create",
                    "response": {"model": model, "input": "ping"}
                }),
                None => json!({
                    "type": "response.create",
                    "response": {"input": "ping"}
                }),
            };
            let transformed = transform_responses_websocket_request(
                &request.to_string(),
                ResponsesWebsocketMode::Grok,
                Some("session-1"),
                Some("grok-4.5"),
            )
            .unwrap();
            let value: Value = serde_json::from_str(&transformed).unwrap();

            assert_eq!(
                value.pointer("/response/model").and_then(Value::as_str),
                Some("grok-4.5")
            );
        }
    }

    #[test]
    fn grok_websocket_uses_edited_single_model() {
        let transformed = transform_responses_websocket_request(
            r#"{"model":"gpt-5.5","input":"ping"}"#,
            ResponsesWebsocketMode::Grok,
            None,
            Some("grok-custom"),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&transformed).unwrap();

        assert_eq!(
            value.pointer("/response/model").and_then(Value::as_str),
            Some("grok-custom")
        );
    }

    #[test]
    fn special_claude_paths_resolve_single_model_before_vendor_normalization() {
        let kiro = stored_provider(
            AppKind::Claude,
            ProviderType::KiroOAuth,
            json!({
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "claude-opus-4-8"
                }
            }),
            Some("kiro-account"),
        );
        let (kiro_body, kiro_selection) = adapters::apply_provider_model_routing(
            Bytes::from_static(br#"{"model":"claude-haiku-4-5","messages":[]}"#),
            &kiro,
            ProxyRoute::ClaudeMessages,
        );
        let kiro_body: Value = serde_json::from_slice(&kiro_body).unwrap();
        let kiro_routed = kiro_body.get("model").and_then(Value::as_str).unwrap();
        assert_eq!(
            kiro_selection.requested_model.as_deref(),
            Some("claude-haiku-4-5")
        );
        assert_eq!(kiro_routed, "claude-opus-4-8");
        assert_eq!(kiro::map_model(kiro_routed), Some("claude-opus-4.8"));

        let deepseek = stored_provider(
            AppKind::Claude,
            ProviderType::DeepSeekAccount,
            json!({
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "deepseek-v4-pro"
                }
            }),
            Some("deepseek-account"),
        );
        let (deepseek_body, deepseek_selection) = adapters::apply_provider_model_routing(
            Bytes::from_static(br#"{"model":"claude-haiku-4-5","messages":[]}"#),
            &deepseek,
            ProxyRoute::ClaudeMessages,
        );
        let deepseek_body: Value = serde_json::from_slice(&deepseek_body).unwrap();
        let deepseek_routed = deepseek_body.get("model").and_then(Value::as_str).unwrap();
        assert_eq!(
            deepseek_selection.requested_model.as_deref(),
            Some("claude-haiku-4-5")
        );
        assert_eq!(deepseek_routed, "deepseek-v4-pro");
        assert_eq!(deepseek::map_model(deepseek_routed), "deepseek-v4-pro");
    }

    #[test]
    fn websocket_reset_classification_covers_windows_and_unix() {
        for code in [54, 104, 995, 10053, 10054] {
            assert!(websocket_expected_reset(&TungsteniteError::Io(
                std::io::Error::from_raw_os_error(code)
            )));
        }
        assert!(websocket_expected_reset(&TungsteniteError::Protocol(
            ProtocolError::ResetWithoutClosingHandshake
        )));
    }

    #[test]
    fn codex_websocket_toggle_defaults_on_and_supports_incident_rollback() {
        let enabled = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("acct-1"),
        );
        assert!(codex_websocket_enabled(&enabled));

        let mut disabled = enabled;
        disabled
            .provider
            .meta
            .get_or_insert_default()
            .codex_websocket_enabled = Some(false);
        assert!(!codex_websocket_enabled(&disabled));
    }

    #[test]
    fn websocket_completed_output_is_rebuilt_in_index_order_and_state_is_cleared() {
        let mut patcher = CodexWebsocketOutputPatcher::default();
        for raw in [
            r#"{"type":"response.output_item.done","output_index":2,"item":{"id":"third"}}"#,
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"first"}}"#,
            r#"{"type":"response.output_item.done","item":{"id":"fallback"}}"#,
        ] {
            let mut message = TungsteniteMessage::Text(raw.to_string());
            patcher.patch_message(&mut message);
        }
        let mut completed = TungsteniteMessage::Text(
            r#"{"type":"response.completed","response":{"output":[]}}"#.to_string(),
        );
        patcher.patch_message(&mut completed);
        let TungsteniteMessage::Text(completed) = completed else {
            panic!("expected text frame");
        };
        let value: Value = serde_json::from_str(&completed).unwrap();
        assert_eq!(
            value.pointer("/response/output/0/id"),
            Some(&json!("first"))
        );
        assert_eq!(
            value.pointer("/response/output/1/id"),
            Some(&json!("third"))
        );
        assert_eq!(
            value.pointer("/response/output/2/id"),
            Some(&json!("fallback"))
        );

        let mut next = TungsteniteMessage::Text(
            r#"{"type":"response.completed","response":{"output":[]}}"#.to_string(),
        );
        patcher.patch_message(&mut next);
        let TungsteniteMessage::Text(next) = next else {
            panic!("expected text frame");
        };
        assert_eq!(
            next,
            r#"{"type":"response.completed","response":{"output":[]}}"#
        );
    }

    #[test]
    fn websocket_completed_output_preserves_existing_and_supports_binary_json() {
        let mut patcher = CodexWebsocketOutputPatcher::default();
        let mut collected = TungsteniteMessage::Binary(
            br#"{"type":"response.output_item.done","output_index":0,"item":{"id":"collected"}}"#
                .to_vec(),
        );
        patcher.patch_message(&mut collected);
        let raw = r#"{"type":"response.completed","response":{"output":[{"id":"existing"}]}}"#;
        let mut completed = TungsteniteMessage::Binary(raw.as_bytes().to_vec());
        patcher.patch_message(&mut completed);
        let TungsteniteMessage::Binary(completed) = completed else {
            panic!("expected binary frame");
        };
        assert_eq!(completed, raw.as_bytes());
    }

    #[test]
    fn custom_tool_stream_bridge_restores_freeform_events_and_completed_output() {
        let mut patcher = CodexCustomToolStreamPatcher::default();
        let chunk = Bytes::from_static(
            br#"event: response.output_item.added
data: {"type":"response.output_item.added","output_index":0,"item":{"id":"ctc_call_1","type":"custom_tool_call","status":"in_progress","input":"","call_id":"call_1","name":"exec","cc_switch_custom_bridge":true}}

event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"input\":\"pwd\"}"}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[]}}

"#,
        );
        let output = String::from_utf8(patcher.push(chunk).to_vec()).unwrap();
        assert!(!output.contains("cc_switch_custom_bridge"));
        assert!(!output.contains("response.function_call_arguments.delta"));
        assert!(output.contains("response.custom_tool_call_input.done"));
        assert!(output.contains("\"input\":\"pwd\""));
        assert!(output.contains("response.output_item.done"));
        assert!(output.contains("\"output\":[{\"id\":\"ctc_call_1\""));
    }
}
