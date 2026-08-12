use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body::{Body as HttpBody, Frame, SizeHint};
use rand::RngCore;

use crate::api::{ApiError, InferenceApiError, InferenceSurface};
use crate::clients::router::ingress::IngressContext;
use crate::domain::providers::model::AppKind;
use crate::logging::{classify_network_error, error_fingerprint, AuditEvent, SharedAuditLog};
use crate::state::ServerState;

#[derive(Debug, Clone)]
pub(crate) struct TransportRequestId(pub String);

#[derive(Debug, Clone)]
struct InferenceRouteInfo {
    app: &'static str,
    surface: &'static str,
    operation: &'static str,
    inference_surface: InferenceSurface,
}

pub(crate) fn new_transport_request_id() -> TransportRequestId {
    let mut random = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut random);
    TransportRequestId(format!("transport_{}", hex::encode(random)))
}

pub(crate) fn is_inference_path(path: &str) -> bool {
    inference_route_info(path).is_some()
}

pub(crate) fn record_ingress_rejection(
    state: &ServerState,
    method: &Method,
    path: &str,
    transport_request_id: &TransportRequestId,
    error_code: &str,
    status: StatusCode,
) {
    let Some(info) = inference_route_info(path) else {
        return;
    };
    let mut event = AuditEvent::new("inference.request.rejected");
    event.transport_request_id = Some(transport_request_id.0.clone());
    event.app = Some(info.app.to_string());
    event.surface = Some(info.surface.to_string());
    event.operation = Some(info.operation.to_string());
    event.route = Some(canonical_route(path).to_string());
    event.method = Some(method.as_str().to_string());
    event.status_code = Some(status.as_u16());
    event.outcome = Some("rejected".to_string());
    event.stage = Some("ingress".to_string());
    event.error_code = Some(error_code.to_string());
    event.error_class = Some("ingress_rejection".to_string());
    event.component = Some("request_ingress".to_string());
    event.failure_kind = Some(error_code.to_string());
    event.error_fingerprint = Some(error_fingerprint("request_ingress", error_code, error_code));
    event.retry_decision = Some("do_not_retry".to_string());
    event.retryable = Some(false);
    state.emit_audit_event_backpressured_best_effort(event);
    tracing::warn!(
        target: "cc_switch_server::request_audit",
        event = "inference.request.rejected",
        transport_request_id = %transport_request_id.0,
        app = info.app,
        operation = info.operation,
        method = method.as_str(),
        route = canonical_route(path),
        status_code = status.as_u16(),
        stage = "ingress",
        error_code,
        "inference request rejected"
    );
}

