use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex as StdMutex, OnceLock,
};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::header::{ACCEPT, CONNECTION, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
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
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::domain::accounts::store::{
    grok_account_capability_enabled, AccountStore, GrokAccountCapability,
};
use crate::domain::health::ProviderRequestOutcome as ProviderOutcome;
use crate::domain::providers::current_provider;
use crate::domain::providers::model::{AppKind, CodexImageToolStripPolicy, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::shares::{ShareInvocationRejection, ShareRejectReason, ShareStore};
use crate::domain::usage::store::{TokenUsage, UsageLogContext, UsageModelMetadata};
use crate::infra::time::now_ms as current_time_ms;
use crate::state::{
    AccountInFlightGuard, AccountInFlightSnapshot, CopilotUpstreamAuthError, DeepSeekUpstreamError,
    GrokMediaSessionBinding, ManagedAccountRefreshError, ServerState, ShareInFlightGuard,
};

use super::adapters::{self, ProviderAdapter, UpstreamFormat};
use super::anthropic_semantics::{
    self, AnthropicJsonObservation, AnthropicObservation, AnthropicSseInspector, AnthropicTerminal,
};
use super::claude_oauth::ClaudeBodyRetryStage;
use super::cursor;
use super::deepseek;
use super::kiro;
use super::provider_ops::{ProviderExecution, ProviderOperation};
use super::request_governance::{
    content_encoding_value, decode_request_body_for_proxy_with_limit,
    decode_response_body_for_proxy, decode_response_body_for_proxy_with_limit,
    ResponseDecodeResult,
};
use super::response_semantics::{
    self, FailureOrigin, ResponsesSseInspector, SemanticFailure, SemanticObservation,
    SemanticTerminal,
};
use super::router::{
    account_concurrency_for_provider, codex_image_generation_provider,
    ensure_provider_account_does_not_need_relogin, ensure_provider_account_usage_available,
    provider_supports_claude_count_tokens, select_exact_provider_with_account_inflight,
    select_failover_provider, select_provider_for_codex_image_generation,
    select_provider_with_account_inflight, ProxyRoute,
};
use super::streaming::{ClaudeSseError, ClaudeSseErrorDetector, StreamUsageAccumulator};
use super::usage::{log_usage, update_stream_usage};
use super::{setting, ProxyError};

const CODEX_IMAGES_RESPONSES_MAIN_MODEL: &str = "gpt-5.4-mini";
const CODEX_IMAGES_DEFAULT_TOOL_MODEL: &str = "gpt-image-2";
const MAX_FORWARD_RETRY_ATTEMPTS: u32 = 3;
const MAX_FORWARD_RETRY_ELAPSED_MS: u128 = 10_000;
const DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS: i64 = 60_000;
const DEFAULT_UPSTREAM_AUTH_FAILURE_COOLDOWN_MS: i64 = 60_000;
const DEFAULT_CODEX_WEBSOCKET_CACHE_MAX_CONNECTIONS: usize = 64;
const DEFAULT_CODEX_WEBSOCKET_MAX_CONNECTIONS: usize = 128;
const DEFAULT_CODEX_WEBSOCKET_CACHE_IDLE_MS: u64 = 5 * 60 * 1000;
const DEFAULT_CODEX_WEBSOCKET_CACHE_MAX_AGE_MS: u64 = 55 * 60 * 1000;
const MAX_CODEX_HTTP_FALLBACK_SSE_EVENT_BYTES: usize = 128 * 1024 * 1024;
const MAX_RESPONSES_SEMANTIC_PRELUDE_MESSAGES: usize = 32;
const MAX_RESPONSES_SEMANTIC_PRELUDE_BYTES: usize = 1024 * 1024;
const CODEX_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const CODEX_RESPONSES_LITE_WS_METADATA: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";
const CODEX_MODELS_MANIFEST_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const CODEX_ALPHA_SEARCH_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const CODEX_OVERFLOW_SUMMARY_TIMEOUT: Duration = Duration::from_secs(120);
const PROXY_REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES: usize = 64 * 1024 * 1024;

type ResponsesUpstreamWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct CachedResponsesWebSocket {
    socket: ResponsesUpstreamWebSocket,
    created_at: Instant,
    last_used_at: Instant,
}

#[derive(Default)]
struct ResponsesWebSocketPool {
    entries: BTreeMap<String, VecDeque<CachedResponsesWebSocket>>,
    total: usize,
}

#[derive(Debug, Clone, Copy)]
struct ResponsesWebSocketPoolPolicy {
    max_connections: usize,
    idle_timeout: Duration,
    max_age: Duration,
}

impl ResponsesWebSocketPoolPolicy {
    fn current() -> Self {
        Self {
            max_connections: codex_websocket_cache_max_connections(),
            idle_timeout: codex_websocket_cache_idle_timeout(),
            max_age: codex_websocket_cache_max_age(),
        }
    }
}

impl ResponsesWebSocketPool {
    fn acquire(&mut self, key: &str) -> Option<CachedResponsesWebSocket> {
        self.acquire_with_policy(key, ResponsesWebSocketPoolPolicy::current())
    }

    fn acquire_with_policy(
        &mut self,
        key: &str,
        policy: ResponsesWebSocketPoolPolicy,
    ) -> Option<CachedResponsesWebSocket> {
        self.prune_expired_with_policy(policy);
        let entry = self.entries.get_mut(key)?.pop_back()?;
        self.total = self.total.saturating_sub(1);
        if self.entries.get(key).is_some_and(VecDeque::is_empty) {
            self.entries.remove(key);
        }
        Some(entry)
    }

    fn release(&mut self, key: String, entry: CachedResponsesWebSocket) {
        self.release_with_policy(key, entry, ResponsesWebSocketPoolPolicy::current());
    }

    fn release_with_policy(
        &mut self,
        key: String,
        mut entry: CachedResponsesWebSocket,
        policy: ResponsesWebSocketPoolPolicy,
    ) {
        if policy.max_connections == 0 || entry.created_at.elapsed() >= policy.max_age {
            return;
        }
        self.prune_expired_with_policy(policy);
        entry.last_used_at = Instant::now();
        self.entries.entry(key).or_default().push_back(entry);
        self.total = self.total.saturating_add(1);
        while self.total > policy.max_connections {
            if !self.evict_oldest() {
                break;
            }
        }
    }

    fn prune_expired_with_policy(&mut self, policy: ResponsesWebSocketPoolPolicy) {
        for entries in self.entries.values_mut() {
            let before = entries.len();
            entries.retain(|entry| {
                entry.last_used_at.elapsed() < policy.idle_timeout
                    && entry.created_at.elapsed() < policy.max_age
            });
            self.total = self
                .total
                .saturating_sub(before.saturating_sub(entries.len()));
        }
        self.entries.retain(|_, entries| !entries.is_empty());
    }

    fn evict_oldest(&mut self) -> bool {
        let oldest_key = self
            .entries
            .iter()
            .filter_map(|(key, entries)| entries.front().map(|entry| (key, entry.last_used_at)))
            .min_by_key(|(_, last_used_at)| *last_used_at)
            .map(|(key, _)| key.clone());
        let Some(key) = oldest_key else {
            return false;
        };
        if let Some(entries) = self.entries.get_mut(&key) {
            if entries.pop_front().is_some() {
                self.total = self.total.saturating_sub(1);
            }
            if entries.is_empty() {
                self.entries.remove(&key);
            }
        }
        true
    }
}

fn responses_websocket_pool() -> &'static StdMutex<ResponsesWebSocketPool> {
    static POOL: OnceLock<StdMutex<ResponsesWebSocketPool>> = OnceLock::new();
    POOL.get_or_init(|| StdMutex::new(ResponsesWebSocketPool::default()))
}

struct ResponsesDownstreamConnectionGuard;

fn responses_downstream_connections() -> &'static AtomicUsize {
    static ACTIVE: AtomicUsize = AtomicUsize::new(0);
    &ACTIVE
}

fn acquire_responses_downstream_connection(
) -> Result<ResponsesDownstreamConnectionGuard, ProxyError> {
    let limit = std::env::var("CC_SWITCH_CODEX_WS_MAX_CONNECTIONS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_CODEX_WEBSOCKET_MAX_CONNECTIONS)
        .clamp(1, 4096);
    let active = responses_downstream_connections();
    let mut current = active.load(Ordering::Acquire);
    loop {
        if current >= limit {
            return Err(ProxyError::rate_limited(
                "responses websocket connection limit has been reached",
                1,
            ));
        }
        match active.compare_exchange_weak(
            current,
            current.saturating_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(ResponsesDownstreamConnectionGuard),
            Err(actual) => current = actual,
        }
    }
}

impl Drop for ResponsesDownstreamConnectionGuard {
    fn drop(&mut self) {
        responses_downstream_connections().fetch_sub(1, Ordering::AcqRel);
    }
}

fn codex_websocket_cache_max_connections() -> usize {
    std::env::var("CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_CODEX_WEBSOCKET_CACHE_MAX_CONNECTIONS)
        .min(512)
}

fn codex_websocket_cache_idle_timeout() -> Duration {
    Duration::from_millis(
        std::env::var("CC_SWITCH_CODEX_WS_CACHE_IDLE_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_CODEX_WEBSOCKET_CACHE_IDLE_MS)
            .clamp(1_000, 30 * 60 * 1000),
    )
}

fn codex_websocket_cache_max_age() -> Duration {
    Duration::from_millis(
        std::env::var("CC_SWITCH_CODEX_WS_CACHE_MAX_AGE_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_CODEX_WEBSOCKET_CACHE_MAX_AGE_MS)
            .clamp(60_000, 6 * 60 * 60 * 1000),
    )
}

#[derive(Debug, Clone)]
struct ForwardAttemptContext {
    attempt: u32,
    started_at_ms: u128,
    body_retry_stage: Option<ClaudeBodyRetryStage>,
    execution: Option<ProviderExecution>,
    auth_refresh_attempted: bool,
    codex_overflow_compact_attempted: bool,
    codex_body_override: Option<Bytes>,
    excluded_provider_ids: BTreeSet<String>,
    grok_session_id: Option<String>,
}

impl Default for ForwardAttemptContext {
    fn default() -> Self {
        Self {
            attempt: 0,
            started_at_ms: current_time_ms(),
            body_retry_stage: None,
            execution: None,
            auth_refresh_attempted: false,
            codex_overflow_compact_attempted: false,
            codex_body_override: None,
            excluded_provider_ids: BTreeSet::new(),
            grok_session_id: None,
        }
    }
}

impl ForwardAttemptContext {
    fn retry_allowed(&self) -> bool {
        self.attempt < MAX_FORWARD_RETRY_ATTEMPTS
            && current_time_ms().saturating_sub(self.started_at_ms) < MAX_FORWARD_RETRY_ELAPSED_MS
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
        next.codex_body_override = None;
        next
    }

    fn after_codex_overflow_compact(&self, execution: &ProviderExecution, body: Bytes) -> Self {
        let mut next = self.next(execution, self.body_retry_stage);
        next.codex_overflow_compact_attempted = true;
        next.codex_body_override = Some(body);
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

struct CodexAuthContext {
    execution: ProviderExecution,
    stored: StoredProvider,
    http_client: reqwest::Client,
    headers: Vec<(String, String)>,
    url: String,
}

async fn select_codex_oauth_surface_execution(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<ProviderExecution, ProxyError> {
    let request_context = request_context_from_headers(headers);
    let accounts = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let execution = if let Some(share_id) = request_context.share_id.as_deref() {
        let shares = state.shares.read().await.clone();
        select_share_execution(&providers, &shares, &accounts, AppKind::Codex, share_id)?.0
    } else {
        let ui_settings = state.ui_settings.read().await.for_frontend();
        let configured_provider_id =
            current_provider::resolve_current_provider_id(&providers, &ui_settings, AppKind::Codex);
        super::router::select_provider(
            &providers,
            &accounts,
            AppKind::Codex,
            headers,
            configured_provider_id.as_deref(),
        )?
        .execution
    };
    if !execution.driver_is("oauth.openai_codex") {
        return Err(ProxyError::bad_request(
            "this endpoint requires the current provider to be codex_oauth",
        ));
    }
    Ok(execution)
}

async fn materialize_codex_auth_context(
    state: &ServerState,
    execution: &ProviderExecution,
    client_headers: &HeaderMap,
    endpoint: String,
) -> Result<CodexAuthContext, ProxyError> {
    ensure_managed_credential_persistence_available(state, execution)?;
    refresh_execution_managed_account_if_needed(state, execution).await?;
    ensure_managed_credential_persistence_available(state, execution)?;
    let stored = execution.runtime_stored_view();
    let accounts = state.accounts_snapshot().await;
    super::router::ensure_codex_oauth_active_account(&stored, &accounts)?;
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let mut headers = adapter.build_headers(AppKind::Codex, &stored, &accounts)?;
    append_codex_client_request_headers(&mut headers, client_headers, false);
    crate::codex_identity::finalize_headers(&mut headers);
    let mut headers = owned_headers(headers);
    let mut url = endpoint;
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut headers, &mut url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut headers, &stored, &accounts)?;
    execution.finalize_outbound_identity(&mut headers)?;
    Ok(CodexAuthContext {
        execution: execution.clone(),
        http_client: forward_http_client(state, &stored).await?,
        stored,
        headers,
        url,
    })
}

async fn force_refresh_codex_auth_context(
    state: &ServerState,
    execution: &ProviderExecution,
) -> Result<(), ProxyError> {
    let (provider_type, account_id) = execution.managed_account_target().ok_or_else(|| {
        ProxyError::bad_request("codex_oauth provider is missing a managed account binding")
    })?;
    state
        .refresh_managed_account_now(provider_type, Some(account_id))
        .await
        .map_err(managed_account_refresh_error_to_proxy_error)?;
    ensure_managed_credential_persistence_available(state, execution)
}

pub async fn forward_codex_models_manifest(
    state: ServerState,
    headers: HeaderMap,
    client_version: Option<String>,
) -> Result<Response, ProxyError> {
    let request_context = request_context_from_headers(&headers);
    let _share_invocation_guard = if let Some(share_id) = request_context.share_id.as_deref() {
        Some(
            validate_and_acquire_share_invocation(
                &state,
                share_id,
                request_context.user_email.as_deref(),
            )
            .await?
            .1,
        )
    } else {
        None
    };
    let execution = select_codex_oauth_surface_execution(&state, &headers).await?;
    let accounts = state.accounts_snapshot().await;
    let snapshot = state.account_in_flight.snapshot();
    let _account_in_flight_guard =
        acquire_account_in_flight(&state, &execution.stored, &accounts, &snapshot)?;
    let endpoint = codex_models_manifest_url(&execution);
    let version = client_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(crate::codex_identity::configured_version);
    let mut auth_refresh_attempted = false;
    loop {
        let context =
            materialize_codex_auth_context(&state, &execution, &headers, endpoint.clone()).await?;
        let mut url = url::Url::parse(&context.url).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Codex models URL: {error}"))
        })?;
        url.query_pairs_mut()
            .append_pair("client_version", &version);
        let mut request = context
            .http_client
            .get(url)
            .header(ACCEPT, "application/json")
            .header("accept-encoding", "identity")
            .header("version", &version)
            .timeout(Duration::from_secs(15));
        if let Some(etag) = optional_header(&headers, "if-none-match") {
            request = request.header("if-none-match", etag);
        }
        for (name, value) in &context.headers {
            request = request.header(name, value);
        }
        let mut upstream = request.send().await.map_err(ProxyError::bad_gateway)?;
        if upstream.status() == StatusCode::UNAUTHORIZED && !auth_refresh_attempted {
            drop(upstream);
            force_refresh_codex_auth_context(&state, &execution).await?;
            auth_refresh_attempted = true;
            continue;
        }
        if upstream.status() == StatusCode::UNAUTHORIZED {
            mark_managed_account_auth_cooldown(
                &state,
                &execution,
                "models_unauthorized_after_refresh",
            )
            .await;
        }
        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let content_encoding = content_encoding_value(&response_headers);
        let decoded = if status == StatusCode::NOT_MODIFIED {
            ResponseDecodeResult {
                body: Bytes::new(),
                preserve_content_encoding: false,
            }
        } else {
            let body = crate::infra::http::read_response_body_limited(
                &mut upstream,
                CODEX_MODELS_MANIFEST_BODY_LIMIT_BYTES,
            )
            .await
            .map_err(ProxyError::bad_gateway)?;
            decode_response_body_for_proxy_with_limit(
                &response_headers,
                body,
                CODEX_MODELS_MANIFEST_BODY_LIMIT_BYTES,
            )?
        };
        if status.is_success() && !decoded.preserve_content_encoding {
            super::codex_models::update_manifest_models(
                &execution.runtime_stored_view(),
                &decoded.body,
            );
        }
        let mut response = Response::new(Body::from(decoded.body));
        *response.status_mut() = status;
        if let Some(etag) = response_headers.get("etag").cloned() {
            response.headers_mut().insert("etag", etag);
        }
        if let Some(content_type) = response_headers.get(CONTENT_TYPE).cloned() {
            response.headers_mut().insert(CONTENT_TYPE, content_type);
        } else {
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        if decoded.preserve_content_encoding {
            if let Some(content_encoding) = content_encoding {
                response
                    .headers_mut()
                    .insert(CONTENT_ENCODING, content_encoding);
            }
        }
        copy_safe_upstream_response_headers(&response_headers, &mut response);
        return Ok(response);
    }
}

pub async fn forward_codex_alpha_search(
    state: ServerState,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = prepare_codex_alpha_search_body(&headers, body)?;
    let request_context = request_context_from_headers(&headers);
    let _share_invocation_guard = if let Some(share_id) = request_context.share_id.as_deref() {
        Some(
            validate_and_acquire_share_invocation(
                &state,
                share_id,
                request_context.user_email.as_deref(),
            )
            .await?
            .1,
        )
    } else {
        None
    };
    let execution = select_codex_oauth_surface_execution(&state, &headers).await?;
    let accounts = state.accounts_snapshot().await;
    let snapshot = state.account_in_flight.snapshot();
    let _account_in_flight_guard =
        acquire_account_in_flight(&state, &execution.stored, &accounts, &snapshot)?;
    let endpoint = codex_alpha_search_url(&execution);
    let mut auth_refresh_attempted = false;
    loop {
        let context =
            materialize_codex_auth_context(&state, &execution, &headers, endpoint.clone()).await?;
        let mut request = context
            .http_client
            .post(&context.url)
            .header(ACCEPT, "application/json")
            .header("accept-encoding", "identity")
            .header(CONTENT_TYPE, "application/json")
            .body(body.clone())
            .timeout(Duration::from_secs(120));
        for (name, value) in &context.headers {
            request = request.header(name, value);
        }
        let mut upstream = request.send().await.map_err(ProxyError::bad_gateway)?;
        if upstream.status() == StatusCode::UNAUTHORIZED && !auth_refresh_attempted {
            drop(upstream);
            force_refresh_codex_auth_context(&state, &execution).await?;
            auth_refresh_attempted = true;
            continue;
        }
        if upstream.status() == StatusCode::UNAUTHORIZED {
            mark_managed_account_auth_cooldown(
                &state,
                &execution,
                "alpha_search_unauthorized_after_refresh",
            )
            .await;
        }
        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let content_encoding = content_encoding_value(&response_headers);
        let response_body = crate::infra::http::read_response_body_limited(
            &mut upstream,
            CODEX_ALPHA_SEARCH_RESPONSE_BODY_LIMIT_BYTES,
        )
        .await
        .map_err(ProxyError::bad_gateway)?;
        let decoded = decode_response_body_for_proxy_with_limit(
            &response_headers,
            response_body,
            CODEX_ALPHA_SEARCH_RESPONSE_BODY_LIMIT_BYTES,
        )?;
        maybe_mark_upstream_rate_limited(
            &state,
            &context.execution,
            status,
            &response_headers,
            &decoded.body,
        )
        .await;
        record_provider_outcome(
            &state,
            &context.stored,
            provider_outcome_from_status(status.as_u16()),
        )
        .await;
        record_share_invocation_result(
            &state,
            request_context.share_id.as_deref(),
            request_context.user_email.as_deref(),
            TokenUsage::default(),
        )
        .await;
        let mut response = Response::new(Body::from(decoded.body));
        *response.status_mut() = status;
        if let Some(content_type) = response_headers.get(CONTENT_TYPE).cloned() {
            response.headers_mut().insert(CONTENT_TYPE, content_type);
        }
        if decoded.preserve_content_encoding {
            if let Some(content_encoding) = content_encoding {
                response
                    .headers_mut()
                    .insert(CONTENT_ENCODING, content_encoding);
            }
        }
        copy_safe_upstream_response_headers(&response_headers, &mut response);
        return Ok(response);
    }
}

fn prepare_codex_alpha_search_body(headers: &HeaderMap, body: Bytes) -> Result<Bytes, ProxyError> {
    let body =
        decode_request_body_for_proxy_with_limit(headers, body, PROXY_REQUEST_BODY_LIMIT_BYTES)?;
    normalize_codex_alpha_search_body(&body)
}

fn normalize_codex_alpha_search_body(body: &[u8]) -> Result<Bytes, ProxyError> {
    if body.len() > PROXY_REQUEST_BODY_LIMIT_BYTES {
        return Err(ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "Codex alpha search request exceeds the 2 MiB limit".to_string(),
        });
    }
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| ProxyError::bad_request(format!("invalid alpha search JSON: {error}")))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("prompt_cache_key");
    }
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode alpha search JSON: {error}")))?;
    Ok(body)
}

fn codex_models_manifest_url(_execution: &ProviderExecution) -> String {
    #[cfg(test)]
    if let Some(url) = _execution
        .plan
        .driver_options
        .get("testCodexModelsUrl")
        .and_then(Value::as_str)
    {
        return url.to_string();
    }
    "https://chatgpt.com/backend-api/codex/models".to_string()
}

fn codex_alpha_search_url(_execution: &ProviderExecution) -> String {
    #[cfg(test)]
    if let Some(url) = _execution
        .plan
        .driver_options
        .get("testCodexAlphaSearchUrl")
        .and_then(Value::as_str)
    {
        return url.to_string();
    }
    "https://chatgpt.com/backend-api/codex/alpha/search".to_string()
}

fn codex_overflow_compact_eligible(
    route: ProxyRoute,
    execution: &ProviderExecution,
    attempt_context: &ForwardAttemptContext,
) -> bool {
    if !execution.driver_is("oauth.openai_codex")
        || !matches!(
            route,
            ProxyRoute::CodexResponses | ProxyRoute::CodexChatCompletions
        )
        || attempt_context.codex_overflow_compact_attempted
        || !attempt_context.retry_allowed()
    {
        return false;
    }
    #[cfg(test)]
    if let Some(enabled) = execution
        .plan
        .driver_options
        .get("testCodexOverflowAutoCompact")
        .and_then(Value::as_bool)
    {
        return enabled;
    }
    super::overflow_compact::enabled()
}

#[allow(clippy::too_many_arguments)]
async fn next_codex_overflow_attempt(
    state: &ServerState,
    route: ProxyRoute,
    execution: &ProviderExecution,
    stored: &StoredProvider,
    attempt_context: &ForwardAttemptContext,
    adapter_request: &adapters::AdapterRequest,
    http_client: &reqwest::Client,
    url: &str,
    target_headers: &[(String, String)],
    request_context: &UsageLogContext,
) -> Option<ForwardAttemptContext> {
    if !codex_overflow_compact_eligible(route, execution, attempt_context) {
        return None;
    }
    let plan = super::overflow_compact::prepare(&adapter_request.body)?;
    let removed_items = plan.removed_items();
    let retained_items = plan.retained_items();
    let summary = match plan.summary_request_body() {
        Some(summary_body) => {
            summarize_codex_overflow(
                state,
                stored,
                http_client,
                url,
                target_headers,
                summary_body,
                request_context,
            )
            .await
        }
        None => None,
    };
    let compacted = plan.finish(summary.as_deref())?;
    tracing::info!(
        provider_id = %stored.provider.id,
        account_id = ?execution.managed_account_id(),
        removed_items,
        retained_items,
        summarized = summary.is_some(),
        compacted_bytes = compacted.len(),
        "retrying Codex request after context overflow compaction"
    );
    Some(attempt_context.after_codex_overflow_compact(execution, compacted))
}

async fn summarize_codex_overflow(
    state: &ServerState,
    stored: &StoredProvider,
    http_client: &reqwest::Client,
    url: &str,
    target_headers: &[(String, String)],
    summary_body: Bytes,
    request_context: &UsageLogContext,
) -> Option<String> {
    let summary_body = match normalize_codex_oauth_responses_body_bytes(
        &summary_body,
        None,
        CodexImageToolStripPolicy::Always,
    ) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!("failed to normalize Codex overflow summary request: {error}");
            return None;
        }
    };
    let model = serde_json::from_slice::<Value>(&summary_body)
        .ok()
        .and_then(|body| {
            body.get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let started = Instant::now();
    let mut client_headers = HeaderMap::new();
    client_headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    client_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let call = async {
        let mut response = build_upstream_post_request(
            http_client,
            url,
            summary_body,
            &client_headers,
            target_headers,
            CODEX_OVERFLOW_SUMMARY_TIMEOUT,
            true,
        )
        .header("accept-encoding", "identity")
        .send()
        .await
        .map_err(|error| error.to_string())?;
        let status = response.status();
        let response_headers = response.headers().clone();
        let body = crate::infra::http::read_response_body_limited(
            &mut response,
            CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?;
        let body = decode_response_body_for_proxy_with_limit(
            &response_headers,
            body,
            CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
        )
        .map_err(|error| error.to_string())?
        .body;
        Ok::<_, String>((status, body))
    };

    let (status, body, stream_status) =
        match tokio::time::timeout(CODEX_OVERFLOW_SUMMARY_TIMEOUT, call).await {
            Ok(Ok((status, body))) => {
                let stream_status = if status.is_success() {
                    "completed"
                } else {
                    "upstream_error"
                };
                (status.as_u16(), body, stream_status)
            }
            Ok(Err(error)) => {
                tracing::warn!("Codex overflow summary request failed: {error}");
                (
                    StatusCode::BAD_GATEWAY.as_u16(),
                    Bytes::new(),
                    "upstream_error",
                )
            }
            Err(_) => {
                tracing::warn!("Codex overflow summary request timed out");
                (
                    StatusCode::GATEWAY_TIMEOUT.as_u16(),
                    Bytes::new(),
                    "timeout",
                )
            }
        };
    let mut usage = StreamUsageAccumulator::new(adapters::usage_input_semantics_for(
        stored,
        ProxyRoute::CodexResponses,
    ));
    usage.push(&body);
    let usage = usage.finish();
    log_usage(
        state,
        stored,
        status,
        started.elapsed().as_millis(),
        UsageModelMetadata {
            model: model.clone(),
            requested_model: model.clone(),
            actual_model: model,
            actual_model_source: Some("codex_overflow_compact_summary".to_string()),
        },
        usage,
        UsageLogContext {
            share_id: request_context.share_id.clone(),
            share_name: request_context.share_name.clone(),
            user_email: request_context.user_email.clone(),
            session_id: request_context.session_id.clone(),
            data_source: Some(super::overflow_compact::SUMMARY_DATA_SOURCE.to_string()),
            user_country: request_context.user_country.clone(),
            user_country_iso3: request_context.user_country_iso3.clone(),
            is_streaming: true,
            stream_status: Some(stream_status.to_string()),
            ..UsageLogContext::default()
        },
    )
    .await;
    record_share_supplemental_usage(
        state,
        request_context.share_id.as_deref(),
        request_context.user_email.as_deref(),
        usage,
    )
    .await;

    if status != StatusCode::OK.as_u16() {
        return None;
    }
    let output = super::overflow_compact::extract_summary_output(&body);
    if output.is_none() {
        tracing::warn!("Codex overflow summary response contained no text output");
    }
    output
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
        let body = decode_request_body_for_proxy_with_limit(
            &headers,
            raw_body_for_retry.clone(),
            PROXY_REQUEST_BODY_LIMIT_BYTES,
        )?;
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
        let (execution, account_in_flight_guard) = if let Some(execution) =
            attempt_context.execution.clone()
        {
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
                select_and_acquire_account_in_flight(&state, &accounts_for_selection, |snapshot| {
                    if matches!(
                        route,
                        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
                    ) {
                        let selection = select_exact_provider_with_account_inflight(
                            &providers,
                            &accounts_for_selection,
                            app,
                            &headers,
                            configured_provider_id.as_deref(),
                            snapshot,
                        )?;
                        if route == ProxyRoute::ClaudeCountTokens
                            && !provider_supports_claude_count_tokens(&selection.execution.stored)
                        {
                            return Err(ProxyError::bad_request(
                                "Claude count_tokens requires a native Anthropic provider",
                            ));
                        }
                        Ok(selection)
                    } else {
                        select_provider_with_account_inflight(
                            &providers,
                            &accounts_for_selection,
                            app,
                            &headers,
                            configured_provider_id.as_deref(),
                            snapshot,
                            request_context.session_id.as_deref(),
                        )
                    }
                    .map(|selection| selection.execution)
                })?
            }
        };
        let stored = execution.runtime_stored_view();
        ensure_managed_credential_persistence_available(&state, &execution)?;
        super::router::ensure_codex_oauth_active_account(&stored, &accounts_for_selection)?;
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
                account_in_flight_guard,
                share_invocation_guard,
                timeouts: cursor::h2_client::CursorH2Timeouts {
                    request: execution.request_timeout(),
                    first_frame: execution.stream_first_byte_timeout(),
                    inter_frame: execution.stream_idle_timeout(),
                },
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
                account_in_flight_guard,
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
                account_in_flight_guard,
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
        let codex_responses_lite = execution.driver_is("oauth.openai_codex")
            && route == ProxyRoute::CodexResponses
            && codex_responses_lite_requested(&headers, &adapter_request.body);
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
                if codex_responses_lite {
                    adapter_request.body = normalize_codex_responses_lite_body_bytes(
                        &adapter_request.body,
                        true,
                        true,
                    )?;
                }
            }
            adapter_request.body =
                super::remote_image::inline_codex_remote_images(&adapter_request.body).await?;
        }
        if execution.driver_is("oauth.openai_codex") {
            if let Some(body) = attempt_context.codex_body_override.clone() {
                adapter_request.body = body;
            }
        }
        execution.enforce_model_policy(&mut adapter_request)?;
        let grok_contract = if execution.driver_is("oauth.grok_responses") {
            let cli_profile = grok_cli_profile(&execution);
            let preserved_session_id = attempt_context.grok_session_id.as_deref();
            let tenant_scope = preserved_session_id
                .is_none()
                .then(|| grok_tenant_scope(&request_context, &stored))
                .flatten();
            let contract = super::grok::apply_forward_contract(
                &mut adapter_request.body,
                &headers,
                route,
                request_context.session_id.as_deref(),
                preserved_session_id,
                tenant_scope.as_deref(),
                cli_profile,
            )?;
            attempt_context.grok_session_id = contract.session_id.clone();
            request_context.session_id = contract.session_id.clone();
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
                if !execution.driver_is("oauth.grok_responses") {
                    execution.enforce_model_policy(&mut adapter_request)?;
                }
                execution.finalize_request(&mut adapter_request)?;
                let mut url = execution.resolve_endpoint(route, gemini_path, &adapter_request)?;
                if execution.driver_is("oauth.grok_responses") {
                    url = super::grok::chat_upstream_url(&url, grok_cli_profile(&execution));
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
                    append_codex_client_request_headers(
                        &mut target_headers,
                        &headers,
                        codex_responses_lite,
                    );
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
                execution.finalize_outbound_identity(&mut target_headers)?;
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
                    if error.is_connect() {
                        "connect_error"
                    } else {
                        "send_error"
                    },
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
        if status == StatusCode::UNAUTHORIZED {
            if let Some(next_attempt) = next_unauthorized_attempt(
                &state,
                route,
                &headers,
                &request_context,
                &attempt_context,
                &execution,
                &stored,
            )
            .await?
            {
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
            && !request_is_provider_pinned(&headers, &request_context)
        {
            if let Some(next_attempt) =
                next_provider_failover(&state, route, &attempt_context, &execution, "http_529")
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
        let mut buffered_upstream_body = None;

        if codex_image_tool_strip_policy(&stored) == CodexImageToolStripPolicy::OnError
            && execution.driver_is("oauth.openai_codex")
            && matches!(
                route,
                ProxyRoute::CodexResponses | ProxyRoute::CodexChatCompletions
            )
            && !status.is_success()
            && status != StatusCode::TOO_MANY_REQUESTS
        {
            let original_bytes = crate::infra::http::read_response_body_limited(
                &mut upstream,
                CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
            )
            .await
            .map_err(ProxyError::bad_gateway)?;
            let original_decoded = decode_response_body_for_proxy_with_limit(
                &response_headers,
                original_bytes,
                CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
            )?;
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
                            if status == StatusCode::UNAUTHORIZED {
                                if let Some(next_attempt) = next_unauthorized_attempt(
                                    &state,
                                    route,
                                    &headers,
                                    &request_context,
                                    &attempt_context,
                                    &execution,
                                    &stored,
                                )
                                .await?
                                {
                                    attempt_context = next_attempt;
                                    drop(upstream);
                                    drop(account_in_flight_guard);
                                    drop(share_invocation_guard);
                                    continue 'attempt;
                                }
                            }
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
                buffered_upstream_body = Some(original_decoded);
            }
        }

        if status == StatusCode::BAD_REQUEST
            && codex_overflow_compact_eligible(route, &execution, &attempt_context)
        {
            let decoded = match buffered_upstream_body.take() {
                Some(decoded) => decoded,
                None => {
                    let bytes = crate::infra::http::read_response_body_limited(
                        &mut upstream,
                        CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
                    )
                    .await
                    .map_err(ProxyError::bad_gateway)?;
                    decode_response_body_for_proxy_with_limit(
                        &response_headers,
                        bytes,
                        CODEX_OVERFLOW_SUMMARY_BODY_LIMIT_BYTES,
                    )?
                }
            };
            if super::overflow_compact::is_context_length_exceeded_body(&decoded.body) {
                if let Some(next_attempt) = next_codex_overflow_attempt(
                    &state,
                    route,
                    &execution,
                    &stored,
                    &attempt_context,
                    &adapter_request,
                    &http_client,
                    &url,
                    &target_headers,
                    &request_context,
                )
                .await
                {
                    attempt_context = next_attempt;
                    drop(account_in_flight_guard);
                    drop(share_invocation_guard);
                    continue 'attempt;
                }
            }
            buffered_upstream_body = Some(decoded);
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            let bytes = match crate::infra::http::read_response_body_limited(
                &mut upstream,
                PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
            )
            .await
            {
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
            let decoded = decode_response_body_for_proxy_with_limit(
                &response_headers,
                bytes,
                PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
            )?;
            maybe_mark_upstream_rate_limited(
                &state,
                &execution,
                status,
                &response_headers,
                &decoded.body,
            )
            .await;
            if !request_is_provider_pinned(&headers, &request_context) {
                if let Some(next_attempt) =
                    next_provider_failover(&state, route, &attempt_context, &execution, "http_429")
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

        if adapter_request.stream_requested && buffered_upstream_body.is_none() {
            let timeouts = StreamTimeoutConfig {
                first_byte: execution.stream_first_byte_timeout(),
                idle: execution.stream_idle_timeout(),
            };
            let mut inner = upstream.bytes_stream().boxed();
            let mut pending_chunk = None;
            let mut sse_error_detector = claude_sse_error_detector_for(&stored, route);
            let mut sse_error_outcome_recorded = false;
            let upstream_format =
                adapters::upstream_format_for_route(&stored, Some(route), &adapter_request.body)
                    .unwrap_or_else(|| adapters::downstream_format_for_route(route));
            let inspect_responses_semantics = status.is_success()
                && (response_semantics::semantic_guard_enabled()
                    || codex_overflow_compact_eligible(route, &execution, &attempt_context))
                && upstream_format == UpstreamFormat::OpenAiResponses;
            let inspect_anthropic_semantics = status.is_success()
                && response_semantics::semantic_guard_enabled()
                && route == ProxyRoute::ClaudeMessages
                && upstream_format == UpstreamFormat::AnthropicMessages;
            let mut responses_semantics =
                inspect_responses_semantics.then(ResponsesSseInspector::default);
            let mut anthropic_semantics =
                inspect_anthropic_semantics.then(AnthropicSseInspector::default);
            if inspect_anthropic_semantics {
                sse_error_detector = None;
            }
            let mut semantic_provider_outcome_recorded = false;
            let mut pending_chunk_already_inspected = false;
            let mut pending_chunk_saw_business_output = false;
            let mut pending_chunk_committed_output = false;
            if sse_error_detector.is_some()
                || inspect_responses_semantics
                || inspect_anthropic_semantics
            {
                let mut prelude = Vec::new();
                let mut detected_error = None;
                let mut detected_anthropic_error = None;
                let mut semantic_decision = None;
                let mut semantic_protocol_error = None;
                let mut anthropic_event_ready = false;
                let semantic_commit_deadline = inspect_responses_semantics
                    .then_some(timeouts.first_byte)
                    .flatten()
                    .or_else(|| {
                        inspect_anthropic_semantics
                            .then_some(timeouts.first_byte)
                            .flatten()
                    })
                    .map(|timeout| tokio::time::Instant::now() + timeout);
                let first_chunk = loop {
                    let next = if let Some(deadline) = semantic_commit_deadline {
                        let timeout = timeouts
                            .first_byte
                            .expect("a semantic commit deadline requires a first-byte timeout");
                        match tokio::time::timeout_at(deadline, inner.try_next()).await {
                            Ok(result) => result.map_err(StreamReadError::Upstream),
                            Err(_) => Err(StreamReadError::Timeout {
                                kind: StreamTimeoutKind::FirstByte,
                                timeout,
                            }),
                        }
                    } else {
                        let (timeout, kind) = if prelude.is_empty() {
                            (timeouts.first_byte, StreamTimeoutKind::FirstByte)
                        } else {
                            (timeouts.idle, StreamTimeoutKind::Idle)
                        };
                        match timeout {
                            Some(timeout) => {
                                match tokio::time::timeout(timeout, inner.try_next()).await {
                                    Ok(result) => result.map_err(StreamReadError::Upstream),
                                    Err(_) => Err(StreamReadError::Timeout { kind, timeout }),
                                }
                            }
                            None => inner.try_next().await.map_err(StreamReadError::Upstream),
                        }
                    };
                    match next {
                        Ok(Some(chunk)) => {
                            prelude.extend_from_slice(&chunk);
                            detected_error = sse_error_detector
                                .as_mut()
                                .and_then(|detector| detector.push(&chunk));
                            if let Some(inspector) = responses_semantics.as_mut() {
                                match inspector.push(&chunk) {
                                    Ok(observations) => {
                                        for observation in &observations {
                                            crate::metrics::record_proxy_semantic_guard(
                                                "http_stream_prime",
                                                observation.metric_kind(),
                                            );
                                        }
                                        semantic_decision =
                                            semantic_prelude_decision(&observations);
                                        pending_chunk_saw_business_output |= observations
                                            .iter()
                                            .any(SemanticObservation::counts_as_business_output);
                                        pending_chunk_committed_output |= observations
                                            .iter()
                                            .any(SemanticObservation::commits_downstream);
                                    }
                                    Err(error) => {
                                        crate::metrics::record_proxy_semantic_guard(
                                            "http_stream_prime",
                                            "protocol_error",
                                        );
                                        semantic_protocol_error = Some(error.to_string());
                                    }
                                }
                            }
                            if let Some(inspector) = anthropic_semantics.as_mut() {
                                match inspector.push(&chunk) {
                                    Ok(observations) => {
                                        for observation in &observations {
                                            crate::metrics::record_proxy_semantic_guard(
                                                "anthropic_stream_prime",
                                                observation.metric_kind(),
                                            );
                                            if let AnthropicObservation::Error(error) = observation
                                            {
                                                detected_anthropic_error = Some(error.clone());
                                            }
                                        }
                                        anthropic_event_ready |= !observations.is_empty();
                                        pending_chunk_saw_business_output |= observations
                                            .iter()
                                            .any(AnthropicObservation::counts_as_business_output);
                                        pending_chunk_committed_output |= observations
                                            .iter()
                                            .any(AnthropicObservation::commits_downstream);
                                    }
                                    Err(error) => {
                                        crate::metrics::record_proxy_semantic_guard(
                                            "anthropic_stream_prime",
                                            "protocol_error",
                                        );
                                        semantic_protocol_error = Some(error.to_string());
                                    }
                                }
                            }
                            let prelude_limit =
                                if inspect_responses_semantics || inspect_anthropic_semantics {
                                    1024 * 1024
                                } else {
                                    64 * 1024
                                };
                            if prelude.len() >= prelude_limit
                                && semantic_decision.is_none()
                                && semantic_protocol_error.is_none()
                                && (inspect_responses_semantics || inspect_anthropic_semantics)
                            {
                                semantic_protocol_error = Some(
                                    "Upstream semantic prelude exceeded 1 MiB before a complete event"
                                        .to_string(),
                                );
                            }
                            let ready = detected_error.is_some()
                                || semantic_decision.is_some()
                                || anthropic_event_ready
                                || semantic_protocol_error.is_some()
                                || (!inspect_responses_semantics
                                    && (sse_error_detector
                                        .as_ref()
                                        .is_some_and(ClaudeSseErrorDetector::prelude_ready)
                                        || prelude.len() >= prelude_limit));
                            if ready {
                                break Ok(Some(Bytes::from(prelude)));
                            }
                        }
                        Ok(None) => {
                            if semantic_decision.is_none() {
                                if let Some(inspector) = responses_semantics.as_mut() {
                                    match inspector.finish() {
                                        Ok(observations) => {
                                            for observation in &observations {
                                                crate::metrics::record_proxy_semantic_guard(
                                                    "http_stream_prime",
                                                    observation.metric_kind(),
                                                );
                                            }
                                            semantic_decision =
                                                semantic_prelude_decision(&observations);
                                        }
                                        Err(error) => {
                                            crate::metrics::record_proxy_semantic_guard(
                                                "http_stream_prime",
                                                "protocol_error",
                                            );
                                            semantic_protocol_error = Some(error.to_string());
                                        }
                                    }
                                }
                            }
                            if let Some(inspector) = anthropic_semantics.as_mut() {
                                if let Err(error) = inspector.finish() {
                                    crate::metrics::record_proxy_semantic_guard(
                                        "anthropic_stream_prime",
                                        "protocol_error",
                                    );
                                    semantic_protocol_error = Some(error.to_string());
                                }
                            }
                            break Ok((!prelude.is_empty()).then(|| Bytes::from(prelude)));
                        }
                        Err(error) => break Err(error),
                    }
                };
                match first_chunk {
                    Ok(Some(chunk)) => {
                        let sse_error = detected_anthropic_error
                            .map(|error| ClaudeSseError {
                                error_type: error.error_type,
                                message: error.message,
                            })
                            .or(detected_error);
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
                        if let Some(error) = semantic_protocol_error {
                            record_provider_outcome(
                                &state,
                                &stored,
                                ProviderOutcome::Failure { status_code: 502 },
                            )
                            .await;
                            if !request_is_provider_pinned(&headers, &request_context) {
                                if let Some(next_attempt) = next_provider_failover(
                                    &state,
                                    route,
                                    &attempt_context,
                                    &execution,
                                    "responses_stream_protocol_error",
                                )
                                .await
                                {
                                    attempt_context = next_attempt;
                                    drop(account_in_flight_guard);
                                    drop(share_invocation_guard);
                                    continue 'attempt;
                                }
                            }
                            return Err(ProxyError::bad_gateway(error));
                        }
                        if let Some(SemanticObservation::Failure(failure)) = &semantic_decision {
                            if !pending_chunk_saw_business_output
                                && super::overflow_compact::is_context_length_failure(
                                    &failure.code,
                                    &failure.message,
                                )
                            {
                                if let Some(next_attempt) = next_codex_overflow_attempt(
                                    &state,
                                    route,
                                    &execution,
                                    &stored,
                                    &attempt_context,
                                    &adapter_request,
                                    &http_client,
                                    &url,
                                    &target_headers,
                                    &request_context,
                                )
                                .await
                                {
                                    attempt_context = next_attempt;
                                    drop(inner);
                                    drop(account_in_flight_guard);
                                    drop(share_invocation_guard);
                                    continue 'attempt;
                                }
                            }
                            if failure.origin == FailureOrigin::Provider {
                                record_provider_outcome(
                                    &state,
                                    &stored,
                                    ProviderOutcome::Failure { status_code: 502 },
                                )
                                .await;
                                semantic_provider_outcome_recorded = true;
                                if !request_is_provider_pinned(&headers, &request_context) {
                                    if let Some(next_attempt) = next_provider_failover(
                                        &state,
                                        route,
                                        &attempt_context,
                                        &execution,
                                        "responses_stream_semantic_failure",
                                    )
                                    .await
                                    {
                                        attempt_context = next_attempt;
                                        drop(account_in_flight_guard);
                                        drop(share_invocation_guard);
                                        continue 'attempt;
                                    }
                                }
                            }
                        }
                        pending_chunk = Some(chunk);
                        pending_chunk_already_inspected = true;
                        if !inspect_responses_semantics && !inspect_anthropic_semantics {
                            pending_chunk_saw_business_output = true;
                            pending_chunk_committed_output = true;
                        }
                    }
                    Ok(None) => {
                        if let Some(error) = semantic_protocol_error {
                            record_provider_outcome(
                                &state,
                                &stored,
                                ProviderOutcome::Failure { status_code: 502 },
                            )
                            .await;
                            if !request_is_provider_pinned(&headers, &request_context) {
                                if let Some(next_attempt) = next_provider_failover(
                                    &state,
                                    route,
                                    &attempt_context,
                                    &execution,
                                    "responses_stream_protocol_eof",
                                )
                                .await
                                {
                                    attempt_context = next_attempt;
                                    drop(account_in_flight_guard);
                                    drop(share_invocation_guard);
                                    continue 'attempt;
                                }
                            }
                            return Err(ProxyError::bad_gateway(error));
                        }
                    }
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
                    adapter_request.responses_tool_context.clone(),
                ),
                claude_tool_name_stream_patcher:
                    super::claude_oauth::ClaudeToolNameStreamPatcher::new(
                        adapter_request.claude_tool_name_map.clone(),
                    ),
                timeouts,
                pending_chunk,
                pending_chunk_already_inspected,
                pending_chunk_saw_business_output,
                pending_chunk_committed_output,
                sse_error_detector,
                sse_error_outcome_recorded,
                responses_semantics,
                anthropic_semantics,
                semantic_provider_outcome_recorded,
                terminal_frame_sent: false,
                interrupted_update_armed,
                _account_in_flight_guard: account_in_flight_guard,
                _share_invocation_guard: share_invocation_guard,
            };
            let stream = stream::try_unfold(stream_state, |mut stream_state| async move {
                if stream_state.terminal_frame_sent {
                    return Ok(None);
                }

                let semantic_terminal_seen = stream_state
                    .responses_semantics
                    .as_ref()
                    .and_then(ResponsesSseInspector::terminal)
                    .is_some()
                    || stream_state
                        .anthropic_semantics
                        .as_ref()
                        .and_then(AnthropicSseInspector::terminal)
                        .is_some();
                let mut chunk_already_inspected = false;
                let next_chunk = if let Some(chunk) = stream_state.pending_chunk.take() {
                    chunk_already_inspected = stream_state.pending_chunk_already_inspected;
                    stream_state.pending_chunk_already_inspected = false;
                    Ok(Some(chunk))
                } else if semantic_terminal_seen {
                    Ok(None)
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
                        let chunk = stream_state.codex_completed_output_patcher.push(chunk);
                        let chunk = stream_state.codex_pending_function_call_patcher.push(chunk);
                        stream_state.usage.push(&chunk);
                        let (mut saw_business_output, mut committed_output) =
                            if chunk_already_inspected {
                                let saw_business = stream_state.pending_chunk_saw_business_output;
                                let committed = stream_state.pending_chunk_committed_output;
                                stream_state.pending_chunk_saw_business_output = false;
                                stream_state.pending_chunk_committed_output = false;
                                (saw_business, committed)
                            } else if let Some(inspector) =
                                stream_state.responses_semantics.as_mut()
                            {
                                let observations = match inspector.push(&chunk) {
                                    Ok(observations) => observations,
                                    Err(error) => {
                                        crate::metrics::record_proxy_semantic_guard(
                                            "http_stream",
                                            "protocol_error",
                                        );
                                        return stream_state
                                            .terminate_transform_error(ProxyError::bad_gateway(
                                                error,
                                            ))
                                            .await;
                                    }
                                };
                                for observation in &observations {
                                    crate::metrics::record_proxy_semantic_guard(
                                        "http_stream",
                                        observation.metric_kind(),
                                    );
                                }
                                (
                                    observations
                                        .iter()
                                        .any(SemanticObservation::counts_as_business_output),
                                    observations
                                        .iter()
                                        .any(SemanticObservation::commits_downstream),
                                )
                            } else if stream_state.anthropic_semantics.is_some() {
                                (false, false)
                            } else {
                                (!chunk.is_empty(), !chunk.is_empty())
                            };
                        if !chunk_already_inspected {
                            if let Some(inspector) = stream_state.anthropic_semantics.as_mut() {
                                let observations = match inspector.push(&chunk) {
                                    Ok(observations) => observations,
                                    Err(error) => {
                                        crate::metrics::record_proxy_semantic_guard(
                                            "anthropic_stream",
                                            "protocol_error",
                                        );
                                        return stream_state
                                            .terminate_transform_error(ProxyError::bad_gateway(
                                                error,
                                            ))
                                            .await;
                                    }
                                };
                                for observation in &observations {
                                    crate::metrics::record_proxy_semantic_guard(
                                        "anthropic_stream",
                                        observation.metric_kind(),
                                    );
                                    if let AnthropicObservation::Error(error) = observation {
                                        if !stream_state.sse_error_outcome_recorded {
                                            if let Some(outcome) =
                                                claude_sse_error_outcome(&error.error_type)
                                            {
                                                record_provider_outcome(
                                                    &stream_state.state,
                                                    &stream_state.stored,
                                                    outcome,
                                                )
                                                .await;
                                                stream_state.sse_error_outcome_recorded = true;
                                            }
                                        }
                                    }
                                }
                                saw_business_output |= observations
                                    .iter()
                                    .any(AnthropicObservation::counts_as_business_output);
                                committed_output |= observations
                                    .iter()
                                    .any(AnthropicObservation::commits_downstream);
                            }
                        }
                        stream_state.received_any_chunk |= committed_output;
                        if !chunk_already_inspected && !stream_state.sse_error_outcome_recorded {
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
                        if stream_state.first_token_ms.is_none() && saw_business_output {
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
                            .claude_tool_name_stream_patcher
                            .push(transformed);
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
                            let (saw_business_output, committed_output) = if let Some(inspector) =
                                stream_state.responses_semantics.as_mut()
                            {
                                let observations = match inspector.push(&chunk) {
                                    Ok(observations) => observations,
                                    Err(error) => {
                                        crate::metrics::record_proxy_semantic_guard(
                                            "http_stream",
                                            "protocol_error",
                                        );
                                        return stream_state
                                            .terminate_transform_error(ProxyError::bad_gateway(
                                                error,
                                            ))
                                            .await;
                                    }
                                };
                                for observation in &observations {
                                    crate::metrics::record_proxy_semantic_guard(
                                        "http_stream",
                                        observation.metric_kind(),
                                    );
                                }
                                (
                                    observations
                                        .iter()
                                        .any(SemanticObservation::counts_as_business_output),
                                    observations
                                        .iter()
                                        .any(SemanticObservation::commits_downstream),
                                )
                            } else {
                                (true, true)
                            };
                            stream_state.received_any_chunk |= committed_output;
                            if stream_state.first_token_ms.is_none() && saw_business_output {
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
                                .claude_tool_name_stream_patcher
                                .push(transformed);
                            let transformed = stream_state
                                .codex_custom_tool_stream_patcher
                                .push(transformed);
                            return Ok(Some((transformed, stream_state)));
                        }
                        if let Some(inspector) = stream_state.responses_semantics.as_mut() {
                            let observations = match inspector.finish() {
                                Ok(observations) => observations,
                                Err(error) => {
                                    crate::metrics::record_proxy_semantic_guard(
                                        "http_stream",
                                        "protocol_error",
                                    );
                                    return stream_state
                                        .terminate_transform_error(ProxyError::bad_gateway(error))
                                        .await;
                                }
                            };
                            for observation in &observations {
                                crate::metrics::record_proxy_semantic_guard(
                                    "http_stream",
                                    observation.metric_kind(),
                                );
                            }
                        }
                        if let Some(inspector) = stream_state.anthropic_semantics.as_mut() {
                            if let Err(error) = inspector.finish() {
                                crate::metrics::record_proxy_semantic_guard(
                                    "anthropic_stream",
                                    "protocol_error",
                                );
                                return stream_state
                                    .terminate_transform_error(ProxyError::bad_gateway(error))
                                    .await;
                            }
                        }
                        let transform_tail = match stream_state.stream_transform.finish() {
                            Ok(tail) => tail,
                            Err(error) => {
                                return stream_state.terminate_transform_error(error).await
                            }
                        };
                        let claude_tail = stream_state
                            .claude_tool_name_stream_patcher
                            .push(transform_tail);
                        let claude_tail = join_bytes(
                            claude_tail,
                            stream_state.claude_tool_name_stream_patcher.finish(),
                        );
                        let transformed_tail = stream_state
                            .codex_custom_tool_stream_patcher
                            .push(claude_tail);
                        let custom_tail = join_bytes(
                            transformed_tail,
                            stream_state.codex_custom_tool_stream_patcher.finish(),
                        );
                        if !custom_tail.is_empty() {
                            return Ok(Some((custom_tail, stream_state)));
                        }
                        let semantic_terminal = stream_state
                            .responses_semantics
                            .as_ref()
                            .and_then(ResponsesSseInspector::terminal)
                            .cloned();
                        let anthropic_terminal = stream_state
                            .anthropic_semantics
                            .as_ref()
                            .and_then(AnthropicSseInspector::terminal)
                            .cloned();
                        let stream_status = anthropic_terminal
                            .as_ref()
                            .map(AnthropicTerminal::stream_status)
                            .or_else(|| {
                                semantic_terminal
                                    .as_ref()
                                    .map(SemanticTerminal::stream_status)
                            })
                            .unwrap_or("completed");
                        let usage = std::mem::take(&mut stream_state.usage).finish();
                        update_stream_usage(
                            &stream_state.state,
                            &stream_state.stored,
                            &stream_state.request_id,
                            stream_state.status_code,
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
                        match anthropic_terminal {
                            Some(AnthropicTerminal::Error(_)) => {
                                if !stream_state.sse_error_outcome_recorded {
                                    record_provider_outcome(
                                        &stream_state.state,
                                        &stream_state.stored,
                                        ProviderOutcome::Failure { status_code: 502 },
                                    )
                                    .await;
                                }
                            }
                            _ => match semantic_terminal {
                                Some(SemanticTerminal::Failure(failure))
                                    if failure.origin == FailureOrigin::Provider =>
                                {
                                    if !stream_state.semantic_provider_outcome_recorded {
                                        record_provider_outcome(
                                            &stream_state.state,
                                            &stream_state.stored,
                                            ProviderOutcome::Failure { status_code: 502 },
                                        )
                                        .await;
                                    }
                                }
                                Some(SemanticTerminal::Failure(_)) => {}
                                _ if !stream_state.sse_error_outcome_recorded => {
                                    record_provider_outcome(
                                        &stream_state.state,
                                        &stream_state.stored,
                                        provider_outcome_from_status(stream_state.status_code),
                                    )
                                    .await;
                                }
                                _ => {}
                            },
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

        let decoded = if let Some(decoded) = buffered_upstream_body {
            decoded
        } else {
            let bytes = match crate::infra::http::read_response_body_limited(
                &mut upstream,
                PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
            )
            .await
            {
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
            decode_response_body_for_proxy_with_limit(
                &response_headers,
                bytes,
                PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
            )?
        };
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
        let (rewritten, grok_version_gate_rewritten) =
            maybe_rewrite_grok_cli_version_gate_body(status, &stored, bytes);
        bytes = rewritten;
        if version_gate_rewritten || grok_version_gate_rewritten {
            preserve_content_encoding = false;
        }
        let semantic_upstream_format =
            adapters::upstream_format_for_route(&stored, Some(route), &adapter_request.body)
                .unwrap_or_else(|| adapters::downstream_format_for_route(route));
        let mut semantic_provider_outcome_recorded = false;
        let anthropic_json_observation = if status.is_success()
            && response_semantics::semantic_guard_enabled()
            && matches!(
                route,
                ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens
            )
            && semantic_upstream_format == UpstreamFormat::AnthropicMessages
        {
            match anthropic_semantics::inspect_json_document(
                &bytes,
                route == ProxyRoute::ClaudeCountTokens,
            ) {
                Ok(observation) => {
                    let metric = match &observation {
                        AnthropicJsonObservation::Success => "success_terminal",
                        AnthropicJsonObservation::Error(_) => "provider_error",
                    };
                    crate::metrics::record_proxy_semantic_guard("anthropic_document", metric);
                    Some(observation)
                }
                Err(error) => {
                    crate::metrics::record_proxy_semantic_guard(
                        "anthropic_document",
                        "protocol_error",
                    );
                    record_provider_outcome(
                        &state,
                        &stored,
                        ProviderOutcome::Failure { status_code: 502 },
                    )
                    .await;
                    return Err(ProxyError::bad_gateway(error));
                }
            }
        } else {
            None
        };
        if let Some(AnthropicJsonObservation::Error(error)) = &anthropic_json_observation {
            status = claude_semantic_error_status(&error.error_type);
            status_code = status.as_u16();
        }
        let semantic_observation = if status.is_success()
            && response_semantics::semantic_guard_enabled()
            && semantic_upstream_format == UpstreamFormat::OpenAiResponses
        {
            let observation = match response_semantics::classify_json_document(&bytes) {
                Ok(observation) => observation,
                Err(error) => {
                    crate::metrics::record_proxy_semantic_guard("http_document", "protocol_error");
                    record_provider_outcome(
                        &state,
                        &stored,
                        ProviderOutcome::Failure { status_code: 502 },
                    )
                    .await;
                    if !request_is_provider_pinned(&headers, &request_context) {
                        if let Some(next_attempt) = next_provider_failover(
                            &state,
                            route,
                            &attempt_context,
                            &execution,
                            "responses_document_protocol_error",
                        )
                        .await
                        {
                            attempt_context = next_attempt;
                            drop(account_in_flight_guard);
                            drop(share_invocation_guard);
                            continue 'attempt;
                        }
                    }
                    return Err(ProxyError::bad_gateway(error));
                }
            };
            crate::metrics::record_proxy_semantic_guard("http_document", observation.metric_kind());
            if let SemanticObservation::Failure(failure) = &observation {
                if super::overflow_compact::is_context_length_failure(
                    &failure.code,
                    &failure.message,
                ) {
                    if let Some(next_attempt) = next_codex_overflow_attempt(
                        &state,
                        route,
                        &execution,
                        &stored,
                        &attempt_context,
                        &adapter_request,
                        &http_client,
                        &url,
                        &target_headers,
                        &request_context,
                    )
                    .await
                    {
                        attempt_context = next_attempt;
                        drop(account_in_flight_guard);
                        drop(share_invocation_guard);
                        continue 'attempt;
                    }
                }
                if failure.origin == FailureOrigin::Provider {
                    record_provider_outcome(
                        &state,
                        &stored,
                        ProviderOutcome::Failure { status_code: 502 },
                    )
                    .await;
                    semantic_provider_outcome_recorded = true;
                    if !request_is_provider_pinned(&headers, &request_context) {
                        if let Some(next_attempt) = next_provider_failover(
                            &state,
                            route,
                            &attempt_context,
                            &execution,
                            "responses_document_semantic_failure",
                        )
                        .await
                        {
                            attempt_context = next_attempt;
                            drop(account_in_flight_guard);
                            drop(share_invocation_guard);
                            continue 'attempt;
                        }
                    }
                }
            }
            Some(observation)
        } else {
            None
        };
        let usage = if route == ProxyRoute::ClaudeCountTokens {
            TokenUsage::default()
        } else {
            adapter.parse_usage(&bytes, &stored, route)
        };
        let bytes =
            adapter.transform_response_for_request(bytes, &stored, route, &adapter_request)?;
        let bytes = super::claude_oauth::restore_claude_tool_names_in_response_bytes(
            bytes,
            &adapter_request.claude_tool_name_map,
        );
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
        match anthropic_json_observation {
            Some(AnthropicJsonObservation::Error(error)) => {
                let outcome = claude_sse_error_outcome(&error.error_type)
                    .unwrap_or(ProviderOutcome::Failure { status_code: 502 });
                record_provider_outcome(&state, &stored, outcome).await;
            }
            _ => match semantic_observation {
                Some(SemanticObservation::Failure(failure))
                    if failure.origin == FailureOrigin::Provider =>
                {
                    if !semantic_provider_outcome_recorded {
                        record_provider_outcome(
                            &state,
                            &stored,
                            ProviderOutcome::Failure { status_code: 502 },
                        )
                        .await;
                    }
                }
                Some(SemanticObservation::Failure(_)) => {}
                _ => {
                    record_provider_outcome(
                        &state,
                        &stored,
                        provider_outcome_from_status(status_code),
                    )
                    .await;
                }
            },
        }

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
    let downstream_connection_guard = acquire_responses_downstream_connection()?;
    let route = ProxyRoute::CodexResponses;
    let app = route.app();
    let mut request_context = request_context_from_headers(&headers);
    request_context.session_id = session_id_from_request(route, &headers, b"");
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
        super::router::select_provider(
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
    ensure_managed_credential_persistence_available(&state, &execution)?;
    if !execution.driver_is("oauth.openai_codex") && !execution.driver_is("oauth.grok_responses") {
        return Err(ProxyError::bad_request(
            "responses websocket is only available for codex_oauth or grok_oauth providers",
        ));
    }
    if execution.driver_is("oauth.grok_responses") {
        ensure_grok_account_capability(&state, &execution, GrokAccountCapability::Websocket)
            .await?;
    }
    if !codex_websocket_enabled(&stored) {
        return Err(ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Codex Responses WebSocket is disabled for this provider; use POST /v1/responses (SSE) until the incident rollback is cleared".to_string(),
        });
    }
    validate_codex_allowed_client(&stored, route, &headers, request_context.share_id.is_some())?;
    refresh_execution_managed_account_if_needed(&state, &execution).await?;
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
    let ws_mode = if execution.driver_is("oauth.grok_responses") {
        ResponsesWebsocketMode::Grok
    } else {
        ResponsesWebsocketMode::Codex
    };
    let grok_turn_index = if matches!(ws_mode, ResponsesWebsocketMode::Grok) {
        super::grok::turn_index_from_headers(&headers)
    } else {
        None
    };
    let mut target = prepare_responses_websocket_target(
        &state,
        &execution,
        ws_mode,
        session_id.as_deref(),
        grok_turn_index,
    )
    .await?;
    append_codex_client_request_headers_owned(&mut target.headers, &headers, false);
    let websocket_upstream_model = match &execution.plan.model_policy {
        crate::domain::providers::runtime::RuntimeModelPolicy::Single { upstream_model } => {
            Some(upstream_model.clone())
        }
        crate::domain::providers::runtime::RuntimeModelPolicy::Passthrough => None,
    };
    let request_timeout = execution.request_timeout();
    let first_byte_timeout = execution.stream_first_byte_timeout();
    let stream_idle_timeout = execution.stream_idle_timeout();
    let share_id = request_context.share_id.clone();
    let user_email = request_context.user_email.clone();
    let state_for_share = state.clone();
    let response = ws.on_upgrade(move |socket| async move {
        let _downstream_connection_guard = downstream_connection_guard;
        let _share_invocation_guard = share_invocation_guard;
        if let Err(error) = bridge_responses_websocket(
            socket,
            ResponsesWebsocketBridgeOptions {
                headers: target.headers,
                connect_timeout: request_timeout,
                first_byte_timeout,
                stream_idle_timeout,
                ws_url: target.ws_url,
                pool_key: target.pool_key,
                mode: ws_mode,
                grok_session_id: session_id,
                grok_turn_index,
                single_upstream_model: websocket_upstream_model,
                state: &state_for_share,
                execution,
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
    super::grok::validate_media_request(&method, &upstream_path)?;
    let body = decode_request_body_for_proxy_with_limit(
        &headers,
        body,
        super::MEDIA_REQUEST_BODY_LIMIT_BYTES,
    )?;
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
    let sticky_media_binding = super::grok::sticky_media_session_key(&upstream_path, &body)
        .and_then(|session_key| state.grok_media_session_binding(&session_key));
    let mut selection_headers = headers.clone();
    if let Some(binding) = sticky_media_binding.as_ref() {
        if selection_headers.get("x-cc-provider-id").is_none() {
            if let Ok(value) = HeaderValue::from_str(&binding.provider_id) {
                selection_headers.insert(HeaderName::from_static("x-cc-provider-id"), value);
            }
        }
    }
    let shares = state.shares.read().await.clone();
    let accounts_for_selection = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let account_in_flight = state.account_in_flight.snapshot();
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
    let account_in_flight_guard = acquire_account_in_flight(
        &state,
        &execution.stored,
        &accounts_for_selection,
        &account_in_flight,
    )?;
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
        sticky_media_binding,
        request_context.share_id,
        request_context.user_email,
        account_in_flight_guard,
        share_invocation_guard,
    )
    .await
}

pub async fn forward_images_generations(
    state: ServerState,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = decode_request_body_for_proxy_with_limit(
        &headers,
        body,
        super::MEDIA_REQUEST_BODY_LIMIT_BYTES,
    )?;
    let mut request_context = request_context_from_headers(&headers);
    request_context.session_id =
        session_id_from_request(ProxyRoute::CodexResponses, &headers, &body);
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
    let (execution, account_in_flight_guard) =
        if let Some(share_id) = request_context.share_id.as_deref() {
            let (execution, _share_name) = select_share_image_generation_execution(
                &providers,
                &shares,
                &accounts_for_selection,
                share_id,
            )?;
            let snapshot = state.account_in_flight.snapshot();
            let guard = acquire_account_in_flight(
                &state,
                &execution.stored,
                &accounts_for_selection,
                &snapshot,
            )?;
            (execution, guard)
        } else {
            select_and_acquire_account_in_flight(&state, &accounts_for_selection, |snapshot| {
                select_provider_for_codex_image_generation(
                    &providers,
                    &accounts_for_selection,
                    &headers,
                    configured_provider_id.as_deref(),
                    snapshot,
                    request_context.session_id.as_deref(),
                )
                .map(|selection| selection.execution)
            })?
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
            None,
            request_context.share_id,
            request_context.user_email,
            account_in_flight_guard,
            share_invocation_guard,
        )
        .await
    } else if execution.driver_is("oauth.openai_codex") {
        let prepared = codex_images_generation_request(&body)?;
        forward_codex_images_request(
            state,
            execution,
            headers,
            prepared,
            request_context,
            account_in_flight_guard,
            share_invocation_guard,
        )
        .await
    } else {
        Err(ProxyError::bad_request(
            "image generation requires a grok_oauth provider or codex_oauth provider with image generation enabled",
        ))
    }
}

pub async fn forward_images_edits(
    state: ServerState,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let body = decode_request_body_for_proxy_with_limit(
        &headers,
        body,
        super::MEDIA_REQUEST_BODY_LIMIT_BYTES,
    )?;
    let mut request_context = request_context_from_headers(&headers);
    request_context.session_id =
        session_id_from_request(ProxyRoute::CodexResponses, &headers, &body);
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
    let (execution, account_in_flight_guard) =
        if let Some(share_id) = request_context.share_id.as_deref() {
            let (execution, _share_name) = select_share_image_generation_execution(
                &providers,
                &shares,
                &accounts_for_selection,
                share_id,
            )?;
            let snapshot = state.account_in_flight.snapshot();
            let guard = acquire_account_in_flight(
                &state,
                &execution.stored,
                &accounts_for_selection,
                &snapshot,
            )?;
            (execution, guard)
        } else {
            select_and_acquire_account_in_flight(&state, &accounts_for_selection, |snapshot| {
                select_provider_for_codex_image_generation(
                    &providers,
                    &accounts_for_selection,
                    &headers,
                    configured_provider_id.as_deref(),
                    snapshot,
                    request_context.session_id.as_deref(),
                )
                .map(|selection| selection.execution)
            })?
        };
    drop(providers);

    if execution.driver_is("oauth.grok_responses") {
        forward_grok_media_with_execution(
            state,
            execution,
            Method::POST,
            "/images/edits".to_string(),
            headers,
            body,
            None,
            request_context.share_id,
            request_context.user_email,
            account_in_flight_guard,
            share_invocation_guard,
        )
        .await
    } else if execution.driver_is("oauth.openai_codex") {
        let prepared = codex_images_edit_request(&headers, body).await?;
        forward_codex_images_request(
            state,
            execution,
            headers,
            prepared,
            request_context,
            account_in_flight_guard,
            share_invocation_guard,
        )
        .await
    } else {
        Err(ProxyError::bad_request(
            "image editing requires a grok_oauth provider or codex_oauth provider with image generation enabled",
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
    sticky_media_binding: Option<GrokMediaSessionBinding>,
    share_id: Option<String>,
    user_email: Option<String>,
    _account_in_flight_guard: Option<AccountInFlightGuard>,
    _share_invocation_guard: Option<ShareInFlightGuard>,
) -> Result<Response, ProxyError> {
    let stored = execution.runtime_stored_view();
    if let Some(binding) = sticky_media_binding.as_ref() {
        ensure_grok_media_session_binding(&execution, binding)?;
    }
    ensure_managed_credential_persistence_available(&state, &execution)?;
    let capability = grok_media_capability(&method, &upstream_path);
    ensure_grok_account_capability(&state, &execution, capability).await?;
    refresh_execution_managed_account_if_needed(&state, &execution).await?;
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let media_session_id = optional_header(&headers, "x-grok-conv-id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|session_id| {
            let tenant_scope =
                grok_tenant_scope_parts(share_id.as_deref(), user_email.as_deref(), &stored);
            super::grok::namespace_session_id(tenant_scope.as_deref(), &session_id)
        });
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
    let http_client = forward_http_client(&state, &stored).await?;
    let started = Instant::now();
    let mut auth_refresh_attempted = false;
    let mut upstream = loop {
        ensure_managed_credential_persistence_available(&state, &execution)?;
        let accounts = state.accounts_snapshot().await;
        let mut target_headers = adapter.build_headers(AppKind::Codex, &stored, &accounts)?;
        if let Some(session_id) = media_session_id.as_deref() {
            replace_or_push_header(
                &mut target_headers,
                "x-grok-conv-id",
                session_id.to_string(),
            );
        }
        replace_or_push_header(
            &mut target_headers,
            "accept",
            "application/json, text/event-stream".to_string(),
        );
        let mut target_headers = owned_headers(target_headers);
        super::grok::apply_cli_identity_headers(
            &mut target_headers,
            super::grok::turn_index_from_headers(&headers),
        );
        let mut url = super::join_url(&execution.plan.endpoint, &upstream_path);
        let materialized_auth = execution.materialize_auth(&accounts)?;
        execution.apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())?;
        apply_account_header_overrides(&mut target_headers, &stored, &accounts)?;
        execution.finalize_outbound_identity(&mut target_headers)?;

        let mut request = http_client
            .request(method.clone(), &url)
            .header(CONTENT_TYPE, content_type.as_str());
        for (name, value) in &target_headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if method != Method::GET {
            request = request.body(body.clone());
        }
        request = request.timeout(execution.request_timeout());
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
        if upstream.status() != StatusCode::UNAUTHORIZED || auth_refresh_attempted {
            break upstream;
        }
        let Some((provider_type, account_id)) = execution.managed_account_target() else {
            break upstream;
        };
        drop(upstream);
        if let Err(error) = state
            .refresh_managed_account_now(provider_type, Some(account_id))
            .await
        {
            mark_managed_account_auth_cooldown(
                &state,
                &execution,
                "grok_media_forced_refresh_failed",
            )
            .await;
            return Err(managed_account_refresh_error_to_proxy_error(error));
        }
        auth_refresh_attempted = true;
        record_forward_retry(
            ProxyRoute::CodexResponses,
            "auth",
            "grok_media_unauthorized",
        );
    };
    if upstream.status() == StatusCode::UNAUTHORIZED && auth_refresh_attempted {
        mark_managed_account_auth_cooldown(
            &state,
            &execution,
            "grok_media_unauthorized_after_refresh",
        )
        .await;
    }
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
    let bytes = crate::infra::http::read_response_body_limited(
        &mut upstream,
        super::MEDIA_RESPONSE_BODY_LIMIT_BYTES,
    )
    .await
    .map_err(ProxyError::bad_gateway)?;
    let decoded = decode_response_body_for_proxy_with_limit(
        &response_headers,
        bytes,
        super::MEDIA_RESPONSE_BODY_LIMIT_BYTES,
    )?;
    let mut preserve_content_encoding = decoded.preserve_content_encoding;
    let (response_body, version_gate_rewritten) =
        maybe_rewrite_grok_cli_version_gate_body(status, &stored, decoded.body);
    if version_gate_rewritten {
        preserve_content_encoding = false;
    }
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
    if status.is_success() {
        record_grok_capability_evidence(&state, &execution, capability).await;
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
    if preserve_content_encoding {
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

fn ensure_grok_media_session_binding(
    execution: &ProviderExecution,
    binding: &GrokMediaSessionBinding,
) -> Result<(), ProxyError> {
    let account_matches = binding.account_id.as_deref().is_some_and(|account_id| {
        execution
            .managed_account_id()
            .is_some_and(|current| current == account_id)
    });
    if binding.provider_id == execution.stored.provider.id && account_matches {
        return Ok(());
    }
    Err(ProxyError {
        status: StatusCode::CONFLICT,
        message: "Grok media session is bound to a different Provider or OAuth account".to_string(),
    })
}

async fn forward_codex_images_request(
    state: ServerState,
    execution: ProviderExecution,
    headers: HeaderMap,
    mut prepared: CodexImagesPreparedRequest,
    request_context: UsageLogContext,
    _account_in_flight_guard: Option<AccountInFlightGuard>,
    _share_invocation_guard: Option<ShareInFlightGuard>,
) -> Result<Response, ProxyError> {
    let stored = execution.runtime_stored_view();
    ensure_managed_credential_persistence_available(&state, &execution)?;
    let accounts = state.accounts_snapshot().await;
    super::router::ensure_codex_oauth_active_account(&stored, &accounts)?;
    validate_codex_allowed_client(
        &stored,
        ProxyRoute::CodexResponses,
        &headers,
        request_context.share_id.is_some(),
    )?;
    prepared.body = super::remote_image::inline_codex_remote_images(&prepared.body).await?;
    let session_id = codex_oauth_session_id_from_request(&headers, &prepared.body);
    let started = Instant::now();
    {
        refresh_execution_managed_account_if_needed(&state, &execution).await?;
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
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        execution.enforce_model_policy(&mut adapter_request)?;
        let mut auth_refresh_attempted = false;
        let mut upstream = loop {
            let upstream = match send_codex_images_attempt(
                &state,
                &execution,
                &stored,
                &adapter_request,
                session_id.as_deref(),
            )
            .await
            {
                Ok(upstream) => upstream,
                Err(error) => {
                    record_provider_outcome(&state, &stored, ProviderOutcome::NetworkFailure).await;
                    return Err(error);
                }
            };
            if upstream.status() == StatusCode::UNAUTHORIZED && !auth_refresh_attempted {
                if let Some((provider_type, account_id)) = execution.managed_account_target() {
                    drop(upstream);
                    let refresh_result = state
                        .refresh_managed_account_now(provider_type, Some(account_id))
                        .await;
                    if let Err(error) = refresh_result {
                        mark_managed_account_auth_cooldown(
                            &state,
                            &execution,
                            "images_forced_refresh_failed",
                        )
                        .await;
                        return Err(managed_account_refresh_error_to_proxy_error(error));
                    }
                    auth_refresh_attempted = true;
                    record_forward_retry(ProxyRoute::CodexResponses, "auth", "images_unauthorized");
                    continue;
                }
            }
            break upstream;
        };
        let status = upstream.status();
        let status_code = status.as_u16();
        if status == StatusCode::UNAUTHORIZED && auth_refresh_attempted {
            mark_managed_account_auth_cooldown(
                &state,
                &execution,
                "images_unauthorized_after_refresh",
            )
            .await;
        }
        let mut response_headers = upstream.headers().clone();
        strip_hop_by_hop_response_headers(&mut response_headers);
        let content_type = response_headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_encoding = content_encoding_value(&response_headers);
        let bytes = crate::infra::http::read_response_body_limited(
            &mut upstream,
            super::MEDIA_RESPONSE_BODY_LIMIT_BYTES,
        )
        .await
        .map_err(ProxyError::bad_gateway)?;
        let decoded = decode_response_body_for_proxy_with_limit(
            &response_headers,
            bytes,
            super::MEDIA_RESPONSE_BODY_LIMIT_BYTES,
        )?;
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
        let mut usage_accumulator = StreamUsageAccumulator::new(
            adapters::usage_input_semantics_for(&stored, ProxyRoute::CodexResponses),
        );
        usage_accumulator.push(&decoded.body);
        let usage = usage_accumulator.finish();
        log_usage(
            &state,
            &stored,
            status_code,
            started.elapsed().as_millis(),
            UsageModelMetadata {
                model: Some(prepared.tool_model.clone()),
                requested_model: Some(prepared.tool_model.clone()),
                actual_model: Some(CODEX_IMAGES_RESPONSES_MAIN_MODEL.to_string()),
                actual_model_source: Some("codex_image_generation_bridge".to_string()),
            },
            usage,
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
            usage,
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
}

async fn send_codex_images_attempt(
    state: &ServerState,
    execution: &ProviderExecution,
    stored: &StoredProvider,
    adapter_request: &adapters::AdapterRequest,
    session_id: Option<&str>,
) -> Result<reqwest::Response, ProxyError> {
    let accounts = state.accounts_snapshot().await;
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let mut target_headers = adapter.build_headers(AppKind::Codex, stored, &accounts)?;
    append_codex_oauth_session_headers(&mut target_headers, session_id);
    crate::codex_identity::finalize_headers(&mut target_headers);
    let mut target_headers = owned_headers(target_headers);
    let mut url = execution.resolve_endpoint(ProxyRoute::CodexResponses, None, adapter_request)?;
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut target_headers, &mut url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut target_headers, stored, &accounts)?;
    execution.finalize_outbound_identity(&mut target_headers)?;
    let http_client = forward_http_client(state, stored).await?;
    let mut request = http_client
        .post(&url)
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(adapter_request.body.clone())
        .timeout(execution.request_timeout());
    for (name, value) in &target_headers {
        request = request.header(name.as_str(), value.as_str());
    }
    request.send().await.map_err(ProxyError::bad_gateway)
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

async fn codex_images_edit_request(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<CodexImagesPreparedRequest, ProxyError> {
    let content_type = optional_header(headers, "content-type").unwrap_or_default();
    let content_type = content_type.trim();
    if content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        let value = serde_json::from_slice::<Value>(&body).map_err(|error| ProxyError {
            status: StatusCode::BAD_REQUEST,
            message: format!("invalid OpenAI image edit request JSON: {error}"),
        })?;
        return codex_images_edit_request_from_value(value);
    }

    let boundary = multer::parse_boundary(content_type)
        .map_err(|error| ProxyError::bad_request(format!("invalid multipart boundary: {error}")))?;
    let stream = stream::once(async move { Ok::<Bytes, std::io::Error>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut fields = serde_json::Map::new();
    let mut images = Vec::new();
    let mut mask = None;
    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ProxyError::bad_request(format!("invalid image edit multipart: {error}"))
    })? {
        let name = field.name().unwrap_or_default().to_string();
        let claimed_content_type = field
            .content_type()
            .map(ToString::to_string)
            .unwrap_or_else(|| "application/octet-stream".to_string());
        if matches!(name.as_str(), "image" | "image[]" | "mask") {
            let bytes = field.bytes().await.map_err(|error| {
                ProxyError::bad_request(format!("read multipart field {name}: {error}"))
            })?;
            if bytes.is_empty() {
                return Err(ProxyError::bad_request(format!(
                    "multipart field {name} is empty"
                )));
            }
            let content_type = super::remote_image::validate_image_bytes(
                &bytes,
                Some(&claimed_content_type),
                super::MEDIA_REQUEST_BODY_LIMIT_BYTES,
            )?;
            let data_url = format!("data:{content_type};base64,{}", STANDARD.encode(bytes));
            if name == "mask" {
                mask = Some(data_url);
            } else {
                images.push(data_url);
            }
            continue;
        }
        let value = field.text().await.map_err(|error| {
            ProxyError::bad_request(format!("read multipart field {name}: {error}"))
        })?;
        fields.insert(name, Value::String(value));
    }
    fields.insert(
        "images".to_string(),
        Value::Array(
            images
                .into_iter()
                .map(|image_url| json!({"image_url": image_url}))
                .collect(),
        ),
    );
    if let Some(mask) = mask {
        fields.insert("mask".to_string(), json!({"image_url": mask}));
    }
    codex_images_edit_request_from_value(Value::Object(fields))
}

fn codex_images_edit_request_from_value(
    value: Value,
) -> Result<CodexImagesPreparedRequest, ProxyError> {
    const MAX_EDIT_IMAGES: usize = 16;
    let prompt = value
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or_else(|| ProxyError::bad_request("image edit prompt is required"))?;
    let mut image_urls = Vec::new();
    for field in ["images", "image"] {
        let Some(images) = value.get(field) else {
            continue;
        };
        let candidates = images
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(images));
        for image in candidates {
            let image_url = image
                .as_str()
                .or_else(|| image.get("image_url").and_then(Value::as_str))
                .map(str::trim)
                .filter(|image_url| !image_url.is_empty())
                .ok_or_else(|| {
                    ProxyError::bad_request(
                        "image edit inputs must use image_url strings; file_id is unsupported",
                    )
                })?;
            image_urls.push(image_url.to_string());
        }
    }
    if image_urls.is_empty() {
        return Err(ProxyError::bad_request(
            "image edit input image is required",
        ));
    }
    if image_urls.len() > MAX_EDIT_IMAGES {
        return Err(ProxyError::bad_request(format!(
            "image edit accepts at most {MAX_EDIT_IMAGES} input images"
        )));
    }
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
    let stream = json_boolish(value.get("stream")).unwrap_or(false);
    let mut tool = json!({
        "type": "image_generation",
        "action": "edit",
        "model": tool_model,
    });
    for field in [
        "size",
        "quality",
        "background",
        "output_format",
        "input_fidelity",
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
        if let Some(number) = value.get(field).and_then(json_i64ish) {
            tool[field] = Value::Number(number.into());
        }
    }
    if let Some(mask) = value
        .pointer("/mask/image_url")
        .and_then(Value::as_str)
        .or_else(|| value.get("mask").and_then(Value::as_str))
        .map(str::trim)
        .filter(|mask| !mask.is_empty())
    {
        tool["input_image_mask"] = json!({"image_url": mask});
    }
    let mut content = vec![json!({"type": "input_text", "text": prompt})];
    content.extend(
        image_urls
            .into_iter()
            .map(|image_url| json!({"type": "input_image", "image_url": image_url})),
    );
    let request = json!({
        "instructions": "",
        "stream": true,
        "reasoning": {"effort": "medium", "summary": "auto"},
        "parallel_tool_calls": false,
        "include": ["reasoning.encrypted_content"],
        "model": CODEX_IMAGES_RESPONSES_MAIN_MODEL,
        "store": false,
        "tool_choice": {"type": "image_generation"},
        "input": [{"type": "message", "role": "user", "content": content}],
        "tools": [tool],
    });
    let body = serde_json::to_vec(&request)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode Codex image edit request failed: {error}"),
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

fn json_boolish(value: Option<&Value>) -> Option<bool> {
    value.and_then(|value| {
        value.as_bool().or_else(|| {
            value.as_str().map(str::trim).and_then(|value| {
                match value.to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" => Some(true),
                    "false" | "0" | "no" => Some(false),
                    _ => None,
                }
            })
        })
    })
}

fn json_i64ish(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesWebsocketMode {
    Codex,
    Grok,
}

fn responses_websocket_http_replay_allowed(
    mode: ResponsesWebsocketMode,
    emitted_business_event: bool,
    response_create_committed: bool,
) -> bool {
    !emitted_business_event
        && (!response_create_committed || matches!(mode, ResponsesWebsocketMode::Grok))
}

struct ResponsesWebsocketBridgeOptions<'a> {
    headers: Vec<(String, String)>,
    connect_timeout: Duration,
    first_byte_timeout: Option<Duration>,
    stream_idle_timeout: Option<Duration>,
    ws_url: String,
    pool_key: Option<String>,
    mode: ResponsesWebsocketMode,
    grok_session_id: Option<String>,
    grok_turn_index: Option<u64>,
    single_upstream_model: Option<String>,
    state: &'a ServerState,
    execution: ProviderExecution,
}

struct PreparedResponsesWebSocketTarget {
    headers: Vec<(String, String)>,
    ws_url: String,
    pool_key: Option<String>,
}

async fn ensure_responses_websocket_turn_allowed(
    state: &ServerState,
    execution: &ProviderExecution,
    mode: ResponsesWebsocketMode,
) -> Result<(), ProxyError> {
    ensure_managed_credential_persistence_available(state, execution)?;
    if matches!(mode, ResponsesWebsocketMode::Codex) {
        let stored = execution.runtime_stored_view();
        let accounts = state.accounts_snapshot().await;
        super::router::ensure_codex_oauth_active_account(&stored, &accounts)?;
    }
    Ok(())
}

async fn prepare_responses_websocket_target(
    state: &ServerState,
    execution: &ProviderExecution,
    mode: ResponsesWebsocketMode,
    session_id: Option<&str>,
    grok_turn_index: Option<u64>,
) -> Result<PreparedResponsesWebSocketTarget, ProxyError> {
    ensure_managed_credential_persistence_available(state, execution)?;
    let stored = execution.runtime_stored_view();
    let accounts = state.accounts_snapshot().await;
    let adapter = adapters::adapter_for(stored.app, stored.provider_type);
    let mut headers = adapter.build_headers(stored.app, &stored, &accounts)?;
    append_codex_oauth_session_headers(&mut headers, session_id);
    if matches!(mode, ResponsesWebsocketMode::Codex) {
        crate::codex_identity::finalize_headers(&mut headers);
    } else if let Some(session_id) = session_id {
        replace_or_push_header(&mut headers, "x-grok-conv-id", session_id.to_string());
    }
    let mut headers = owned_headers(headers);
    if matches!(mode, ResponsesWebsocketMode::Grok) {
        super::grok::apply_cli_identity_headers(&mut headers, grok_turn_index);
    }
    let mut ws_url = if matches!(mode, ResponsesWebsocketMode::Grok) {
        grok_responses_websocket_url(execution)
    } else {
        codex_responses_websocket_url(execution)
    };
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut headers, &mut ws_url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut headers, &stored, &accounts)?;
    execution.finalize_outbound_identity(&mut headers)?;
    let pool_key = if matches!(mode, ResponsesWebsocketMode::Codex) {
        session_id.map(|session_id| {
            responses_websocket_pool_key(state, execution, session_id, &ws_url, &headers)
        })
    } else {
        None
    };
    Ok(PreparedResponsesWebSocketTarget {
        headers,
        ws_url,
        pool_key,
    })
}

fn codex_responses_websocket_url(_execution: &ProviderExecution) -> String {
    #[cfg(test)]
    if let Some(url) = _execution
        .plan
        .driver_options
        .get("testCodexWebsocketUrl")
        .and_then(Value::as_str)
    {
        return url.to_string();
    }
    "wss://chatgpt.com/backend-api/codex/responses".to_string()
}

fn grok_responses_websocket_url(_execution: &ProviderExecution) -> String {
    #[cfg(test)]
    if let Some(url) = _execution
        .plan
        .driver_options
        .get("testGrokWebsocketUrl")
        .and_then(Value::as_str)
    {
        return url.to_string();
    }
    super::grok::websocket_url().to_string()
}

fn responses_websocket_pool_key(
    state: &ServerState,
    execution: &ProviderExecution,
    session_id: &str,
    ws_url: &str,
    headers: &[(String, String)],
) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for value in [
        state.process_instance_id.as_str(),
        execution.stored.provider.id.as_str(),
        execution.plan.runtime_fingerprint.as_str(),
        session_id,
        ws_url,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    for (name, value) in headers {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

async fn bridge_responses_websocket(
    downstream: WebSocket,
    options: ResponsesWebsocketBridgeOptions<'_>,
) -> Result<(), ProxyError> {
    let ResponsesWebsocketBridgeOptions {
        mut headers,
        connect_timeout,
        first_byte_timeout,
        stream_idle_timeout,
        mut ws_url,
        mut pool_key,
        mode,
        grok_session_id,
        grok_turn_index,
        single_upstream_model,
        state,
        mut execution,
    } = options;
    let mut account_in_flight_guard = None;
    let mut entry = None;
    let mut entry_was_cached = false;
    let mut downstream = downstream;
    let mut response_in_flight = false;
    let mut response_create_committed = false;
    let mut emitted_business_event = false;
    let mut pending_lifecycle_messages = Vec::new();
    let mut semantic_provider_outcome_recorded = false;
    let mut active_response_body = None;
    let mut auth_refresh_attempted = false;
    let mut refresh_target_before_connect = false;
    let mut upstream_read_deadline = None;
    let mut output_patcher = CodexWebsocketOutputPatcher::default();
    loop {
        tokio::select! {
            downstream_message = downstream.next() => {
                let Some(message) = downstream_message else {
                    break;
                };
                let message = message
                    .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
                match message {
                    AxumWsMessage::Close(_) => break,
                    AxumWsMessage::Ping(bytes) => {
                        downstream
                            .send(AxumWsMessage::Pong(bytes))
                            .await
                            .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
                        continue;
                    }
                    AxumWsMessage::Pong(_) => continue,
                    AxumWsMessage::Text(_) | AxumWsMessage::Binary(_) => {}
                }
                let Some(message) = axum_ws_message_to_tungstenite(
                    message,
                    mode,
                    grok_session_id.as_deref(),
                    single_upstream_model.as_deref(),
                ) else {
                    break;
                };
                let starts_response = responses_websocket_request_starts_response(&message);
                if starts_response {
                    ensure_responses_websocket_turn_allowed(state, &execution, mode).await?;
                }
                let message = if matches!(mode, ResponsesWebsocketMode::Codex) {
                    prepare_codex_responses_websocket_request(message).await?
                } else {
                    message
                };
                if starts_response {
                    if response_in_flight {
                        return Err(ProxyError::bad_request(
                            "responses websocket received response.create while a response is in flight",
                        ));
                    }
                    active_response_body = Some(responses_websocket_http_body(&message)?);
                    let accounts = state.accounts_snapshot().await;
                    let snapshot = state.account_in_flight.snapshot();
                    account_in_flight_guard = acquire_account_in_flight(
                        state,
                        &execution.stored,
                        &accounts,
                        &snapshot,
                    )?;
                    response_in_flight = true;
                    response_create_committed = false;
                    emitted_business_event = false;
                    pending_lifecycle_messages.clear();
                    semantic_provider_outcome_recorded = false;
                    auth_refresh_attempted = false;
                    if entry.is_none() {
                        if refresh_target_before_connect {
                            let forwarded_headers = codex_client_headers_from_owned(&headers);
                            let target = prepare_responses_websocket_target(
                                state,
                                &execution,
                                mode,
                                grok_session_id.as_deref(),
                                grok_turn_index,
                            )
                            .await?;
                            headers = target.headers;
                            merge_owned_headers(&mut headers, forwarded_headers);
                            ws_url = target.ws_url;
                            pool_key = target.pool_key;
                            refresh_target_before_connect = false;
                        }
                        (entry, entry_was_cached) = acquire_cached_responses_websocket(&pool_key);
                    }
                    if entry.is_none() {
                        match connect_responses_websocket(
                            state,
                            &mut execution,
                            mode,
                            grok_session_id.as_deref(),
                            connect_timeout,
                            &mut headers,
                            &mut ws_url,
                            &mut pool_key,
                            &mut auth_refresh_attempted,
                        )
                        .await
                        {
                            Ok(connected) => {
                                entry = Some(connected);
                                entry_was_cached = false;
                            }
                            Err(failure) => {
                                let Some(source) = failure.fallback_source else {
                                    if response_in_flight {
                                        let error_code = if failure.error.status
                                            == StatusCode::UNAUTHORIZED
                                        {
                                            "upstream_auth_error"
                                        } else if failure.error.status
                                            == StatusCode::GATEWAY_TIMEOUT
                                        {
                                            "upstream_connect_timeout"
                                        } else {
                                            "upstream_connect_error"
                                        };
                                        let error_body = websocket_stream_error_body(
                                            failure.error.client_message(),
                                            error_code,
                                        );
                                        let provider_outcome = provider_outcome_from_status(
                                            failure.error.status.as_u16(),
                                        );
                                        return terminate_responses_websocket_with_error(
                                            &mut downstream,
                                            &mut output_patcher,
                                            mode,
                                            state,
                                            &execution,
                                            &mut pending_lifecycle_messages,
                                            failure.error,
                                            Some("connect_error"),
                                            error_body,
                                            Some(provider_outcome),
                                        )
                                        .await;
                                    }
                                    return Err(failure.error);
                                };
                                let body = active_response_body.as_ref().ok_or_else(|| {
                                    ProxyError::bad_request(
                                        "response.create body is unavailable for HTTP fallback",
                                    )
                                })?;
                                pending_lifecycle_messages.clear();
                                let outcome = run_codex_websocket_http_fallback(
                                    &mut downstream,
                                    state,
                                    &mut execution,
                                    body,
                                    grok_session_id.as_deref(),
                                    grok_turn_index,
                                    first_byte_timeout,
                                    stream_idle_timeout,
                                    source,
                                    &mut auth_refresh_attempted,
                                    &mut output_patcher,
                                )
                                .await?;
                                if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                                    return Ok(());
                                }
                                response_in_flight = false;
                                response_create_committed = false;
                                account_in_flight_guard.take();
                                active_response_body = None;
                                refresh_target_before_connect = true;
                                continue;
                            }
                        }
                    }
                }
                if entry.is_none() {
                    return Err(ProxyError::bad_request(
                        "the first responses websocket request must be response.create",
                    ));
                }
                let send_result = entry
                    .as_mut()
                    .expect("upstream websocket is connected")
                    .socket
                    .send(message)
                    .await;
                if let Err(error) = send_result {
                    let _failed_entry = entry.take();
                    if response_in_flight
                        && responses_websocket_http_replay_allowed(
                            mode,
                            emitted_business_event,
                            response_create_committed,
                        )
                    {
                        let body = active_response_body.as_ref().ok_or_else(|| {
                            ProxyError::bad_request(
                                "response.create body is unavailable for HTTP fallback",
                            )
                        })?;
                        let source = if entry_was_cached {
                            "cached_stale"
                        } else {
                            "send_failure"
                        };
                        pending_lifecycle_messages.clear();
                        let outcome = run_codex_websocket_http_fallback(
                            &mut downstream,
                            state,
                            &mut execution,
                            body,
                            grok_session_id.as_deref(),
                            grok_turn_index,
                            first_byte_timeout,
                            stream_idle_timeout,
                            source,
                            &mut auth_refresh_attempted,
                            &mut output_patcher,
                        )
                        .await?;
                        if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                            return Ok(());
                        }
                        response_in_flight = false;
                        response_create_committed = false;
                        account_in_flight_guard.take();
                        active_response_body = None;
                        refresh_target_before_connect = true;
                        continue;
                    }
                    let error = ProxyError::bad_gateway(error.to_string());
                    if response_in_flight {
                        let error_body = websocket_stream_error_body(
                            error.client_message(),
                            "upstream_stream_transport_error",
                        );
                        return terminate_responses_websocket_with_error(
                            &mut downstream,
                            &mut output_patcher,
                            mode,
                            state,
                            &execution,
                            &mut pending_lifecycle_messages,
                            error,
                            Some("transport_error"),
                            error_body,
                            Some(ProviderOutcome::NetworkFailure),
                        )
                        .await;
                    }
                    return Err(error);
                }
                if starts_response {
                    response_create_committed = true;
                    upstream_read_deadline = first_byte_timeout
                        .map(|timeout| tokio::time::Instant::now() + timeout);
                }
            }
            upstream_read = next_responses_websocket_message(&mut entry, upstream_read_deadline) => {
                let upstream_message = match upstream_read {
                    ResponsesWebsocketRead::Message(message) => message,
                    ResponsesWebsocketRead::TimedOut => {
                        let _timed_out_entry = entry.take();
                        if response_in_flight
                            && responses_websocket_http_replay_allowed(
                                mode,
                                emitted_business_event,
                                response_create_committed,
                            )
                        {
                            let body = active_response_body.as_ref().ok_or_else(|| {
                                ProxyError::bad_request(
                                    "response.create body is unavailable for HTTP fallback",
                                )
                            })?;
                            pending_lifecycle_messages.clear();
                            let outcome = run_codex_websocket_http_fallback(
                                &mut downstream,
                                state,
                                &mut execution,
                                body,
                                grok_session_id.as_deref(),
                                grok_turn_index,
                                first_byte_timeout,
                                stream_idle_timeout,
                                "first_byte_timeout",
                                &mut auth_refresh_attempted,
                                &mut output_patcher,
                            )
                            .await?;
                            if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                                return Ok(());
                            }
                            response_in_flight = false;
                            response_create_committed = false;
                            account_in_flight_guard.take();
                            active_response_body = None;
                            upstream_read_deadline = None;
                            refresh_target_before_connect = true;
                            continue;
                        }
                        let error = ProxyError {
                            status: StatusCode::GATEWAY_TIMEOUT,
                            message: if emitted_business_event {
                                "responses websocket stream idle timeout".to_string()
                            } else {
                                "responses websocket first byte timeout".to_string()
                            },
                        };
                        let error_body = websocket_stream_error_body(
                            error.client_message(),
                            "upstream_stream_timeout",
                        );
                        return terminate_responses_websocket_with_error(
                            &mut downstream,
                            &mut output_patcher,
                            mode,
                            state,
                            &execution,
                            &mut pending_lifecycle_messages,
                            error,
                            Some("transport_error"),
                            error_body,
                            Some(ProviderOutcome::NetworkFailure),
                        )
                        .await;
                    }
                };
                let Some(message) = upstream_message else {
                    let _closed_entry = entry.take();
                    if response_in_flight
                        && responses_websocket_http_replay_allowed(
                            mode,
                            emitted_business_event,
                            response_create_committed,
                        )
                    {
                        let body = active_response_body.as_ref().ok_or_else(|| {
                            ProxyError::bad_request(
                                "response.create body is unavailable for HTTP fallback",
                            )
                        })?;
                        let source = if entry_was_cached {
                            "cached_stale"
                        } else {
                            "closed_before_event"
                        };
                        pending_lifecycle_messages.clear();
                        let outcome = run_codex_websocket_http_fallback(
                            &mut downstream,
                            state,
                            &mut execution,
                            body,
                            grok_session_id.as_deref(),
                            grok_turn_index,
                            first_byte_timeout,
                            stream_idle_timeout,
                            source,
                            &mut auth_refresh_attempted,
                            &mut output_patcher,
                        )
                        .await?;
                        if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                            return Ok(());
                        }
                        response_in_flight = false;
                        response_create_committed = false;
                        account_in_flight_guard.take();
                        active_response_body = None;
                        refresh_target_before_connect = true;
                        continue;
                    }
                    if response_in_flight {
                        return terminate_responses_websocket_without_terminal(
                            &mut downstream,
                            &mut output_patcher,
                            mode,
                            state,
                            &execution,
                            &mut pending_lifecycle_messages,
                            None,
                        )
                        .await;
                    }
                    refresh_target_before_connect = true;
                    continue;
                };
                let message = match message {
                    Ok(message) => message,
                    Err(error)
                        if response_in_flight
                            && responses_websocket_http_replay_allowed(
                                mode,
                                emitted_business_event,
                                response_create_committed,
                            )
                            && websocket_read_fallback_source(&error, entry_was_cached).is_some() =>
                    {
                        let source = websocket_read_fallback_source(&error, entry_was_cached)
                            .expect("checked websocket fallback source");
                        let _failed_entry = entry.take();
                        let body = active_response_body.as_ref().ok_or_else(|| {
                            ProxyError::bad_request(
                                "response.create body is unavailable for HTTP fallback",
                            )
                        })?;
                        pending_lifecycle_messages.clear();
                        let outcome = run_codex_websocket_http_fallback(
                            &mut downstream,
                            state,
                            &mut execution,
                            body,
                            grok_session_id.as_deref(),
                            grok_turn_index,
                            first_byte_timeout,
                            stream_idle_timeout,
                                source,
                                &mut auth_refresh_attempted,
                                &mut output_patcher,
                        )
                        .await?;
                        if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                            return Ok(());
                        }
                        response_in_flight = false;
                        response_create_committed = false;
                        account_in_flight_guard.take();
                        active_response_body = None;
                        refresh_target_before_connect = true;
                        continue;
                    }
                    Err(error) if websocket_message_too_big(&error) => {
                        let body = websocket_message_too_big_error_body();
                        let error = ProxyError {
                            status: StatusCode::PAYLOAD_TOO_LARGE,
                            message: "upstream websocket message too big".to_string(),
                        };
                        if response_in_flight {
                            return terminate_responses_websocket_with_error(
                                &mut downstream,
                                &mut output_patcher,
                                mode,
                                state,
                                &execution,
                                &mut pending_lifecycle_messages,
                                error,
                                Some("transport_error"),
                                body,
                                Some(ProviderOutcome::Failure { status_code: 413 }),
                            )
                            .await;
                        }
                        let _ = downstream.send(AxumWsMessage::Text(body)).await;
                        return Err(error);
                    }
                    Err(error) if websocket_expected_reset(&error) && !response_in_flight => {
                        let _closed_entry = entry.take();
                        refresh_target_before_connect = true;
                        continue;
                    }
                    Err(error) => {
                        let error = ProxyError::bad_gateway(error.to_string());
                        if response_in_flight {
                            let error_body = websocket_stream_error_body(
                                error.client_message(),
                                "upstream_stream_transport_error",
                            );
                            return terminate_responses_websocket_with_error(
                                &mut downstream,
                                &mut output_patcher,
                                mode,
                                state,
                                &execution,
                                &mut pending_lifecycle_messages,
                                error,
                                Some("transport_error"),
                                error_body,
                                Some(ProviderOutcome::NetworkFailure),
                            )
                            .await;
                        }
                        return Err(error);
                    }
                };
                let upstream_closed = matches!(message, TungsteniteMessage::Close(_));
                if upstream_closed {
                    let fallback_source = websocket_close_fallback_source(
                        &message,
                        entry_was_cached,
                    );
                    let _closed_entry = entry.take();
                    if let Some(source) = fallback_source
                        .filter(|_| {
                            response_in_flight
                                && responses_websocket_http_replay_allowed(
                                    mode,
                                    emitted_business_event,
                                    response_create_committed,
                                )
                        })
                    {
                        let body = active_response_body.as_ref().ok_or_else(|| {
                            ProxyError::bad_request(
                                "response.create body is unavailable for HTTP fallback",
                            )
                        })?;
                        pending_lifecycle_messages.clear();
                        let outcome = run_codex_websocket_http_fallback(
                            &mut downstream,
                            state,
                            &mut execution,
                            body,
                            grok_session_id.as_deref(),
                            grok_turn_index,
                            first_byte_timeout,
                            stream_idle_timeout,
                            source,
                            &mut auth_refresh_attempted,
                            &mut output_patcher,
                        )
                        .await?;
                        if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                            return Ok(());
                        }
                        response_in_flight = false;
                        response_create_committed = false;
                        account_in_flight_guard.take();
                        active_response_body = None;
                        refresh_target_before_connect = true;
                        continue;
                    }
                    if !response_in_flight {
                        refresh_target_before_connect = true;
                        continue;
                    }
                    return terminate_responses_websocket_without_terminal(
                        &mut downstream,
                        &mut output_patcher,
                        mode,
                        state,
                        &execution,
                        &mut pending_lifecycle_messages,
                        Some(&message),
                    )
                    .await;
                }
                let semantic_observation = if response_in_flight {
                    match classify_responses_websocket_message(&message) {
                        Ok(observation) => observation,
                        Err(error) => {
                            crate::metrics::record_proxy_semantic_guard(
                                "websocket",
                                "protocol_error",
                            );
                            if responses_websocket_http_replay_allowed(
                                mode,
                                emitted_business_event,
                                response_create_committed,
                            ) {
                                let failed = execution.runtime_stored_view();
                                record_provider_outcome(
                                    state,
                                    &failed,
                                    ProviderOutcome::Failure { status_code: 502 },
                                )
                                .await;
                                let _failed_entry = entry.take();
                                {
                                    pending_lifecycle_messages.clear();
                                    output_patcher.clear();
                                    let body = active_response_body.as_ref().ok_or_else(|| {
                                        ProxyError::bad_request(
                                            "response.create body is unavailable for HTTP fallback",
                                        )
                                    })?;
                                    let outcome = run_codex_websocket_http_fallback(
                                        &mut downstream,
                                        state,
                                        &mut execution,
                                        body,
                                        grok_session_id.as_deref(),
                                        grok_turn_index,
                                        first_byte_timeout,
                                        stream_idle_timeout,
                                        "semantic_protocol_error",
                                        &mut auth_refresh_attempted,
                                        &mut output_patcher,
                                    )
                                    .await?;
                                    if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                                        return Ok(());
                                    }
                                    response_in_flight = false;
                                    response_create_committed = false;
                                    account_in_flight_guard.take();
                                    active_response_body = None;
                                    upstream_read_deadline = None;
                                    refresh_target_before_connect = true;
                                    continue;
                                }
                            }
                            let error_body = websocket_stream_error_body(
                                error.client_message(),
                                "upstream_protocol_error",
                            );
                            return terminate_responses_websocket_with_error(
                                &mut downstream,
                                &mut output_patcher,
                                mode,
                                state,
                                &execution,
                                &mut pending_lifecycle_messages,
                                error,
                                None,
                                error_body,
                                Some(ProviderOutcome::Failure { status_code: 502 }),
                            )
                            .await;
                        }
                    }
                } else {
                    None
                };
                if let Some(observation) = &semantic_observation {
                    crate::metrics::record_proxy_semantic_guard(
                        "websocket",
                        observation.metric_kind(),
                    );
                }
                if matches!(semantic_observation, Some(SemanticObservation::Lifecycle))
                    && !emitted_business_event
                {
                    let buffered_bytes = pending_lifecycle_messages
                        .iter()
                        .map(websocket_message_payload_len)
                        .sum::<usize>()
                        .saturating_add(websocket_message_payload_len(&message));
                    if pending_lifecycle_messages.len()
                        >= MAX_RESPONSES_SEMANTIC_PRELUDE_MESSAGES
                        || buffered_bytes > MAX_RESPONSES_SEMANTIC_PRELUDE_BYTES
                    {
                        let error = ProxyError::bad_gateway(
                            "Responses websocket lifecycle prelude exceeded its bound",
                        );
                        let error_body = websocket_stream_error_body(
                            error.client_message(),
                            "upstream_protocol_error",
                        );
                        return terminate_responses_websocket_with_error(
                            &mut downstream,
                            &mut output_patcher,
                            mode,
                            state,
                            &execution,
                            &mut pending_lifecycle_messages,
                            error,
                            Some("protocol_error"),
                            error_body,
                            Some(ProviderOutcome::Failure { status_code: 502 }),
                        )
                        .await;
                    }
                    pending_lifecycle_messages.push(message);
                    continue;
                }
                if let Some(SemanticObservation::Failure(failure)) = &semantic_observation {
                    if failure.origin == FailureOrigin::Provider
                        && responses_websocket_http_replay_allowed(
                            mode,
                            emitted_business_event,
                            response_create_committed,
                        )
                    {
                        let failed = execution.runtime_stored_view();
                        if !semantic_provider_outcome_recorded {
                            record_provider_outcome(
                                state,
                                &failed,
                                ProviderOutcome::Failure { status_code: 502 },
                            )
                            .await;
                            semantic_provider_outcome_recorded = true;
                        }
                        let _failed_entry = entry.take();
                        {
                            pending_lifecycle_messages.clear();
                            output_patcher.clear();
                            let body = active_response_body.as_ref().ok_or_else(|| {
                                ProxyError::bad_request(
                                    "response.create body is unavailable for HTTP fallback",
                                )
                            })?;
                            let outcome = run_codex_websocket_http_fallback(
                                &mut downstream,
                                state,
                                &mut execution,
                                body,
                                grok_session_id.as_deref(),
                                grok_turn_index,
                                first_byte_timeout,
                                stream_idle_timeout,
                                "semantic_failure",
                                &mut auth_refresh_attempted,
                                &mut output_patcher,
                            )
                            .await?;
                            if outcome == CodexHttpFallbackOutcome::DownstreamClosed {
                                return Ok(());
                            }
                            response_in_flight = false;
                            response_create_committed = false;
                            account_in_flight_guard.take();
                            active_response_body = None;
                            upstream_read_deadline = None;
                            refresh_target_before_connect = true;
                            continue;
                        }
                    }
                }

                for pending in pending_lifecycle_messages.drain(..) {
                    if send_responses_websocket_message(
                        &mut downstream,
                        &mut output_patcher,
                        mode,
                        pending,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                }
                let semantic_terminal = match &semantic_observation {
                    Some(SemanticObservation::SuccessTerminal) => Some(SemanticTerminal::Success),
                    Some(SemanticObservation::IncompleteTerminal) => {
                        Some(SemanticTerminal::Incomplete)
                    }
                    Some(SemanticObservation::Failure(failure)) => {
                        Some(SemanticTerminal::Failure(failure.clone()))
                    }
                    Some(SemanticObservation::Lifecycle | SemanticObservation::Business) | None => {
                        None
                    }
                };
                let terminal = semantic_terminal.is_some()
                    || (semantic_observation.is_none()
                        && responses_websocket_response_is_terminal(&message));
                if terminal {
                    response_in_flight = false;
                    response_create_committed = false;
                    account_in_flight_guard.take();
                    active_response_body = None;
                    upstream_read_deadline = None;
                    match semantic_terminal {
                        Some(SemanticTerminal::Failure(failure))
                            if failure.origin == FailureOrigin::Provider =>
                        {
                            if !semantic_provider_outcome_recorded {
                                record_provider_outcome(
                                    state,
                                    &execution.runtime_stored_view(),
                                    ProviderOutcome::Failure { status_code: 502 },
                                )
                                .await;
                            }
                        }
                        Some(SemanticTerminal::Failure(_)) => {}
                        Some(SemanticTerminal::Success | SemanticTerminal::Incomplete) => {
                            record_provider_outcome(
                                state,
                                &execution.runtime_stored_view(),
                                ProviderOutcome::Success { status_code: 200 },
                            )
                            .await;
                        }
                        None => {}
                    }
                } else if semantic_observation
                    .as_ref()
                    .is_some_and(SemanticObservation::counts_as_business_output)
                    || (semantic_observation.is_none()
                        && response_in_flight
                        && matches!(
                            message,
                            TungsteniteMessage::Text(_) | TungsteniteMessage::Binary(_)
                        ))
                {
                    emitted_business_event = true;
                    upstream_read_deadline = stream_idle_timeout
                        .map(|timeout| tokio::time::Instant::now() + timeout);
                }
                let closes = send_responses_websocket_message(
                    &mut downstream,
                    &mut output_patcher,
                    mode,
                    message,
                )
                .await?;
                if closes || upstream_closed {
                    return Ok(());
                }
            }
        }
    }

    if !response_in_flight {
        if let (Some(pool_key), Some(entry)) = (pool_key, entry.take()) {
            responses_websocket_pool()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .release(pool_key, entry);
            crate::metrics::record_codex_websocket_cache("release");
            return Ok(());
        }
    }
    if let Some(mut entry) = entry {
        let _ = entry.socket.close(None).await;
    }
    Ok(())
}

fn acquire_cached_responses_websocket(
    pool_key: &Option<String>,
) -> (Option<CachedResponsesWebSocket>, bool) {
    let Some(pool_key) = pool_key.as_deref() else {
        return (None, false);
    };
    let cached = responses_websocket_pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .acquire(pool_key);
    if cached.is_some() {
        crate::metrics::record_codex_websocket_cache("hit");
        (cached, true)
    } else {
        crate::metrics::record_codex_websocket_cache("miss");
        (None, false)
    }
}

enum ResponsesWebsocketRead {
    Message(Option<Result<TungsteniteMessage, TungsteniteError>>),
    TimedOut,
}

async fn next_responses_websocket_message(
    entry: &mut Option<CachedResponsesWebSocket>,
    deadline: Option<tokio::time::Instant>,
) -> ResponsesWebsocketRead {
    match entry {
        Some(entry) => {
            if let Some(deadline) = deadline {
                match tokio::time::timeout_at(deadline, entry.socket.next()).await {
                    Ok(message) => ResponsesWebsocketRead::Message(message),
                    Err(_) => ResponsesWebsocketRead::TimedOut,
                }
            } else {
                ResponsesWebsocketRead::Message(entry.socket.next().await)
            }
        }
        None => std::future::pending().await,
    }
}

struct ResponsesWebsocketConnectFailure {
    error: ProxyError,
    fallback_source: Option<&'static str>,
}

fn grok_turn_index_from_headers(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-grok-turn-idx"))
        .and_then(|(_, value)| value.parse::<u64>().ok())
}

#[allow(clippy::too_many_arguments)] // The connect loop updates one authenticated WS target in place.
async fn connect_responses_websocket(
    state: &ServerState,
    execution: &mut ProviderExecution,
    mode: ResponsesWebsocketMode,
    session_id: Option<&str>,
    connect_timeout: Duration,
    headers: &mut Vec<(String, String)>,
    ws_url: &mut String,
    pool_key: &mut Option<String>,
    auth_refresh_attempted: &mut bool,
) -> Result<CachedResponsesWebSocket, ResponsesWebsocketConnectFailure> {
    loop {
        let mut request = ws_url.clone().into_client_request().map_err(|error| {
            ResponsesWebsocketConnectFailure {
                error: ProxyError::bad_gateway(format!(
                    "build responses websocket request: {error}"
                )),
                fallback_source: None,
            }
        })?;
        for (name, value) in headers.iter() {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(value) else {
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

        let connect = crate::infra::http::connect_websocket(request);
        let connect_result = match tokio::time::timeout(connect_timeout, connect).await {
            Ok(result) => result,
            Err(_) => {
                return Err(ResponsesWebsocketConnectFailure {
                    error: ProxyError {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        message: "responses websocket connect timeout".to_string(),
                    },
                    fallback_source: Some("connect_timeout"),
                })
            }
        };
        match connect_result {
            Ok((upstream, _)) => {
                if matches!(mode, ResponsesWebsocketMode::Grok) {
                    record_grok_capability_evidence(
                        state,
                        execution,
                        GrokAccountCapability::Websocket,
                    )
                    .await;
                }
                return Ok(CachedResponsesWebSocket {
                    socket: upstream,
                    created_at: Instant::now(),
                    last_used_at: Instant::now(),
                });
            }
            Err(error)
                if !*auth_refresh_attempted
                    && responses_websocket_http_error(&error)
                        .is_some_and(|(status, _, _)| status == StatusCode::UNAUTHORIZED) =>
            {
                let Some((provider_type, account_id)) = execution.managed_account_target() else {
                    return Err(ResponsesWebsocketConnectFailure {
                        error: responses_websocket_connect_error(state, execution, error).await,
                        fallback_source: None,
                    });
                };
                let refresh_result = state
                    .refresh_managed_account_now(provider_type, Some(account_id))
                    .await;
                if let Err(error) = refresh_result {
                    mark_managed_account_auth_cooldown(
                        state,
                        execution,
                        "websocket_forced_refresh_failed",
                    )
                    .await;
                    return Err(ResponsesWebsocketConnectFailure {
                        error: managed_account_refresh_error_to_proxy_error(error),
                        fallback_source: None,
                    });
                }
                *auth_refresh_attempted = true;
                record_forward_retry(ProxyRoute::CodexResponses, "auth", "websocket_unauthorized");
                let target = prepare_responses_websocket_target(
                    state,
                    execution,
                    mode,
                    session_id,
                    if matches!(mode, ResponsesWebsocketMode::Grok) {
                        grok_turn_index_from_headers(headers)
                    } else {
                        None
                    },
                )
                .await
                .map_err(|error| ResponsesWebsocketConnectFailure {
                    error,
                    fallback_source: None,
                })?;
                let forwarded_headers = codex_client_headers_from_owned(headers);
                *headers = target.headers;
                merge_owned_headers(headers, forwarded_headers);
                *ws_url = target.ws_url;
                *pool_key = target.pool_key;
            }
            Err(error) => {
                let fallback_source = websocket_connect_fallback_source(mode, &error);
                let error = responses_websocket_connect_error(state, execution, error).await;
                if error.status == StatusCode::UNAUTHORIZED && *auth_refresh_attempted {
                    mark_managed_account_auth_cooldown(
                        state,
                        execution,
                        "websocket_unauthorized_after_refresh",
                    )
                    .await;
                }
                return Err(ResponsesWebsocketConnectFailure {
                    error,
                    fallback_source,
                });
            }
        }
    }
}

fn websocket_connect_fallback_source(
    _mode: ResponsesWebsocketMode,
    error: &TungsteniteError,
) -> Option<&'static str> {
    match responses_websocket_http_error(error) {
        Some((status, _, _)) if status.is_server_error() => Some("handshake_server_error"),
        Some(_) => None,
        None => Some("connect_transport"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexHttpFallbackOutcome {
    Completed,
    DownstreamClosed,
}

enum CodexHttpRelayOutcome {
    Completed(SemanticTerminal),
    DownstreamClosed,
    ProviderFailureBeforeCommit {
        failure: SemanticFailure,
        replay_payloads: Vec<String>,
    },
    Interrupted {
        error: ProxyError,
        committed_business_event: bool,
        replay_payloads: Vec<String>,
    },
}

enum CodexHttpRelayEventOutcome {
    Continue,
    Terminal(SemanticTerminal),
    ProviderFailureBeforeCommit(SemanticFailure),
}

enum CodexHttpRelayFailure {
    Upstream(ProxyError),
    Client(ProxyError),
    DownstreamClosed,
}

struct PreparedCodexHttpFallbackTarget {
    http_client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    body: Bytes,
}

#[allow(clippy::too_many_arguments)] // Fallback preserves the active bridge, identity, timeout, and patch state.
async fn run_codex_websocket_http_fallback(
    downstream: &mut WebSocket,
    state: &ServerState,
    execution: &mut ProviderExecution,
    response_body: &Value,
    session_id: Option<&str>,
    grok_turn_index: Option<u64>,
    first_event_timeout: Option<Duration>,
    stream_idle_timeout: Option<Duration>,
    source: &'static str,
    auth_refresh_attempted: &mut bool,
    output_patcher: &mut CodexWebsocketOutputPatcher,
) -> Result<CodexHttpFallbackOutcome, ProxyError> {
    record_forward_retry(ProxyRoute::CodexResponses, "transport", source);
    crate::metrics::record_codex_websocket_fallback(source, "attempt");

    loop {
        let stored = execution.runtime_stored_view();
        let target = match prepare_codex_http_fallback_target(
            state,
            execution,
            response_body,
            session_id,
            grok_turn_index,
        )
        .await
        {
            Ok(target) => target,
            Err(error) => {
                if error.status.is_server_error() {
                    record_provider_outcome(
                        state,
                        &stored,
                        ProviderOutcome::Failure {
                            status_code: error.status.as_u16(),
                        },
                    )
                    .await;
                }
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    Vec::new(),
                    error,
                    "upstream_target_error",
                    Some("protocol_error"),
                )
                .await;
            }
        };
        let mut request = target
            .http_client
            .post(&target.url)
            .header(ACCEPT, "text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(target.body);
        for (name, value) in &target.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let first_event_deadline =
            first_event_timeout.map(|timeout| tokio::time::Instant::now() + timeout);
        let send_result = match first_event_deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, request.send())
                .await
                .map_err(|_| ()),
            None => Ok(request.send().await),
        };
        let mut upstream = match send_result {
            Ok(Ok(upstream)) => upstream,
            Ok(Err(error)) => {
                record_provider_outcome(state, &stored, ProviderOutcome::NetworkFailure).await;
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    Vec::new(),
                    ProxyError::bad_gateway(error),
                    "upstream_stream_transport_error",
                    Some("transport_error"),
                )
                .await;
            }
            Err(_) => {
                record_provider_outcome(state, &stored, ProviderOutcome::NetworkFailure).await;
                let error = ProxyError {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    message: format!(
                        "Responses HTTP fallback first event timeout after {}ms",
                        first_event_timeout
                            .expect("a deadline exists only when the timeout is enabled")
                            .as_millis()
                    ),
                };
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    Vec::new(),
                    error,
                    "upstream_stream_timeout",
                    Some("transport_error"),
                )
                .await;
            }
        };
        let status = upstream.status();
        if status == StatusCode::UNAUTHORIZED && !*auth_refresh_attempted {
            let Some((provider_type, account_id)) = execution.managed_account_target() else {
                let error = ProxyError {
                    status,
                    message: "Responses HTTP fallback upstream rejected authentication".to_string(),
                };
                record_provider_outcome(
                    state,
                    &stored,
                    ProviderOutcome::Failure {
                        status_code: status.as_u16(),
                    },
                )
                .await;
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    Vec::new(),
                    error,
                    "upstream_auth_error",
                    None,
                )
                .await;
            };
            drop(upstream);
            let refresh_result = state
                .refresh_managed_account_now(provider_type, Some(account_id))
                .await;
            if let Err(error) = refresh_result {
                mark_managed_account_auth_cooldown(
                    state,
                    execution,
                    "websocket_http_fallback_refresh_failed",
                )
                .await;
                let error = managed_account_refresh_error_to_proxy_error(error);
                record_provider_outcome(
                    state,
                    &stored,
                    ProviderOutcome::Failure {
                        status_code: error.status.as_u16(),
                    },
                )
                .await;
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    Vec::new(),
                    error,
                    "upstream_auth_refresh_error",
                    None,
                )
                .await;
            }
            *auth_refresh_attempted = true;
            record_forward_retry(
                ProxyRoute::CodexResponses,
                "auth",
                "websocket_http_fallback_unauthorized",
            );
            continue;
        }
        if status == StatusCode::UNAUTHORIZED && *auth_refresh_attempted {
            mark_managed_account_auth_cooldown(
                state,
                execution,
                "websocket_http_fallback_unauthorized_after_refresh",
            )
            .await;
        }
        if !status.is_success() {
            let response_headers = upstream.headers().clone();
            let body_result = match first_event_deadline {
                Some(deadline) => match tokio::time::timeout_at(
                    deadline,
                    crate::infra::http::read_response_body_limited(
                        &mut upstream,
                        PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
                    ),
                )
                .await
                {
                    Ok(body) => body.map_err(ProxyError::bad_gateway),
                    Err(_) => Err(ProxyError {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        message: format!(
                            "Responses HTTP fallback first event timeout after {}ms",
                            first_event_timeout
                                .expect("a deadline exists only when the timeout is enabled")
                                .as_millis()
                        ),
                    }),
                },
                None => crate::infra::http::read_response_body_limited(
                    &mut upstream,
                    PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
                )
                .await
                .map_err(ProxyError::bad_gateway),
            };
            let body_result = body_result.and_then(|body| {
                decode_response_body_for_proxy_with_limit(
                    &response_headers,
                    body,
                    PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
                )
                .map(|decoded| decoded.body)
            });
            let body = match body_result {
                Ok(body) => body,
                Err(error) => {
                    record_provider_outcome(state, &stored, ProviderOutcome::NetworkFailure).await;
                    let code = if error.status == StatusCode::GATEWAY_TIMEOUT {
                        "upstream_stream_timeout"
                    } else {
                        "upstream_stream_transport_error"
                    };
                    return terminate_codex_http_fallback_with_error(
                        downstream,
                        state,
                        execution,
                        output_patcher,
                        source,
                        Vec::new(),
                        error,
                        code,
                        Some("transport_error"),
                    )
                    .await;
                }
            };
            maybe_mark_upstream_rate_limited(state, execution, status, &response_headers, &body)
                .await;
            record_provider_outcome(
                state,
                &stored,
                provider_outcome_from_status(status.as_u16()),
            )
            .await;
            let message = if execution.driver_is("oauth.grok_responses")
                && is_grok_cli_version_gate_message(&upstream_error_message(&body))
            {
                record_grok_cli_version_gate(&stored, "websocket_http_fallback");
                grok_cli_version_gate_admin_message()
            } else {
                format!(
                    "Responses HTTP fallback upstream returned HTTP {}",
                    status.as_u16()
                )
            };
            let error = ProxyError { status, message };
            return terminate_codex_http_fallback_with_error(
                downstream,
                state,
                execution,
                output_patcher,
                source,
                Vec::new(),
                error,
                "upstream_http_error",
                None,
            )
            .await;
        }

        match relay_codex_http_fallback_stream(
            downstream,
            upstream,
            first_event_deadline,
            first_event_timeout,
            stream_idle_timeout,
            output_patcher,
        )
        .await
        {
            Ok(CodexHttpRelayOutcome::Completed(terminal)) => {
                match terminal {
                    SemanticTerminal::Failure(failure)
                        if failure.origin == FailureOrigin::Provider =>
                    {
                        record_provider_outcome(
                            state,
                            &stored,
                            ProviderOutcome::Failure { status_code: 502 },
                        )
                        .await;
                    }
                    SemanticTerminal::Failure(_) => {}
                    SemanticTerminal::Success | SemanticTerminal::Incomplete => {
                        record_provider_outcome(
                            state,
                            &stored,
                            ProviderOutcome::Success { status_code: 200 },
                        )
                        .await;
                    }
                }
                crate::metrics::record_codex_websocket_fallback(source, "success");
                return Ok(CodexHttpFallbackOutcome::Completed);
            }
            Ok(CodexHttpRelayOutcome::DownstreamClosed) => {
                crate::metrics::record_codex_websocket_fallback(source, "success");
                return Ok(CodexHttpFallbackOutcome::DownstreamClosed);
            }
            Ok(CodexHttpRelayOutcome::ProviderFailureBeforeCommit {
                failure,
                replay_payloads,
            }) => {
                record_provider_outcome(
                    state,
                    &stored,
                    ProviderOutcome::Failure { status_code: 502 },
                )
                .await;
                tracing::debug!(
                    error = %failure.display_message(),
                    "forwarding Responses semantic failure after HTTP fallback failover exhausted"
                );
                for payload in replay_payloads {
                    relay_codex_http_fallback_event(downstream, output_patcher, payload).await?;
                }
                crate::metrics::record_codex_websocket_fallback(source, "semantic_failure");
                return Ok(CodexHttpFallbackOutcome::Completed);
            }
            Ok(CodexHttpRelayOutcome::Interrupted {
                error,
                committed_business_event: _,
                replay_payloads,
            }) => {
                record_provider_outcome(state, &stored, ProviderOutcome::NetworkFailure).await;
                let error_code = if error.status == StatusCode::GATEWAY_TIMEOUT {
                    "upstream_stream_timeout"
                } else if error.status == StatusCode::PAYLOAD_TOO_LARGE {
                    "upstream_stream_too_large"
                } else {
                    "upstream_stream_error"
                };
                return terminate_codex_http_fallback_with_error(
                    downstream,
                    state,
                    execution,
                    output_patcher,
                    source,
                    replay_payloads,
                    error,
                    error_code,
                    Some("transport_error"),
                )
                .await;
            }
            Err(error) => {
                crate::metrics::record_codex_websocket_fallback(source, "error");
                return Err(error);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn terminate_codex_http_fallback_with_error(
    downstream: &mut WebSocket,
    state: &ServerState,
    execution: &ProviderExecution,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    source: &'static str,
    replay_payloads: Vec<String>,
    error: ProxyError,
    error_code: &'static str,
    metric_kind: Option<&'static str>,
) -> Result<CodexHttpFallbackOutcome, ProxyError> {
    crate::metrics::record_codex_websocket_fallback(source, "error");
    if let Some(metric_kind) = metric_kind {
        crate::metrics::record_proxy_semantic_guard("websocket_http_fallback", metric_kind);
    }
    let error_body = websocket_stream_error_body(error.client_message(), error_code);
    let mut pending_messages = replay_payloads
        .into_iter()
        .map(TungsteniteMessage::Text)
        .collect();
    let mode = if execution.driver_is("oauth.grok_responses") {
        ResponsesWebsocketMode::Grok
    } else {
        ResponsesWebsocketMode::Codex
    };
    terminate_responses_websocket_with_error(
        downstream,
        output_patcher,
        mode,
        state,
        execution,
        &mut pending_messages,
        error,
        None,
        error_body,
        None,
    )
    .await
    .map(|()| CodexHttpFallbackOutcome::Completed)
}

async fn prepare_codex_http_fallback_target(
    state: &ServerState,
    execution: &ProviderExecution,
    response_body: &Value,
    session_id: Option<&str>,
    grok_turn_index: Option<u64>,
) -> Result<PreparedCodexHttpFallbackTarget, ProxyError> {
    let stored = execution.runtime_stored_view();
    let grok = execution.driver_is("oauth.grok_responses");
    let responses_lite = !grok && codex_responses_lite_requested_value(response_body);
    let adapter = adapters::adapter_for(AppKind::Codex, stored.provider_type);
    let body = serde_json::to_vec(response_body)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("encode response.create body: {error}"))
        })?;
    let mut adapter_request =
        adapter.transform_request_for_route(body, &stored, ProxyRoute::CodexResponses, None)?;
    if !grok {
        adapter_request.body = normalize_codex_oauth_responses_body_bytes(
            &adapter_request.body,
            session_id,
            codex_image_tool_strip_policy(&stored),
        )?;
        if responses_lite {
            adapter_request.body =
                normalize_codex_responses_lite_body_bytes(&adapter_request.body, true, true)?;
        }
        adapter_request.body =
            super::remote_image::inline_codex_remote_images(&adapter_request.body).await?;
    }
    adapter_request.stream_requested = true;
    execution.enforce_model_policy(&mut adapter_request)?;
    let grok_contract = if grok {
        let mut contract = super::grok::apply_forward_contract(
            &mut adapter_request.body,
            &HeaderMap::new(),
            ProxyRoute::CodexResponses,
            None,
            session_id,
            None,
            grok_cli_profile(execution),
        )?;
        super::grok::set_forward_contract_turn_index(&mut contract, grok_turn_index);
        adapter_request.model = Some(contract.actual_model.clone());
        adapter_request.actual_model = Some(contract.actual_model.clone());
        Some(contract)
    } else {
        None
    };
    if !grok {
        execution.enforce_model_policy(&mut adapter_request)?;
    }
    execution.finalize_request(&mut adapter_request)?;
    let mut url = execution.resolve_endpoint(ProxyRoute::CodexResponses, None, &adapter_request)?;
    if grok {
        url = super::grok::chat_upstream_url(&url, grok_cli_profile(execution));
    }

    refresh_execution_managed_account_if_needed(state, execution).await?;
    ensure_managed_credential_persistence_available(state, execution)?;
    let accounts = state.accounts_snapshot().await;
    let mut headers = adapter.build_headers(AppKind::Codex, &stored, &accounts)?;
    headers.extend(adapter_request.upstream_headers.iter().cloned());
    if let Some(contract) = grok_contract {
        for (name, value) in contract.headers {
            replace_or_push_header(&mut headers, name, value);
        }
    } else {
        append_codex_oauth_session_headers(&mut headers, session_id);
        if responses_lite {
            replace_or_push_header(
                &mut headers,
                CODEX_RESPONSES_LITE_HEADER,
                "true".to_string(),
            );
        }
        crate::codex_identity::finalize_headers(&mut headers);
    }
    let mut headers = owned_headers(headers);
    let materialized_auth = execution.materialize_auth(&accounts)?;
    execution.apply_auth(&mut headers, &mut url, materialized_auth.as_ref())?;
    apply_account_header_overrides(&mut headers, &stored, &accounts)?;
    execution.finalize_outbound_identity(&mut headers)?;

    Ok(PreparedCodexHttpFallbackTarget {
        http_client: forward_http_client(state, &stored).await?,
        url,
        headers,
        body: adapter_request.body,
    })
}

fn responses_websocket_http_body(message: &TungsteniteMessage) -> Result<Value, ProxyError> {
    let bytes = match message {
        TungsteniteMessage::Text(text) => text.as_bytes(),
        TungsteniteMessage::Binary(bytes) => bytes.as_slice(),
        _ => {
            return Err(ProxyError::bad_request(
                "response.create must be a text or binary JSON frame",
            ))
        }
    };
    let mut value = serde_json::from_slice::<Value>(bytes).map_err(|error| {
        ProxyError::bad_request(format!("invalid response.create JSON: {error}"))
    })?;
    if value.get("type").and_then(Value::as_str) != Some("response.create") {
        return Err(ProxyError::bad_request(
            "Responses HTTP fallback requires a response.create frame",
        ));
    }
    if value.get("response").is_some() {
        return value
            .get("response")
            .filter(|response| response.is_object())
            .cloned()
            .ok_or_else(|| ProxyError::bad_request("response.create.response must be an object"));
    }
    let Some(object) = value.as_object_mut() else {
        return Err(ProxyError::bad_request(
            "response.create payload must be an object",
        ));
    };
    object.remove("type");
    Ok(value)
}

async fn prepare_codex_responses_websocket_request(
    message: TungsteniteMessage,
) -> Result<TungsteniteMessage, ProxyError> {
    if websocket_message_json_type(&message).as_deref() != Some("response.create") {
        return Ok(message);
    }
    let binary = matches!(message, TungsteniteMessage::Binary(_));
    let bytes = match &message {
        TungsteniteMessage::Text(text) => text.as_bytes(),
        TungsteniteMessage::Binary(bytes) => bytes.as_slice(),
        _ => return Ok(message),
    };
    let mut frame = serde_json::from_slice::<Value>(bytes).map_err(|error| {
        ProxyError::bad_request(format!("invalid response.create JSON: {error}"))
    })?;
    let target = if frame.get("response").is_some() {
        frame
            .get_mut("response")
            .filter(|response| response.is_object())
            .ok_or_else(|| ProxyError::bad_request("response.create.response must be an object"))?
    } else {
        &mut frame
    };
    sanitize_codex_oauth_request_body(target);
    strip_codex_oauth_unsupported_responses_fields(target);
    if codex_responses_lite_requested_value(target) {
        normalize_codex_responses_lite_body(target, false, false);
    }
    let target_body = serde_json::to_vec(target)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("encode response.create body: {error}"))
        })?;
    *target = serde_json::from_slice(
        &super::remote_image::inline_codex_remote_images(&target_body).await?,
    )
    .map_err(|error| {
        ProxyError::bad_request(format!("decode prepared response.create body: {error}"))
    })?;

    if binary {
        serde_json::to_vec(&frame)
            .map(TungsteniteMessage::Binary)
            .map_err(|error| {
                ProxyError::bad_request(format!("encode response.create frame: {error}"))
            })
    } else {
        serde_json::to_string(&frame)
            .map(TungsteniteMessage::Text)
            .map_err(|error| {
                ProxyError::bad_request(format!("encode response.create frame: {error}"))
            })
    }
}

async fn relay_codex_http_fallback_stream(
    downstream: &mut WebSocket,
    upstream: reqwest::Response,
    first_event_deadline: Option<tokio::time::Instant>,
    first_event_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    output_patcher: &mut CodexWebsocketOutputPatcher,
) -> Result<CodexHttpRelayOutcome, ProxyError> {
    let mut upstream = upstream.bytes_stream();
    let mut decoder = CodexHttpFallbackSseDecoder::default();
    let mut committed_business_event = false;
    let mut pending_lifecycle_payloads = Vec::new();
    let mut deadline = first_event_deadline;

    let relay_result: Result<CodexHttpRelayOutcome, CodexHttpRelayFailure> = async {
        loop {
            let current_deadline = deadline;
            let waiting_for_first_event = !committed_business_event;
            let upstream_next = async {
                if let Some(deadline) = current_deadline {
                    tokio::time::timeout_at(deadline, upstream.try_next())
                        .await
                        .map_err(|_| {
                            CodexHttpRelayFailure::Upstream(ProxyError {
                                status: StatusCode::GATEWAY_TIMEOUT,
                                message: if waiting_for_first_event {
                                    format!(
                                        "Responses HTTP fallback first event timeout after {}ms",
                                        first_event_timeout
                                            .expect(
                                                "a first-event deadline has a configured timeout",
                                            )
                                            .as_millis()
                                    )
                                } else {
                                    "Responses HTTP fallback stream idle timeout".to_string()
                                },
                            })
                        })?
                        .map_err(|error| {
                            CodexHttpRelayFailure::Upstream(ProxyError::bad_gateway(error))
                        })
                } else {
                    upstream.try_next().await.map_err(|error| {
                        CodexHttpRelayFailure::Upstream(ProxyError::bad_gateway(error))
                    })
                }
            };
            tokio::pin!(upstream_next);

            let next_chunk = tokio::select! {
                downstream_message = downstream.next() => {
                    let Some(message) = downstream_message else {
                        return Ok(CodexHttpRelayOutcome::DownstreamClosed);
                    };
                    let Ok(message) = message else {
                        return Ok(CodexHttpRelayOutcome::DownstreamClosed);
                    };
                    match message {
                        AxumWsMessage::Close(_) => {
                            return Ok(CodexHttpRelayOutcome::DownstreamClosed);
                        }
                        AxumWsMessage::Ping(bytes) => {
                            if downstream.send(AxumWsMessage::Pong(bytes)).await.is_err() {
                                return Ok(CodexHttpRelayOutcome::DownstreamClosed);
                            }
                        }
                        AxumWsMessage::Pong(_) => {}
                        AxumWsMessage::Text(_) | AxumWsMessage::Binary(_) => {
                            return Err(CodexHttpRelayFailure::Client(ProxyError::bad_request(
                                "responses websocket received a request while HTTP fallback was in flight",
                            )));
                        }
                    }
                    continue;
                }
                chunk = &mut upstream_next => chunk?,
            };

            match next_chunk {
                Some(chunk) => {
                    let payloads = decoder
                        .push(&chunk)
                        .map_err(CodexHttpRelayFailure::Upstream)?;
                    if let Some(outcome) = relay_codex_http_fallback_payloads(
                        downstream,
                        output_patcher,
                        payloads,
                        &mut pending_lifecycle_payloads,
                        &mut committed_business_event,
                    )
                    .await?
                    {
                        return Ok(outcome);
                    }
                    if committed_business_event {
                        deadline =
                            idle_timeout.map(|timeout| tokio::time::Instant::now() + timeout);
                    }
                }
                None => {
                    let payloads = decoder
                        .finish()
                        .map_err(CodexHttpRelayFailure::Upstream)?;
                    if let Some(outcome) = relay_codex_http_fallback_payloads(
                        downstream,
                        output_patcher,
                        payloads,
                        &mut pending_lifecycle_payloads,
                        &mut committed_business_event,
                    )
                    .await?
                    {
                        return Ok(outcome);
                    }
                    return Err(CodexHttpRelayFailure::Upstream(ProxyError::bad_gateway(
                        "Responses HTTP fallback stream ended before a terminal response event",
                    )));
                }
            }
        }
    }
    .await;

    match relay_result {
        Ok(outcome) => Ok(outcome),
        Err(CodexHttpRelayFailure::Upstream(error)) => Ok(CodexHttpRelayOutcome::Interrupted {
            error,
            committed_business_event,
            replay_payloads: pending_lifecycle_payloads,
        }),
        Err(CodexHttpRelayFailure::Client(error)) => Err(error),
        Err(CodexHttpRelayFailure::DownstreamClosed) => Ok(CodexHttpRelayOutcome::DownstreamClosed),
    }
}

async fn relay_codex_http_fallback_payloads(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    payloads: Vec<String>,
    pending_lifecycle_payloads: &mut Vec<String>,
    committed_business_event: &mut bool,
) -> Result<Option<CodexHttpRelayOutcome>, CodexHttpRelayFailure> {
    if !*committed_business_event {
        if let Some((failure_index, failure)) =
            codex_http_fallback_batch_provider_failure(&payloads)
                .map_err(CodexHttpRelayFailure::Upstream)?
        {
            crate::metrics::record_proxy_semantic_guard(
                "websocket_http_fallback",
                "provider_failure",
            );
            for payload in payloads.into_iter().take(failure_index + 1) {
                buffer_codex_http_fallback_semantic_prelude(pending_lifecycle_payloads, payload)
                    .map_err(CodexHttpRelayFailure::Upstream)?;
            }
            return Ok(Some(CodexHttpRelayOutcome::ProviderFailureBeforeCommit {
                failure,
                replay_payloads: std::mem::take(pending_lifecycle_payloads),
            }));
        }
    }

    for payload in payloads {
        match relay_codex_http_fallback_semantic_event(
            downstream,
            output_patcher,
            payload,
            pending_lifecycle_payloads,
            committed_business_event,
        )
        .await?
        {
            CodexHttpRelayEventOutcome::Continue => {}
            CodexHttpRelayEventOutcome::Terminal(terminal) => {
                return Ok(Some(CodexHttpRelayOutcome::Completed(terminal)));
            }
            CodexHttpRelayEventOutcome::ProviderFailureBeforeCommit(failure) => {
                return Ok(Some(CodexHttpRelayOutcome::ProviderFailureBeforeCommit {
                    failure,
                    replay_payloads: std::mem::take(pending_lifecycle_payloads),
                }));
            }
        }
    }
    Ok(None)
}

fn codex_http_fallback_batch_provider_failure(
    payloads: &[String],
) -> Result<Option<(usize, SemanticFailure)>, ProxyError> {
    if !response_semantics::semantic_guard_enabled() {
        return Ok(None);
    }
    for (index, payload) in payloads.iter().enumerate() {
        let value = serde_json::from_str::<Value>(payload).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Responses event: {error}"))
        })?;
        match response_semantics::classify_value(&value) {
            SemanticObservation::Failure(failure) if failure.origin == FailureOrigin::Provider => {
                return Ok(Some((index, failure)));
            }
            SemanticObservation::SuccessTerminal
            | SemanticObservation::IncompleteTerminal
            | SemanticObservation::Failure(_) => return Ok(None),
            SemanticObservation::Lifecycle | SemanticObservation::Business => {}
        }
    }
    Ok(None)
}

async fn relay_codex_http_fallback_semantic_event(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    payload: String,
    pending_lifecycle_payloads: &mut Vec<String>,
    committed_business_event: &mut bool,
) -> Result<CodexHttpRelayEventOutcome, CodexHttpRelayFailure> {
    if !response_semantics::semantic_guard_enabled() {
        let terminal = responses_payload_terminal(&payload);
        relay_codex_http_fallback_event(downstream, output_patcher, payload)
            .await
            .map_err(|_| CodexHttpRelayFailure::DownstreamClosed)?;
        *committed_business_event = true;
        return Ok(terminal
            .map(CodexHttpRelayEventOutcome::Terminal)
            .unwrap_or(CodexHttpRelayEventOutcome::Continue));
    }

    let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
        CodexHttpRelayFailure::Upstream(ProxyError::bad_gateway(format!(
            "invalid Responses event: {error}"
        )))
    })?;
    let observation = response_semantics::classify_value(&value);
    crate::metrics::record_proxy_semantic_guard(
        "websocket_http_fallback",
        observation.metric_kind(),
    );
    if matches!(observation, SemanticObservation::Lifecycle) && !*committed_business_event {
        buffer_codex_http_fallback_semantic_prelude(pending_lifecycle_payloads, payload)
            .map_err(CodexHttpRelayFailure::Upstream)?;
        return Ok(CodexHttpRelayEventOutcome::Continue);
    }
    if let SemanticObservation::Failure(failure) = &observation {
        if failure.origin == FailureOrigin::Provider && !*committed_business_event {
            buffer_codex_http_fallback_semantic_prelude(pending_lifecycle_payloads, payload)
                .map_err(CodexHttpRelayFailure::Upstream)?;
            return Ok(CodexHttpRelayEventOutcome::ProviderFailureBeforeCommit(
                failure.clone(),
            ));
        }
    }

    for pending in pending_lifecycle_payloads.drain(..) {
        relay_codex_http_fallback_event(downstream, output_patcher, pending)
            .await
            .map_err(|_| CodexHttpRelayFailure::DownstreamClosed)?;
    }
    relay_codex_http_fallback_event(downstream, output_patcher, payload)
        .await
        .map_err(|_| CodexHttpRelayFailure::DownstreamClosed)?;
    *committed_business_event |= observation.counts_as_business_output();
    Ok(match observation {
        SemanticObservation::SuccessTerminal => {
            CodexHttpRelayEventOutcome::Terminal(SemanticTerminal::Success)
        }
        SemanticObservation::IncompleteTerminal => {
            CodexHttpRelayEventOutcome::Terminal(SemanticTerminal::Incomplete)
        }
        SemanticObservation::Failure(failure) => {
            CodexHttpRelayEventOutcome::Terminal(SemanticTerminal::Failure(failure))
        }
        SemanticObservation::Lifecycle | SemanticObservation::Business => {
            CodexHttpRelayEventOutcome::Continue
        }
    })
}

fn buffer_codex_http_fallback_semantic_prelude(
    pending: &mut Vec<String>,
    payload: String,
) -> Result<(), ProxyError> {
    let buffered_bytes = pending
        .iter()
        .map(String::len)
        .sum::<usize>()
        .saturating_add(payload.len());
    if pending.len() >= MAX_RESPONSES_SEMANTIC_PRELUDE_MESSAGES
        || buffered_bytes > MAX_RESPONSES_SEMANTIC_PRELUDE_BYTES
    {
        return Err(ProxyError::bad_gateway(
            "Responses HTTP fallback lifecycle prelude exceeded its bound",
        ));
    }
    pending.push(payload);
    Ok(())
}

fn responses_payload_terminal(payload: &str) -> Option<SemanticTerminal> {
    let value = serde_json::from_str::<Value>(payload).ok()?;
    match response_semantics::classify_value(&value) {
        SemanticObservation::SuccessTerminal => Some(SemanticTerminal::Success),
        SemanticObservation::IncompleteTerminal => Some(SemanticTerminal::Incomplete),
        SemanticObservation::Failure(failure) => Some(SemanticTerminal::Failure(failure)),
        SemanticObservation::Lifecycle | SemanticObservation::Business => None,
    }
}

async fn relay_codex_http_fallback_event(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    payload: String,
) -> Result<bool, ProxyError> {
    let mut message = TungsteniteMessage::Text(payload);
    let terminal = responses_websocket_response_is_terminal(&message)
        || websocket_message_json_type(&message).as_deref() == Some("error");
    output_patcher.patch_message(&mut message);
    let Some(message) = tungstenite_message_to_axum_ws(message) else {
        return Ok(terminal);
    };
    downstream
        .send(message)
        .await
        .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
    Ok(terminal)
}

#[derive(Debug, Default)]
struct CodexHttpFallbackSseDecoder {
    buffer: Vec<u8>,
}

impl CodexHttpFallbackSseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProxyError> {
        self.buffer.extend_from_slice(chunk);
        self.drain(false)
    }

    fn finish(&mut self) -> Result<Vec<String>, ProxyError> {
        self.drain(true)
    }

    fn drain(&mut self, finish: bool) -> Result<Vec<String>, ProxyError> {
        self.drain_with_limit(finish, MAX_CODEX_HTTP_FALLBACK_SSE_EVENT_BYTES)
    }

    fn drain_with_limit(
        &mut self,
        finish: bool,
        max_event_bytes: usize,
    ) -> Result<Vec<String>, ProxyError> {
        let mut payloads = Vec::new();
        while let Some((event_end, delimiter_len)) = codex_sse_event_boundary(&self.buffer) {
            if event_end > max_event_bytes {
                return Err(codex_http_fallback_sse_event_too_large());
            }
            let event = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + delimiter_len);
            if let Some(payload) = codex_sse_json_payload(&event)? {
                payloads.push(payload);
            }
        }
        let pending_event_bytes = if finish {
            self.buffer.len()
        } else {
            self.buffer
                .len()
                .saturating_sub(codex_sse_delimiter_prefix_len(&self.buffer))
        };
        if pending_event_bytes > max_event_bytes {
            return Err(codex_http_fallback_sse_event_too_large());
        }
        if finish && !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            if let Some(payload) = codex_sse_json_payload(&event)? {
                payloads.push(payload);
            }
        }
        Ok(payloads)
    }
}

fn codex_http_fallback_sse_event_too_large() -> ProxyError {
    ProxyError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        message: "Responses HTTP fallback SSE event exceeded 128 MiB".to_string(),
    }
}

fn codex_sse_delimiter_prefix_len(buffer: &[u8]) -> usize {
    [b"\r\n\r".as_slice(), b"\r\n", b"\r", b"\n"]
        .into_iter()
        .find(|prefix| buffer.ends_with(prefix))
        .map_or(0, <[u8]>::len)
}

fn codex_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            return Some((index, 2));
        }
        if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

fn codex_sse_json_payload(event: &[u8]) -> Result<Option<String>, ProxyError> {
    let event = std::str::from_utf8(event)
        .map_err(|error| ProxyError::bad_gateway(format!("Codex SSE is not UTF-8: {error}")))?;
    let data = event
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>();
    let payload = if data.is_empty() {
        let event = event.trim();
        if !event.starts_with('{') {
            return Ok(None);
        }
        event.to_string()
    } else {
        data.join("\n").trim().to_string()
    };
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str::<Value>(&payload).map_err(|error| {
        ProxyError::bad_gateway(format!("Codex SSE data is not valid JSON: {error}"))
    })?;
    Ok(Some(payload))
}

fn responses_websocket_request_starts_response(message: &TungsteniteMessage) -> bool {
    websocket_message_json_type(message).as_deref() == Some("response.create")
}

fn responses_websocket_response_is_terminal(message: &TungsteniteMessage) -> bool {
    matches!(
        websocket_message_json_type(message).as_deref(),
        Some("response.completed" | "response.failed" | "response.incomplete" | "error")
    )
}

fn websocket_message_json_type(message: &TungsteniteMessage) -> Option<String> {
    let bytes = match message {
        TungsteniteMessage::Text(text) => text.as_bytes(),
        TungsteniteMessage::Binary(bytes) => bytes.as_slice(),
        _ => return None,
    };
    serde_json::from_slice::<Value>(bytes)
        .ok()?
        .get("type")?
        .as_str()
        .map(str::to_string)
}

fn classify_responses_websocket_message(
    message: &TungsteniteMessage,
) -> Result<Option<SemanticObservation>, ProxyError> {
    if !response_semantics::semantic_guard_enabled() {
        return Ok(None);
    }
    let bytes = match message {
        TungsteniteMessage::Text(text) => text.as_bytes(),
        TungsteniteMessage::Binary(bytes) => bytes.as_slice(),
        _ => return Ok(None),
    };
    response_semantics::classify_json_document(bytes)
        .map(Some)
        .map_err(ProxyError::bad_gateway)
}

fn semantic_prelude_decision(observations: &[SemanticObservation]) -> Option<SemanticObservation> {
    observations
        .iter()
        .find(|observation| {
            matches!(
                observation,
                SemanticObservation::Failure(SemanticFailure {
                    origin: FailureOrigin::Provider,
                    ..
                })
            )
        })
        .or_else(|| {
            observations
                .iter()
                .find(|observation| observation.commits_downstream())
        })
        .cloned()
}

fn websocket_message_payload_len(message: &TungsteniteMessage) -> usize {
    match message {
        TungsteniteMessage::Text(text) => text.len(),
        TungsteniteMessage::Binary(bytes) => bytes.len(),
        _ => 0,
    }
}

async fn send_responses_websocket_message(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    mode: ResponsesWebsocketMode,
    mut message: TungsteniteMessage,
) -> Result<bool, ProxyError> {
    if matches!(mode, ResponsesWebsocketMode::Codex) {
        output_patcher.patch_message(&mut message);
    }
    let Some(message) = tungstenite_message_to_axum_ws(message) else {
        return Ok(false);
    };
    let closes = matches!(message, AxumWsMessage::Close(_));
    downstream
        .send(message)
        .await
        .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
    Ok(closes)
}

async fn terminate_responses_websocket_without_terminal(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    mode: ResponsesWebsocketMode,
    state: &ServerState,
    execution: &ProviderExecution,
    pending_lifecycle_messages: &mut Vec<TungsteniteMessage>,
    close_message: Option<&TungsteniteMessage>,
) -> Result<(), ProxyError> {
    const MESSAGE: &str = "upstream websocket closed before a terminal response event";
    let size_error = close_message.is_some_and(websocket_close_is_size);
    let error = if size_error {
        ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "upstream websocket message too big".to_string(),
        }
    } else {
        ProxyError::bad_gateway(MESSAGE)
    };
    let error_body = if size_error {
        websocket_message_too_big_error_body()
    } else {
        websocket_missing_terminal_error_body()
    };
    let outcome = if size_error {
        ProviderOutcome::Failure { status_code: 413 }
    } else {
        ProviderOutcome::Failure { status_code: 502 }
    };
    terminate_responses_websocket_with_error(
        downstream,
        output_patcher,
        mode,
        state,
        execution,
        pending_lifecycle_messages,
        error,
        Some("missing_terminal"),
        error_body,
        Some(outcome),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn terminate_responses_websocket_with_error(
    downstream: &mut WebSocket,
    output_patcher: &mut CodexWebsocketOutputPatcher,
    mode: ResponsesWebsocketMode,
    state: &ServerState,
    execution: &ProviderExecution,
    pending_lifecycle_messages: &mut Vec<TungsteniteMessage>,
    error: ProxyError,
    metric_kind: Option<&'static str>,
    error_body: String,
    provider_outcome: Option<ProviderOutcome>,
) -> Result<(), ProxyError> {
    if let Some(metric_kind) = metric_kind {
        crate::metrics::record_proxy_semantic_guard("websocket", metric_kind);
    }
    if let Some(outcome) = provider_outcome {
        record_provider_outcome(state, &execution.runtime_stored_view(), outcome).await;
    }

    for pending in pending_lifecycle_messages.drain(..) {
        if send_responses_websocket_message(downstream, output_patcher, mode, pending).await? {
            return Ok(());
        }
    }
    if send_responses_websocket_message(
        downstream,
        output_patcher,
        mode,
        TungsteniteMessage::Text(error_body),
    )
    .await?
    {
        return Ok(());
    }
    let _ = send_responses_websocket_message(
        downstream,
        output_patcher,
        mode,
        TungsteniteMessage::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: CloseCode::Error,
            reason: "upstream response ended without terminal event".into(),
        })),
    )
    .await?;
    Err(error)
}

async fn responses_websocket_connect_error(
    state: &ServerState,
    execution: &ProviderExecution,
    error: TungsteniteError,
) -> ProxyError {
    let Some((status, headers, body)) = responses_websocket_http_error(&error) else {
        return ProxyError::bad_gateway(format!("responses websocket connect: {error}"));
    };
    maybe_mark_upstream_rate_limited(state, execution, status, &headers, &body).await;
    if execution.driver_is("oauth.grok_responses") {
        let stored = execution.runtime_stored_view();
        maybe_update_grok_entitlement(state, &stored, &headers).await;
        maybe_mark_grok_cooldown(state, &stored, status, &headers).await;
        if is_grok_cli_version_gate_message(&upstream_error_message(&body)) {
            record_grok_cli_version_gate(&stored, "websocket_handshake");
            return ProxyError {
                status,
                message: grok_cli_version_gate_admin_message(),
            };
        }
    }
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
            Some("response.failed") | Some("response.incomplete") | Some("error") => self.clear(),
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
        if value.get("response").is_some() {
            value.get_mut("response")
        } else {
            Some(value)
        }
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

fn websocket_read_fallback_source(error: &TungsteniteError, cached: bool) -> Option<&'static str> {
    if websocket_message_too_big(error) {
        return Some("message_too_big");
    }
    if websocket_expected_reset(error) {
        return Some(if cached {
            "cached_stale"
        } else {
            "read_failure"
        });
    }
    if matches!(
        error,
        TungsteniteError::ConnectionClosed
            | TungsteniteError::AlreadyClosed
            | TungsteniteError::Io(_)
    ) {
        return Some(if cached {
            "cached_stale"
        } else {
            "read_failure"
        });
    }
    None
}

fn websocket_close_fallback_source(
    message: &TungsteniteMessage,
    cached: bool,
) -> Option<&'static str> {
    let TungsteniteMessage::Close(frame) = message else {
        return None;
    };
    let Some(frame) = frame else {
        return Some(if cached {
            "cached_stale"
        } else {
            "closed_before_event"
        });
    };
    let code: u16 = frame.code.into();
    if code == 1009 {
        return Some("message_too_big");
    }
    matches!(code, 1000 | 1001 | 1006 | 1011 | 1012 | 1013).then_some(if cached {
        "cached_stale"
    } else {
        "closed_before_event"
    })
}

fn websocket_close_is_size(message: &TungsteniteMessage) -> bool {
    matches!(
        message,
        TungsteniteMessage::Close(Some(frame)) if frame.code == CloseCode::Size
    )
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

fn websocket_missing_terminal_error_body() -> String {
    websocket_stream_error_body(
        "upstream websocket closed before a terminal response event",
        "upstream_closed_before_terminal",
    )
}

fn websocket_stream_error_body(message: &str, code: &str) -> String {
    json!({
        "type": "error",
        "error": {
            "message": message,
            "type": "upstream_error",
            "code": code
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
        .find_account_for_provider(provider_type, Some(requested_account_id))
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

async fn mark_managed_account_auth_cooldown(
    state: &ServerState,
    execution: &ProviderExecution,
    source: &'static str,
) {
    let Some((provider_type, requested_account_id)) = execution.managed_account_target() else {
        return;
    };
    let Some(account_id) = state
        .find_account_for_provider(provider_type, Some(requested_account_id))
        .await
        .map(|account| account.id)
    else {
        return;
    };
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let until = now.saturating_add(DEFAULT_UPSTREAM_AUTH_FAILURE_COOLDOWN_MS);
    state
        .mark_account_rate_limited_until(
            &account_id,
            until,
            Some(format!(
                "upstream authentication remained unauthorized after refresh ({source})"
            )),
        )
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
    account_in_flight_guard: Option<AccountInFlightGuard>,
    share_invocation_guard: Option<ShareInFlightGuard>,
    started: Instant,
}

struct ClaudeDeepSeekForwardOptions {
    state: ServerState,
    execution: ProviderExecution,
    stored: StoredProvider,
    body: Bytes,
    request_context: UsageLogContext,
    account_in_flight_guard: Option<AccountInFlightGuard>,
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
        account_in_flight_guard,
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
        responses_tool_context: Default::default(),
        claude_tool_name_map: Default::default(),
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
            let _account_in_flight_guard = account_in_flight_guard;
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
        account_in_flight_guard,
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
        responses_tool_context: Default::default(),
        claude_tool_name_map: Default::default(),
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
            account_in_flight_guard,
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
    account_in_flight_guard: Option<AccountInFlightGuard>,
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
        account_in_flight_guard,
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
        let _account_in_flight_guard = account_in_flight_guard;
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
                Some("client_cancelled"),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                usage,
            )
            .await;
            crate::metrics::record_stream_client_cancelled(stored.app.as_str());
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

fn select_and_acquire_account_in_flight(
    state: &ServerState,
    accounts: &crate::domain::accounts::store::AccountStore,
    mut select: impl FnMut(&AccountInFlightSnapshot) -> Result<ProviderExecution, ProxyError>,
) -> Result<(ProviderExecution, Option<AccountInFlightGuard>), ProxyError> {
    const MAX_SELECTION_ATTEMPTS: usize = 3;

    for attempt in 0..MAX_SELECTION_ATTEMPTS {
        let snapshot = state.account_in_flight.snapshot();
        let execution = select(&snapshot)?;
        match try_acquire_account_in_flight(state, &execution.stored, accounts, &snapshot) {
            AccountInFlightAcquire::Acquired(guard) => return Ok((execution, Some(guard))),
            AccountInFlightAcquire::NotManaged => return Ok((execution, None)),
            AccountInFlightAcquire::Saturated if attempt + 1 < MAX_SELECTION_ATTEMPTS => {}
            AccountInFlightAcquire::Saturated => {
                return Err(account_concurrency_proxy_error(&execution.stored));
            }
        }
    }
    unreachable!("account selection attempts are bounded and non-zero")
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
    ProxyError::rate_limited(
        format!(
            "provider {} account concurrency limit has been reached",
            stored.provider.id
        ),
        1,
    )
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
    if status == StatusCode::TOO_MANY_REQUESTS {
        ProxyError::rate_limited(rejection.formatted_message(), 1)
    } else {
        ProxyError {
            status,
            message: rejection.formatted_message(),
        }
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
        .record_share_invocation_result(
            share_id,
            user_email,
            share_usage_tokens(usage),
            crate::infra::time::now_ms() as i64,
        )
        .await;
}

async fn record_share_supplemental_usage(
    state: &ServerState,
    share_id: Option<&str>,
    user_email: Option<&str>,
    usage: TokenUsage,
) {
    let Some(share_id) = share_id else {
        return;
    };
    state
        .record_share_supplemental_usage(
            share_id,
            user_email,
            share_usage_tokens(usage),
            crate::infra::time::now_ms() as i64,
        )
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
    let bytes = rewrite_error_message_body(&body, &admin_message, "claude_code_version_gate")
        .unwrap_or_else(|| Bytes::from(admin_message));
    (bytes, true)
}

fn maybe_rewrite_grok_cli_version_gate_body(
    status: StatusCode,
    stored: &StoredProvider,
    body: Bytes,
) -> (Bytes, bool) {
    if stored.provider_type != ProviderType::GrokOAuth
        || !(status.is_client_error() || status.is_server_error())
    {
        return (body, false);
    }
    let upstream_message = upstream_error_message(&body);
    if !is_grok_cli_version_gate_message(&upstream_message) {
        return (body, false);
    }

    record_grok_cli_version_gate(stored, "http");
    let admin_message = grok_cli_version_gate_admin_message();
    let bytes = rewrite_error_message_body(&body, &admin_message, "grok_cli_version_gate")
        .unwrap_or_else(|| Bytes::from(admin_message));
    (bytes, true)
}

fn is_grok_cli_version_gate_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("x-grok-client-version")
        || message.contains("grok-shell@latest")
        || ((message.contains("grok") || message.contains("client version"))
            && [
                "out of date",
                "outdated",
                "too old",
                "minimum version",
                "unsupported version",
                "please update",
                "please upgrade",
            ]
            .iter()
            .any(|marker| message.contains(marker)))
}

fn record_grok_cli_version_gate(stored: &StoredProvider, transport: &'static str) {
    tracing::error!(
        provider_id = %stored.provider.id,
        cli_version = %crate::domain::grok_cli::grok_cli_version(),
        transport,
        "Grok OAuth upstream rejected the advertised Grok CLI version; set CC_SWITCH_GROK_CLI_VERSION or CC_SWITCH_GROK_CLI_USER_AGENT to a currently accepted value"
    );
    crate::metrics::record_grok_cli_version_gate();
}

fn grok_cli_version_gate_admin_message() -> String {
    format!(
        "Grok OAuth upstream rejected the advertised Grok CLI version (currently {}). cc-switch-server admin: bump CC_SWITCH_GROK_CLI_VERSION or CC_SWITCH_GROK_CLI_USER_AGENT to a currently accepted Grok CLI identity.",
        crate::domain::grok_cli::grok_cli_version()
    )
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

fn rewrite_error_message_body(body: &[u8], message: &str, error_type: &str) -> Option<Bytes> {
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
                "type": error_type,
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

fn claude_semantic_error_status(error_type: &str) -> StatusCode {
    match error_type {
        "invalid_request_error" => StatusCode::BAD_REQUEST,
        "authentication_error" => StatusCode::UNAUTHORIZED,
        "permission_error" => StatusCode::FORBIDDEN,
        "not_found_error" => StatusCode::NOT_FOUND,
        "request_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
        "overloaded_error" => StatusCode::from_u16(529).expect("529 is a valid status code"),
        _ => StatusCode::BAD_GATEWAY,
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
        .refresh_managed_account_if_needed(provider_type, Some(account_id))
        .await
        .map_err(managed_account_refresh_error_to_proxy_error)?;
    ensure_managed_credential_persistence_available(state, execution)
}

fn ensure_managed_credential_persistence_available(
    state: &ServerState,
    execution: &ProviderExecution,
) -> Result<(), ProxyError> {
    if execution.managed_account_target().is_some() && state.credential_persistence_degraded() {
        return Err(ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "managed account credentials are waiting for durable persistence".to_string(),
        });
    }
    Ok(())
}

async fn ensure_grok_account_capability(
    state: &ServerState,
    execution: &ProviderExecution,
    capability: GrokAccountCapability,
) -> Result<(), ProxyError> {
    let Some((ProviderType::GrokOAuth, account_id)) = execution.managed_account_target() else {
        return Err(ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Grok OAuth capability requires a bound managed account".to_string(),
        });
    };
    let account = state
        .find_account_for_provider(ProviderType::GrokOAuth, Some(account_id))
        .await
        .ok_or_else(|| ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Grok OAuth capability account is unavailable".to_string(),
        })?;
    if grok_account_capability_enabled(&account, capability) {
        return Ok(());
    }
    Err(ProxyError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: format!(
            "Grok OAuth {} capability is unverified; explicitly enable it with CC_SWITCH_GROK_OAUTH_CAPABILITIES or persist successful probe evidence",
            capability.as_str()
        ),
    })
}

fn grok_media_capability(method: &Method, upstream_path: &str) -> GrokAccountCapability {
    if upstream_path.contains("/images/edits") {
        GrokAccountCapability::ImageEdit
    } else if upstream_path.contains("/videos/")
        || (method == Method::POST && upstream_path.contains("/videos"))
    {
        GrokAccountCapability::VideoGeneration
    } else {
        GrokAccountCapability::ImageGeneration
    }
}

async fn record_grok_capability_evidence(
    state: &ServerState,
    execution: &ProviderExecution,
    capability: GrokAccountCapability,
) {
    let Some((ProviderType::GrokOAuth, account_id)) = execution.managed_account_target() else {
        return;
    };
    if let Err(error) = state
        .record_grok_capability_evidence(account_id, capability, "upstream_success")
        .await
    {
        tracing::warn!(
            capability = capability.as_str(),
            error = %error,
            "persist Grok OAuth capability evidence failed"
        );
    }
}

async fn next_claude_transport_attempt(
    _state: &ServerState,
    route: ProxyRoute,
    _headers: &HeaderMap,
    _request_context: &UsageLogContext,
    attempt_context: &ForwardAttemptContext,
    failed: &ProviderExecution,
    reason: &'static str,
) -> Option<ForwardAttemptContext> {
    if !attempt_context.retry_allowed() {
        return None;
    }
    let replay_safe = route == ProxyRoute::ClaudeCountTokens
        || (route == ProxyRoute::ClaudeMessages
            && reason == "connect_error"
            && attempt_context.attempt == 0);
    if replay_safe {
        record_forward_retry(route, "transport", reason);
        return Some(attempt_context.next(failed, attempt_context.body_retry_stage));
    }
    None
}

async fn next_unauthorized_attempt(
    state: &ServerState,
    route: ProxyRoute,
    headers: &HeaderMap,
    request_context: &UsageLogContext,
    attempt_context: &ForwardAttemptContext,
    execution: &ProviderExecution,
    stored: &StoredProvider,
) -> Result<Option<ForwardAttemptContext>, ProxyError> {
    if !supports_forced_auth_refresh(route, execution) {
        return Ok(None);
    }

    if !attempt_context.auth_refresh_attempted && attempt_context.retry_allowed() {
        let Some((provider_type, account_id)) = execution.managed_account_target() else {
            return Ok(None);
        };
        if let Err(error) = state
            .refresh_managed_account_now(provider_type, Some(account_id))
            .await
        {
            mark_managed_account_auth_cooldown(state, execution, "forced_refresh_failed").await;
            if !request_is_provider_pinned(headers, request_context) {
                if let Some(next_attempt) = next_provider_failover(
                    state,
                    route,
                    attempt_context,
                    execution,
                    "auth_refresh_failed",
                )
                .await
                {
                    record_provider_outcome(
                        state,
                        stored,
                        ProviderOutcome::Failure { status_code: 401 },
                    )
                    .await;
                    return Ok(Some(next_attempt));
                }
            }
            return Err(managed_account_refresh_error_to_proxy_error(error));
        }
        if route == ProxyRoute::ClaudeCountTokens {
            crate::metrics::record_claude_count_tokens_outcome("auth_refresh");
        }
        record_forward_retry(route, "auth", "unauthorized");
        return Ok(Some(attempt_context.after_auth_refresh(execution)));
    }

    if attempt_context.auth_refresh_attempted {
        mark_managed_account_auth_cooldown(state, execution, "unauthorized_after_refresh").await;
        if !request_is_provider_pinned(headers, request_context) {
            if let Some(next_attempt) = next_provider_failover(
                state,
                route,
                attempt_context,
                execution,
                "unauthorized_after_refresh",
            )
            .await
            {
                record_provider_outcome(
                    state,
                    stored,
                    ProviderOutcome::Failure { status_code: 401 },
                )
                .await;
                return Ok(Some(next_attempt));
            }
        }
    }

    Ok(None)
}

async fn next_provider_failover(
    state: &ServerState,
    route: ProxyRoute,
    attempt_context: &ForwardAttemptContext,
    failed: &ProviderExecution,
    reason: &'static str,
) -> Option<ForwardAttemptContext> {
    if matches!(route.app(), AppKind::Claude | AppKind::Codex)
        || failed.driver_is("oauth.openai_codex")
        || !attempt_context.retry_allowed()
    {
        return None;
    }
    if failed.driver_is("oauth.grok_responses") {
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
        "switching request to failover Provider"
    );
    record_forward_retry(route, "provider", reason);
    Some(attempt_context.after_provider_failover(failed, &next))
}

fn request_is_provider_pinned(headers: &HeaderMap, request_context: &UsageLogContext) -> bool {
    request_context.share_id.is_some()
        || headers
            .get("x-cc-provider-id")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.trim().is_empty())
}

fn supports_forced_auth_refresh(route: ProxyRoute, execution: &ProviderExecution) -> bool {
    if execution.driver_is("oauth.grok_responses") {
        return true;
    }
    match route {
        ProxyRoute::ClaudeMessages | ProxyRoute::ClaudeCountTokens => {
            execution.driver_is("oauth.claude_messages")
        }
        ProxyRoute::CodexChatCompletions
        | ProxyRoute::CodexResponses
        | ProxyRoute::CodexResponsesCompact => execution.driver_is("oauth.openai_codex"),
        ProxyRoute::Gemini => false,
    }
}

fn record_forward_retry(route: ProxyRoute, stage: &'static str, source: &'static str) {
    crate::metrics::record_forward_retry(route.app().as_str(), stage, source);
    if route.app() == AppKind::Claude {
        crate::metrics::record_claude_retry(stage, source);
    }
}

fn managed_account_id(stored: &StoredProvider) -> Option<&str> {
    stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
}

fn grok_cli_profile(execution: &ProviderExecution) -> bool {
    execution.driver_is("oauth.grok_responses")
        && matches!(
            execution.managed_account_target(),
            Some((ProviderType::GrokOAuth, _))
        )
}

fn grok_tenant_scope(context: &UsageLogContext, stored: &StoredProvider) -> Option<String> {
    grok_tenant_scope_parts(
        context.share_id.as_deref(),
        context.user_email.as_deref(),
        stored,
    )
}

fn grok_tenant_scope_parts(
    share_id: Option<&str>,
    user_email: Option<&str>,
    stored: &StoredProvider,
) -> Option<String> {
    if stored.provider_type != ProviderType::GrokOAuth {
        return None;
    }
    Some(format!(
        "share={}|user={}|provider={}|account={}",
        share_id.unwrap_or("direct"),
        user_email.unwrap_or("anonymous"),
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

pub(crate) fn managed_account_refresh_error_to_proxy_error(
    error: ManagedAccountRefreshError,
) -> ProxyError {
    match error {
        ManagedAccountRefreshError::Conflict { provider_type } => ProxyError::conflict(format!(
            "{} account refresh is already in progress",
            provider_type.as_str()
        )),
        ManagedAccountRefreshError::NotFound => ProxyError::not_found("managed account not found"),
        ManagedAccountRefreshError::InactiveCodexAccount => ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Codex OAuth account is not active".to_string(),
        },
        ManagedAccountRefreshError::CredentialPersistenceDegraded => ProxyError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "managed account credentials are waiting for durable persistence".to_string(),
        },
        ManagedAccountRefreshError::Refresh {
            status_code,
            message: _,
            retry_after_ms,
        } => {
            let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);
            let message = match status_code {
                400 | 401 | 403 => {
                    "managed account refresh was rejected; sign in again".to_string()
                }
                429 => "managed account refresh was rate limited; retry later".to_string(),
                _ => "managed account refresh failed".to_string(),
            };
            if status == StatusCode::TOO_MANY_REQUESTS {
                let seconds = retry_after_ms
                    .and_then(|value| u64::try_from(value.max(0)).ok())
                    .unwrap_or(1_000)
                    .saturating_add(999)
                    / 1_000;
                ProxyError::rate_limited(message, seconds.max(1))
            } else {
                ProxyError { status, message }
            }
        }
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
    let Some(account_id) = managed_account_id(stored) else {
        return Ok(());
    };
    let Some(account) = accounts.find_for_provider(stored.provider_type, Some(account_id)) else {
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
    if provider_type == ProviderType::GrokOAuth
        && matches!(
            normalized.as_str(),
            "x-xai-token-auth"
                | "x-grok-client-identifier"
                | "x-grok-client-version"
                | "x-grok-client-surface"
                | "x-authenticateresponse"
                | "x-grok-conv-id"
                | "x-grok-cache-identity"
                | "x-grok-turn-idx"
        )
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

const CODEX_ALLOWED_CLIENT_REQUEST_HEADERS: &[&str] = &[
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-responsesapi-include-timing-metrics",
    "x-oai-attestation",
];

fn append_codex_client_request_headers(
    target: &mut Vec<(&'static str, String)>,
    client: &HeaderMap,
    responses_lite: bool,
) {
    for &name in CODEX_ALLOWED_CLIENT_REQUEST_HEADERS {
        if let Some(value) = optional_header(client, name) {
            replace_or_push_header(target, name, value.to_string());
        }
    }
    if responses_lite {
        replace_or_push_header(target, CODEX_RESPONSES_LITE_HEADER, "true".to_string());
    }
}

fn append_codex_client_request_headers_owned(
    target: &mut Vec<(String, String)>,
    client: &HeaderMap,
    responses_lite: bool,
) {
    let forwarded = CODEX_ALLOWED_CLIENT_REQUEST_HEADERS
        .iter()
        .filter_map(|name| {
            optional_header(client, name).map(|value| ((*name).to_string(), value.to_string()))
        })
        .chain(
            responses_lite.then(|| (CODEX_RESPONSES_LITE_HEADER.to_string(), "true".to_string())),
        )
        .collect();
    merge_owned_headers(target, forwarded);
}

fn codex_client_headers_from_owned(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            CODEX_ALLOWED_CLIENT_REQUEST_HEADERS
                .iter()
                .any(|allowed| name.eq_ignore_ascii_case(allowed))
        })
        .cloned()
        .collect()
}

fn merge_owned_headers(target: &mut Vec<(String, String)>, headers: Vec<(String, String)>) {
    for (name, value) in headers {
        replace_or_push_owned_header(target, name, value);
    }
}

const CODEX_OAUTH_UNSUPPORTED_RESPONSES_FIELDS: &[&str] = &[
    "max_tokens",
    "max_completion_tokens",
    "max_output_tokens",
    "reasoning_effort",
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
    "prompt_cache_retention",
    "metadata",
    "safety_identifier",
    "previous_response_id",
];

const CODEX_OAUTH_SERVER_ITEM_ID_PREFIXES: &[&str] = &["rs_", "fc_", "resp_", "msg_"];

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

fn codex_responses_lite_requested(headers: &HeaderMap, body: &[u8]) -> bool {
    optional_header(headers, CODEX_RESPONSES_LITE_HEADER)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
        || serde_json::from_slice::<Value>(body)
            .ok()
            .as_ref()
            .is_some_and(codex_responses_lite_requested_value)
}

fn codex_responses_lite_requested_value(body: &Value) -> bool {
    body.pointer(&format!(
        "/client_metadata/{CODEX_RESPONSES_LITE_WS_METADATA}"
    ))
    .and_then(Value::as_str)
    .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

fn normalize_codex_responses_lite_body_bytes(
    body: &Bytes,
    strip_hosted_image_tools: bool,
    remove_ws_metadata: bool,
) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid Codex Responses Lite body: {error}"),
    })?;
    normalize_codex_responses_lite_body(&mut value, strip_hosted_image_tools, remove_ws_metadata);
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("encode Codex Responses Lite body failed: {error}"),
        })
}

fn normalize_codex_responses_lite_body(
    body: &mut Value,
    strip_hosted_image_tools: bool,
    remove_ws_metadata: bool,
) {
    body["parallel_tool_calls"] = Value::Bool(false);
    if !body.get("reasoning").is_some_and(Value::is_object) {
        body["reasoning"] = Value::Object(serde_json::Map::new());
    }
    body["reasoning"]["context"] = Value::String("all_turns".to_string());
    if strip_hosted_image_tools {
        strip_codex_image_generation_tools(body);
    }
    if remove_ws_metadata {
        if let Some(metadata) = body
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
        {
            metadata.remove(CODEX_RESPONSES_LITE_WS_METADATA);
            if metadata.is_empty() {
                if let Some(object) = body.as_object_mut() {
                    object.remove("client_metadata");
                }
            }
        }
    }
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
    sanitize_codex_oauth_request_body(&mut value);
    strip_codex_oauth_unsupported_responses_fields(&mut value);
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
    sanitize_codex_oauth_request_body(&mut body);

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

    strip_codex_oauth_unsupported_responses_fields(&mut body);

    body
}

fn sanitize_codex_oauth_request_body(body: &mut Value) {
    normalize_codex_oauth_reasoning_effort(body);

    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        input.retain(|item| {
            !item.as_str().is_some_and(is_codex_oauth_server_item_id)
                && item.get("type").and_then(Value::as_str) != Some("item_reference")
        });
        for item in input {
            let has_server_item_id = item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(is_codex_oauth_server_item_id);
            if has_server_item_id {
                if let Some(object) = item.as_object_mut() {
                    object.remove("id");
                }
            }
            let message_type = item.get("type").and_then(Value::as_str);
            if matches!(message_type, None | Some("message"))
                && item.get("role").and_then(Value::as_str) == Some("system")
            {
                item["role"] = Value::String("developer".to_string());
            }
        }
    }

    let mut valid_function_names = BTreeSet::new();
    let mut valid_namespace_functions = BTreeSet::new();
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        let mut normalized_tools = Vec::with_capacity(tools.len());
        for tool in std::mem::take(tools) {
            let Some(object) = tool.as_object() else {
                continue;
            };
            match object.get("type").and_then(Value::as_str) {
                Some("function") => {
                    let Some((tool, name)) = normalize_codex_function_tool(object) else {
                        continue;
                    };
                    valid_function_names.insert(name);
                    normalized_tools.push(tool);
                }
                Some("namespace") => {
                    let namespace = object
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string);
                    if let Some(namespace_tools) = object.get("tools").and_then(Value::as_array) {
                        for namespace_tool in namespace_tools {
                            let Some(name) = namespace_tool
                                .get("name")
                                .and_then(normalized_codex_tool_name)
                            else {
                                continue;
                            };
                            valid_function_names.insert(name.clone());
                            if let Some(namespace) = namespace.as_ref() {
                                valid_namespace_functions.insert((namespace.clone(), name.clone()));
                            }
                        }
                    }
                    normalized_tools.push(tool);
                }
                Some("custom") => normalized_tools.push(tool),
                Some(tool_type) if codex_oauth_hosted_tool_type(tool_type) => {
                    normalized_tools.push(tool);
                }
                _ => {}
            }
        }
        *tools = normalized_tools;
    }

    let normalized_tool_choice = body.get("tool_choice").and_then(|choice| match choice {
        Value::String(value) if matches!(value.as_str(), "auto" | "none" | "required") => {
            Some(choice.clone())
        }
        Value::Object(object) => {
            let choice_type = object.get("type").and_then(Value::as_str)?.trim();
            if choice_type.is_empty() {
                return None;
            }
            if matches!(choice_type, "function" | "custom") {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        object
                            .get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                    })?
                    .trim();
                if name.is_empty() {
                    return None;
                }
                let name = name.chars().take(128).collect::<String>();
                if choice_type == "function" {
                    let namespace = object
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|namespace| !namespace.is_empty());
                    let valid = namespace.map_or_else(
                        || valid_function_names.contains(&name),
                        |namespace| {
                            valid_namespace_functions
                                .contains(&(namespace.to_string(), name.clone()))
                        },
                    );
                    if !valid {
                        return None;
                    }
                    let mut normalized = serde_json::Map::new();
                    normalized.insert("type".to_string(), Value::String(choice_type.to_string()));
                    normalized.insert("name".to_string(), Value::String(name));
                    if let Some(namespace) = namespace {
                        normalized.insert(
                            "namespace".to_string(),
                            Value::String(namespace.to_string()),
                        );
                    }
                    return Some(Value::Object(normalized));
                }
                return Some(serde_json::json!({"type": choice_type, "name": name}));
            }
            Some(choice.clone())
        }
        _ => None,
    });
    if body.get("tool_choice").is_some() {
        if let Some(choice) = normalized_tool_choice {
            body["tool_choice"] = choice;
        } else if let Some(object) = body.as_object_mut() {
            object.remove("tool_choice");
        }
    }
}

fn normalize_codex_oauth_reasoning_effort(body: &mut Value) {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    let effort = body
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .or_else(|| body.get("reasoning_effort").and_then(Value::as_str))
        .map(str::to_string);

    if let Some(effort) = effort.as_deref() {
        let normalized = model.as_deref().map_or_else(
            || effort.to_string(),
            |model| super::codex_models::normalize_reasoning_effort(model, effort),
        );
        if !body.get("reasoning").is_some_and(Value::is_object) {
            body["reasoning"] = Value::Object(serde_json::Map::new());
        }
        body["reasoning"]["effort"] = Value::String(normalized);
    }
    if let Some(object) = body.as_object_mut() {
        object.remove("reasoning_effort");
    }
}

fn is_codex_oauth_server_item_id(id: &str) -> bool {
    CODEX_OAUTH_SERVER_ITEM_ID_PREFIXES
        .iter()
        .any(|prefix| id.starts_with(prefix))
}

fn strip_codex_oauth_unsupported_responses_fields(body: &mut Value) {
    if let Some(object) = body.as_object_mut() {
        for field in CODEX_OAUTH_UNSUPPORTED_RESPONSES_FIELDS {
            object.remove(*field);
        }
    }
}

fn normalize_codex_function_tool(
    object: &serde_json::Map<String, Value>,
) -> Option<(Value, String)> {
    let nested = object.get("function").and_then(Value::as_object);
    let name = object
        .get("name")
        .and_then(normalized_codex_tool_name)
        .or_else(|| {
            nested
                .and_then(|function| function.get("name"))
                .and_then(normalized_codex_tool_name)
        })?;
    let description = object
        .get("description")
        .filter(|value| value.is_string())
        .or_else(|| {
            nested
                .and_then(|function| function.get("description"))
                .filter(|value| value.is_string())
        });
    let parameters = object
        .get("parameters")
        .filter(|value| value.is_object())
        .or_else(|| {
            nested
                .and_then(|function| function.get("parameters"))
                .filter(|value| value.is_object())
        })
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
    let strict = object
        .get("strict")
        .filter(|value| value.is_boolean())
        .or_else(|| {
            nested
                .and_then(|function| function.get("strict"))
                .filter(|value| value.is_boolean())
        });

    let mut normalized = serde_json::Map::new();
    normalized.insert("type".to_string(), Value::String("function".to_string()));
    normalized.insert("name".to_string(), Value::String(name.clone()));
    if let Some(description) = description {
        normalized.insert("description".to_string(), description.clone());
    }
    normalized.insert("parameters".to_string(), parameters);
    if let Some(strict) = strict {
        normalized.insert("strict".to_string(), strict.clone());
    }
    Some((Value::Object(normalized), name))
}

fn normalized_codex_tool_name(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(128).collect())
}

fn codex_oauth_hosted_tool_type(tool_type: &str) -> bool {
    matches!(
        tool_type,
        "image_generation"
            | "web_search"
            | "web_search_preview"
            | "file_search"
            | "computer"
            | "computer_use_preview"
            | "code_interpreter"
            | "mcp"
            | "local_shell"
            | "tool_search"
    )
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
    if body
        .get("tool_choice")
        .is_some_and(is_codex_image_generation_tool_choice)
    {
        if let Some(object) = body.as_object_mut() {
            object.remove("tool_choice");
            stripped = true;
        }
    }
    stripped
}

fn is_codex_image_generation_tool_choice(choice: &Value) -> bool {
    choice
        .as_str()
        .is_some_and(|choice| matches!(choice, "image_generation" | "image_gen"))
        || is_codex_image_generation_tool(choice)
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
    claude_tool_name_stream_patcher: super::claude_oauth::ClaudeToolNameStreamPatcher,
    timeouts: StreamTimeoutConfig,
    pending_chunk: Option<Bytes>,
    pending_chunk_already_inspected: bool,
    pending_chunk_saw_business_output: bool,
    pending_chunk_committed_output: bool,
    sse_error_detector: Option<ClaudeSseErrorDetector>,
    sse_error_outcome_recorded: bool,
    responses_semantics: Option<ResponsesSseInspector>,
    anthropic_semantics: Option<AnthropicSseInspector>,
    semantic_provider_outcome_recorded: bool,
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
                Some("client_cancelled"),
            )
            .await;
            record_share_invocation_result(
                &state,
                share_id.as_deref(),
                user_email.as_deref(),
                usage,
            )
            .await;
            crate::metrics::record_stream_client_cancelled(stored.app.as_str());
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
    crate::domain::sharing::subscription_identity::validate_share_subscription_binding(
        share_id, providers, accounts, shares,
    )
    .map_err(|error| ProxyError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: format!("[{}] {error}", error.code()),
    })?;
    let (stored, share_name) = select_share_provider(providers, shares, app, share_id)?;
    super::router::ensure_codex_oauth_active_account(&stored, accounts)?;
    ensure_provider_account_does_not_need_relogin(&stored, accounts)?;
    ensure_provider_account_usage_available(&stored, accounts, current_time_ms())?;
    let execution = ProviderExecution::from_store(providers, stored)?;
    execution.ensure_operation_supported(ProviderOperation::Forward)?;
    Ok((execution, share_name))
}

fn select_share_image_generation_execution(
    providers: &ProviderStore,
    shares: &ShareStore,
    accounts: &AccountStore,
    share_id: &str,
) -> Result<(ProviderExecution, Option<String>), ProxyError> {
    let selection = select_share_execution(providers, shares, accounts, AppKind::Codex, share_id)?;
    if !codex_image_generation_provider(&selection.0.stored) {
        return Err(ProxyError::bad_request(
            "image generation requires a grok_oauth provider or codex_oauth provider with image generation enabled",
        ));
    }
    Ok(selection)
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

    #[test]
    fn images_and_websocket_never_reselect_inactive_account_after_acquire_race() {
        let mut accounts = AccountStore::default();
        for account_id in ["race-account-1", "race-account-2"] {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": account_id,
                    "providerType": "codex_oauth",
                    "accessToken": format!("{account_id}-token"),
                    "profile": {
                        "codexWorkspaceProvenance": {
                            "workspaceId": format!("{account_id}-workspace"),
                            "source": "test_fixture"
                        }
                    }
                }))
                .unwrap(),
            );
        }
        accounts
            .select_active_codex_oauth_account("race-account-1")
            .unwrap();
        let mut first = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({"ACCOUNT_MAX_CONCURRENT": 1}),
            Some("race-account-1"),
        );
        first.provider.id = "race-provider-1".to_string();
        let first_meta = first.provider.meta.as_mut().unwrap();
        first_meta.codex_image_generation_enabled = Some(true);
        first_meta.codex_websocket_enabled = Some(true);
        let mut second = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({"ACCOUNT_MAX_CONCURRENT": 1}),
            Some("race-account-2"),
        );
        second.provider.id = "race-provider-2".to_string();
        let second_meta = second.provider.meta.as_mut().unwrap();
        second_meta.codex_image_generation_enabled = Some(true);
        second_meta.codex_websocket_enabled = Some(true);
        let mut providers = ProviderStore {
            providers: vec![first, second],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();

        for selection_kind in ["images", "websocket"] {
            let state = forwarder_test_state(selection_kind);
            let mut competing_guard = None;
            let mut selection_calls = 0;
            let error = select_and_acquire_account_in_flight(&state, &accounts, |snapshot| {
                selection_calls += 1;
                let selection = if selection_kind == "images" {
                    select_provider_for_codex_image_generation(
                        &providers,
                        &accounts,
                        &HeaderMap::new(),
                        Some("race-provider-1"),
                        snapshot,
                        None,
                    )
                } else {
                    select_provider_with_account_inflight(
                        &providers,
                        &accounts,
                        AppKind::Codex,
                        &HeaderMap::new(),
                        Some("race-provider-1"),
                        snapshot,
                        None,
                    )
                }?;
                if selection_calls == 1 {
                    assert_eq!(selection.execution.stored.provider.id, "race-provider-1");
                    competing_guard = state.account_in_flight.try_acquire(
                        ProviderType::CodexOAuth,
                        "race-account-1",
                        1,
                    );
                    assert!(competing_guard.is_some());
                }
                Ok(selection.execution)
            })
            .unwrap_err();

            assert_eq!(selection_calls, 2, "{selection_kind}");
            assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
            let snapshot = state.account_in_flight.snapshot();
            assert_eq!(
                snapshot.current(ProviderType::CodexOAuth, "race-account-1"),
                1
            );
            assert_eq!(
                snapshot.current(ProviderType::CodexOAuth, "race-account-2"),
                0
            );
            drop(competing_guard);
            let snapshot = state.account_in_flight.snapshot();
            assert_eq!(
                snapshot.current(ProviderType::CodexOAuth, "race-account-1"),
                0
            );
            assert_eq!(
                snapshot.current(ProviderType::CodexOAuth, "race-account-2"),
                0
            );
        }
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
                    profile: Some(json!({"accountUUID": "legacy-principal"})),
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
        let runtime_base_url = base_url.clone();
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
                                auth_identity_generation: Some(1),
                            }),
                            ..Default::default()
                        }),
                        extra: Default::default(),
                    },
                );
            })
            .await
            .unwrap();
        state
            .override_provider_runtime_endpoint_for_test(
                AppKind::Claude,
                "legacy-refresh-provider",
                runtime_base_url,
            )
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
        let runtime_base_url = base_url.clone();
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
                                    auth_identity_generation: Some(1),
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
        for kind in ["messages", "count"] {
            state
                .override_provider_runtime_endpoint_for_test(
                    AppKind::Claude,
                    &format!("unauthorized-{kind}-provider"),
                    runtime_base_url.clone(),
                )
                .await
                .unwrap();
        }

        for (kind, route) in [
            ("messages", ProxyRoute::ClaudeMessages),
            ("count", ProxyRoute::ClaudeCountTokens),
        ] {
            let provider_id = format!("unauthorized-{kind}-provider");
            state
                .apply_ui_settings_patch_immediate(json!({
                    "currentProviderClaude": provider_id.clone()
                }))
                .await
                .unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cc-provider-id",
                HeaderValue::from_str(&provider_id).unwrap(),
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
    async fn claude_unauthorized_after_refresh_stays_on_bound_account() {
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
                    profile: Some(json!({"accountUUID": "rejected-principal"})),
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
        let rejected_runtime_base_url = rejected_base_url.clone();
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
                                auth_identity_generation: Some(1),
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
            .override_provider_runtime_endpoint_for_test(
                AppKind::Claude,
                "auth-failover-oauth",
                rejected_runtime_base_url,
            )
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

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["type"],
            "error"
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
            0,
            "direct Claude inference must not cross providers after a rejected refresh"
        );
        let usage = state.usage_snapshot().await;
        assert_eq!(usage.logs.len(), 1);
        assert_eq!(usage.logs[0].provider_id, "auth-failover-oauth");
        let failed_account = state
            .find_account_by_id("auth-failover-account")
            .await
            .unwrap();
        assert!(failed_account
            .rate_limited_until
            .is_some_and(|until| until > crate::infra::time::now_ms() as i64));

        let pinned_token_url = format!("http://{token_address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                let account = accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == "auth-failover-account")
                    .expect("managed account");
                account.rate_limited_until = None;
                account.raw = Some(json!({
                    "testOAuthTokenUrl": pinned_token_url,
                    "account": {"uuid": "rejected-principal"}
                }));
            })
            .await
            .unwrap();
        let mut pinned_headers = HeaderMap::new();
        pinned_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        pinned_headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_static("auth-failover-oauth"),
        );
        let pinned = forward(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            None,
            pinned_headers,
            Bytes::from_static(
                br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"stay pinned"}]}"#,
            ),
        )
        .await
        .unwrap();
        assert_eq!(pinned.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            fallback_requests.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "an explicitly pinned request must not cross providers"
        );
        assert_eq!(rejected_authorizations.lock().unwrap().len(), 4);
        let failed_account = state
            .find_account_by_id("auth-failover-account")
            .await
            .unwrap();
        assert!(failed_account
            .rate_limited_until
            .is_some_and(|until| until > crate::infra::time::now_ms() as i64));
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
                        auth_identity_generation: Some(1),
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

    fn share_image_generation_fixture(
        provider_type: ProviderType,
        codex_image_generation_enabled: Option<bool>,
    ) -> (ProviderStore, ShareStore, AccountStore) {
        let account_id = format!("{}-share-image-account", provider_type.as_str());
        let provider_id = format!("{}-share-image-provider", provider_type.as_str());
        let share_id = format!("{}-share-image", provider_type.as_str());
        let profile = if provider_type == ProviderType::CodexOAuth {
            json!({
                "verifiedOpenAiClaims": {
                    "subject": "share-image-subject",
                    "chatgpt_account_id": "share-image-workspace"
                },
                "workspaces": [{
                    "id": "share-image-workspace",
                    "name": "Share Image Workspace"
                }]
            })
        } else {
            Value::Null
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(
            serde_json::from_value(json!({
                "id": account_id,
                "providerType": provider_type.as_str(),
                "accessToken": "share-image-access-token",
                "profile": profile
            }))
            .unwrap(),
        );

        let mut provider =
            stored_provider(AppKind::Codex, provider_type, json!({}), Some(&account_id));
        provider.provider.id = provider_id.clone();
        provider
            .provider
            .meta
            .as_mut()
            .unwrap()
            .codex_image_generation_enabled = codex_image_generation_enabled;
        let mut providers = ProviderStore {
            providers: vec![provider],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();

        let share = serde_json::from_value(json!({
            "id": share_id,
            "app": "codex",
            "providerId": provider_id,
            "providerType": provider_type.as_str(),
            "enabled": true,
            "status": "active",
            "bindings": [{
                "app": "codex",
                "providerId": provider_id,
                "providerType": provider_type.as_str()
            }]
        }))
        .unwrap();
        let shares = ShareStore {
            shares: vec![share],
            ..ShareStore::default()
        };
        (providers, shares, accounts)
    }

    async fn test_responses_upstream_socket(
    ) -> (ResponsesUpstreamWebSocket, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while websocket.next().await.is_some() {}
        });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .unwrap();
        (client, server)
    }

    fn forwarder_test_state(name: &str) -> ServerState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir().join(format!("cc-switch-server-forwarder-{name}-{nanos}")),
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

    async fn install_grok_test_execution(
        state: &ServerState,
        name: &str,
        endpoint: String,
        websocket_url: Option<String>,
        access_token: &str,
        refresh_url: Option<String>,
        capabilities: &[GrokAccountCapability],
    ) -> ProviderExecution {
        let account_id = format!("{name}-account");
        let provider_id = format!("{name}-provider");
        let mut capability_evidence = serde_json::Map::new();
        for capability in capabilities {
            capability_evidence.insert(
                capability.as_str().to_string(),
                json!({
                    "status": "supported",
                    "source": "test_fixture",
                    "observedAtMs": 1
                }),
            );
        }
        let account_id_for_state = account_id.clone();
        let name_for_state = name.to_string();
        let access_token = access_token.to_string();
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id_for_state,
                        "providerType": "grok_oauth",
                        "email": format!("{name_for_state}@example.com"),
                        "accessToken": access_token,
                        "refreshToken": "grok-test-refresh-token",
                        "tokenType": "Bearer",
                        "expiresAt": i64::MAX / 2,
                        "profile": {
                            "verifiedGrokClaims": {
                                "subject": format!("subject-{name_for_state}"),
                                "email": format!("{name_for_state}@example.com")
                            },
                            "grokCapabilities": Value::Object(capability_evidence)
                        },
                        "raw": {"testOAuthTokenUrl": refresh_url}
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();

        let mut stored = stored_provider(
            AppKind::Codex,
            ProviderType::GrokOAuth,
            json!({}),
            Some(&account_id),
        );
        stored.provider.id = provider_id;
        let accounts = state.accounts_snapshot().await;
        let mut providers = ProviderStore {
            providers: vec![stored.clone()],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        let mut plan = providers
            .runtime_plan(AppKind::Codex, &stored.provider.id)
            .unwrap()
            .as_ref()
            .clone();
        plan.endpoint = endpoint;
        plan.runtime_fingerprint = format!("{name}-grok-test-runtime");
        if let Some(websocket_url) = websocket_url {
            plan.driver_options.insert(
                "testGrokWebsocketUrl".to_string(),
                Value::String(websocket_url),
            );
        }
        std::sync::Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        let execution = ProviderExecution::from_store(&providers, stored).unwrap();
        state.replace_provider_store_for_test(providers).await;
        execution
    }

    async fn spawn_test_grok_responses_bridge(
        state: ServerState,
        execution: ProviderExecution,
        session_id: &str,
        turn_index: Option<u64>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let session_id = session_id.to_string();
        let target = prepare_responses_websocket_target(
            &state,
            &execution,
            ResponsesWebsocketMode::Grok,
            Some(&session_id),
            turn_index,
        )
        .await
        .unwrap();
        let app = axum::Router::new().route(
            "/bridge",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let state = state.clone();
                let execution = execution.clone();
                let session_id = session_id.clone();
                let target_headers = target.headers.clone();
                let target_url = target.ws_url.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        let _ = bridge_responses_websocket(
                            socket,
                            ResponsesWebsocketBridgeOptions {
                                headers: target_headers,
                                connect_timeout: Duration::from_secs(1),
                                first_byte_timeout: Some(Duration::from_secs(1)),
                                stream_idle_timeout: Some(Duration::from_secs(1)),
                                ws_url: target_url,
                                pool_key: None,
                                mode: ResponsesWebsocketMode::Grok,
                                grok_session_id: Some(session_id),
                                grok_turn_index: turn_index,
                                single_upstream_model: Some("grok-4.5".to_string()),
                                state: &state,
                                execution,
                            },
                        )
                        .await;
                    })
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (address, server)
    }

    #[tokio::test]
    async fn kiro_stream_holds_account_lease_until_response_body_is_dropped() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = axum::Router::new().route(
            "/stream",
            axum::routing::get(|| async {
                let pending =
                    futures_util::stream::pending::<Result<Bytes, std::convert::Infallible>>();
                Response::new(Body::from_stream(pending))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let upstream = reqwest::Client::new()
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .unwrap();

        let state = forwarder_test_state("kiro-stream-account-lease");
        let account_id = "kiro-stream-account";
        let account_in_flight_guard = state
            .account_in_flight
            .try_acquire(ProviderType::KiroOAuth, account_id, 1)
            .unwrap();
        let stored = stored_provider(
            AppKind::Claude,
            ProviderType::KiroOAuth,
            json!({}),
            Some(account_id),
        );
        let response = forward_claude_kiro_stream(ClaudeKiroStreamOptions {
            state: state.clone(),
            stored,
            upstream,
            response_model: "claude-sonnet-4-6".to_string(),
            model_metadata: UsageModelMetadata::default(),
            request_body: json!({
                "model": "claude-sonnet-4-6",
                "messages": []
            }),
            tool_name_map: Default::default(),
            request_context: UsageLogContext::default(),
            account_in_flight_guard: Some(account_in_flight_guard),
            share_invocation_guard: None,
            started: Instant::now(),
            status: reqwest::StatusCode::OK,
            status_code: StatusCode::OK.as_u16(),
        })
        .await
        .unwrap();

        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::KiroOAuth, account_id),
            1
        );
        let mut body = response.into_body().into_data_stream();
        let first_chunk = tokio::time::timeout(Duration::from_secs(1), body.next())
            .await
            .unwrap()
            .expect("Kiro stream should emit its initial SSE frame")
            .unwrap();
        assert!(!first_chunk.is_empty());
        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::KiroOAuth, account_id),
            1
        );

        drop(body);
        assert_eq!(
            state
                .account_in_flight
                .snapshot()
                .current(ProviderType::KiroOAuth, account_id),
            0
        );
        server.abort();
    }

    async fn codex_bridge_test_context(
        name: &str,
        endpoint: String,
    ) -> (ServerState, ProviderExecution) {
        let state = forwarder_test_state(name);
        let account_input = crate::domain::accounts::store::UpsertAccountInput {
            id: Some(format!("{name}-account")),
            provider_type: ProviderType::CodexOAuth,
            email: Some(format!("{name}@example.com")),
            access_token: Some(format!("{name}-access-token")),
            refresh_token: None,
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: Some(json!({
                "verifiedOpenAiClaims": {
                    "subject": format!("subject-{name}-workspace"),
                    "chatgpt_account_id": format!("{name}-workspace")
                },
                "codexWorkspaceProvenance": {
                    "workspaceId": format!("{name}-workspace"),
                    "source": "test_fixture"
                }
            })),
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
        };
        let account_input_for_state = account_input.clone();
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(account_input_for_state);
            })
            .await
            .unwrap();

        let account_id = format!("{name}-account");
        let mut stored = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some(&account_id),
        );
        stored.provider.id = format!("{name}-provider");
        let mut accounts = AccountStore::default();
        accounts.upsert(account_input);
        let mut providers = ProviderStore {
            providers: vec![stored.clone()],
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        let mut plan = providers
            .runtime_plan(AppKind::Codex, &stored.provider.id)
            .unwrap()
            .as_ref()
            .clone();
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
        plan.endpoint = endpoint;
        plan.runtime_fingerprint = format!("{name}-runtime");
        (
            state,
            ProviderExecution {
                stored,
                plan: std::sync::Arc::new(plan),
            },
        )
    }

    #[derive(Clone)]
    struct TestOverflowReply {
        status: StatusCode,
        content_type: &'static str,
        body: String,
    }

    #[derive(Debug)]
    struct TestOverflowRequest {
        authorization: String,
        account_id: String,
        accept_encoding: String,
        body: Value,
    }

    struct TestOverflowUpstream {
        address: std::net::SocketAddr,
        requests: std::sync::Arc<std::sync::Mutex<Vec<TestOverflowRequest>>>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn spawn_test_overflow_upstream(replies: Vec<TestOverflowReply>) -> TestOverflowUpstream {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let replies = std::sync::Arc::new(replies);
        let app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let requests = std::sync::Arc::clone(&requests_for_route);
                let replies = std::sync::Arc::clone(&replies);
                async move {
                    let body = serde_json::from_slice::<Value>(&body).unwrap();
                    let mut requests = requests.lock().unwrap();
                    let index = requests.len();
                    requests.push(TestOverflowRequest {
                        authorization: headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        account_id: headers
                            .get("chatgpt-account-id")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        accept_encoding: headers
                            .get(axum::http::header::ACCEPT_ENCODING)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        body,
                    });
                    drop(requests);
                    let reply = replies.get(index).cloned().unwrap_or(TestOverflowReply {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        content_type: "application/json",
                        body: json!({"error":{"message":"unexpected request"}}).to_string(),
                    });
                    Response::builder()
                        .status(reply.status)
                        .header(CONTENT_TYPE, reply.content_type)
                        .body(Body::from(reply.body))
                        .unwrap()
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestOverflowUpstream {
            address,
            requests,
            server,
        }
    }

    fn test_overflow_request_body(stream: bool) -> Bytes {
        let old_user = "u".repeat(120 * 1024);
        let old_assistant = "a".repeat(120 * 1024);
        Bytes::from(
            serde_json::to_vec(&json!({
                "model": "gpt-5.4",
                "stream": stream,
                "input": [
                    {"type":"message","role":"developer","content":[{"type":"input_text","text":"keep rules"}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":old_user}]},
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":old_assistant}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"latest question"}]}
                ]
            }))
            .unwrap(),
        )
    }

    fn set_test_overflow_compact(execution: &mut ProviderExecution, enabled: bool) {
        std::sync::Arc::make_mut(&mut execution.plan)
            .driver_options
            .insert(
                "testCodexOverflowAutoCompact".to_string(),
                Value::Bool(enabled),
            );
    }

    fn context_overflow_reply() -> TestOverflowReply {
        TestOverflowReply {
            status: StatusCode::BAD_REQUEST,
            content_type: "application/json",
            body: json!({
                "error": {
                    "type": "invalid_request_error",
                    "code": "context_length_exceeded",
                    "message": "Input exceeds the context window"
                }
            })
            .to_string(),
        }
    }

    fn summary_success_reply() -> TestOverflowReply {
        TestOverflowReply {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"dense summary\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"dense summary\"}]}],\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"total_tokens\":10}}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        }
    }

    #[tokio::test]
    async fn codex_overflow_auto_compact_disabled_makes_no_internal_call() {
        let upstream = spawn_test_overflow_upstream(vec![context_overflow_reply()]).await;
        let name = "codex-overflow-disabled";
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        set_test_overflow_compact(&mut execution, false);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            headers,
            test_overflow_request_body(false),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(upstream.requests.lock().unwrap().len(), 1);
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_http_overflow_compacts_once_on_same_provider_and_logs_summary_usage() {
        let upstream = spawn_test_overflow_upstream(vec![
            context_overflow_reply(),
            summary_success_reply(),
            TestOverflowReply {
                status: StatusCode::OK,
                content_type: "application/json",
                body: json!({
                    "id":"resp-overflow-recovered",
                    "status":"completed",
                    "model":"gpt-5.4",
                    "output":[],
                    "usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}
                })
                .to_string(),
            },
        ])
        .await;
        let name = "codex-overflow-http";
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        set_test_overflow_compact(&mut execution, true);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            test_overflow_request_body(false),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        {
            let requests = upstream.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert!(requests[1]
                .body
                .to_string()
                .contains("conversation compaction assistant"));
            assert_eq!(requests[1].accept_encoding, "identity");
            assert!(requests[2].body.to_string().contains("dense summary"));
            assert!(requests[2].body.to_string().contains("latest question"));
            for request in requests.iter() {
                assert_eq!(request.authorization, format!("Bearer {name}-access-token"));
                assert_eq!(request.account_id, format!("{name}-workspace"));
            }
        }
        let usage = state.usage_snapshot().await;
        let summary_log = usage
            .logs
            .iter()
            .find(|log| {
                log.data_source.as_deref()
                    == Some(crate::proxy::overflow_compact::SUMMARY_DATA_SOURCE)
            })
            .unwrap();
        assert_eq!(summary_log.provider_id, format!("{name}-provider"));
        assert_eq!(summary_log.status_code, StatusCode::OK.as_u16());
        assert_eq!(summary_log.input_tokens, Some(7));
        assert_eq!(summary_log.output_tokens, Some(3));
        assert_eq!(summary_log.total_tokens, Some(10));
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_overflow_summary_failure_falls_back_and_never_compacts_twice() {
        let upstream = spawn_test_overflow_upstream(vec![
            context_overflow_reply(),
            TestOverflowReply {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                content_type: "application/json",
                body: json!({"error":{"type":"server_error"}}).to_string(),
            },
            context_overflow_reply(),
        ])
        .await;
        let name = "codex-overflow-summary-failure";
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        set_test_overflow_compact(&mut execution, true);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            test_overflow_request_body(false),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        {
            let requests = upstream.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert!(requests[2]
                .body
                .to_string()
                .contains("Earlier conversation turns were omitted"));
        }
        let usage = state.usage_snapshot().await;
        let summary_logs = usage
            .logs
            .iter()
            .filter(|log| {
                log.data_source.as_deref()
                    == Some(crate::proxy::overflow_compact::SUMMARY_DATA_SOURCE)
            })
            .collect::<Vec<_>>();
        assert_eq!(summary_logs.len(), 1);
        assert_eq!(
            summary_logs[0].status_code,
            StatusCode::INTERNAL_SERVER_ERROR.as_u16()
        );
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_sse_overflow_retries_only_before_business_output_is_committed() {
        let overflow_sse = TestOverflowReply {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-overflow\"}}\n\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"Input exceeds the context window\"}}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        };
        let upstream = spawn_test_overflow_upstream(vec![
            overflow_sse,
            summary_success_reply(),
            TestOverflowReply {
                status: StatusCode::OK,
                content_type: "text/event-stream",
                body: concat!(
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-recovered\"}}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"recovered\"}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":1,\"total_tokens\":6}}}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_string(),
            },
        ])
        .await;
        let name = "codex-overflow-sse";
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        set_test_overflow_compact(&mut execution, true);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            headers,
            test_overflow_request_body(true),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("recovered"));
        assert!(!body.contains("context_length_exceeded"));
        assert_eq!(upstream.requests.lock().unwrap().len(), 3);
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_sse_overflow_after_business_output_does_not_replay() {
        let upstream = spawn_test_overflow_upstream(vec![TestOverflowReply {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-committed\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"already committed\"}\n\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"Input exceeds the context window\"}}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        }])
        .await;
        let name = "codex-overflow-committed";
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        set_test_overflow_compact(&mut execution, true);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            headers,
            test_overflow_request_body(true),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        assert!(body.contains("already committed"));
        assert!(body.contains("context_length_exceeded"));
        assert_eq!(upstream.requests.lock().unwrap().len(), 1);
        upstream.server.abort();
    }

    struct TestCodexProviderSpec {
        name: String,
        endpoint: String,
        websocket_url: Option<String>,
    }

    async fn install_codex_test_provider_set(
        state: &ServerState,
        specs: Vec<TestCodexProviderSpec>,
    ) -> Vec<ProviderExecution> {
        let accounts = state.accounts_snapshot().await;
        let mut stored_providers = Vec::new();
        for spec in &specs {
            let account_id = format!("{}-account", spec.name);
            let mut stored = stored_provider(
                AppKind::Codex,
                ProviderType::CodexOAuth,
                json!({}),
                Some(&account_id),
            );
            stored.provider.id = format!("{}-provider", spec.name);
            let meta = stored.provider.meta.get_or_insert_with(Default::default);
            meta.codex_image_generation_enabled = Some(true);
            meta.codex_websocket_enabled = Some(true);
            stored_providers.push(stored);
        }
        let mut providers = ProviderStore {
            providers: stored_providers,
            ..ProviderStore::default()
        };
        providers.rebuild_runtime_index(&accounts).unwrap();
        for (stored, spec) in providers.providers.iter().zip(&specs) {
            let mut plan = providers
                .runtime_plan(AppKind::Codex, &stored.provider.id)
                .unwrap()
                .as_ref()
                .clone();
            plan.driver_id =
                crate::domain::providers::registry::DriverId::parse("oauth.openai_codex").unwrap();
            plan.endpoint = spec.endpoint.clone();
            plan.runtime_fingerprint = format!("{}-runtime", spec.name);
            if let Some(websocket_url) = spec.websocket_url.as_ref() {
                plan.driver_options.insert(
                    "testCodexWebsocketUrl".to_string(),
                    Value::String(websocket_url.clone()),
                );
            }
            std::sync::Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        }
        let executions = providers
            .providers
            .iter()
            .cloned()
            .map(|stored| ProviderExecution::from_store(&providers, stored).unwrap())
            .collect::<Vec<_>>();
        state.replace_provider_store_for_test(providers).await;
        executions
    }

    async fn insert_static_codex_test_account(state: &ServerState, name: &str, access_token: &str) {
        let account_id = format!("{name}-account");
        let active_account_id = account_id.clone();
        let workspace_id = format!("{name}-workspace");
        let email = format!("{name}@example.com");
        let access_token = access_token.to_string();
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id),
                    provider_type: ProviderType::CodexOAuth,
                    email: Some(email),
                    access_token: Some(access_token),
                    refresh_token: None,
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: Some(json!({
                        "verifiedOpenAiClaims": {
                            "subject": format!("subject-{workspace_id}"),
                            "chatgpt_account_id": workspace_id.clone()
                        },
                        "codexWorkspaceProvenance": {
                            "workspaceId": workspace_id.clone(),
                            "source": "test_fixture"
                        }
                    })),
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
                if accounts.active_codex_oauth_account_id.is_none() {
                    accounts
                        .select_active_codex_oauth_account(&active_account_id)
                        .expect("inserted Codex test account must be selectable");
                }
            })
            .await
            .unwrap();
    }

    async fn spawn_test_responses_bridge(
        state: ServerState,
        execution: ProviderExecution,
        upstream_ws_url: String,
        pool_key: Option<String>,
        session_id: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        spawn_test_responses_bridge_with_timeouts(
            state,
            execution,
            upstream_ws_url,
            pool_key,
            session_id,
            Some(Duration::from_secs(1)),
            Some(Duration::from_secs(1)),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_test_responses_bridge_with_timeouts(
        state: ServerState,
        execution: ProviderExecution,
        upstream_ws_url: String,
        pool_key: Option<String>,
        session_id: &str,
        first_event_timeout: Option<Duration>,
        idle_timeout: Option<Duration>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let session_id = session_id.to_string();
        let prepared = prepare_responses_websocket_target(
            &state,
            &execution,
            ResponsesWebsocketMode::Codex,
            Some(&session_id),
            None,
        )
        .await
        .unwrap();
        let upstream_headers = prepared.headers;
        let app = axum::Router::new().route(
            "/bridge",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let state = state.clone();
                let execution = execution.clone();
                let upstream_ws_url = upstream_ws_url.clone();
                let pool_key = pool_key.clone();
                let session_id = session_id.clone();
                let upstream_headers = upstream_headers.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        let _ = bridge_responses_websocket(
                            socket,
                            ResponsesWebsocketBridgeOptions {
                                headers: upstream_headers,
                                connect_timeout: Duration::from_secs(1),
                                first_byte_timeout: first_event_timeout,
                                stream_idle_timeout: idle_timeout,
                                ws_url: upstream_ws_url,
                                pool_key,
                                mode: ResponsesWebsocketMode::Codex,
                                grok_session_id: Some(session_id),
                                grok_turn_index: None,
                                single_upstream_model: None,
                                state: &state,
                                execution,
                            },
                        )
                        .await;
                    })
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (address, server)
    }

    async fn send_test_bridge_request(address: std::net::SocketAddr, input: &str) -> Vec<Value> {
        send_test_bridge_request_with_close(address, input).await.0
    }

    async fn send_test_bridge_request_with_close(
        address: std::net::SocketAddr,
        input: &str,
    ) -> (Vec<Value>, Option<CloseCode>) {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
            .await
            .unwrap();
        socket
            .send(TungsteniteMessage::Text(
                json!({
                    "type": "response.create",
                    "model": "gpt-5.4",
                    "input": input
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let mut events = Vec::new();
        let mut close_code = None;
        loop {
            let message = match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
            };
            match message {
                TungsteniteMessage::Text(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        let terminal = matches!(
                            value.get("type").and_then(Value::as_str),
                            Some("response.completed" | "response.failed" | "response.incomplete")
                        );
                        events.push(value);
                        if terminal {
                            break;
                        }
                    }
                }
                TungsteniteMessage::Binary(bytes) => {
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                        events.push(value);
                    }
                }
                TungsteniteMessage::Close(frame) => {
                    close_code = frame.map(|frame| frame.code);
                    break;
                }
                _ => {}
            }
        }
        let _ = socket.close(None).await;
        (events, close_code)
    }

    async fn unavailable_test_address() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        address
    }

    async fn run_codex_http_fallback_single_provider_case(
        case: &str,
        behavior: TestCodexWebSocketBehavior,
        first_event_timeout: Option<Duration>,
        idle_timeout: Option<Duration>,
    ) -> (
        Vec<Value>,
        Option<CloseCode>,
        TestCodexUpstream,
        TestCodexUpstream,
        tokio::task::JoinHandle<()>,
    ) {
        let primary_name = format!("{case}-primary");
        let fallback_name = format!("{case}-fallback");
        let primary = spawn_test_codex_upstream(behavior).await;
        let fallback = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let state = forwarder_test_state(&format!("codex-ws-{case}"));
        insert_static_codex_test_account(&state, &primary_name, "primary-token").await;
        insert_static_codex_test_account(&state, &fallback_name, "fallback-token").await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: primary_name,
                    endpoint: format!("http://{}", primary.address),
                    websocket_url: Some(format!("ws://{}/ws", primary.address)),
                },
                TestCodexProviderSpec {
                    name: fallback_name,
                    endpoint: format!("http://{}", fallback.address),
                    websocket_url: Some(format!("ws://{}/ws", fallback.address)),
                },
            ],
        )
        .await;
        let execution = executions.remove(0);
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge_with_timeouts(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            &format!("{case}-session"),
            first_event_timeout,
            idle_timeout,
        )
        .await;
        let (events, close_code) = send_test_bridge_request_with_close(bridge_address, case).await;
        (events, close_code, primary, fallback, bridge_server)
    }

    async fn wait_for_cached_responses_websocket(pool_key: &str) {
        for _ in 0..100 {
            let present = responses_websocket_pool()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entries
                .contains_key(pool_key);
            if present {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("responses websocket was not released into the cache");
    }

    async fn wait_for_codex_account_leases_to_release(state: &ServerState, names: &[&str]) {
        for _ in 0..100 {
            let snapshot = state.account_in_flight.snapshot();
            if names.iter().all(|name| {
                snapshot.current(ProviderType::CodexOAuth, &format!("{name}-account")) == 0
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("Codex account lease was not released after websocket completion");
    }

    async fn remove_cached_responses_websocket(pool_key: &str) {
        let entry = responses_websocket_pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .acquire(pool_key);
        if let Some(mut entry) = entry {
            let _ = entry.socket.close(None).await;
        }
    }

    #[derive(Clone, Copy)]
    enum TestCodexWebSocketBehavior {
        Complete,
        CompleteThenClose,
        CloseSize,
        CloseSizeThenDelayHttpFirstEvent,
        CloseSizeThenStallHttpErrorBody,
        CloseSizeThenHttpBusinessThenEof,
        CloseSizeThenHttpBusinessThenStall,
        CloseSizeThenHttpMalformed,
        CloseSizeThenHttpBusinessThenProviderFailure,
        CloseSizeThenHttpRateLimited,
        CloseSizeThenHttpServerError,
        EmitThenCloseSize,
        EmitBusinessThenCloseSize,
        EmitBusinessThenCloseNormal,
        CloseSizeThenHttpProviderFailure,
        CloseSizeThenHttpClientFailure,
        Silent,
    }

    const TEST_OPENAI_RSA_KID: &str = "cc-switch-openai-test-rsa";
    const TEST_OPENAI_RSA_N: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";
    const TEST_OPENAI_RSA_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJETqse41HRBsc
7cfcq3ak4oZWFCoZlcic525A3FfO4qW9BMtRO/iXiyCCHn8JhiL9y8j5JdVP2Q9Z
IpfElcFd3/guS9w+5RqQGgCR+H56IVUyHZWtTJbKPcwWXQdNUX0rBFcsBzCRESJL
eelOEdHIjG7LRkx5l/FUvlqsyHDVJEQsHwegZ8b8C0fz0EgT2MMEdn10t6Ur1rXz
jMB/wvCg8vG8lvciXmedyo9xJ8oMOh0wUEgxziVDMMovmC+aJctcHUAYubwoGN8T
yzcvnGqL7JSh36Pwy28iPzXZ2RLhAyJFU39vLaHdljwthUaupldlNyCfa6Ofy4qN
ctlUPlN1AgMBAAECggEAdESTQjQ70O8QIp1ZSkCYXeZjuhj081CK7jhhp/4ChK7J
GlFQZMwiBze7d6K84TwAtfQGZhQ7km25E1kOm+3hIDCoKdVSKch/oL54f/BK6sKl
qlIzQEAenho4DuKCm3I4yAw9gEc0DV70DuMTR0LEpYyXcNJY3KNBOTjN5EYQAR9s
2MeurpgK2MdJlIuZaIbzSGd+diiz2E6vkmcufJLtmYUT/k/ddWvEtz+1DnO6bRHh
xuuDMeJA/lGB/EYloSLtdyCF6sII6C6slJJtgfb0bPy7l8VtL5iDyz46IKyzdyzW
tKAn394dm7MYR1RlUBEfqFUyNK7C+pVMVoTwCC2V4QKBgQD64syfiQ2oeUlLYDm4
CcKSP3RnES02bcTyEDFSuGyyS1jldI4A8GXHJ/lG5EYgiYa1RUivge4lJrlNfjyf
dV230xgKms7+JiXqag1FI+3mqjAgg4mYiNjaao8N8O3/PD59wMPeWYImsWXNyeHS
55rUKiHERtCcvdzKl4u35ZtTqQKBgQDNKnX2bVqOJ4WSqCgHRhOm386ugPHfy+8j
m6cicmUR46ND6ggBB03bCnEG9OtGisxTo/TuYVRu3WP4KjoJs2LD5fwdwJqpgtHl
yVsk45Y1Hfo+7M6lAuR8rzCi6kHHNb0HyBmZjysHWZsn79ZM+sQnLpgaYgQGRbKV
DZWlbw7g7QKBgQCl1u+98UGXAP1jFutwbPsx40IVszP4y5ypCe0gqgon3UiY/G+1
zTLp79GGe/SjI2VpQ7AlW7TI2A0bXXvDSDi3/5Dfya9ULnFXv9yfvH1QwWToySpW
Kvd1gYSoiX84/WCtjZOr0e0HmLIb0vw0hqZA4szJSqoxQgvF22EfIWaIaQKBgQCf
34+OmMYw8fEvSCPxDxVvOwW2i7pvV14hFEDYIeZKW2W1HWBhVMzBfFB5SE8yaCQy
pRfOzj9aKOCm2FjjiErVNpkQoi6jGtLvScnhZAt/lr2TXTrl8OwVkPrIaN0bG/AS
aUYxmBPCpXu3UjhfQiWqFq/mFyzlqlgvuCc9g95HPQKBgAscKP8mLxdKwOgX8yFW
GcZ0izY/30012ajdHY+/QK5lsMoxTnn0skdS+spLxaS5ZEO4qvPVb8RAoCkWMMal
2pOhmquJQVDPDLuZHdrIiKiDM20dy9sMfHygWcZjQ4WSxf/J7T9canLZIXFhHAZT
3wc9h4G8BBCtWN2TN/LsGZdB
-----END PRIVATE KEY-----"#;

    async fn signed_openai_access_token(workspace_id: &str) -> String {
        let jwk = serde_json::from_value(json!({
            "kty": "RSA",
            "n": TEST_OPENAI_RSA_N,
            "e": "AQAB",
            "kid": TEST_OPENAI_RSA_KID,
            "alg": "RS256",
            "use": "sig"
        }))
        .unwrap();
        crate::state::install_openai_test_jwk(jwk).await;

        let now = chrono::Utc::now().timestamp();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(TEST_OPENAI_RSA_KID.to_string());
        jsonwebtoken::encode(
            &header,
            &json!({
                "iss": "https://auth.openai.com",
                "aud": "https://api.openai.com/v1",
                "sub": format!("subject-{workspace_id}"),
                "iat": now,
                "nbf": now - 1,
                "exp": now + 3600,
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": workspace_id,
                    "chatgpt_plan_type": "plus"
                }
            }),
            &jsonwebtoken::EncodingKey::from_rsa_pem(TEST_OPENAI_RSA_PRIVATE_KEY.as_bytes())
                .unwrap(),
        )
        .unwrap()
    }

    struct TestCodexUpstream {
        address: std::net::SocketAddr,
        websocket_connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        websocket_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        http_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        http_observations: std::sync::Arc<std::sync::Mutex<Vec<(String, String, Value)>>>,
        server: tokio::task::JoinHandle<()>,
    }

    struct TestCodexWebsocketAuthUpstream {
        address: std::net::SocketAddr,
        connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        authorizations: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn spawn_test_codex_websocket_auth_upstream(
        accepted_authorization: Option<String>,
    ) -> TestCodexWebsocketAuthUpstream {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let connections_for_route = std::sync::Arc::clone(&connections);
        let authorizations_for_route = std::sync::Arc::clone(&authorizations);
        let app = axum::Router::new().route(
            "/ws",
            axum::routing::get(move |headers: HeaderMap, ws: WebSocketUpgrade| {
                let connections = std::sync::Arc::clone(&connections_for_route);
                let authorizations = std::sync::Arc::clone(&authorizations_for_route);
                let accepted_authorization = accepted_authorization.clone();
                async move {
                    let authorization = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    authorizations.lock().unwrap().push(authorization.clone());
                    if accepted_authorization.as_deref() != Some(authorization.as_str()) {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Body::from("unauthorized"))
                            .unwrap();
                    }
                    ws.on_upgrade(move |mut socket| async move {
                        connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        while let Some(Ok(message)) = socket.recv().await {
                            if matches!(message, AxumWsMessage::Text(_) | AxumWsMessage::Binary(_))
                            {
                                let _ = socket
                                    .send(AxumWsMessage::Text(
                                        json!({
                                            "type": "response.completed",
                                            "response": {
                                                "id": "resp-auth-failover",
                                                "status": "completed",
                                                "output": []
                                            }
                                        })
                                        .to_string(),
                                    ))
                                    .await;
                            }
                        }
                    })
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestCodexWebsocketAuthUpstream {
            address,
            connections,
            authorizations,
            server,
        }
    }

    async fn spawn_test_codex_upstream(behavior: TestCodexWebSocketBehavior) -> TestCodexUpstream {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let websocket_connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let websocket_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let http_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let http_observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let ws_connections_for_route = std::sync::Arc::clone(&websocket_connections);
        let ws_requests_for_route = std::sync::Arc::clone(&websocket_requests);
        let http_requests_for_route = std::sync::Arc::clone(&http_requests);
        let http_observations_for_route = std::sync::Arc::clone(&http_observations);
        let app = axum::Router::new()
            .route(
                "/ws",
                axum::routing::get(move |ws: WebSocketUpgrade| {
                    let connections = std::sync::Arc::clone(&ws_connections_for_route);
                    let requests = std::sync::Arc::clone(&ws_requests_for_route);
                    async move {
                        ws.on_upgrade(move |mut socket| async move {
                            connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            while let Some(Ok(message)) = socket.recv().await {
                                if matches!(message, AxumWsMessage::Close(_)) {
                                    break;
                                }
                                if !matches!(
                                    message,
                                    AxumWsMessage::Text(_) | AxumWsMessage::Binary(_)
                                ) {
                                    continue;
                                }
                                let request_index = requests
                                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                                    + 1;
                                match behavior {
                                    TestCodexWebSocketBehavior::Complete => {
                                        let _ = socket
                                            .send(AxumWsMessage::Text(
                                                json!({
                                                    "type": "response.completed",
                                                    "response": {
                                                        "id": format!("resp-ws-{request_index}"),
                                                        "status": "completed",
                                                        "output": []
                                                    }
                                                })
                                                .to_string(),
                                            ))
                                            .await;
                                    }
                                    TestCodexWebSocketBehavior::CompleteThenClose => {
                                        let _ = socket
                                            .send(AxumWsMessage::Text(
                                                json!({
                                                    "type": "response.completed",
                                                    "response": {
                                                        "id": "resp-before-stale",
                                                        "status": "completed",
                                                        "output": []
                                                    }
                                                })
                                                .to_string(),
                                            ))
                                            .await;
                                        tokio::time::sleep(Duration::from_millis(150)).await;
                                        let _ = socket
                                            .send(AxumWsMessage::Close(Some(
                                                axum::extract::ws::CloseFrame {
                                                    code: 1000,
                                                    reason: "idle close".into(),
                                                },
                                            )))
                                            .await;
                                        break;
                                    }
                                    TestCodexWebSocketBehavior::CloseSize
                                    | TestCodexWebSocketBehavior::CloseSizeThenDelayHttpFirstEvent
                                    | TestCodexWebSocketBehavior::CloseSizeThenStallHttpErrorBody
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenEof
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenStall
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpMalformed
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenProviderFailure
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpRateLimited
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpServerError
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpProviderFailure
                                    | TestCodexWebSocketBehavior::CloseSizeThenHttpClientFailure => {
                                        let _ = socket
                                            .send(AxumWsMessage::Close(Some(
                                                axum::extract::ws::CloseFrame {
                                                    code: 1009,
                                                    reason: "message too big".into(),
                                                },
                                            )))
                                            .await;
                                        break;
                                    }
                                    TestCodexWebSocketBehavior::EmitThenCloseSize => {
                                        let _ = socket
                                            .send(AxumWsMessage::Text(
                                                json!({
                                                    "type": "response.created",
                                                    "response": {
                                                        "id": "resp-committed",
                                                        "status": "in_progress"
                                                    }
                                                })
                                                .to_string(),
                                            ))
                                            .await;
                                        let _ = socket
                                            .send(AxumWsMessage::Close(Some(
                                                axum::extract::ws::CloseFrame {
                                                    code: 1009,
                                                    reason: "message too big".into(),
                                                },
                                            )))
                                            .await;
                                        break;
                                    }
                                    TestCodexWebSocketBehavior::EmitBusinessThenCloseSize
                                    | TestCodexWebSocketBehavior::EmitBusinessThenCloseNormal => {
                                        for event in [
                                            json!({
                                                "type": "response.created",
                                                "response": {
                                                    "id": "resp-committed",
                                                    "status": "in_progress"
                                                }
                                            }),
                                            json!({
                                                "type": "response.output_text.delta",
                                                "delta": "committed"
                                            }),
                                        ] {
                                            let _ = socket
                                                .send(AxumWsMessage::Text(event.to_string()))
                                                .await;
                                        }
                                        let _ = socket
                                            .send(AxumWsMessage::Close(Some(
                                                axum::extract::ws::CloseFrame {
                                                    code: if matches!(
                                                        behavior,
                                                        TestCodexWebSocketBehavior::EmitBusinessThenCloseNormal
                                                    ) {
                                                        1000
                                                    } else {
                                                        1009
                                                    },
                                                    reason: "upstream closed".into(),
                                                },
                                            )))
                                            .await;
                                        break;
                                    }
                                    TestCodexWebSocketBehavior::Silent => {}
                                }
                            }
                        })
                    }
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                    let requests = std::sync::Arc::clone(&http_requests_for_route);
                    let observations = std::sync::Arc::clone(&http_observations_for_route);
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        let workspace = headers
                            .get("chatgpt-account-id")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        let body = serde_json::from_slice::<Value>(&body).unwrap();
                        observations
                            .lock()
                            .unwrap()
                            .push((authorization, workspace, body));
                        let sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-http\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"fallback\"}\n\n",
                            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-http\",\"status\":\"completed\",\"output\":[]}}\n\n",
                            "data: [DONE]\n\n"
                        );
                        let provider_failure_sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-provider-failed\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-provider-failed\",\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"busy\"}}}\n\n",
                            "data: [DONE]\n\n"
                        );
                        let client_failure_sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-client-failed\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-client-failed\",\"status\":\"failed\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad tool\"}}}\n\n",
                            "data: [DONE]\n\n"
                        );
                        let business_without_terminal_sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-interrupted\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"committed-http\"}\n\n"
                        );
                        let malformed_sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-malformed\",\"status\":\"in_progress\"}}\n\n",
                            "data: {not-json}\n\n"
                        );
                        let business_then_provider_failure_sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-batch-failed\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"discard-me\"}\n\n",
                            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-batch-failed\",\"status\":\"failed\",\"error\":{\"type\":\"server_error\",\"message\":\"busy\"}}}\n\n"
                        );
                        let (status, body) = match behavior {
                            TestCodexWebSocketBehavior::CloseSizeThenDelayHttpFirstEvent => (
                                StatusCode::OK,
                                Body::from_stream(futures_util::stream::once(async move {
                                    tokio::time::sleep(Duration::from_millis(200)).await;
                                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                                        sse.as_bytes(),
                                    ))
                                })),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenStallHttpErrorBody => (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Body::from_stream(futures_util::stream::pending::<Result<
                                    Bytes,
                                    std::convert::Infallible,
                                >>()),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenEof => (
                                StatusCode::OK,
                                Body::from(business_without_terminal_sse),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenStall => (
                                StatusCode::OK,
                                Body::from_stream(
                                    futures_util::stream::once(async move {
                                        Ok::<_, std::convert::Infallible>(Bytes::from_static(
                                            business_without_terminal_sse.as_bytes(),
                                        ))
                                    })
                                    .chain(futures_util::stream::pending::<Result<
                                        Bytes,
                                        std::convert::Infallible,
                                    >>()),
                                ),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenHttpMalformed => {
                                (StatusCode::OK, Body::from(malformed_sse))
                            }
                            TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenProviderFailure => {
                                (StatusCode::OK, Body::from(business_then_provider_failure_sse))
                            }
                            TestCodexWebSocketBehavior::CloseSizeThenHttpRateLimited => (
                                StatusCode::TOO_MANY_REQUESTS,
                                Body::from(r#"{\"error\":{\"message\":\"rate limited\"}}"#),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenHttpServerError => (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Body::from(r#"{\"error\":{\"message\":\"unavailable\"}}"#),
                            ),
                            TestCodexWebSocketBehavior::CloseSizeThenHttpProviderFailure => {
                                (StatusCode::OK, Body::from(provider_failure_sse))
                            }
                            TestCodexWebSocketBehavior::CloseSizeThenHttpClientFailure => {
                                (StatusCode::OK, Body::from(client_failure_sse))
                            }
                            _ => (StatusCode::OK, Body::from(sse)),
                        };
                        let mut response = Response::new(body);
                        *response.status_mut() = status;
                        response.headers_mut().insert(
                            CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        );
                        response
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        TestCodexUpstream {
            address,
            websocket_connections,
            websocket_requests,
            http_requests,
            http_observations,
            server,
        }
    }

    struct TestCodexRefreshEndpoint {
        url: String,
        requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn spawn_test_codex_refresh_endpoint(access_token: String) -> TestCodexRefreshEndpoint {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                let access_token = access_token.clone();
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "access_token": access_token,
                        "refresh_token": "rotated-refresh-token",
                        "token_type": "Bearer",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestCodexRefreshEndpoint {
            url: format!("http://{address}/token"),
            requests,
            server,
        }
    }

    async fn configure_codex_refresh_test_account(
        state: &ServerState,
        name: &str,
        token_url: String,
    ) {
        let account_id = format!("{name}-account");
        let active_account_id = account_id.clone();
        let workspace_id = format!("{name}-workspace");
        let email = format!("{name}@example.com");
        let access_token = format!("{name}-access-token");
        let refresh_token = format!("{name}-initial-refresh-token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
                    id: Some(account_id),
                    provider_type: ProviderType::CodexOAuth,
                    email: Some(email),
                    access_token: Some(access_token),
                    refresh_token: Some(refresh_token),
                    id_token: None,
                    token_type: Some("Bearer".to_string()),
                    api_key: None,
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: Some(json!({
                        "verifiedOpenAiClaims": {
                            "subject": format!("subject-{workspace_id}"),
                            "chatgpt_account_id": workspace_id.clone()
                        },
                        "codexWorkspaceProvenance": {
                            "workspaceId": workspace_id,
                            "source": "test_fixture"
                        }
                    })),
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
                if accounts.active_codex_oauth_account_id.is_none() {
                    accounts
                        .select_active_codex_oauth_account(&active_account_id)
                        .expect("inserted Codex refresh test account must be selectable");
                }
            })
            .await
            .unwrap();
    }

    struct TestUnauthorizedCodexUpstream {
        address: std::net::SocketAddr,
        authorizations: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn spawn_test_unauthorized_codex_upstream(
        refreshed_access_token: String,
        success_content_type: &'static str,
        success_body: String,
    ) -> TestUnauthorizedCodexUpstream {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let authorizations_for_route = std::sync::Arc::clone(&authorizations);
        let app = axum::Router::new()
            .route(
                "/ws",
                axum::routing::get(|ws: WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            if matches!(message, AxumWsMessage::Text(_) | AxumWsMessage::Binary(_))
                            {
                                let _ = socket
                                    .send(AxumWsMessage::Close(Some(
                                        axum::extract::ws::CloseFrame {
                                            code: 1009,
                                            reason: "message too big".into(),
                                        },
                                    )))
                                    .await;
                                break;
                            }
                        }
                    })
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(move |headers: HeaderMap| {
                    let authorizations = std::sync::Arc::clone(&authorizations_for_route);
                    let refreshed_access_token = refreshed_access_token.clone();
                    let success_body = success_body.clone();
                    async move {
                        let authorization = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        authorizations.lock().unwrap().push(authorization.clone());
                        if authorization != format!("Bearer {refreshed_access_token}") {
                            return Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({"error": {"type": "authentication_error"}}).to_string(),
                                ))
                                .unwrap();
                        }
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, success_content_type)
                            .body(Body::from(success_body))
                            .unwrap()
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestUnauthorizedCodexUpstream {
            address,
            authorizations,
            server,
        }
    }

    async fn spawn_test_oversized_codex_upstream() -> TestUnauthorizedCodexUpstream {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let authorizations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let authorizations_for_route = std::sync::Arc::clone(&authorizations);
        let app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |headers: HeaderMap| {
                let authorizations = std::sync::Arc::clone(&authorizations_for_route);
                async move {
                    let authorization = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    authorizations.lock().unwrap().push(authorization);
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "text/event-stream")
                        .header(
                            axum::http::header::CONTENT_LENGTH,
                            (crate::proxy::MEDIA_RESPONSE_BODY_LIMIT_BYTES + 1).to_string(),
                        )
                        .body(Body::from_stream(futures_util::stream::pending::<
                            Result<Bytes, std::io::Error>,
                        >()))
                        .unwrap()
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestUnauthorizedCodexUpstream {
            address,
            authorizations,
            server,
        }
    }

    #[test]
    fn share_execution_rejects_explicit_account_usage_block() {
        let mut provider = stored_provider(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            json!({}),
            Some("share-account"),
        );
        provider
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .auth_identity_generation = Some(1);
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
            "status": "active",
            "bindings": [{
                "app": "codex",
                "providerId": provider_id,
                "providerType": "codex_oauth"
            }]
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
                "profile": {
                    "verifiedOpenAiClaims": {
                        "subject": "share-account-subject",
                        "chatgpt_account_id": "share-account-workspace"
                    },
                    "workspaces": [{
                        "id": "share-account-workspace",
                        "name": "Share Account Workspace"
                    }]
                },
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
    fn share_image_generation_honors_provider_capability() {
        let (providers, shares, accounts) =
            share_image_generation_fixture(ProviderType::CodexOAuth, None);
        let error = select_share_image_generation_execution(
            &providers,
            &shares,
            &accounts,
            "codex_oauth-share-image",
        )
        .expect_err("Codex image generation must be disabled by default for a Share");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image generation enabled"));

        let (providers, shares, accounts) =
            share_image_generation_fixture(ProviderType::CodexOAuth, Some(true));
        let (execution, _) = select_share_image_generation_execution(
            &providers,
            &shares,
            &accounts,
            "codex_oauth-share-image",
        )
        .expect("an explicitly enabled Codex Provider must support Share image generation");
        assert_eq!(execution.stored.provider_type, ProviderType::CodexOAuth);

        let (providers, shares, accounts) =
            share_image_generation_fixture(ProviderType::GrokOAuth, None);
        let (execution, _) = select_share_image_generation_execution(
            &providers,
            &shares,
            &accounts,
            "grok_oauth-share-image",
        )
        .expect("Grok Share image generation must remain enabled");
        assert_eq!(execution.stored.provider_type, ProviderType::GrokOAuth);
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
    fn grok_oauth_account_headers_cannot_override_cli_contract() {
        for name in [
            "x-xai-token-auth",
            "X-Grok-Client-Identifier",
            "x-grok-client-version",
            "x-grok-client-surface",
            "x-authenticateresponse",
            "x-grok-conv-id",
            "x-grok-cache-identity",
            "x-grok-turn-idx",
        ] {
            assert!(account_header_override_blocked(
                name,
                ProviderType::GrokOAuth
            ));
        }
        assert!(!account_header_override_blocked(
            "x-enterprise-sso",
            ProviderType::GrokOAuth
        ));
    }

    #[test]
    fn grok_media_conversation_scope_separates_shares_and_users() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::GrokOAuth,
            json!({}),
            Some("grok-account"),
        );
        let first_scope =
            grok_tenant_scope_parts(Some("share-a"), Some("user-a@example.com"), &stored).unwrap();
        let second_scope =
            grok_tenant_scope_parts(Some("share-a"), Some("user-b@example.com"), &stored).unwrap();

        let first = super::super::grok::namespace_session_id(Some(&first_scope), "client-session");
        let second =
            super::super::grok::namespace_session_id(Some(&second_scope), "client-session");
        assert_ne!(first, "client-session");
        assert_ne!(first, second);
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
    fn refresh_failures_keep_status_without_exposing_provider_diagnostics() {
        let proxy_error =
            managed_account_refresh_error_to_proxy_error(ManagedAccountRefreshError::Refresh {
                status_code: 429,
                message: "rate limited; refresh_token=oauth-secret".to_string(),
                retry_after_ms: Some(5_000),
            });

        assert_eq!(proxy_error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            proxy_error.client_message(),
            "managed account refresh was rate limited; retry later"
        );
        assert!(!proxy_error.message.contains("oauth-secret"));
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
            parallel.client_message(),
            "Share parallel limit has been reached. [ParallelLimit]"
        );
        assert_eq!(parallel.retry_after_seconds(), Some(1));
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
    fn grok_version_gate_error_is_rewritten_for_admin() {
        let stored = stored_provider(
            AppKind::Codex,
            ProviderType::GrokOAuth,
            json!({}),
            Some("acct-1"),
        );
        let body = Bytes::from_static(
            br#"{"error":{"type":"invalid_request_error","message":"grok client version is too old; please update"}}"#,
        );

        let (rewritten, changed) =
            maybe_rewrite_grok_cli_version_gate_body(StatusCode::BAD_REQUEST, &stored, body);
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap();

        assert!(changed);
        assert!(message.contains("cc-switch-server admin"));
        assert!(message.contains("CC_SWITCH_GROK_CLI_VERSION"));
        assert!(!message.contains("please update"));

        let (rewritten, changed) = maybe_rewrite_grok_cli_version_gate_body(
            StatusCode::BAD_REQUEST,
            &stored,
            Bytes::from_static(br#"{"error":"x-grok-client-version is unsupported"}"#),
        );
        let value: Value = serde_json::from_slice(&rewritten).unwrap();
        assert!(changed);
        assert_eq!(
            value.pointer("/error/type").and_then(Value::as_str),
            Some("grok_cli_version_gate")
        );
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
        assert!(normalized.pointer("/input/1/id").is_none());
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
                "tool_choice": {"type": "image_generation"},
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
        assert!(normalized.get("tool_choice").is_none());
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
            "max_tokens": 128,
            "max_completion_tokens": 128,
            "max_output_tokens": 128,
            "reasoning_effort": "high",
            "temperature": 0.2,
            "prompt_cache_retention": "24h",
            "metadata": {"source": "cursor"},
            "safety_identifier": "droid-user",
            "previous_response_id": "resp_previous"
        });
        let normalized =
            normalize_codex_oauth_responses_body(body, None, CodexImageToolStripPolicy::Never);
        for field in [
            "max_tokens",
            "max_completion_tokens",
            "max_output_tokens",
            "reasoning_effort",
            "temperature",
            "prompt_cache_retention",
            "metadata",
            "safety_identifier",
            "previous_response_id",
        ] {
            assert!(normalized.get(field).is_none(), "field {field} survived");
        }
        assert_eq!(
            normalized.pointer("/reasoning/effort"),
            Some(&json!("high"))
        );
    }

    #[test]
    fn codex_oauth_request_sanitizer_normalizes_proven_incompatible_shapes() {
        let normalized = normalize_codex_oauth_responses_body(
            json!({
                "model": "gpt-5.1-codex",
                "input": [
                    "resp_stored_response",
                    "plain user input",
                    {"type": "item_reference", "id": "item_1"},
                    {"type": "message", "id": "msg_stored_message", "role": "system", "content": "be precise"},
                    {"type": "message", "id": "client-message", "role": "user", "content": "hello"},
                    {"type": "function_call", "id": "fc_stored_call", "name": "lookup"},
                    {"type": "reasoning", "id": "rs_stored_reasoning", "summary": []}
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "description": "lookup a value",
                        "parameters": {"type": "object"},
                        "strict": true
                    }
                }],
                "tool_choice": {"type": "function", "function": {"name": "lookup"}}
            }),
            None,
            CodexImageToolStripPolicy::Never,
        );

        assert_eq!(normalized["input"].as_array().unwrap().len(), 5);
        assert_eq!(normalized["input"][0], "plain user input");
        assert_eq!(normalized["input"][1]["role"], "developer");
        assert!(normalized["input"][1].get("id").is_none());
        assert!(normalized["input"][2].get("id").is_none());
        assert!(normalized["input"][3].get("id").is_none());
        assert!(normalized["input"][4].get("id").is_none());
        assert_eq!(normalized["tools"][0]["name"], "lookup");
        assert!(normalized["tools"][0].get("function").is_none());
        assert_eq!(
            normalized["tool_choice"],
            json!({"type": "function", "name": "lookup"})
        );
    }

    #[test]
    fn codex_oauth_request_sanitizer_removes_invalid_tool_choice() {
        let mut body = json!({"tool_choice": {"type": "function", "function": {}}});
        sanitize_codex_oauth_request_body(&mut body);
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn codex_oauth_request_sanitizer_removes_unknown_function_tool_choice() {
        let mut body = json!({
            "tools": [{"type": "function", "name": "known", "parameters": {"type": "object"}}],
            "tool_choice": {"type": "function", "name": "unknown"}
        });
        sanitize_codex_oauth_request_body(&mut body);
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn codex_oauth_request_sanitizer_filters_malformed_and_unsupported_tools() {
        let mut body = json!({
            "tools": [
                null,
                "invalid",
                {"type": "function", "parameters": {"type": "object"}},
                {"type": "unsupported_hosted_tool"},
                {"type": "web_search_preview"},
                {"type": "custom", "name": "exec", "format": {"type": "text"}},
                {"type": "function", "function": {"name": "lookup"}}
            ]
        });

        sanitize_codex_oauth_request_body(&mut body);

        assert_eq!(body["tools"].as_array().unwrap().len(), 3);
        assert_eq!(body["tools"][0]["type"], "web_search_preview");
        assert_eq!(body["tools"][1]["type"], "custom");
        assert_eq!(body["tools"][2]["name"], "lookup");
        assert_eq!(
            body["tools"][2]["parameters"],
            json!({"type": "object", "properties": {}})
        );
        assert!(body["tools"][2].get("function").is_none());
    }

    #[test]
    fn codex_oauth_request_sanitizer_preserves_namespace_tool_choice() {
        let mut body = json!({
            "tools": [{
                "type": "namespace",
                "name": "mcp_files",
                "tools": [{"name": "read"}]
            }],
            "tool_choice": {
                "type": "function",
                "name": "read",
                "namespace": "mcp_files"
            }
        });

        sanitize_codex_oauth_request_body(&mut body);

        assert_eq!(
            body["tool_choice"],
            json!({"type": "function", "name": "read", "namespace": "mcp_files"})
        );
    }

    #[test]
    fn codex_oauth_request_sanitizer_converts_untyped_system_messages() {
        let mut body = json!({
            "input": [{"role": "system", "content": "be precise"}],
            "reasoning_effort": "high"
        });
        sanitize_codex_oauth_request_body(&mut body);
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("high")));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn codex_websocket_request_uses_the_same_sanitizer() {
        let prepared = prepare_codex_responses_websocket_request(TungsteniteMessage::Text(
            json!({
                "type": "response.create",
                "response": {
                    "model": "gpt-5.5",
                    "input": [
                        "msg_stored_message",
                        {"type": "item_reference", "id": "item_1"},
                        {"type": "message", "id": "msg_stored_message", "role": "system", "content": "be precise"}
                    ],
                    "previous_response_id": "resp_previous",
                    "reasoning_effort": "max",
                    "metadata": {"source": "cursor"},
                    "tools": [{
                        "type": "function",
                        "function": {"name": "lookup", "parameters": {"type": "object"}}
                    }]
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let TungsteniteMessage::Text(prepared) = prepared else {
            panic!("prepared request must remain a text frame");
        };
        let prepared: Value = serde_json::from_str(&prepared).unwrap();
        assert_eq!(prepared["response"]["input"].as_array().unwrap().len(), 1);
        assert_eq!(prepared["response"]["input"][0]["role"], "developer");
        assert!(prepared["response"]["input"][0].get("id").is_none());
        assert!(prepared["response"].get("previous_response_id").is_none());
        assert!(prepared["response"].get("metadata").is_none());
        assert_eq!(
            prepared["response"].pointer("/reasoning/effort"),
            Some(&json!("xhigh"))
        );
        assert_eq!(prepared["response"]["tools"][0]["name"], "lookup");
        assert!(prepared["response"]["tools"][0].get("function").is_none());
    }

    #[tokio::test]
    async fn codex_websocket_request_enforces_remote_image_count_before_fetching() {
        let content = (0..=crate::proxy::remote_image::MAX_IMAGES_PER_REQUEST)
            .map(|index| {
                json!({
                    "type": "input_image",
                    "image_url": format!("https://example.com/{index}.png")
                })
            })
            .collect::<Vec<_>>();
        let error = prepare_codex_responses_websocket_request(TungsteniteMessage::Text(
            json!({
                "type": "response.create",
                "response": {"input": [{"type": "message", "content": content}]}
            })
            .to_string(),
        ))
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image limit"));
    }

    #[test]
    fn share_requests_always_pin_their_provider() {
        let headers = HeaderMap::new();
        let context = UsageLogContext {
            share_id: Some("share-one-account".to_string()),
            ..UsageLogContext::default()
        };
        assert!(request_is_provider_pinned(&headers, &context));
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
            br#"{"model":"gpt-5.5","stream":true,"store":true,"prompt_cache_key":"pck","temperature":0.2,"max_output_tokens":128,"input":[{"type":"message","role":"user"},{"type":"compaction_trigger"}]}"#,
        );
        assert!(codex_responses_body_has_compaction_trigger(&body));
        let normalized = normalize_codex_oauth_compact_body_bytes(&body).unwrap();
        let value: Value = serde_json::from_slice(&normalized).unwrap();
        assert!(value.get("stream").is_none());
        assert!(value.get("store").is_none());
        assert!(value.get("prompt_cache_key").is_none());
        assert!(value.get("temperature").is_none());
        assert!(value.get("max_output_tokens").is_none());
        assert_eq!(
            codex_compact_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );
    }

    #[test]
    fn codex_alpha_search_strips_only_internal_prompt_cache_key() {
        let normalized = normalize_codex_alpha_search_body(
            br#"{"query":"rust","prompt_cache_key":"internal","filters":{"path":"src"}}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&normalized).unwrap();
        assert_eq!(value["query"], "rust");
        assert_eq!(value.pointer("/filters/path"), Some(&json!("src")));
        assert!(value.get("prompt_cache_key").is_none());
    }

    #[test]
    fn codex_alpha_search_decodes_compressed_requests_before_normalizing() {
        use std::io::Write;

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(br#"{"query":"rust","prompt_cache_key":"internal"}"#)
            .unwrap();
        let body = Bytes::from(encoder.finish().unwrap());
        let mut headers = HeaderMap::new();
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));

        let prepared = prepare_codex_alpha_search_body(&headers, body).unwrap();
        let value: Value = serde_json::from_slice(&prepared).unwrap();
        assert_eq!(value["query"], "rust");
        assert!(value.get("prompt_cache_key").is_none());
    }

    #[test]
    fn codex_alpha_search_rejects_decompressed_requests_above_the_proxy_limit() {
        use std::io::Write;

        let oversized = json!({"query": "x".repeat(PROXY_REQUEST_BODY_LIMIT_BYTES)});
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&serde_json::to_vec(&oversized).unwrap())
            .unwrap();
        let body = Bytes::from(encoder.finish().unwrap());
        assert!(body.len() < PROXY_REQUEST_BODY_LIMIT_BYTES);
        let mut headers = HeaderMap::new();
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));

        let error = prepare_codex_alpha_search_body(&headers, body).unwrap_err();

        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(error.message.contains("decoded request body exceeds"));
        assert!(error.message.contains("2097152"));
    }

    #[test]
    fn codex_responses_lite_normalizes_body_and_forwards_only_safe_headers() {
        let metadata_key = CODEX_RESPONSES_LITE_WS_METADATA;
        let mut body = json!({
            "client_metadata": {(metadata_key): "true", "keep": "value"},
            "tools": [
                {"type": "image_generation"},
                {"type": "function", "name": "lookup", "parameters": {"type": "object"}}
            ]
        });
        assert!(codex_responses_lite_requested_value(&body));
        normalize_codex_responses_lite_body(&mut body, true, true);
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(
            body.pointer("/reasoning/context"),
            Some(&json!("all_turns"))
        );
        assert_eq!(body.pointer("/client_metadata/keep"), Some(&json!("value")));
        assert!(body
            .pointer(&format!("/client_metadata/{metadata_key}"))
            .is_none());
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body.pointer("/tools/0/name"), Some(&json!("lookup")));

        let mut headers = HeaderMap::new();
        headers.insert(
            CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
        headers.insert("x-codex-turn-state", HeaderValue::from_static("turn-1"));
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-secret"),
        );
        headers.insert("cookie", HeaderValue::from_static("private=value"));
        headers.insert("x-untrusted", HeaderValue::from_static("drop"));
        assert!(codex_responses_lite_requested(&headers, b"{}"));

        let mut forwarded = Vec::new();
        append_codex_client_request_headers(&mut forwarded, &headers, true);
        assert!(forwarded.contains(&(CODEX_RESPONSES_LITE_HEADER, "true".to_string())));
        assert!(forwarded.contains(&("x-codex-turn-state", "turn-1".to_string())));
        assert!(!forwarded
            .iter()
            .any(|(name, _)| { matches!(*name, "authorization" | "cookie" | "x-untrusted") }));
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
    fn codex_images_edit_maps_images_mask_and_string_form_fields() {
        let prepared = codex_images_edit_request_from_value(json!({
            "prompt": "replace the sky",
            "model": "gpt-image-2",
            "image": {"image_url": "data:image/png;base64,aW1hZ2U="},
            "mask": {"image_url": "data:image/png;base64,bWFzaw=="},
            "stream": "true",
            "n": "2",
            "input_fidelity": "high"
        }))
        .unwrap();
        let request: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(prepared.stream);
        assert_eq!(request.pointer("/tools/0/action"), Some(&json!("edit")));
        assert_eq!(request.pointer("/tools/0/n"), Some(&json!(2)));
        assert_eq!(
            request.pointer("/tools/0/input_fidelity"),
            Some(&json!("high"))
        );
        assert_eq!(
            request.pointer("/tools/0/input_image_mask/image_url"),
            Some(&json!("data:image/png;base64,bWFzaw=="))
        );
        assert_eq!(
            request.pointer("/input/0/content/1/image_url"),
            Some(&json!("data:image/png;base64,aW1hZ2U="))
        );
    }

    #[tokio::test]
    async fn codex_images_edit_accepts_octet_stream_with_valid_image_signature() {
        let boundary = "cc-switch-image-boundary";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nreplace the sky\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"input.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"\x89PNG\r\n\x1a\nfixture");
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}")).unwrap(),
        );

        let prepared = codex_images_edit_request(&headers, Bytes::from(body))
            .await
            .unwrap();
        let request: Value = serde_json::from_slice(&prepared.body).unwrap();

        assert!(request
            .pointer("/input/0/content/1/image_url")
            .and_then(Value::as_str)
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));
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

    #[tokio::test]
    async fn responses_lifecycle_events_do_not_extend_first_semantic_event_deadline() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(|| async move {
                let events = futures_util::stream::unfold(0_u8, |index| async move {
                    if index >= 5 {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    let event = if index < 4 {
                        json!({
                            "type": "response.created",
                            "response": {
                                "id": format!("resp-lifecycle-{index}"),
                                "status": "in_progress"
                            }
                        })
                    } else {
                        json!({
                            "type": "response.output_text.delta",
                            "delta": "too late"
                        })
                    };
                    Some((
                        Ok::<_, std::convert::Infallible>(Bytes::from(format!(
                            "data: {event}\n\n"
                        ))),
                        index + 1,
                    ))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(events))
                    .unwrap()
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (state, mut execution) =
            codex_bridge_test_context("codex-semantic-deadline", format!("http://{address}")).await;
        let plan = std::sync::Arc::make_mut(&mut execution.plan);
        plan.transport_policy.stream_first_byte_timeout_ms = Some(100);
        plan.transport_policy.stream_idle_timeout_ms = Some(500);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let result = tokio::time::timeout(
            Duration::from_millis(350),
            forward_with_attempt(
                state,
                ProxyRoute::CodexResponses,
                None,
                headers,
                Bytes::from_static(br#"{"model":"gpt-5.4","input":"ping","stream":true}"#),
                ForwardAttemptContext {
                    execution: Some(execution),
                    ..ForwardAttemptContext::default()
                },
            ),
        )
        .await
        .expect("the absolute first semantic event deadline must terminate priming");
        let error = match result {
            Ok(_) => panic!("lifecycle events must not satisfy the semantic commit deadline"),
            Err(error) => error,
        };
        assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
        assert!(error.message.contains("first byte timeout"));

        server.abort();
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

    #[tokio::test]
    async fn grok_websocket_handshake_updates_entitlement_and_cooldown() {
        let state = forwarder_test_state("grok-ws-handshake-state");
        let execution = install_grok_test_execution(
            &state,
            "grok-ws-handshake-state",
            super::super::grok::default_base_url().to_string(),
            None,
            "grok-ws-handshake-access",
            None,
            &[GrokAccountCapability::Websocket],
        )
        .await;
        let account_id = execution.managed_account_id().unwrap().to_string();
        let before = crate::infra::time::now_ms() as i64;
        let response = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(403)
            .header("xai-subscription-tier", "supergrok")
            .header("xai-entitlement-status", "blocked")
            .body(Some(br#"{"error":"subscription blocked"}"#.to_vec()))
            .unwrap();

        let error =
            responses_websocket_connect_error(&state, &execution, TungsteniteError::Http(response))
                .await;

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert_eq!(account.subscription_level.as_deref(), Some("supergrok"));
        assert_eq!(account.entitlement_status.as_deref(), Some("blocked"));
        assert!(account
            .rate_limited_until
            .is_some_and(|until| until >= before + 30 * 60_000));
    }

    #[test]
    fn websocket_handshake_client_errors_never_enable_http_fallback() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../assets/contract/openai-oauth-protocol.json"
        ))
        .unwrap();
        let statuses = |pointer: &str| {
            fixture
                .pointer(pointer)
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .map(|status| status.as_u64().unwrap() as u16)
                .collect::<Vec<_>>()
        };

        for status in statuses("/websocketFallback/handshakeNoFallbackStatuses") {
            let response = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(Some(Vec::new()))
                .unwrap();
            let error = TungsteniteError::Http(response);

            assert_eq!(
                websocket_connect_fallback_source(ResponsesWebsocketMode::Codex, &error),
                None,
                "HTTP {status} must remain an application/authentication response"
            );
        }

        for status in statuses("/websocketFallback/handshakeFallbackStatuses") {
            let response = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(Some(Vec::new()))
                .unwrap();
            let error = TungsteniteError::Http(response);

            assert_eq!(
                websocket_connect_fallback_source(ResponsesWebsocketMode::Codex, &error),
                Some("handshake_server_error")
            );
        }
    }

    #[test]
    fn websocket_http_fallback_extracts_flat_and_nested_response_create_bodies() {
        let flat = TungsteniteMessage::Text(
            r#"{"type":"response.create","model":"gpt-5.4","input":"ping"}"#.to_string(),
        );
        let nested = TungsteniteMessage::Text(
            r#"{"type":"response.create","response":{"model":"gpt-5.4","input":"ping"}}"#
                .to_string(),
        );

        for body in [
            responses_websocket_http_body(&flat).unwrap(),
            responses_websocket_http_body(&nested).unwrap(),
        ] {
            assert_eq!(body["model"], "gpt-5.4");
            assert_eq!(body["input"], "ping");
            assert!(body.get("type").is_none());
        }
    }

    #[test]
    fn codex_websocket_single_model_updates_flat_response_create() {
        let transformed = transform_responses_websocket_request(
            r#"{"type":"response.create","model":"gpt-5.4","input":"ping"}"#,
            ResponsesWebsocketMode::Codex,
            Some("session-1"),
            Some("gpt-5.5"),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&transformed).unwrap();

        assert_eq!(value.get("model").and_then(Value::as_str), Some("gpt-5.5"));
    }

    #[test]
    fn codex_http_fallback_sse_decoder_handles_fragmented_crlf_and_ignores_metadata() {
        let mut decoder = CodexHttpFallbackSseDecoder::default();
        assert!(decoder
            .push(b"event: response.created\r\ndata: {\"type\":\"response.cre")
            .unwrap()
            .is_empty());
        let payloads = decoder
            .push(b"ated\",\"response\":{}}\r\n\r\n: keepalive\r\n\r\n")
            .unwrap();

        assert_eq!(payloads.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&payloads[0]).unwrap()["type"],
            "response.created"
        );
        assert!(decoder.finish().unwrap().is_empty());
    }

    #[test]
    fn codex_http_fallback_sse_decoder_bounds_pending_and_final_events() {
        let mut decoder = CodexHttpFallbackSseDecoder {
            buffer: b"data:{}\n\ndata:123456789".to_vec(),
        };
        let error = decoder.drain_with_limit(false, 8).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);

        let mut fragmented_delimiter = CodexHttpFallbackSseDecoder {
            buffer: b"12345678\r\n\r".to_vec(),
        };
        assert!(fragmented_delimiter.drain_with_limit(false, 8).is_ok());
        let error = fragmented_delimiter.drain_with_limit(true, 8).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);

        let mut final_event = CodexHttpFallbackSseDecoder {
            buffer: b"123456789".to_vec(),
        };
        let error = final_event.drain_with_limit(true, 8).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn semantic_prelude_prioritizes_provider_failure_in_same_batch() {
        let failure = SemanticFailure {
            origin: FailureOrigin::Provider,
            code: "server_error".to_string(),
            message: "busy".to_string(),
        };

        assert_eq!(
            semantic_prelude_decision(&[
                SemanticObservation::Lifecycle,
                SemanticObservation::Business,
                SemanticObservation::Failure(failure.clone()),
            ]),
            Some(SemanticObservation::Failure(failure))
        );
    }

    #[test]
    fn codex_http_fallback_detects_provider_failure_after_same_batch_business() {
        let payloads = vec![
            json!({"type": "response.output_text.delta", "delta": "discard"}).to_string(),
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {"type": "server_error", "message": "busy"}
                }
            })
            .to_string(),
        ];

        let (index, failure) = codex_http_fallback_batch_provider_failure(&payloads)
            .unwrap()
            .expect("provider failure");
        assert_eq!(index, 1);
        assert_eq!(failure.origin, FailureOrigin::Provider);
    }

    #[test]
    fn codex_http_fallback_ignores_events_after_same_batch_terminal() {
        let payloads = vec![
            json!({
                "type": "response.completed",
                "response": {"status": "completed", "output": []}
            })
            .to_string(),
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {"type": "server_error", "message": "late"}
                }
            })
            .to_string(),
        ];

        assert!(codex_http_fallback_batch_provider_failure(&payloads)
            .unwrap()
            .is_none());
    }

    #[test]
    fn codex_http_fallback_semantic_prelude_enforces_message_bound() {
        let mut pending = Vec::new();
        for _ in 0..MAX_RESPONSES_SEMANTIC_PRELUDE_MESSAGES {
            buffer_codex_http_fallback_semantic_prelude(&mut pending, "x".to_string()).unwrap();
        }
        assert_eq!(pending.len(), MAX_RESPONSES_SEMANTIC_PRELUDE_MESSAGES);

        let error =
            buffer_codex_http_fallback_semantic_prelude(&mut pending, "x".to_string()).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("lifecycle prelude exceeded"));
    }

    #[test]
    fn codex_http_fallback_semantic_prelude_enforces_byte_bound() {
        let mut pending = Vec::new();
        buffer_codex_http_fallback_semantic_prelude(
            &mut pending,
            "x".repeat(MAX_RESPONSES_SEMANTIC_PRELUDE_BYTES),
        )
        .unwrap();

        let error =
            buffer_codex_http_fallback_semantic_prelude(&mut pending, "x".to_string()).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("lifecycle prelude exceeded"));
    }

    #[test]
    fn websocket_fallback_rejects_policy_close_but_accepts_size_and_transport_close() {
        let close = |code| {
            TungsteniteMessage::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code,
                reason: "test".into(),
            }))
        };

        assert_eq!(
            websocket_close_fallback_source(&close(CloseCode::Size), false),
            Some("message_too_big")
        );
        assert_eq!(
            websocket_close_fallback_source(&close(CloseCode::Error), false),
            Some("closed_before_event")
        );
        assert_eq!(
            websocket_close_fallback_source(&close(CloseCode::Policy), true),
            None
        );
    }

    #[tokio::test]
    async fn responses_websocket_pool_enforces_capacity_idle_ttl_and_max_age() {
        let policy = ResponsesWebSocketPoolPolicy {
            max_connections: 1,
            idle_timeout: Duration::from_secs(1),
            max_age: Duration::from_secs(10),
        };
        let (socket_a, server_a) = test_responses_upstream_socket().await;
        let (socket_b, server_b) = test_responses_upstream_socket().await;
        let mut pool = ResponsesWebSocketPool::default();
        pool.release_with_policy(
            "a".to_string(),
            CachedResponsesWebSocket {
                socket: socket_a,
                created_at: Instant::now(),
                last_used_at: Instant::now(),
            },
            policy,
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
        pool.release_with_policy(
            "b".to_string(),
            CachedResponsesWebSocket {
                socket: socket_b,
                created_at: Instant::now(),
                last_used_at: Instant::now(),
            },
            policy,
        );

        assert_eq!(pool.total, 1);
        assert!(pool.acquire_with_policy("a", policy).is_none());
        let entry = pool.acquire_with_policy("b", policy).unwrap();
        pool.release_with_policy("b".to_string(), entry, policy);
        pool.entries
            .get_mut("b")
            .unwrap()
            .front_mut()
            .unwrap()
            .last_used_at = Instant::now() - Duration::from_secs(2);
        assert!(pool.acquire_with_policy("b", policy).is_none());
        assert_eq!(pool.total, 0);

        let (socket_c, server_c) = test_responses_upstream_socket().await;
        pool.release_with_policy(
            "c".to_string(),
            CachedResponsesWebSocket {
                socket: socket_c,
                created_at: Instant::now() - Duration::from_secs(11),
                last_used_at: Instant::now(),
            },
            policy,
        );
        assert_eq!(pool.total, 0);

        server_a.abort();
        server_b.abort();
        server_c.abort();
    }

    #[tokio::test]
    async fn codex_non_stream_401_forces_one_signed_refresh_and_replays_same_provider() {
        let name = "codex-http-401";
        let workspace = format!("{name}-workspace");
        let refreshed_access_token = signed_openai_access_token(&workspace).await;
        let upstream = spawn_test_unauthorized_codex_upstream(
            refreshed_access_token.clone(),
            "application/json",
            json!({
                "id": "resp-refreshed-http",
                "object": "response",
                "status": "completed",
                "model": "gpt-5.4",
                "output": [],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            })
            .to_string(),
        )
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
        assert!(supports_forced_auth_refresh(
            ProxyRoute::CodexResponses,
            &execution
        ));
        assert!(execution.managed_account_target().is_some());
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"ping","stream":false}"#),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "refresh_requests={}, authorizations={:?}",
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            upstream.authorizations.lock().unwrap()
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["id"],
            "resp-refreshed-http"
        );
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {name}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );
        let usage = state.usage.read().await;
        let log = usage.logs.last().expect("Codex response usage log");
        assert_eq!(log.input_tokens, Some(1));
        assert_eq!(log.output_tokens, Some(1));
        assert_eq!(log.total_tokens, Some(2));

        upstream.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_sse_401_forces_one_signed_refresh_before_downstream_commit() {
        let name = "codex-sse-401";
        let workspace = format!("{name}-workspace");
        let refreshed_access_token = signed_openai_access_token(&workspace).await;
        let upstream = spawn_test_unauthorized_codex_upstream(
            refreshed_access_token.clone(),
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-refreshed-sse\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-refreshed-sse\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        )
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
        assert!(supports_forced_auth_refresh(
            ProxyRoute::CodexResponses,
            &execution
        ));
        assert!(execution.managed_account_target().is_some());
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"ping","stream":true}"#),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "refresh_requests={}, authorizations={:?}",
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            upstream.authorizations.lock().unwrap()
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("resp-refreshed-sse"));
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {name}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );
        let usage = state.usage.read().await;
        let log = usage.logs.last().expect("Codex SSE usage log");
        assert_eq!(log.input_tokens, Some(1));
        assert_eq!(log.output_tokens, Some(1));
        assert_eq!(log.total_tokens, Some(2));

        upstream.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_image_tool_retry_401_reenters_forced_refresh_flow() {
        let name = "codex-image-tool-retry-401";
        let workspace = format!("{name}-workspace");
        let refreshed_access_token = signed_openai_access_token(&workspace).await;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, bool)>::new()));
        let observations_for_route = std::sync::Arc::clone(&observations);
        let refreshed_for_route = refreshed_access_token.clone();
        let upstream_app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let observations = std::sync::Arc::clone(&observations_for_route);
                let refreshed_access_token = refreshed_for_route.clone();
                async move {
                    let authorization = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    let has_image_tool = body
                        .get("tools")
                        .and_then(Value::as_array)
                        .is_some_and(|tools| tools.iter().any(is_codex_image_generation_tool));
                    observations
                        .lock()
                        .unwrap()
                        .push((authorization.clone(), has_image_tool));

                    if has_image_tool {
                        return Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .header(CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({"error": {"message": "unsupported image_generation tool"}})
                                    .to_string(),
                            ))
                            .unwrap();
                    }
                    if authorization != format!("Bearer {refreshed_access_token}") {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .header(CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({"error": {"type": "authentication_error"}}).to_string(),
                            ))
                            .unwrap();
                    }
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({
                                "id": "resp-image-tool-refreshed",
                                "object": "response",
                                "status": "completed",
                                "model": "gpt-5.4",
                                "output": [],
                                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                            })
                            .to_string(),
                        ))
                        .unwrap()
                }
            }),
        );
        let upstream_server = tokio::spawn(async move {
            axum::serve(listener, upstream_app).await.unwrap();
        });
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let (state, mut execution) =
            codex_bridge_test_context(name, format!("http://{address}")).await;
        execution
            .stored
            .provider
            .meta
            .as_mut()
            .unwrap()
            .codex_image_tool_strip_policy = Some(CodexImageToolStripPolicy::OnError);
        configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(
                br#"{"model":"gpt-5.4","input":"ping","stream":false,"tools":[{"type":"image_generation"}]}"#,
            ),
            ForwardAttemptContext {
                execution: Some(execution),
                ..ForwardAttemptContext::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["id"],
            "resp-image-tool-refreshed"
        );
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            observations.lock().unwrap().as_slice(),
            [
                (format!("Bearer {name}-access-token"), true),
                (format!("Bearer {name}-access-token"), false),
                (format!("Bearer {refreshed_access_token}"), true),
                (format!("Bearer {refreshed_access_token}"), false),
            ]
        );

        upstream_server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_images_401_forces_one_signed_refresh_and_replays_same_account() {
        let name = "codex-images-401";
        let workspace = format!("{name}-workspace");
        let refreshed_access_token = signed_openai_access_token(&workspace).await;
        let upstream = spawn_test_unauthorized_codex_upstream(
            refreshed_access_token.clone(),
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_1\",\"type\":\"image_generation_call\",\"result\":\"aGVsbG8=\",\"output_format\":\"png\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"created_at\":1800000000,\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        )
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let prepared = codex_images_generation_request(
            br#"{"model":"gpt-image-2","prompt":"cat","response_format":"b64_json"}"#,
        )
        .unwrap();
        let response = forward_codex_images_request(
            state.clone(),
            execution,
            headers,
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(body.pointer("/data/0/b64_json"), Some(&json!("aGVsbG8=")));
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {name}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );
        let usage = state.usage.read().await;
        let log = usage.logs.last().expect("Codex image usage log");
        assert_eq!(log.input_tokens, Some(1));
        assert_eq!(log.output_tokens, Some(1));
        assert_eq!(log.total_tokens, Some(2));

        upstream.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_images_rejects_oversized_upstream_response_body() {
        let name = "codex-images-oversized-response";
        let upstream = spawn_test_oversized_codex_upstream().await;
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let prepared = codex_images_generation_request(
            br#"{"model":"gpt-image-2","prompt":"bounded response"}"#,
        )
        .unwrap();

        let error = forward_codex_images_request(
            state,
            execution,
            headers,
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            error.message,
            format!(
                "proxy upstream request failed: upstream response body exceeds the {} byte limit",
                crate::proxy::MEDIA_RESPONSE_BODY_LIMIT_BYTES
            )
        );
        assert_eq!(upstream.authorizations.lock().unwrap().len(), 1);
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_images_persistent_401_stays_on_active_account() {
        let primary = "codex-images-persistent-primary";
        let fallback = "codex-images-persistent-fallback";
        let refreshed_access_token =
            signed_openai_access_token(&format!("{primary}-workspace")).await;
        let rejected = spawn_test_unauthorized_codex_upstream(
            "never-accepted".to_string(),
            "application/json",
            "{}".to_string(),
        )
        .await;
        let fallback_access_token = "images-fallback-access-token";
        let accepted = spawn_test_unauthorized_codex_upstream(
            fallback_access_token.to_string(),
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_failover\",\"type\":\"image_generation_call\",\"result\":\"ZmFpbG92ZXI=\",\"output_format\":\"png\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"created_at\":1800000000,\"output\":[]}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        )
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let state = forwarder_test_state("codex-images-persistent-failover");
        configure_codex_refresh_test_account(&state, primary, refresh.url.clone()).await;
        insert_static_codex_test_account(&state, fallback, fallback_access_token).await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: primary.to_string(),
                    endpoint: format!("http://{}", rejected.address),
                    websocket_url: None,
                },
                TestCodexProviderSpec {
                    name: fallback.to_string(),
                    endpoint: format!("http://{}", accepted.address),
                    websocket_url: None,
                },
            ],
        )
        .await;
        let execution = executions.remove(0);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let accounts = state.accounts_snapshot().await;
        let in_flight = state.account_in_flight.snapshot();
        let account_in_flight_guard =
            acquire_account_in_flight(&state, &execution.stored, &accounts, &in_flight)
                .expect("primary image account lease");

        let prepared = codex_images_generation_request(
            br#"{"model":"gpt-image-2","prompt":"stay pinned","response_format":"b64_json"}"#,
        )
        .unwrap();
        let response = forward_codex_images_request(
            state.clone(),
            execution,
            headers,
            prepared,
            UsageLogContext::default(),
            account_in_flight_guard,
            None,
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"error": {"type": "authentication_error"}})
        );
        assert_eq!(
            rejected.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {primary}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );
        assert!(accepted.authorizations.lock().unwrap().is_empty());
        let primary_account = state
            .find_account_by_id(&format!("{primary}-account"))
            .await
            .unwrap();
        assert!(primary_account
            .rate_limited_until
            .is_some_and(|until| until > crate::infra::time::now_ms() as i64));
        let in_flight = state.account_in_flight.snapshot();
        assert_eq!(
            in_flight.current(ProviderType::CodexOAuth, &format!("{primary}-account")),
            0
        );
        assert_eq!(
            in_flight.current(ProviderType::CodexOAuth, &format!("{fallback}-account")),
            0
        );

        rejected.server.abort();
        accepted.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_inactive_account_is_rejected_before_any_image_outbound() {
        let active_name = "codex-active-outbound";
        let inactive_name = "codex-inactive-outbound";
        let active = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let inactive = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let state = forwarder_test_state("codex-inactive-zero-outbound");
        insert_static_codex_test_account(&state, active_name, "active-token").await;
        insert_static_codex_test_account(&state, inactive_name, "inactive-token").await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: active_name.to_string(),
                    endpoint: format!("http://{}", active.address),
                    websocket_url: None,
                },
                TestCodexProviderSpec {
                    name: inactive_name.to_string(),
                    endpoint: format!("http://{}", inactive.address),
                    websocket_url: None,
                },
            ],
        )
        .await;
        let inactive_execution = executions.remove(1);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let prepared = codex_images_edit_request_from_value(json!({
            "model": "gpt-image-2",
            "prompt": "must not leave the server",
            "image": "file:///must-not-be-inspected.png"
        }))
        .unwrap();

        let error = forward_codex_images_request(
            state,
            inactive_execution,
            headers,
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.message.contains("inactive account"));
        assert_eq!(
            active
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            inactive
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        active.server.abort();
        inactive.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_rechecks_active_account_before_each_response() {
        let name = "codex-websocket-active-turn";
        let (state, execution) =
            codex_bridge_test_context(name, "http://127.0.0.1:9".to_string()).await;
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "codex-websocket-next-account",
                        "providerType": "codex_oauth",
                        "accessToken": "next-account-token",
                        "profile": {
                            "verifiedOpenAiClaims": {
                                "subject": "next-account-subject",
                                "chatgpt_account_id": "next-account-workspace"
                            },
                            "codexWorkspaceProvenance": {
                                "workspaceId": "next-account-workspace",
                                "source": "test_fixture"
                            }
                        }
                    }))
                    .unwrap(),
                );
                accounts
                    .select_active_codex_oauth_account("codex-websocket-next-account")
                    .unwrap();
            })
            .await
            .unwrap();

        let error = ensure_responses_websocket_turn_allowed(
            &state,
            &execution,
            ResponsesWebsocketMode::Codex,
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.message.contains("inactive account"));
    }

    #[tokio::test]
    async fn codex_images_rejects_disallowed_client_before_remote_image_processing() {
        let (state, execution) = codex_bridge_test_context(
            "codex-images-disallowed-before-remote-image",
            "http://127.0.0.1:9".to_string(),
        )
        .await;
        let prepared = codex_images_edit_request_from_value(json!({
            "model": "gpt-image-2",
            "prompt": "edit",
            "image": "file:///must-not-be-inspected.png"
        }))
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("curl/8.0"));

        let error = forward_codex_images_request(
            state,
            execution,
            headers,
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(error.message.contains("allowed Codex client signature"));
    }

    #[tokio::test]
    async fn codex_images_rejects_degraded_credentials_before_remote_image_processing() {
        let name = "codex-images-degraded-before-remote-image";
        let (state, execution) =
            codex_bridge_test_context(name, "http://127.0.0.1:9".to_string()).await;
        let account_id = execution.managed_account_id().unwrap();
        let account = state.find_account_by_id(account_id).await.unwrap();
        state.inject_account_refresh_persist_failures(1);
        let commit = state
            .commit_native_refresh_success(
                &account,
                crate::domain::accounts::store::AccountRefreshUpdate {
                    access_token: Some(format!("{name}-rotated-access-token")),
                    ..Default::default()
                },
            )
            .await;
        assert!(commit.is_err());
        assert!(state.credential_persistence_degraded());
        let prepared = codex_images_edit_request_from_value(json!({
            "model": "gpt-image-2",
            "prompt": "edit",
            "image": "file:///must-not-be-inspected.png"
        }))
        .unwrap();

        let error = forward_codex_images_request(
            state,
            execution,
            HeaderMap::new(),
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.message.contains("durable persistence"));
    }

    #[tokio::test]
    async fn codex_websocket_persistent_handshake_401_stays_on_active_account() {
        let primary = "codex-ws-persistent-primary";
        let fallback = "codex-ws-persistent-fallback";
        let refreshed_access_token =
            signed_openai_access_token(&format!("{primary}-workspace")).await;
        let rejected = spawn_test_codex_websocket_auth_upstream(None).await;
        let fallback_access_token = "ws-fallback-access-token";
        let accepted = spawn_test_codex_websocket_auth_upstream(Some(format!(
            "Bearer {fallback_access_token}"
        )))
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let state = forwarder_test_state("codex-ws-persistent-failover");
        configure_codex_refresh_test_account(&state, primary, refresh.url.clone()).await;
        insert_static_codex_test_account(&state, fallback, fallback_access_token).await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: primary.to_string(),
                    endpoint: format!("http://{}", rejected.address),
                    websocket_url: Some(format!("ws://{}/ws", rejected.address)),
                },
                TestCodexProviderSpec {
                    name: fallback.to_string(),
                    endpoint: format!("http://{}", accepted.address),
                    websocket_url: Some(format!("ws://{}/ws", accepted.address)),
                },
            ],
        )
        .await;
        let execution = executions.remove(0);
        let state_for_assert = state.clone();
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", rejected.address),
            None,
            "persistent-auth-session",
        )
        .await;

        let (events, close_code) =
            send_test_bridge_request_with_close(bridge_address, "reject inactive fallback").await;
        wait_for_codex_account_leases_to_release(&state_for_assert, &[primary, fallback]).await;

        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_auth_error")
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            rejected.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {primary}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );
        assert!(accepted.authorizations.lock().unwrap().is_empty());
        assert_eq!(
            accepted
                .connections
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let primary_account = state_for_assert
            .find_account_by_id(&format!("{primary}-account"))
            .await
            .unwrap();
        assert!(primary_account
            .rate_limited_until
            .is_some_and(|until| until > crate::infra::time::now_ms() as i64));

        bridge_server.abort();
        rejected.server.abort();
        accepted.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_http_fallback_401_refreshes_once_before_replay() {
        let name = "codex-ws-http-401";
        let workspace = format!("{name}-workspace");
        let refreshed_access_token = signed_openai_access_token(&workspace).await;
        let upstream = spawn_test_unauthorized_codex_upstream(
            refreshed_access_token.clone(),
            "text/event-stream",
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-ws-http-refresh\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-ws-http-refresh\",\"status\":\"completed\",\"output\":[]}}\n\n",
                "data: [DONE]\n\n"
            )
            .to_string(),
        )
        .await;
        let refresh = spawn_test_codex_refresh_endpoint(refreshed_access_token.clone()).await;
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{}", upstream.address)).await;
        configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "ws-http-refresh-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "refresh fallback").await;
        assert!(events.iter().any(|event| {
            event.pointer("/response/id").and_then(Value::as_str) == Some("resp-ws-http-refresh")
                && event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(
            refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream.authorizations.lock().unwrap().as_slice(),
            [
                format!("Bearer {name}-access-token"),
                format!("Bearer {refreshed_access_token}")
            ]
        );

        bridge_server.abort();
        upstream.server.abort();
        refresh.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_cache_reuses_same_session_connection() {
        let upstream = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-reuse", endpoint).await;
        let pool_key = format!(
            "ws-reuse-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            Some(pool_key.clone()),
            "reuse-session",
        )
        .await;

        let first = send_test_bridge_request(bridge_address, "first").await;
        assert_eq!(
            first
                .last()
                .and_then(|event| event.pointer("/response/id"))
                .and_then(Value::as_str),
            Some("resp-ws-1")
        );
        wait_for_cached_responses_websocket(&pool_key).await;
        let second = send_test_bridge_request(bridge_address, "second").await;
        assert_eq!(
            second
                .last()
                .and_then(|event| event.pointer("/response/id"))
                .and_then(Value::as_str),
            Some("resp-ws-2")
        );
        wait_for_cached_responses_websocket(&pool_key).await;

        assert_eq!(
            upstream
                .websocket_connections
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream
                .websocket_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        remove_cached_responses_websocket(&pool_key).await;
        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_pool_key_separates_credentials_and_workspaces() {
        let (state, execution) =
            codex_bridge_test_context("ws-key", "http://127.0.0.1:9".to_string()).await;
        let base = vec![
            ("authorization".to_string(), "Bearer token-a".to_string()),
            ("chatgpt-account-id".to_string(), "workspace-a".to_string()),
        ];
        let credential_changed = vec![
            ("authorization".to_string(), "Bearer token-b".to_string()),
            ("chatgpt-account-id".to_string(), "workspace-a".to_string()),
        ];
        let workspace_changed = vec![
            ("authorization".to_string(), "Bearer token-a".to_string()),
            ("chatgpt-account-id".to_string(), "workspace-b".to_string()),
        ];

        let key = responses_websocket_pool_key(
            &state,
            &execution,
            "session-a",
            "wss://chatgpt.com/backend-api/codex/responses",
            &base,
        );
        assert_ne!(
            key,
            responses_websocket_pool_key(
                &state,
                &execution,
                "session-a",
                "wss://chatgpt.com/backend-api/codex/responses",
                &credential_changed,
            )
        );
        assert_ne!(
            key,
            responses_websocket_pool_key(
                &state,
                &execution,
                "session-a",
                "wss://chatgpt.com/backend-api/codex/responses",
                &workspace_changed,
            )
        );
    }

    #[tokio::test]
    async fn codex_websocket_size_close_after_commit_does_not_replay() {
        let upstream = spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSize).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-size", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            None,
            "size-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "size close").await;
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("message_too_big")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_first_byte_timeout_after_commit_does_not_replay() {
        let upstream = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Silent).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-timeout", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge_with_timeouts(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            None,
            "timeout-session",
            Some(Duration::from_millis(50)),
            Some(Duration::from_secs(1)),
        )
        .await;

        let (events, close_code) =
            send_test_bridge_request_with_close(bridge_address, "timeout without replay").await;
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_timeout")
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            upstream
                .websocket_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_headers_do_not_satisfy_first_event_timeout() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSizeThenDelayHttpFirstEvent)
                .await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) =
            codex_bridge_test_context("ws-http-first-event-timeout", endpoint).await;
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge_with_timeouts(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "http-first-event-timeout-session",
            Some(Duration::from_millis(50)),
            Some(Duration::from_secs(1)),
        )
        .await;

        let (events, close_code) =
            send_test_bridge_request_with_close(bridge_address, "delayed first event").await;
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_timeout")
        }));
        assert!(!events.iter().any(|event| {
            matches!(
                event.get("type").and_then(Value::as_str),
                Some("response.output_text.delta" | "response.completed")
            )
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            upstream
                .websocket_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_error_body_obeys_first_event_timeout() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSizeThenStallHttpErrorBody)
                .await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) =
            codex_bridge_test_context("ws-http-error-body-timeout", endpoint).await;
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge_with_timeouts(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "http-error-body-timeout-session",
            Some(Duration::from_millis(50)),
            Some(Duration::from_secs(1)),
        )
        .await;

        let (events, close_code) = tokio::time::timeout(
            Duration::from_millis(500),
            send_test_bridge_request_with_close(bridge_address, "stalled error body"),
        )
        .await
        .expect("the HTTP error body must not outlive the first-event deadline");
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_timeout")
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            upstream
                .websocket_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_handshake_transport_failure_falls_back_to_http() {
        let upstream = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let unavailable_listener =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
        let unavailable_address = unavailable_listener.local_addr().unwrap();
        drop(unavailable_listener);
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-connect", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "connect-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "connect fallback").await;
        assert!(events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            upstream
                .websocket_connections
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_stale_cached_socket_after_send_does_not_replay() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::CompleteThenClose).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-stale", endpoint).await;
        let pool_key = format!(
            "ws-stale-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            Some(pool_key.clone()),
            "stale-session",
        )
        .await;

        let first = send_test_bridge_request(bridge_address, "seed cache").await;
        assert!(first.iter().any(|event| {
            event.pointer("/response/id").and_then(Value::as_str) == Some("resp-before-stale")
        }));
        wait_for_cached_responses_websocket(&pool_key).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let second = send_test_bridge_request(bridge_address, "reject stale replay").await;
        assert!(second.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str)
                == Some("upstream_closed_before_terminal")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            upstream
                .websocket_connections
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        remove_cached_responses_websocket(&pool_key).await;
        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_lifecycle_event_after_request_commit_does_not_replay() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::EmitThenCloseSize).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-lifecycle", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            None,
            "lifecycle-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "preserve lifecycle").await;
        assert_eq!(
            events
                .iter()
                .filter(
                    |event| event.get("type").and_then(Value::as_str) == Some("response.created")
                )
                .count(),
            1
        );
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("message_too_big")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_does_not_fallback_after_business_event_is_emitted() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::EmitBusinessThenCloseSize).await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-committed", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            None,
            "committed-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "no replay").await;
        assert!(events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.created")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_websocket_normal_close_without_terminal_reports_failure_without_replay() {
        let upstream =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::EmitBusinessThenCloseNormal)
                .await;
        let endpoint = format!("http://{}", upstream.address);
        let (state, execution) = codex_bridge_test_context("ws-missing-terminal", endpoint).await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{}/ws", upstream.address),
            None,
            "missing-terminal-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "no silent success").await;
        assert!(events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_text.delta")
        }));
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str)
                == Some("upstream_closed_before_terminal")
        }));
        assert!(!events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(
            upstream
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        upstream.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_provider_failure_stays_on_active_provider() {
        let primary_name = "ws-semantic-primary";
        let fallback_name = "ws-semantic-fallback";
        let primary =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSizeThenHttpProviderFailure)
                .await;
        let fallback = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let state = forwarder_test_state("codex-ws-semantic-provider-failover");
        insert_static_codex_test_account(&state, primary_name, "primary-token").await;
        insert_static_codex_test_account(&state, fallback_name, "fallback-token").await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: primary_name.to_string(),
                    endpoint: format!("http://{}", primary.address),
                    websocket_url: Some(format!("ws://{}/ws", primary.address)),
                },
                TestCodexProviderSpec {
                    name: fallback_name.to_string(),
                    endpoint: format!("http://{}", fallback.address),
                    websocket_url: Some(format!("ws://{}/ws", fallback.address)),
                },
            ],
        )
        .await;
        let execution = executions.remove(0);
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "semantic-provider-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "provider failure").await;
        assert!(events.iter().any(|event| {
            event.pointer("/response/id").and_then(Value::as_str) == Some("resp-provider-failed")
                && event.get("type").and_then(Value::as_str) == Some("response.failed")
        }));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_client_failure_is_not_failed_over() {
        let primary_name = "ws-client-primary";
        let fallback_name = "ws-client-fallback";
        let primary =
            spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSizeThenHttpClientFailure)
                .await;
        let fallback = spawn_test_codex_upstream(TestCodexWebSocketBehavior::Complete).await;
        let state = forwarder_test_state("codex-ws-semantic-client-error");
        insert_static_codex_test_account(&state, primary_name, "primary-token").await;
        insert_static_codex_test_account(&state, fallback_name, "fallback-token").await;
        let mut executions = install_codex_test_provider_set(
            &state,
            vec![
                TestCodexProviderSpec {
                    name: primary_name.to_string(),
                    endpoint: format!("http://{}", primary.address),
                    websocket_url: Some(format!("ws://{}/ws", primary.address)),
                },
                TestCodexProviderSpec {
                    name: fallback_name.to_string(),
                    endpoint: format!("http://{}", fallback.address),
                    websocket_url: Some(format!("ws://{}/ws", fallback.address)),
                },
            ],
        )
        .await;
        let execution = executions.remove(0);
        let unavailable_address = unavailable_test_address().await;
        let (bridge_address, bridge_server) = spawn_test_responses_bridge(
            state,
            execution,
            format!("ws://{unavailable_address}/ws"),
            None,
            "semantic-client-session",
        )
        .await;

        let events = send_test_bridge_request(bridge_address, "client error").await;
        assert!(events.iter().any(|event| {
            event
                .pointer("/response/error/type")
                .and_then(Value::as_str)
                == Some("invalid_request_error")
        }));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_eof_after_business_does_not_switch_provider() {
        let (events, close_code, primary, fallback, bridge_server) =
            run_codex_http_fallback_single_provider_case(
                "fallback-eof-after-business",
                TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenEof,
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(1)),
            )
            .await;

        assert!(events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.output_text.delta")
                && event.get("delta").and_then(Value::as_str) == Some("committed-http")
        }));
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_error")
        }));
        assert!(!events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_idle_timeout_after_business_does_not_switch_provider() {
        let (events, close_code, primary, fallback, bridge_server) =
            run_codex_http_fallback_single_provider_case(
                "fallback-idle-after-business",
                TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenStall,
                Some(Duration::from_secs(1)),
                Some(Duration::from_millis(50)),
            )
            .await;

        assert!(events
            .iter()
            .any(|event| { event.get("delta").and_then(Value::as_str) == Some("committed-http") }));
        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_timeout")
        }));
        assert_eq!(close_code, Some(CloseCode::Error));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_malformed_event_does_not_switch_provider() {
        let (events, _, primary, fallback, bridge_server) =
            run_codex_http_fallback_single_provider_case(
                "fallback-malformed",
                TestCodexWebSocketBehavior::CloseSizeThenHttpMalformed,
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(1)),
            )
            .await;

        assert!(events.iter().any(|event| {
            event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_stream_error")
        }));
        assert!(!events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_same_batch_failure_preserves_events_without_switching() {
        let (events, _, primary, fallback, bridge_server) =
            run_codex_http_fallback_single_provider_case(
                "fallback-batch-provider-failure",
                TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenProviderFailure,
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(1)),
            )
            .await;

        assert!(events
            .iter()
            .any(|event| { event.get("delta").and_then(Value::as_str) == Some("discard-me") }));
        assert!(events.iter().any(|event| {
            event.pointer("/response/id").and_then(Value::as_str) == Some("resp-batch-failed")
                && event.get("type").and_then(Value::as_str) == Some("response.failed")
        }));
        assert_eq!(
            primary
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .http_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        bridge_server.abort();
        primary.server.abort();
        fallback.server.abort();
    }

    #[tokio::test]
    async fn codex_http_fallback_retryable_status_does_not_switch_provider() {
        for (case, behavior) in [
            (
                "fallback-rate-limit",
                TestCodexWebSocketBehavior::CloseSizeThenHttpRateLimited,
            ),
            (
                "fallback-server-error",
                TestCodexWebSocketBehavior::CloseSizeThenHttpServerError,
            ),
        ] {
            let (events, _, primary, fallback, bridge_server) =
                run_codex_http_fallback_single_provider_case(
                    case,
                    behavior,
                    Some(Duration::from_secs(1)),
                    Some(Duration::from_secs(1)),
                )
                .await;

            assert!(events.iter().any(|event| {
                event.pointer("/error/code").and_then(Value::as_str) == Some("upstream_http_error")
            }));
            assert_eq!(
                primary
                    .http_requests
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                fallback
                    .http_requests
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );

            bridge_server.abort();
            primary.server.abort();
            fallback.server.abort();
        }
    }

    #[tokio::test]
    async fn managed_grok_cli_profile_ignores_a_stale_provider_secret() {
        let state = forwarder_test_state("grok-managed-profile");
        let mut execution = install_grok_test_execution(
            &state,
            "grok-managed-profile",
            super::super::grok::default_base_url().to_string(),
            None,
            "grok-managed-access",
            None,
            &[],
        )
        .await;
        execution.stored.provider.settings_config =
            json!({"env": {"OPENAI_API_KEY": "stale-provider-secret"}});

        assert!(grok_cli_profile(&execution));
        assert_eq!(
            super::super::grok::chat_upstream_url(
                "https://api.x.ai/v1/responses",
                grok_cli_profile(&execution),
            ),
            "https://cli-chat-proxy.grok.com/v1/responses",
        );

        let mut direct_plan = execution.plan.as_ref().clone();
        direct_plan.auth_ref =
            crate::domain::providers::runtime::RuntimeAuthRef::StaticCredential {
                auth_scheme: crate::domain::providers::registry::AuthScheme::Bearer,
                slots: vec!["/settingsConfig/env/OPENAI_API_KEY".to_string()],
                credential_generation: 0,
            };
        execution.plan = std::sync::Arc::new(direct_plan);
        assert!(!grok_cli_profile(&execution));
    }

    #[tokio::test]
    async fn grok_refresh_and_websocket_target_fail_closed_after_concurrent_persistence_loss() {
        let state = forwarder_test_state("grok-refresh-degraded-window");
        let execution = install_grok_test_execution(
            &state,
            "grok-refresh-degraded-window",
            super::super::grok::default_base_url().to_string(),
            None,
            "grok-before-rotation",
            None,
            &[GrokAccountCapability::Websocket],
        )
        .await;
        let account_id = execution.managed_account_id().unwrap().to_string();
        let snapshot = state.find_account_by_id(&account_id).await.unwrap();
        state.inject_account_refresh_persist_failures(1);
        let commit = state
            .commit_native_refresh_success(
                &snapshot,
                crate::domain::accounts::store::AccountRefreshUpdate {
                    access_token: Some("grok-live-but-not-durable".to_string()),
                    refresh_token: Some("grok-rotated-not-durable".to_string()),
                    expires_at: Some(i64::MAX / 2),
                    ..Default::default()
                },
            )
            .await;
        assert!(commit.is_err());
        assert!(state.credential_persistence_degraded());

        let refresh_error = refresh_execution_managed_account_if_needed(&state, &execution)
            .await
            .unwrap_err();
        assert_eq!(refresh_error.status, StatusCode::SERVICE_UNAVAILABLE);

        let target_error = match prepare_responses_websocket_target(
            &state,
            &execution,
            ResponsesWebsocketMode::Grok,
            Some("degraded-session"),
            None,
        )
        .await
        {
            Ok(_) => panic!("degraded Grok websocket target must fail closed"),
            Err(error) => error,
        };
        assert_eq!(target_error.status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn grok_http_single_model_alias_is_normalized_in_the_upstream_body() {
        let upstream_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let observation = std::sync::Arc::new(std::sync::Mutex::new(None));
        let observation_for_route = std::sync::Arc::clone(&observation);
        let upstream_app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let observation = std::sync::Arc::clone(&observation_for_route);
                async move {
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    *observation.lock().unwrap() = Some((
                        body.get("model")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        body.get("prompt_cache_key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        headers
                            .get("x-grok-conv-id")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    ));
                    axum::Json(json!({
                        "id": "resp-grok-model-alias",
                        "object": "response",
                        "status": "completed",
                        "output": [],
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }))
                }
            }),
        );
        let upstream_server = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = forwarder_test_state("grok-http-model-alias");
        let mut execution = install_grok_test_execution(
            &state,
            "grok-http-model-alias",
            format!("http://{upstream_address}/v1"),
            None,
            "grok-model-alias-access",
            None,
            &[],
        )
        .await;
        let mut providers = state.providers.read().await.clone();
        let mut plan = providers
            .runtime_plan(AppKind::Codex, &execution.stored.provider.id)
            .unwrap()
            .as_ref()
            .clone();
        plan.model_policy = crate::domain::providers::runtime::RuntimeModelPolicy::Single {
            upstream_model: "grok-composer".to_string(),
        };
        execution.plan = std::sync::Arc::new(plan.clone());
        std::sync::Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        state.replace_provider_store_for_test(providers).await;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_str(&execution.stored.provider.id).unwrap(),
        );
        headers.insert("x-session-id", HeaderValue::from_static("client-session"));
        let response = forward(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(
                br#"{"model":"client-model","input":"ping","prompt_cache_key":"client-controlled"}"#,
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let (model, prompt_cache_key, conversation_id) =
            observation.lock().unwrap().clone().unwrap();
        assert_eq!(model, "grok-composer-2.5-fast");
        assert_ne!(prompt_cache_key, "client-controlled");
        assert!(!prompt_cache_key.is_empty());
        assert_eq!(prompt_cache_key, conversation_id);

        let fallback = prepare_codex_http_fallback_target(
            &state,
            &execution,
            &json!({
                "model": "client-model",
                "input": "fallback",
                "prompt_cache_key": "client-controlled"
            }),
            Some("fallback-session"),
            Some(9),
        )
        .await
        .unwrap();
        let fallback_body: Value = serde_json::from_slice(&fallback.body).unwrap();
        assert_eq!(fallback_body["model"], "grok-composer-2.5-fast");
        assert_eq!(fallback_body["prompt_cache_key"], "fallback-session");
        upstream_server.abort();
    }

    #[tokio::test]
    async fn grok_http_sse_401_refresh_reuses_authorization_context_and_client_turn() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "access_token": "grok-http-refreshed-access",
                        "refresh_token": "grok-http-rotated-refresh",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let token_server = tokio::spawn(async move {
            axum::serve(token_listener, token_app).await.unwrap();
        });

        let upstream_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observations_for_route = std::sync::Arc::clone(&observations);
        let upstream_app = axum::Router::new().route(
            "/v1/responses",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let observations = std::sync::Arc::clone(&observations_for_route);
                async move {
                    let streaming = serde_json::from_slice::<Value>(&body)
                        .ok()
                        .and_then(|body| body.get("stream").and_then(Value::as_bool))
                        .unwrap_or(false);
                    let header = |name: &str| {
                        headers
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string()
                    };
                    let authorization = header("authorization");
                    observations.lock().unwrap().push((
                        authorization.clone(),
                        header("x-grok-conv-id"),
                        header("x-grok-turn-idx"),
                    ));
                    if authorization == "Bearer grok-http-old-access" {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .header(CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({"error": {"type": "authentication_error"}}).to_string(),
                            ))
                            .unwrap();
                    }
                    if streaming {
                        let sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-grok-http\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-grok-http\",\"status\":\"completed\",\"output\":[]}}\n\n",
                            "data: [DONE]\n\n"
                        );
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap()
                    } else {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({
                                    "id": "resp-grok-json",
                                    "object": "response",
                                    "status": "completed",
                                    "output": [],
                                    "usage": {"input_tokens": 1, "output_tokens": 1}
                                })
                                .to_string(),
                            ))
                            .unwrap()
                    }
                }
            }),
        );
        let upstream_server = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = forwarder_test_state("grok-http-401");
        let provider_name = "grok-http-401";
        let token_url = format!("http://{token_address}/token");
        let execution = install_grok_test_execution(
            &state,
            provider_name,
            format!("http://{upstream_address}/v1"),
            None,
            "grok-http-old-access",
            Some(token_url.clone()),
            &[],
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_str(&execution.stored.provider.id).unwrap(),
        );
        headers.insert("session_id", HeaderValue::from_static("client-session-17"));
        headers.insert("x-grok-turn-idx", HeaderValue::from_static("17"));
        let response = forward(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"grok","input":"ping","stream":true,"store":false}"#),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("response.completed"));
        let account_id = execution.managed_account_id().unwrap().to_string();
        state
            .mutate_accounts_immediate(move |accounts| {
                let account = accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == account_id)
                    .unwrap();
                account.access_token = Some("grok-http-old-access".to_string());
                account.expires_at = Some(i64::MAX / 2);
                account.raw = Some(json!({"testOAuthTokenUrl": token_url}));
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_str(&execution.stored.provider.id).unwrap(),
        );
        headers.insert("session_id", HeaderValue::from_static("client-session-17"));
        headers.insert("x-grok-turn-idx", HeaderValue::from_static("17"));
        let response = forward(
            state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"grok","input":"ping","stream":false,"store":false}"#),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["status"],
            "completed"
        );
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 4);
        assert_eq!(observations[0].0, "Bearer grok-http-old-access");
        assert_eq!(observations[1].0, "Bearer grok-http-refreshed-access");
        assert_eq!(observations[2].0, "Bearer grok-http-old-access");
        assert_eq!(observations[3].0, "Bearer grok-http-refreshed-access");
        for pair in observations.chunks_exact(2) {
            assert!(!pair[0].1.is_empty());
            assert_eq!(pair[0].1, pair[1].1);
            assert_eq!(pair[0].2, "17");
            assert_eq!(pair[0].2, pair[1].2);
        }
        token_server.abort();
        upstream_server.abort();
    }

    #[tokio::test]
    async fn grok_media_is_fail_closed_then_refreshes_once_with_persisted_evidence() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "access_token": "grok-media-refreshed-access",
                        "refresh_token": "grok-media-rotated-refresh",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let token_server = tokio::spawn(async move {
            axum::serve(token_listener, token_app).await.unwrap();
        });
        let upstream_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let identities = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let identities_for_route = std::sync::Arc::clone(&identities);
        let upstream_app = axum::Router::new().route(
            "/v1/images/generations",
            axum::routing::post(move |headers: HeaderMap| {
                let identities = std::sync::Arc::clone(&identities_for_route);
                async move {
                    let header = |name: &str| {
                        headers
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string()
                    };
                    let authorization = header("authorization");
                    identities.lock().unwrap().push((
                        authorization.clone(),
                        header("x-xai-token-auth"),
                        header("x-grok-client-identifier"),
                        header("x-grok-client-version"),
                        header("x-authenticateresponse"),
                    ));
                    if authorization == "Bearer grok-media-old-access" {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Body::from("unauthorized"))
                            .unwrap();
                    }
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            r#"{"data":[{"url":"https://example.test/image.png"}]}"#,
                        ))
                        .unwrap()
                }
            }),
        );
        let upstream_server = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = forwarder_test_state("grok-media-401");
        let execution = install_grok_test_execution(
            &state,
            "grok-media-401",
            format!("http://{upstream_address}/v1"),
            None,
            "grok-media-old-access",
            Some(format!("http://{token_address}/token")),
            &[],
        )
        .await;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_str(&execution.stored.provider.id).unwrap(),
        );
        let body = Bytes::from_static(br#"{"model":"grok-4.5","prompt":"draw"}"#);
        let blocked = forward_grok_media(
            state.clone(),
            Method::POST,
            "/images/generations".to_string(),
            headers.clone(),
            body.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(blocked.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(identities.lock().unwrap().is_empty());

        let account_id = execution.managed_account_id().unwrap().to_string();
        assert!(state
            .record_grok_capability_evidence(
                &account_id,
                GrokAccountCapability::ImageGeneration,
                "mock_probe_success",
            )
            .await
            .unwrap());
        let first = forward_grok_media(
            state.clone(),
            Method::POST,
            "/images/generations".to_string(),
            headers.clone(),
            body.clone(),
        )
        .await
        .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = forward_grok_media(
            state.clone(),
            Method::POST,
            "/images/generations".to_string(),
            headers,
            body,
        )
        .await
        .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        let identities = identities.lock().unwrap().clone();
        assert_eq!(
            identities
                .iter()
                .map(|identity| identity.0.as_str())
                .collect::<Vec<_>>(),
            [
                "Bearer grok-media-old-access",
                "Bearer grok-media-refreshed-access",
                "Bearer grok-media-refreshed-access"
            ]
        );
        for identity in identities.iter() {
            assert_eq!(identity.1, crate::domain::grok_cli::GROK_CLI_TOKEN_AUTH);
            assert_eq!(
                identity.2,
                crate::domain::grok_cli::GROK_CLI_CLIENT_IDENTIFIER
            );
            assert!(!identity.3.is_empty());
            assert_eq!(identity.4, "authenticate-response");
        }
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert!(
            crate::domain::accounts::store::grok_account_capability_evidence_present(
                &account,
                GrokAccountCapability::ImageGeneration,
            )
        );
        token_server.abort();
        upstream_server.abort();
    }

    #[tokio::test]
    async fn grok_video_status_rejects_provider_or_account_binding_drift() {
        let observed_conversation_id = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let observed_for_route = std::sync::Arc::clone(&observed_conversation_id);
        let upstream = axum::Router::new().route(
            "/v1/videos/request-1",
            axum::routing::get(move |headers: HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    *observed.lock().unwrap() = headers
                        .get("x-grok-conv-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    axum::Json(json!({"status": "pending"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = forwarder_test_state("grok-video-sticky-binding");
        let execution = install_grok_test_execution(
            &state,
            "grok-video-sticky-binding",
            format!("http://{upstream_address}/v1"),
            None,
            "grok-video-access",
            None,
            &[GrokAccountCapability::VideoGeneration],
        )
        .await;
        let provider_id = execution.stored.provider.id.clone();
        let account_id = execution.managed_account_id().unwrap().to_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-provider-id",
            HeaderValue::from_str(&provider_id).unwrap(),
        );

        state.remember_grok_media_session(
            "grok-video:request-1".to_string(),
            "different-provider".to_string(),
            Some(account_id.clone()),
            60_000,
        );
        let provider_drift = forward_grok_media(
            state.clone(),
            Method::GET,
            "/videos/request-1".to_string(),
            headers.clone(),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(provider_drift.status, StatusCode::CONFLICT);

        state.remember_grok_media_session(
            "grok-video:request-1".to_string(),
            provider_id.clone(),
            Some("different-account".to_string()),
            60_000,
        );
        let account_drift = forward_grok_media(
            state.clone(),
            Method::GET,
            "/videos/request-1".to_string(),
            headers.clone(),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(account_drift.status, StatusCode::CONFLICT);

        ensure_grok_media_session_binding(
            &execution,
            &GrokMediaSessionBinding {
                provider_id,
                account_id: Some(account_id),
                expires_at_ms: i64::MAX,
            },
        )
        .unwrap();

        state.remember_grok_media_session(
            "grok-video:request-1".to_string(),
            execution.stored.provider.id.clone(),
            execution.managed_account_id().map(str::to_string),
            60_000,
        );
        let response = forward_grok_media(
            state,
            Method::GET,
            "/videos/request-1".to_string(),
            headers,
            Bytes::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*observed_conversation_id.lock().unwrap(), None);
        upstream_server.abort();
    }

    #[tokio::test]
    async fn grok_websocket_401_and_http_fallback_keep_one_identity_and_turn() {
        let token_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let token_address = token_listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let token_app = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&token_requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "access_token": "grok-ws-refreshed-access",
                        "refresh_token": "grok-ws-rotated-refresh",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let token_server = tokio::spawn(async move {
            axum::serve(token_listener, token_app).await.unwrap();
        });

        let upstream_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let websocket_observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let http_observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let websocket_for_route = std::sync::Arc::clone(&websocket_observations);
        let http_for_route = std::sync::Arc::clone(&http_observations);
        let upstream_app = axum::Router::new()
            .route(
                "/ws",
                axum::routing::get(move |headers: HeaderMap, ws: WebSocketUpgrade| {
                    let observations = std::sync::Arc::clone(&websocket_for_route);
                    async move {
                        let header = |name: &str| {
                            headers
                                .get(name)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string()
                        };
                        let authorization = header("authorization");
                        observations.lock().unwrap().push((
                            authorization.clone(),
                            header("x-grok-conv-id"),
                            header("x-grok-turn-idx"),
                        ));
                        if authorization == "Bearer grok-ws-old-access" {
                            return Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .body(Body::from("unauthorized"))
                                .unwrap();
                        }
                        ws.on_upgrade(|mut socket| async move {
                            if let Some(Ok(_)) = socket.recv().await {
                                let _ = socket
                                    .send(AxumWsMessage::Close(Some(
                                        axum::extract::ws::CloseFrame {
                                            code: 1009,
                                            reason: "fallback".into(),
                                        },
                                    )))
                                    .await;
                            }
                        })
                    }
                }),
            )
            .route(
                "/v1/responses",
                axum::routing::post(move |headers: HeaderMap| {
                    let observations = std::sync::Arc::clone(&http_for_route);
                    async move {
                        let header = |name: &str| {
                            headers
                                .get(name)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string()
                        };
                        observations.lock().unwrap().push((
                            header("authorization"),
                            header("x-grok-conv-id"),
                            header("x-grok-turn-idx"),
                        ));
                        let sse = concat!(
                            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-grok-fallback\",\"status\":\"in_progress\"}}\n\n",
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"fallback\"}\n\n",
                            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-grok-fallback\",\"status\":\"completed\",\"output\":[]}}\n\n",
                            "data: [DONE]\n\n"
                        );
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap()
                    }
                }),
            );
        let upstream_server = tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = forwarder_test_state("grok-ws-401-fallback");
        let execution = install_grok_test_execution(
            &state,
            "grok-ws-401-fallback",
            format!("http://{upstream_address}/v1"),
            Some(format!("ws://{upstream_address}/ws")),
            "grok-ws-old-access",
            Some(format!("http://{token_address}/token")),
            &[GrokAccountCapability::Websocket],
        )
        .await;
        let (bridge_address, bridge_server) =
            spawn_test_grok_responses_bridge(state, execution, "grok-ws-session", Some(23)).await;
        let events = send_test_bridge_request(bridge_address, "grok fallback").await;
        assert!(events.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("response.completed")
        }));
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        let websocket = websocket_observations.lock().unwrap();
        assert_eq!(websocket.len(), 2);
        assert_eq!(websocket[0].0, "Bearer grok-ws-old-access");
        assert_eq!(websocket[1].0, "Bearer grok-ws-refreshed-access");
        assert_eq!(websocket[0].1, "grok-ws-session");
        assert_eq!(websocket[0].1, websocket[1].1);
        assert_eq!(websocket[0].2, "23");
        assert_eq!(websocket[0].2, websocket[1].2);
        drop(websocket);
        let http = http_observations.lock().unwrap();
        assert_eq!(http.len(), 1);
        assert_eq!(http[0].0, "Bearer grok-ws-refreshed-access");
        assert_eq!(http[0].1, "grok-ws-session");
        assert_eq!(http[0].2, "23");
        bridge_server.abort();
        token_server.abort();
        upstream_server.abort();
    }

    #[tokio::test]
    async fn grok_execution_never_enters_generic_provider_failover() {
        let state = forwarder_test_state("grok-no-provider-failover");
        let execution = install_grok_test_execution(
            &state,
            "grok-no-provider-failover",
            "http://127.0.0.1:9/v1".to_string(),
            None,
            "grok-no-failover-access",
            None,
            &[],
        )
        .await;
        let accounts = state.accounts_snapshot().await;
        let mut providers = state.providers.read().await.clone();
        let mut fallback = stored_provider(
            AppKind::Codex,
            ProviderType::Codex,
            json!({
                "env": {
                    "OPENAI_API_KEY": "fallback-key",
                    "OPENAI_BASE_URL": "http://127.0.0.1:9/v1"
                }
            }),
            None,
        );
        fallback.provider.id = "available-generic-fallback".to_string();
        providers.providers.push(fallback);
        providers.rebuild_runtime_index(&accounts).unwrap();
        let mut excluded = BTreeSet::new();
        excluded.insert(execution.stored.provider.id.clone());
        assert!(select_failover_provider(
            &providers,
            &accounts,
            ProxyRoute::CodexResponses,
            &state.account_in_flight.snapshot(),
            &excluded,
        )
        .is_some());
        state.replace_provider_store_for_test(providers).await;

        assert!(next_provider_failover(
            &state,
            ProxyRoute::CodexResponses,
            &ForwardAttemptContext::default(),
            &execution,
            "test_failure",
        )
        .await
        .is_none());
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
    fn grok_websocket_session_is_fixed_for_the_connection_lifecycle() {
        let messages = [
            json!({
                "type": "response.create",
                "response": {
                    "model": "client-model",
                    "input": "first",
                    "prompt_cache_key": "client-first"
                }
            }),
            json!({
                "type": "response.create",
                "response": {
                    "model": "client-model",
                    "input": "second",
                    "prompt_cache_key": "client-second"
                }
            }),
        ];

        for message in messages {
            let transformed = transform_responses_websocket_request(
                &message.to_string(),
                ResponsesWebsocketMode::Grok,
                Some("handshake-session"),
                Some("grok-composer"),
            )
            .unwrap();
            let value: Value = serde_json::from_str(&transformed).unwrap();
            assert_eq!(
                value
                    .pointer("/response/prompt_cache_key")
                    .and_then(Value::as_str),
                Some("handshake-session")
            );
            assert_eq!(
                value.pointer("/response/model").and_then(Value::as_str),
                Some("grok-composer-2.5-fast")
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
    fn websocket_error_is_terminal_and_clears_collected_output() {
        let mut patcher = CodexWebsocketOutputPatcher::default();
        let mut collected = TungsteniteMessage::Text(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"stale"}}"#
                .to_string(),
        );
        patcher.patch_message(&mut collected);

        let mut error = TungsteniteMessage::Text(
            r#"{"type":"error","error":{"message":"request failed"}}"#.to_string(),
        );
        assert!(responses_websocket_response_is_terminal(&error));
        patcher.patch_message(&mut error);

        let raw = r#"{"type":"response.completed","response":{"output":[]}}"#;
        let mut completed = TungsteniteMessage::Text(raw.to_string());
        patcher.patch_message(&mut completed);
        assert_eq!(completed, TungsteniteMessage::Text(raw.to_string()));
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

    #[test]
    fn stateful_chat_responses_pipeline_emits_custom_tool_completion_once() {
        let stored = stored_provider(AppKind::Codex, ProviderType::Nvidia, json!({}), None);
        let mut transformer = crate::proxy::stream_transforms::StreamEventTransformer::new(
            &stored,
            ProxyRoute::CodexResponses,
            BTreeSet::from(["exec".to_string()]),
        );
        let upstream = Bytes::from_static(
            br#"data: {"id":"chatcmpl_pipeline","model":"chat","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_exec","function":{"name":"exec","arguments":"{\"input\":\"pwd\"}"}}]},"finish_reason":null}]}

data: {"id":"chatcmpl_pipeline","model":"chat","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
        );
        let transformed = transformer.push(upstream).unwrap();
        let transformed = join_bytes(transformed, transformer.finish().unwrap());
        let mut patcher = CodexCustomToolStreamPatcher::default();
        let output = join_bytes(patcher.push(transformed), patcher.finish());
        let output = String::from_utf8(output.to_vec()).unwrap();
        let events = output
            .split("\n\n")
            .filter_map(first_sse_data_payload)
            .filter(|payload| payload.starts_with('{'))
            .map(|payload| serde_json::from_str::<Value>(payload).unwrap())
            .collect::<Vec<_>>();

        assert!(!output.contains("cc_switch_custom_bridge"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "response.custom_tool_call_input.delta")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "response.custom_tool_call_input.done")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "response.output_item.done")
                .count(),
            1
        );
        let completed = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .unwrap();
        assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 1);
        assert_eq!(completed["response"]["output"][0]["input"], "pwd");
    }
}
