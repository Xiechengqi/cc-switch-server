use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::header::{ACCEPT, CONNECTION, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::stream::{self, BoxStream};
use futures_util::{StreamExt, TryStreamExt};
use serde_json::json;

use crate::core::account_refresh::{
    account_needs_native_refresh, execute_native_account_refresh, AccountRefreshFailure,
};
use crate::core::failover::{current_time_ms, ProviderOutcome};
use crate::core::provider::{AppKind, ProviderType};
use crate::core::providers::{ProviderStore, StoredProvider};
use crate::core::shares::{ShareInvocationRejection, ShareRejectReason, ShareStore};
use crate::core::usage::{TokenUsage, UsageLogContext, UsageModelMetadata};
use crate::state::{
    build_provider_http_client, save_accounts_debounced, save_shares_debounced, ServerState,
    ShareInFlightGuard,
};

use super::adapters::{self, ProviderAdapter};
use super::cursor;
use super::request_governance::{
    content_encoding_value, decode_request_body_for_proxy, decode_response_body_for_proxy,
};
use super::router::{select_provider, ProxyRoute};
use super::streaming::StreamUsageAccumulator;
use super::usage::{log_usage, update_stream_usage};
use super::{setting, ProxyError};

pub async fn forward(
    state: ServerState,
    route: ProxyRoute,
    gemini_path: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = decode_request_body_for_proxy(&headers, body)?;
    let app = route.app();
    let mut request_context = request_context_from_headers(&headers);
    request_context.session_id = session_id_from_request(route, &headers, &body);
    let share_invocation_guard = if let Some(share_id) = request_context.share_id.clone() {
        let (share_name, guard) = validate_and_acquire_share_invocation(&state, &share_id).await?;
        request_context.share_name = Some(share_name);
        Some(guard)
    } else {
        None
    };
    let shares = state.shares.read().await.clone();
    let providers = state.providers.read().await;
    let stored = if let Some(share_id) = request_context.share_id.as_deref() {
        let (stored, _share_name) = select_share_provider(&providers, &shares, app, share_id)?;
        stored
    } else {
        let mut failover = state.failover.write().await;
        let selected = select_provider(&providers, &mut failover, app, &headers)?;
        if selected.failover_state_changed {
            drop(failover);
            if let Err(error) = state.save_failover().await {
                tracing::warn!("failed to persist failover selection state: {error}");
            }
        }
        selected.provider
    };
    drop(providers);
    let started = Instant::now();
    if cursor::agentservice_driver_requested(&stored) {
        let adapter_request =
            adapters::cursor_agentservice_request(body, &stored, route, gemini_path.as_deref())?;
        refresh_managed_account_if_needed(&state, app, &stored).await?;
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
    let url =
        adapter.resolve_endpoint_for_request(route, gemini_path, &stored, &adapter_request)?;
    refresh_managed_account_if_needed(&state, app, &stored).await?;
    let accounts = state.accounts.read().await.clone();
    let mut target_headers = adapter.build_headers(app, &stored, &accounts)?;
    target_headers.extend(adapter_request.upstream_headers.iter().cloned());
    if stored.provider_type == ProviderType::CodexOAuth {
        append_codex_oauth_session_headers(&mut target_headers, codex_oauth_session_id.as_deref());
    }

    let http_client = forward_http_client(&state, &stored).await?;
    let mut request = http_client
        .post(&url)
        .body(adapter_request.body.clone())
        .header(ACCEPT, copy_header(&headers, ACCEPT).unwrap_or("*/*"));

    if let Some(content_type) = copy_header(&headers, CONTENT_TYPE) {
        request = request.header(CONTENT_TYPE, content_type);
    } else {
        request = request.header(CONTENT_TYPE, "application/json");
    }

    for (name, value) in target_headers {
        request = request.header(name, value);
    }
    if !adapter_request.stream_requested {
        request = request.timeout(provider_upstream_timeout(&stored));
    }

    let upstream_result = if adapter_request.stream_requested {
        match stream_first_byte_timeout(&stored) {
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
    let content_type = response_headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_encoding = content_encoding_value(&response_headers);

    if adapter_request.stream_requested {
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

        let stream_adapter = adapter;
        let stream_stored = stored.clone();
        let interrupted_update_armed = Arc::new(AtomicBool::new(true));
        let stream_state = StreamForwardState {
            inner: upstream.bytes_stream().boxed(),
            adapter: stream_adapter,
            stored: stream_stored,
            state: state.clone(),
            route,
            request_id,
            status_code,
            share_id: request_context.share_id.clone(),
            started,
            first_token_ms: None,
            received_any_chunk: false,
            usage: StreamUsageAccumulator::default(),
            timeouts: stream_timeout_config(&stored),
            terminal_frame_sent: false,
            interrupted_update_armed,
            _share_invocation_guard: share_invocation_guard,
        };
        let stream = stream::try_unfold(stream_state, |mut stream_state| async move {
            if stream_state.terminal_frame_sent {
                return Ok(None);
            }

            let timeout_kind = stream_state.next_timeout_kind();
            let next_chunk = match stream_state.next_timeout() {
                Some(timeout) => {
                    match tokio::time::timeout(timeout, stream_state.inner.try_next()).await {
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
            };

            match next_chunk {
                Ok(Some(chunk)) => {
                    stream_state.received_any_chunk = true;
                    stream_state.usage.push(&chunk);
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
                    let transformed = stream_state
                        .adapter
                        .transform_stream_event(chunk, &stream_state.stored, stream_state.route)
                        .map_err(std::io::Error::other)?;
                    Ok(Some((transformed, stream_state)))
                }
                Ok(None) => {
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
                        usage,
                    )
                    .await;
                    record_provider_outcome(
                        &stream_state.state,
                        &stream_state.stored,
                        ProviderOutcome::from_status(stream_state.status_code),
                    )
                    .await;
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
        return Ok(response);
    }

    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
            return Err(ProxyError::bad_gateway(error));
        }
    };
    let decoded = decode_response_body_for_proxy(&response_headers, bytes);
    let preserve_content_encoding = decoded.preserve_content_encoding;
    let bytes = decoded.body;
    let usage = adapter.parse_usage(&bytes);
    let bytes = adapter.transform_response(bytes, &stored, route)?;
    let share_id_for_record = request_context.share_id.clone();
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
    record_share_invocation_result(&state, share_id_for_record.as_deref(), usage).await;
    record_provider_outcome(&state, &stored, ProviderOutcome::from_status(status_code)).await;

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
    Ok(response)
}

async fn validate_and_acquire_share_invocation(
    state: &ServerState,
    share_id: &str,
) -> Result<(String, ShareInFlightGuard), ProxyError> {
    let validation = {
        let mut shares = state.shares.write().await;
        shares.validate_for_invocation(share_id, crate::core::usage::now_ms() as i64)
    };

    let invocation = match validation {
        Ok(invocation) => invocation,
        Err(rejection) => {
            if rejection.status_changed {
                save_shares_debounced(state);
            }
            return Err(share_rejection_to_proxy_error(rejection));
        }
    };

    let guard = state
        .share_in_flight
        .try_acquire(&invocation.share_id, invocation.parallel_limit)
        .ok_or_else(|| {
            share_rejection_to_proxy_error(ShareInvocationRejection {
                reason: ShareRejectReason::ParallelLimit,
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
        ShareRejectReason::ParallelLimit => StatusCode::TOO_MANY_REQUESTS,
        ShareRejectReason::Inactive | ShareRejectReason::Expired | ShareRejectReason::Exhausted => {
            StatusCode::FORBIDDEN
        }
    };
    ProxyError {
        status,
        message: rejection.formatted_message(),
    }
}

pub(super) async fn record_share_invocation_result(
    state: &ServerState,
    share_id: Option<&str>,
    usage: TokenUsage,
) {
    let Some(share_id) = share_id else {
        return;
    };
    {
        let mut shares = state.shares.write().await;
        shares.record_invocation_result(share_id, share_usage_tokens(usage));
    }
    save_shares_debounced(state);
}

pub(super) async fn record_provider_outcome(
    state: &ServerState,
    stored: &StoredProvider,
    outcome: ProviderOutcome,
) {
    let updated = {
        let mut failover = state.failover.write().await;
        failover.record_outcome(stored.app, &stored.provider.id, outcome, current_time_ms())
    };
    if updated {
        if let Err(error) = state.save_failover().await {
            tracing::warn!("failed to persist failover breaker state: {error}");
        }
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

async fn refresh_managed_account_if_needed(
    state: &ServerState,
    app: AppKind,
    stored: &StoredProvider,
) -> Result<(), ProxyError> {
    if provider_secret_configured(app, stored) {
        return Ok(());
    }

    let account_id = managed_account_id(stored);
    let now = crate::core::usage::now_ms() as i64;
    let account = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(stored.provider_type, account_id)
            .cloned()
    };
    let Some(account) = account else {
        return Ok(());
    };
    if !account_needs_native_refresh(&account, now) {
        return Ok(());
    }

    let _refresh_guard = state
        .account_refresh_locks
        .try_lock(account.provider_type, &account.id)
        .ok_or_else(|| {
            ProxyError::conflict(format!(
                "{} account refresh is already in progress",
                account.provider_type.as_str()
            ))
        })?;

    let account = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(stored.provider_type, account_id)
            .cloned()
    }
    .ok_or_else(|| ProxyError::not_found("managed account not found"))?;
    if !account_needs_native_refresh(&account, now) {
        return Ok(());
    }

    let http_client = state.http_client().await;
    let update = match execute_native_account_refresh(&http_client, &account, now).await {
        Ok(update) => update,
        Err(error) => {
            {
                let mut accounts = state.accounts.write().await;
                accounts.mark_refresh_failure(&account.id, error.message.clone());
            }
            save_accounts_debounced(state);
            return Err(refresh_failure_to_proxy_error(error));
        }
    };

    {
        let mut accounts = state.accounts.write().await;
        accounts
            .mark_refresh_success(&account.id, update)
            .ok_or_else(|| ProxyError::not_found("managed account not found"))?;
    }
    save_accounts_debounced(state);
    Ok(())
}

fn managed_account_id(stored: &StoredProvider) -> Option<&str> {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
}

fn provider_secret_configured(app: AppKind, stored: &StoredProvider) -> bool {
    let provider = &stored.provider;
    match auth_header_app_for(app, stored.provider_type) {
        AppKind::Claude => setting(
            provider,
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "API_KEY"],
        )
        .is_some(),
        AppKind::Codex => setting(
            provider,
            &[
                "OPENAI_API_KEY",
                "CODEX_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "API_KEY",
            ],
        )
        .is_some(),
        AppKind::Gemini => {
            setting(provider, &["GEMINI_API_KEY", "GOOGLE_API_KEY", "API_KEY"]).is_some()
        }
    }
}

fn auth_header_app_for(app: AppKind, provider_type: ProviderType) -> AppKind {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
            AppKind::Claude
        }
        ProviderType::Codex | ProviderType::CodexOAuth | ProviderType::OllamaCloud => {
            AppKind::Codex
        }
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

fn refresh_failure_to_proxy_error(error: AccountRefreshFailure) -> ProxyError {
    ProxyError {
        status: StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
        message: format!("managed account refresh failed: {}", error.message),
    }
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

struct StreamForwardState {
    inner: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    adapter: adapters::GenericForwardingAdapter,
    stored: StoredProvider,
    state: ServerState,
    route: ProxyRoute,
    request_id: String,
    status_code: u16,
    share_id: Option<String>,
    started: Instant,
    first_token_ms: Option<u128>,
    received_any_chunk: bool,
    usage: StreamUsageAccumulator,
    timeouts: StreamTimeoutConfig,
    terminal_frame_sent: bool,
    interrupted_update_armed: Arc<AtomicBool>,
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
                Default::default(),
                Some("interrupted"),
            )
            .await;
            record_share_invocation_result(&state, share_id.as_deref(), Default::default()).await;
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
    stored: &StoredProvider,
) -> Result<reqwest::Client, ProxyError> {
    if let Some(proxy_url) = provider_upstream_proxy_url(stored) {
        return build_provider_http_client(&proxy_url, state.bind_addr)
            .map_err(|error| ProxyError::bad_request(format!("provider proxy invalid: {error}")));
    }
    Ok(state.http_client().await)
}

fn provider_upstream_proxy_url(stored: &StoredProvider) -> Option<String> {
    setting(
        &stored.provider,
        &[
            "UPSTREAM_PROXY_URL",
            "PROVIDER_PROXY_URL",
            "PROXY_URL",
            "HTTPS_PROXY",
            "HTTP_PROXY",
        ],
    )
    .or_else(|| {
        stored
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.local_proxy_request_overrides.as_ref())
            .and_then(|value| {
                [
                    "/upstreamProxyUrl",
                    "/upstream_proxy_url",
                    "/proxyUrl",
                    "/proxy_url",
                    "/httpsProxy",
                    "/httpProxy",
                ]
                .into_iter()
                .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
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
        ProxyRoute::ClaudeMessages => claude_session_id_from_request(headers, body),
        ProxyRoute::CodexChatCompletions | ProxyRoute::CodexResponses => {
            codex_oauth_session_id_from_request(headers, body)
        }
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

fn model_metadata(request: &adapters::AdapterRequest) -> UsageModelMetadata {
    UsageModelMetadata {
        model: request.model.clone(),
        requested_model: request.requested_model.clone(),
        actual_model: request.actual_model.clone(),
        actual_model_source: request.actual_model_source.clone(),
        pricing_model: request.pricing_model.clone(),
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
        ProxyRoute::CodexResponses => Some(Bytes::from(format!(
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
        ))),
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
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::core::accounts::Account;
    use crate::core::oauth_clients::OAuthErrorKind;
    use crate::core::provider::{AppKind, AuthBinding, Provider, ProviderMeta, ProviderType};

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
                    }),
                    provider_type: Some(provider_type.as_str().to_string()),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
        }
    }

    fn account(
        provider_type: ProviderType,
        access_token: Option<&str>,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
    ) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type,
            email: Some("test@example.com".to_string()),
            access_token: access_token.map(str::to_string),
            refresh_token: refresh_token.map(str::to_string),
            id_token: None,
            token_type: None,
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at,
            last_refresh_error: None,
        }
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
    fn native_refresh_decision_requires_refresh_token_and_expired_or_missing_access() {
        let now_ms = 1_000_000;

        assert!(account_needs_native_refresh(
            &account(ProviderType::CodexOAuth, None, Some("refresh"), None),
            now_ms
        ));
        assert!(account_needs_native_refresh(
            &account(
                ProviderType::CodexOAuth,
                Some("access"),
                Some("refresh"),
                Some(now_ms + 1_000)
            ),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(
                ProviderType::CodexOAuth,
                Some("access"),
                Some("refresh"),
                Some(now_ms + 3_600_000)
            ),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(ProviderType::CodexOAuth, None, None, None),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(ProviderType::Codex, None, Some("refresh"), None),
            now_ms
        ));
    }

    #[test]
    fn refresh_failures_keep_oauth_status_and_managed_account_context() {
        let proxy_error = refresh_failure_to_proxy_error(AccountRefreshFailure {
            status_code: 429,
            message: "rate limited by provider".to_string(),
            kind: OAuthErrorKind::RateLimited,
            retryable: true,
        });

        assert_eq!(proxy_error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            proxy_error.message,
            "managed account refresh failed: rate limited by provider"
        );
    }

    #[test]
    fn share_rejections_use_desktop_reason_suffix_and_status_mapping() {
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
}