pub(crate) async fn audit_inference_request(
    State(state): State<ServerState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let Some(info) = inference_route_info(&path) else {
        return next.run(request).await;
    };
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| canonical_route(&path))
        .to_string();
    let method = request.method().as_str().to_string();
    let started = Instant::now();
    let started_at_ms = crate::infra::time::now_ms();
    let transport_request_id = request
        .extensions()
        .get::<TransportRequestId>()
        .cloned()
        .unwrap_or_else(new_transport_request_id);
    let ingress = request.extensions().get::<IngressContext>().cloned();

    let request_id = ingress.as_ref().map(|context| context.request_id.clone());
    let mut audit_admitted = false;
    if let Some(context) = ingress.as_ref() {
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some(context.request_id.clone());
        accepted.transport_request_id = Some(transport_request_id.0.clone());
        accepted.app = Some(info.app.to_string());
        accepted.surface = Some(info.surface.to_string());
        accepted.operation = Some(info.operation.to_string());
        accepted.route = Some(route.clone());
        accepted.method = Some(method.clone());
        accepted.body_sha256 =
            (!context.body_sha256.is_empty()).then(|| context.body_sha256.clone());
        match state.emit_audit_event(accepted) {
            Ok(cursor) => audit_admitted = cursor.is_some(),
            Err(error) => {
                let mut rejected = AuditEvent::new("inference.request.rejected");
                rejected.request_id = Some(context.request_id.clone());
                rejected.transport_request_id = Some(transport_request_id.0.clone());
                rejected.app = Some(info.app.to_string());
                rejected.surface = Some(info.surface.to_string());
                rejected.operation = Some(info.operation.to_string());
                rejected.route = Some(route.clone());
                rejected.method = Some(method.clone());
                rejected.body_sha256 =
                    (!context.body_sha256.is_empty()).then(|| context.body_sha256.clone());
                rejected.status_code = Some(StatusCode::SERVICE_UNAVAILABLE.as_u16());
                rejected.outcome = Some("rejected".to_string());
                rejected.stage = Some("audit_admission".to_string());
                rejected.error_code = Some("cc_switch_audit_unavailable".to_string());
                rejected.error_class = Some("audit_unavailable".to_string());
                rejected.component = Some("audit_spool".to_string());
                rejected.failure_kind = Some("admission".to_string());
                rejected.error_fingerprint = Some(error_fingerprint(
                    "audit_spool",
                    "admission",
                    &error.to_string(),
                ));
                rejected.retry_decision = Some("retry_later".to_string());
                rejected.retryable = Some(true);
                state.emit_audit_event_backpressured_best_effort(rejected);
                tracing::error!(
                    target: "cc_switch_server::request_audit",
                    request_id = %context.request_id,
                    error = %error,
                    error_code = "cc_switch_audit_unavailable",
                    "request audit admission failed"
                );
                return InferenceApiError::api(
                    info.inference_surface,
                    Some(context.request_id.clone()),
                    ApiError::service_unavailable_code(
                        "cc_switch_audit_unavailable",
                        "request audit is temporarily unavailable",
                    ),
                )
                .into_response();
            }
        }
        if audit_admitted {
            state.register_audit_request(&context.request_id);
        }
        tracing::info!(
            target: "cc_switch_server::request_audit",
            event = "inference.request.accepted",
            request_id = %context.request_id,
            transport_request_id = %transport_request_id.0,
            app = info.app,
            operation = info.operation,
            method,
            route,
            "inference request accepted"
        );
    }

    let mut pending_request = audit_admitted.then(|| PendingRequestAudit {
        lifecycle: Some(RequestAuditLifecycle {
            state: state.clone(),
            audit: state.audit_log(),
            audit_admitted: true,
            request_id: request_id.clone(),
            share_id: ingress
                .as_ref()
                .and_then(|context| context.share_id.clone()),
            user_email: ingress
                .as_ref()
                .and_then(|context| context.user_email.clone()),
            transport_request_id: transport_request_id.0.clone(),
            app: info.app,
            surface: info.surface,
            operation: info.operation,
            route: route.clone(),
            method: method.clone(),
            status: StatusCode::from_u16(499).expect("499 is a valid HTTP status code"),
            error_code: None,
            retryable: false,
            streaming: false,
            started,
            started_at_ms,
            finished: Arc::new(AtomicBool::new(false)),
        }),
    });
    let response = next.run(request).await;
    if let Some(pending_request) = pending_request.as_mut() {
        pending_request.disarm();
    }
    let status = response.status();
    let error_code = response
        .headers()
        .get("x-cc-switch-error-code")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let retryable = response
        .headers()
        .get("x-should-retry")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or_else(|| {
            matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
            )
        });
    let streaming = status == StatusCode::SWITCHING_PROTOCOLS
        || response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
    let lifecycle = RequestAuditLifecycle {
        state: state.clone(),
        audit: state.audit_log(),
        audit_admitted,
        request_id,
        share_id: ingress
            .as_ref()
            .and_then(|context| context.share_id.clone()),
        user_email: ingress
            .as_ref()
            .and_then(|context| context.user_email.clone()),
        transport_request_id: transport_request_id.0,
        app: info.app,
        surface: info.surface,
        operation: info.operation,
        route,
        method,
        status,
        error_code,
        retryable,
        streaming,
        started,
        started_at_ms,
        finished: Arc::new(AtomicBool::new(false)),
    };
    attach_audited_response_body(response, lifecycle)
}

fn attach_audited_response_body(response: Response, lifecycle: RequestAuditLifecycle) -> Response {
    let (parts, body) = response.into_parts();
    if body.is_end_stream() {
        lifecycle.finish(RequestFinish::Completed);
        Response::from_parts(parts, body)
    } else {
        Response::from_parts(
            parts,
            Body::new(AuditedResponseBody {
                remaining_exact: body.size_hint().exact().filter(|remaining| *remaining > 0),
                inner: body,
                lifecycle: Some(lifecycle),
            }),
        )
    }
}

struct RequestAuditLifecycle {
    state: ServerState,
    audit: SharedAuditLog,
    audit_admitted: bool,
    request_id: Option<String>,
    share_id: Option<String>,
    user_email: Option<String>,
    transport_request_id: String,
    app: &'static str,
    surface: &'static str,
    operation: &'static str,
    route: String,
    method: String,
    status: StatusCode,
    error_code: Option<String>,
    retryable: bool,
    streaming: bool,
    started: Instant,
    started_at_ms: u128,
    finished: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestFinish {
    Completed,
    StreamError,
    DownstreamCancelled,
    HandlerCancelled,
}

impl RequestAuditLifecycle {
    fn finish(&self, finish: RequestFinish) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let interrupted = finish != RequestFinish::Completed;
        let details = self
            .request_id
            .as_deref()
            .map(|request_id| self.audit.take_request_details(request_id))
            .unwrap_or_default();
        let status =
            semantic_terminal_status(self.status, finish, details.stream_status.as_deref());
        let attempt_count = details
            .attempt
            .or_else(|| details.retry_count.map(|count| count.saturating_add(1)))
            .unwrap_or(1);
        let event_name = request_terminal_event_name(status, interrupted);
        let mut event = AuditEvent::new(event_name);
        event.request_id.clone_from(&self.request_id);
        event.transport_request_id = Some(self.transport_request_id.clone());
        event.app = Some(self.app.to_string());
        event.surface = Some(self.surface.to_string());
        event.operation = Some(self.operation.to_string());
        event.route = Some(self.route.clone());
        event.method = Some(self.method.clone());
        event.status_code = Some(status.as_u16());
        event.duration_ms =
            Some(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        event.streaming = Some(self.streaming);
        event.retryable = Some(self.retryable);
        details.apply_to(&mut event);
        if finish == RequestFinish::HandlerCancelled {
            event.outcome = Some("interrupted".to_string());
            event.stage = Some("handler_execution".to_string());
            event.error_code = Some("request_cancelled".to_string());
            event.error_class = Some("request_interrupted".to_string());
            event.stream_status = Some("cancelled".to_string());
        } else if finish == RequestFinish::DownstreamCancelled {
            event.outcome = Some("interrupted".to_string());
            event.stage = Some("response_finalize".to_string());
            event.error_code = Some("downstream_cancelled".to_string());
            event.error_class = Some("downstream_disconnect".to_string());
            event.stream_status = Some("cancelled".to_string());
        } else if finish == RequestFinish::StreamError {
            event.outcome = Some("interrupted".to_string());
            event.stage = Some("stream_transform".to_string());
            event.error_code = Some("response_body_error".to_string());
            event.error_class = Some("stream_error".to_string());
            event.stream_status = Some("transport_error".to_string());
        } else {
            event.outcome = Some(
                if status.is_success() || status.is_redirection() {
                    "completed"
                } else if status.is_client_error() {
                    "rejected"
                } else {
                    "failed"
                }
                .to_string(),
            );
            event.error_code = self
                .error_code
                .clone()
                .or_else(|| fallback_error_code(status).map(str::to_string));
            event.stage = event
                .error_code
                .as_deref()
                .map(error_stage)
                .map(str::to_string);
            event.error_class = status
                .is_client_error()
                .then(|| "client_error".to_string())
                .or_else(|| status.is_server_error().then(|| "server_error".to_string()));
        }
        if let Some(failure_kind) = event.error_code.clone() {
            let component = event
                .stage
                .as_deref()
                .map(|stage| format!("request_{stage}"))
                .unwrap_or_else(|| "request_execution".to_string());
            event.component = Some(component.clone());
            event.failure_kind = Some(failure_kind.clone());
            event.network_error_kind = classify_network_error(&failure_kind).map(str::to_string);
            event.error_fingerprint =
                Some(error_fingerprint(&component, &failure_kind, &failure_kind));
            event.retry_decision = Some(
                if self.retryable {
                    "client_retry"
                } else {
                    "do_not_retry"
                }
                .to_string(),
            );
        }
        let outcome = event.outcome.as_deref().unwrap_or("-").to_string();
        let stage = event.stage.as_deref().unwrap_or("-").to_string();
        let error_code = event.error_code.as_deref().unwrap_or("-").to_string();
        if status != StatusCode::SWITCHING_PROTOCOLS && self.operation != "count_tokens" {
            if let (Some(request_id), Some(app)) =
                (self.request_id.clone(), usage_app_kind(self.app))
            {
                let state = self.state.clone();
                let share_id = self.share_id.clone();
                let user_email = self.user_email.clone();
                let failure_kind = event.error_code.clone();
                let started_at_ms = self.started_at_ms;
                let completed_at_ms = crate::infra::time::now_ms();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        state
                            .finalize_usage_request_lifecycle(
                                app,
                                request_id,
                                share_id,
                                user_email,
                                status.as_u16(),
                                started_at_ms,
                                completed_at_ms,
                                attempt_count,
                                interrupted,
                                failure_kind,
                            )
                            .await;
                    });
                }
            }
        }
        if self.audit_admitted {
            self.audit.emit_terminal_best_effort(event);
        } else if self.request_id.is_none() {
            self.audit.emit_best_effort(event);
        }
        tracing::info!(
            target: "cc_switch_server::request_audit",
            event = event_name,
            request_id = self.request_id.as_deref().unwrap_or("-"),
            transport_request_id = %self.transport_request_id,
            app = self.app,
            operation = self.operation,
            method = %self.method,
            route = %self.route,
            status_code = status.as_u16(),
            duration_ms = self.started.elapsed().as_millis(),
            streaming = self.streaming,
            outcome,
            stage,
            error_code,
            "inference request finished"
        );
    }
}

fn usage_app_kind(app: &str) -> Option<AppKind> {
    match app {
        "claude" => Some(AppKind::Claude),
        "codex" => Some(AppKind::Codex),
        "gemini" => Some(AppKind::Gemini),
        _ => None,
    }
}

fn semantic_terminal_status(
    http_status: StatusCode,
    finish: RequestFinish,
    stream_status: Option<&str>,
) -> StatusCode {
    if finish != RequestFinish::Completed {
        return http_status;
    }
    match stream_status {
        Some("provider_failed") => StatusCode::BAD_GATEWAY,
        Some("client_error") => StatusCode::BAD_REQUEST,
        _ => http_status,
    }
}

struct PendingRequestAudit {
    lifecycle: Option<RequestAuditLifecycle>,
}

impl PendingRequestAudit {
    fn disarm(&mut self) {
        self.lifecycle = None;
    }
}

impl Drop for PendingRequestAudit {
    fn drop(&mut self) {
        if let Some(lifecycle) = self.lifecycle.take() {
            lifecycle.finish(RequestFinish::HandlerCancelled);
        }
    }
}

fn request_terminal_event_name(status: StatusCode, interrupted: bool) -> &'static str {
    if interrupted {
        "inference.request.interrupted"
    } else if status.is_client_error() {
        "inference.request.rejected"
    } else if status.is_server_error() {
        "inference.request.failed"
    } else {
        "inference.request.completed"
    }
}

struct AuditedResponseBody {
    inner: Body,
    lifecycle: Option<RequestAuditLifecycle>,
    remaining_exact: Option<u64>,
}

impl HttpBody for AuditedResponseBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(context) {
            Poll::Ready(None) => {
                if let Some(lifecycle) = self.lifecycle.take() {
                    lifecycle.finish(RequestFinish::Completed);
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                if let Some(lifecycle) = self.lifecycle.take() {
                    lifecycle.finish(RequestFinish::StreamError);
                }
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(frame))) => {
                let exact_body_consumed = frame.data_ref().is_some_and(|data| {
                    let Some(remaining) = self.remaining_exact else {
                        return false;
                    };
                    let Ok(data_len) = u64::try_from(data.len()) else {
                        self.remaining_exact = None;
                        return false;
                    };
                    let Some(next) = remaining.checked_sub(data_len) else {
                        self.remaining_exact = None;
                        return false;
                    };
                    self.remaining_exact = Some(next);
                    next == 0
                });
                if exact_body_consumed || self.inner.is_end_stream() {
                    if let Some(lifecycle) = self.lifecycle.take() {
                        lifecycle.finish(RequestFinish::Completed);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for AuditedResponseBody {
    fn drop(&mut self) {
        if let Some(lifecycle) = self.lifecycle.take() {
            lifecycle.finish(RequestFinish::DownstreamCancelled);
        }
    }
}

fn fallback_error_code(status: StatusCode) -> Option<&'static str> {
    match status {
        StatusCode::BAD_REQUEST => Some("cc_switch_invalid_request"),
        StatusCode::UNAUTHORIZED => Some("cc_switch_auth_error"),
        StatusCode::FORBIDDEN => Some("cc_switch_forbidden"),
        StatusCode::NOT_FOUND => Some("cc_switch_not_found"),
        StatusCode::CONFLICT => Some("cc_switch_conflict"),
        StatusCode::TOO_MANY_REQUESTS => Some("cc_switch_rate_limited"),
        StatusCode::BAD_GATEWAY => Some("cc_switch_upstream_error"),
        StatusCode::SERVICE_UNAVAILABLE => Some("cc_switch_unavailable"),
        StatusCode::GATEWAY_TIMEOUT => Some("cc_switch_upstream_timeout"),
        _ if status.is_server_error() => Some("cc_switch_internal_error"),
        _ => None,
    }
}

fn error_stage(code: &str) -> &'static str {
    if code.contains("concurrency") || code.contains("parallel") {
        "concurrency"
    } else if code.contains("oauth") || code.contains("credential") {
        "oauth_refresh"
    } else if code.contains("provider") || code.contains("unavailable") {
        "provider_selection"
    } else if code.contains("timeout") || code.contains("upstream") {
        "upstream_response"
    } else if code.contains("auth") || code.contains("forbidden") {
        "share_policy"
    } else if code.contains("invalid") || code.contains("request") {
        "validation"
    } else {
        "response_finalize"
    }
}

fn canonical_route(path: &str) -> &str {
    if path.starts_with("/v1beta/") {
        "/v1beta/*path"
    } else if path.starts_with("/gemini/v1beta/") {
        "/gemini/v1beta/*path"
    } else if path.starts_with("/gemini/v1/") {
        "/gemini/v1/*path"
    } else if path.starts_with("/v1/images/files/") {
        "/v1/images/files/:token"
    } else if matches!(path, "/v1/videos/generations" | "/videos/generations") {
        path
    } else if path.starts_with("/v1/videos/") {
        "/v1/videos/:request_id"
    } else if path.starts_with("/videos/") {
        "/videos/:request_id"
    } else {
        path
    }
}

fn inference_route_info(path: &str) -> Option<InferenceRouteInfo> {
    let (app, surface, operation, inference_surface) = match canonical_route(path) {
        "/v1/messages" | "/claude/v1/messages" => (
            "claude",
            "anthropic",
            "messages",
            InferenceSurface::Anthropic,
        ),
        "/v1/messages/count_tokens" | "/claude/v1/messages/count_tokens" => (
            "claude",
            "anthropic",
            "count_tokens",
            InferenceSurface::Anthropic,
        ),
        "/v1/chat/completions"
        | "/v1/v1/chat/completions"
        | "/chat/completions"
        | "/codex/v1/chat/completions" => (
            "codex",
            "openai",
            "chat_completions",
            InferenceSurface::OpenAi,
        ),
        "/v1/responses"
        | "/v1/v1/responses"
        | "/responses"
        | "/codex/v1/responses"
        | "/backend-api/codex/responses" => {
            ("codex", "openai", "responses", InferenceSurface::OpenAi)
        }
        "/v1/responses/compact"
        | "/v1/v1/responses/compact"
        | "/responses/compact"
        | "/codex/v1/responses/compact"
        | "/backend-api/codex/responses/compact" => (
            "codex",
            "openai",
            "responses_compact",
            InferenceSurface::OpenAi,
        ),
        "/v1/responses/input_tokens" | "/responses/input_tokens" => (
            "codex",
            "openai",
            "responses_input_tokens",
            InferenceSurface::OpenAi,
        ),
        "/v1/models" | "/models" | "/backend-api/codex/models" => {
            ("codex", "openai", "models", InferenceSurface::OpenAi)
        }
        "/alpha/search" | "/v1/alpha/search" | "/backend-api/codex/alpha/search" => {
            ("codex", "openai", "alpha_search", InferenceSurface::OpenAi)
        }
        "/v1/images/files/:token" => ("codex", "openai", "image_file", InferenceSurface::OpenAi),
        "/v1/images/generations" | "/images/generations" => (
            "codex",
            "openai",
            "image_generation",
            InferenceSurface::OpenAi,
        ),
        "/v1/images/edits" | "/images/edits" => {
            ("codex", "openai", "image_edit", InferenceSurface::OpenAi)
        }
        "/v1/videos/generations" | "/videos/generations" => (
            "codex",
            "openai",
            "video_generation",
            InferenceSurface::OpenAi,
        ),
        "/v1/videos/:request_id" | "/videos/:request_id" => {
            ("codex", "openai", "video_status", InferenceSurface::OpenAi)
        }
        "/v1beta/*path" | "/gemini/v1/*path" | "/gemini/v1beta/*path" => {
            let operation = if path.contains(":countTokens") {
                "count_tokens"
            } else if path.contains(":streamGenerateContent") {
                "stream_generate_content"
            } else if path.contains(":generateContent") {
                "generate_content"
            } else {
                "gemini_api"
            };
            ("gemini", "gemini", operation, InferenceSurface::Gemini)
        }
        _ => return None,
    };
    Some(InferenceRouteInfo {
        app,
        surface,
        operation,
        inference_surface,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cc-switch-server-request-audit-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn test_state(name: &str) -> (ServerState, std::path::PathBuf) {
        let directory = test_dir(&format!("state-{name}"));
        let state = crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(directory.clone()),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        (state, directory)
    }

    #[test]
    fn inference_routes_are_normalized_without_resource_tokens() {
        assert_eq!(
            canonical_route("/v1/images/files/private-token"),
            "/v1/images/files/:token"
        );
        assert_eq!(
            canonical_route("/v1/videos/request-secret"),
            "/v1/videos/:request_id"
        );
        assert_eq!(
            inference_route_info("/v1beta/models/gemini:streamGenerateContent")
                .unwrap()
                .operation,
            "stream_generate_content"
        );
        assert_eq!(
            inference_route_info("/responses/input_tokens")
                .unwrap()
                .operation,
            "responses_input_tokens"
        );
        assert_eq!(
            inference_route_info("/alpha/search").unwrap().operation,
            "alpha_search"
        );
        assert_eq!(
            inference_route_info("/v1/videos/generations")
                .unwrap()
                .operation,
            "video_generation"
        );
        assert_eq!(
            inference_route_info("/videos/generations")
                .unwrap()
                .operation,
            "video_generation"
        );
        assert_eq!(
            inference_route_info("/v1/videos/request-secret")
                .unwrap()
                .operation,
            "video_status"
        );
    }

    #[test]
    fn terminal_event_names_distinguish_failure_rejection_and_interruption() {
        assert_eq!(
            request_terminal_event_name(StatusCode::OK, false),
            "inference.request.completed"
        );
        assert_eq!(
            request_terminal_event_name(StatusCode::TOO_MANY_REQUESTS, false),
            "inference.request.rejected"
        );
        assert_eq!(
            request_terminal_event_name(StatusCode::BAD_GATEWAY, false),
            "inference.request.failed"
        );
        assert_eq!(
            request_terminal_event_name(StatusCode::OK, true),
            "inference.request.interrupted"
        );
    }

    #[test]
    fn dropping_pending_handler_emits_the_missing_terminal_event() {
        let directory = test_dir("handler-cancelled");
        std::fs::create_dir_all(&directory).unwrap();
        let audit = crate::logging::AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        audit.emit(accepted).unwrap().unwrap();
        audit.register_request("request-a");
        let (state, state_directory) = test_state("handler-cancelled");

        drop(PendingRequestAudit {
            lifecycle: Some(RequestAuditLifecycle {
                state: state.clone(),
                audit: audit.clone(),
                audit_admitted: true,
                request_id: Some("request-a".to_string()),
                share_id: None,
                user_email: None,
                transport_request_id: "transport-a".to_string(),
                app: "codex",
                surface: "openai",
                operation: "responses",
                route: "/v1/responses".to_string(),
                method: "POST".to_string(),
                status: StatusCode::from_u16(499).unwrap(),
                error_code: None,
                retryable: false,
                streaming: false,
                started: Instant::now(),
                started_at_ms: crate::infra::time::now_ms(),
                finished: Arc::new(AtomicBool::new(false)),
            }),
        });

        let events = audit.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.interrupted");
        assert_eq!(events[1].status_code, Some(499));
        assert_eq!(events[1].stage.as_deref(), Some("handler_execution"));
        assert_eq!(events[1].error_code.as_deref(), Some("request_cancelled"));
        drop(audit);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(state_directory).unwrap();
    }

    #[test]
    fn empty_response_body_completes_instead_of_looking_cancelled() {
        let directory = test_dir("empty-response");
        std::fs::create_dir_all(&directory).unwrap();
        let audit = crate::logging::AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        audit.emit(accepted).unwrap().unwrap();
        audit.register_request("request-a");
        let (state, state_directory) = test_state("empty-response");

        let response = attach_audited_response_body(
            Response::new(Body::empty()),
            RequestAuditLifecycle {
                state: state.clone(),
                audit: audit.clone(),
                audit_admitted: true,
                request_id: Some("request-a".to_string()),
                share_id: None,
                user_email: None,
                transport_request_id: "transport-a".to_string(),
                app: "codex",
                surface: "openai",
                operation: "models",
                route: "/v1/models".to_string(),
                method: "GET".to_string(),
                status: StatusCode::OK,
                error_code: None,
                retryable: false,
                streaming: false,
                started: Instant::now(),
                started_at_ms: crate::infra::time::now_ms(),
                finished: Arc::new(AtomicBool::new(false)),
            },
        );
        drop(response);

        let events = audit.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.completed");
        assert_eq!(events[1].outcome.as_deref(), Some("completed"));
        drop(audit);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(state_directory).unwrap();
    }

    #[tokio::test]
    async fn final_data_frame_completes_without_an_extra_eof_poll() {
        use http_body_util::BodyExt;

        let directory = test_dir("single-frame-response");
        std::fs::create_dir_all(&directory).unwrap();
        let audit = crate::logging::AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        audit.emit(accepted).unwrap().unwrap();
        audit.register_request("request-a");
        let (state, state_directory) = test_state("single-frame-response");

        let response = attach_audited_response_body(
            Response::new(Body::from(Bytes::from_static(b"{\"ok\":true}"))),
            RequestAuditLifecycle {
                state: state.clone(),
                audit: audit.clone(),
                audit_admitted: true,
                request_id: Some("request-a".to_string()),
                share_id: None,
                user_email: None,
                transport_request_id: "transport-a".to_string(),
                app: "codex",
                surface: "openai",
                operation: "chat_completions",
                route: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
                status: StatusCode::OK,
                error_code: None,
                retryable: false,
                streaming: false,
                started: Instant::now(),
                started_at_ms: crate::infra::time::now_ms(),
                finished: Arc::new(AtomicBool::new(false)),
            },
        );
        let mut body = response.into_body();
        let frame = body
            .frame()
            .await
            .expect("one data frame")
            .expect("successful data frame");
        assert_eq!(
            frame.into_data().unwrap(),
            Bytes::from_static(b"{\"ok\":true}")
        );
        drop(body);

        let events = audit.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.completed");
        assert_eq!(events[1].outcome.as_deref(), Some("completed"));
        drop(audit);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(state_directory).unwrap();
    }

    #[tokio::test]
    async fn dropping_before_the_final_frame_remains_downstream_cancelled() {
        use http_body_util::BodyExt;

        let directory = test_dir("partial-response");
        std::fs::create_dir_all(&directory).unwrap();
        let audit = crate::logging::AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        audit.emit(accepted).unwrap().unwrap();
        audit.register_request("request-a");
        let (state, state_directory) = test_state("partial-response");
        let stream = futures_util::stream::iter([
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"first")),
            Ok(Bytes::from_static(b"second")),
        ]);

        let response = attach_audited_response_body(
            Response::new(Body::from_stream(stream)),
            RequestAuditLifecycle {
                state: state.clone(),
                audit: audit.clone(),
                audit_admitted: true,
                request_id: Some("request-a".to_string()),
                share_id: None,
                user_email: None,
                transport_request_id: "transport-a".to_string(),
                app: "codex",
                surface: "openai",
                operation: "responses",
                route: "/v1/responses".to_string(),
                method: "POST".to_string(),
                status: StatusCode::OK,
                error_code: None,
                retryable: false,
                streaming: true,
                started: Instant::now(),
                started_at_ms: crate::infra::time::now_ms(),
                finished: Arc::new(AtomicBool::new(false)),
            },
        );
        let mut body = response.into_body();
        let first = body
            .frame()
            .await
            .expect("first data frame")
            .expect("successful first frame")
            .into_data()
            .unwrap();
        assert_eq!(first, Bytes::from_static(b"first"));
        drop(body);

        let events = audit.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.interrupted");
        assert_eq!(
            events[1].error_code.as_deref(),
            Some("downstream_cancelled")
        );
        drop(audit);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(state_directory).unwrap();
    }

    #[test]
    fn in_band_provider_failure_overrides_successful_http_headers() {
        let directory = test_dir("provider-failed-stream");
        std::fs::create_dir_all(&directory).unwrap();
        let audit = crate::logging::AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        audit.emit(accepted).unwrap().unwrap();
        audit.register_request("request-a");
        audit.enrich_request(
            "request-a",
            crate::logging::AuditRequestDetails {
                upstream_status: Some(StatusCode::BAD_GATEWAY.as_u16()),
                stream_status: Some("provider_failed".to_string()),
                ..crate::logging::AuditRequestDetails::default()
            },
        );
        let (state, state_directory) = test_state("provider-failed-stream");

        RequestAuditLifecycle {
            state: state.clone(),
            audit: audit.clone(),
            audit_admitted: true,
            request_id: Some("request-a".to_string()),
            share_id: None,
            user_email: None,
            transport_request_id: "transport-a".to_string(),
            app: "claude",
            surface: "anthropic",
            operation: "messages",
            route: "/v1/messages".to_string(),
            method: "POST".to_string(),
            status: StatusCode::OK,
            error_code: None,
            retryable: false,
            streaming: true,
            started: Instant::now(),
            started_at_ms: crate::infra::time::now_ms(),
            finished: Arc::new(AtomicBool::new(false)),
        }
        .finish(RequestFinish::Completed);

        let events = audit.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.failed");
        assert_eq!(
            events[1].status_code,
            Some(StatusCode::BAD_GATEWAY.as_u16())
        );
        assert_eq!(events[1].outcome.as_deref(), Some("failed"));
        assert_eq!(events[1].stream_status.as_deref(), Some("provider_failed"));
        drop(audit);
        drop(state);
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(state_directory).unwrap();
    }
}
