use axum::http::{header::RETRY_AFTER, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::clients::oauth::codex_device::CodexDeviceError;
use crate::clients::oauth::copilot_device::CopilotDeviceError;
use crate::clients::oauth::grok_device::GrokDeviceError;
use crate::clients::oauth::kimi_device::KimiDeviceError;
use crate::clients::oauth::kiro_device::KiroDeviceError;
use crate::clients::router::email_auth::EmailAuthError;
use crate::proxy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InferenceSurface {
    OpenAi,
    Anthropic,
    Gemini,
}

#[derive(Debug)]
pub(crate) struct InferenceApiError {
    surface: InferenceSurface,
    status: StatusCode,
    message: String,
    code: &'static str,
    error_type: &'static str,
    retryable: bool,
    retry_after_seconds: Option<u64>,
    scope: Option<&'static str>,
    current: Option<u32>,
    limit: Option<u32>,
    request_id: Option<String>,
    param: Option<&'static str>,
}

impl InferenceApiError {
    pub(crate) fn proxy(
        surface: InferenceSurface,
        request_id: Option<String>,
        error: proxy::ProxyError,
    ) -> Self {
        let concurrency = error.concurrency_metadata();
        Self {
            surface,
            status: error.status,
            message: error.client_message().to_string(),
            code: error.error_code(),
            error_type: error.error_type(),
            retryable: error.retryable(),
            retry_after_seconds: error.retry_after_seconds(),
            scope: error.error_scope(),
            current: concurrency.map(|metadata| metadata.current),
            limit: concurrency.map(|metadata| metadata.limit),
            request_id,
            param: error.error_param(),
        }
    }

    pub(crate) fn api(
        surface: InferenceSurface,
        request_id: Option<String>,
        error: ApiError,
    ) -> Self {
        Self {
            surface,
            status: error.status,
            message: error.message,
            code: error.code.unwrap_or_else(|| api_error_code(error.status)),
            error_type: error
                .error_type
                .unwrap_or_else(|| api_error_type(error.status)),
            retryable: error.retryable.unwrap_or(false),
            retry_after_seconds: error.retry_after_seconds,
            scope: None,
            current: None,
            limit: None,
            request_id,
            param: None,
        }
    }

    fn details(&self) -> Value {
        let mut details = Map::new();
        details.insert("retryable".to_string(), Value::Bool(self.retryable));
        if let Some(scope) = self.scope {
            details.insert("scope".to_string(), Value::String(scope.to_string()));
        }
        if let Some(current) = self.current {
            details.insert("current".to_string(), Value::from(current));
        }
        if let Some(limit) = self.limit {
            details.insert("limit".to_string(), Value::from(limit));
        }
        Value::Object(details)
    }

    fn body(&self) -> Value {
        match self.surface {
            InferenceSurface::OpenAi => json!({
                "error": {
                    "message": self.message,
                    "type": self.error_type,
                    "code": self.code,
                    "param": self.param,
                    "details": self.details(),
                },
                "request_id": self.request_id,
            }),
            InferenceSurface::Anthropic => json!({
                "type": "error",
                "error": {
                    "type": self.error_type,
                    "message": self.message,
                    "code": self.code,
                    "details": self.details(),
                },
                "request_id": self.request_id,
            }),
            InferenceSurface::Gemini => {
                let mut metadata = Map::new();
                metadata.insert("code".to_string(), Value::String(self.code.to_string()));
                metadata.insert(
                    "retryable".to_string(),
                    Value::String(self.retryable.to_string()),
                );
                if let Some(scope) = self.scope {
                    metadata.insert("scope".to_string(), Value::String(scope.to_string()));
                }
                if let Some(current) = self.current {
                    metadata.insert("current".to_string(), Value::String(current.to_string()));
                }
                if let Some(limit) = self.limit {
                    metadata.insert("limit".to_string(), Value::String(limit.to_string()));
                }
                if let Some(request_id) = self.request_id.as_deref() {
                    metadata.insert(
                        "requestId".to_string(),
                        Value::String(request_id.to_string()),
                    );
                }
                json!({
                    "error": {
                        "code": self.status.as_u16(),
                        "message": self.message,
                        "status": gemini_rpc_status(self.status),
                        "details": [{
                            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                            "reason": self.code.to_ascii_uppercase(),
                            "domain": "cc-switch",
                            "metadata": metadata,
                        }],
                    }
                })
            }
        }
    }
}

impl IntoResponse for InferenceApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body())).into_response();
        response.headers_mut().insert(
            "x-cc-switch-error-code",
            HeaderValue::from_static(self.code),
        );
        if let Some(scope) = self.scope {
            response
                .headers_mut()
                .insert("x-cc-switch-error-scope", HeaderValue::from_static(scope));
        }
        if let Some(request_id) = self.request_id.as_deref() {
            if let Ok(value) = HeaderValue::from_str(request_id) {
                response
                    .headers_mut()
                    .insert("x-cc-switch-request-id", value.clone());
                response.headers_mut().insert("x-request-id", value);
            }
        }
        if let Some(seconds) = self.retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        if self.surface == InferenceSurface::Anthropic {
            response.headers_mut().insert(
                "x-should-retry",
                HeaderValue::from_static(if self.retryable { "true" } else { "false" }),
            );
        }
        response
    }
}

fn api_error_code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "cc_switch_invalid_request",
        StatusCode::UNAUTHORIZED => "cc_switch_auth_error",
        StatusCode::FORBIDDEN => "cc_switch_forbidden",
        StatusCode::NOT_FOUND => "cc_switch_not_found",
        StatusCode::CONFLICT => "cc_switch_conflict",
        StatusCode::TOO_MANY_REQUESTS => "cc_switch_rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "cc_switch_no_available_provider",
        _ if status.is_server_error() => "cc_switch_proxy_error",
        _ => "cc_switch_invalid_request",
    }
}

fn api_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::CONFLICT => "conflict_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable_error",
        _ if status.is_server_error() => "proxy_error",
        _ => "invalid_request_error",
    }
}

fn gemini_rpc_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => "INVALID_ARGUMENT",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::CONFLICT => "ABORTED",
        StatusCode::TOO_MANY_REQUESTS => "RESOURCE_EXHAUSTED",
        StatusCode::GATEWAY_TIMEOUT => "DEADLINE_EXCEEDED",
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE => "UNAVAILABLE",
        _ => "INTERNAL",
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) ok: bool,
    pub(crate) error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<&'static str>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) error_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retryable: Option<bool>,
    #[serde(rename = "retryAfterSeconds", skip_serializing_if = "Option::is_none")]
    pub(crate) retry_after_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) details: Option<Value>,
}

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
    pub(crate) code: Option<&'static str>,
    pub(crate) error_type: Option<&'static str>,
    pub(crate) retryable: Option<bool>,
    pub(crate) retry_after_seconds: Option<u64>,
    pub(crate) details: Option<Box<Value>>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            error_type: None,
            retryable: None,
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn bad_request(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error.to_string())
    }

    pub(crate) fn bad_request_code(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.into(),
            code: Some(code),
            error_type: None,
            retryable: None,
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn unauthorized(error: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error.into())
    }

    pub(crate) fn forbidden(error: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, error.into())
    }

    pub(crate) fn conflict(error: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, error.into())
    }

    pub(crate) fn conflict_code(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: error.into(),
            code: Some(code),
            error_type: None,
            retryable: None,
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn unprocessable_code(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: error.into(),
            code: Some(code),
            error_type: None,
            retryable: Some(false),
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn provider_contract_mismatch(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: error.into(),
            code: Some("cc_switch_provider_contract_mismatch"),
            error_type: Some("provider_contract_mismatch"),
            retryable: Some(false),
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn not_implemented(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, error.to_string())
    }

    pub(crate) fn feature_disabled(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: error.into(),
            code: Some("cc_switch_feature_disabled"),
            error_type: Some("feature_disabled"),
            retryable: Some(false),
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn web_invoke_unknown(command: impl Into<String>) -> Self {
        let command = command.into();
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: format!(
                "legacy invoke command '{command}' is not registered in cc-switch-server"
            ),
            code: Some("cc_switch_web_invoke_unknown"),
            error_type: Some("web_invoke_unknown"),
            retryable: Some(false),
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn web_invoke_not_wired(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: error.into(),
            code: Some("cc_switch_web_invoke_not_wired"),
            error_type: Some("web_invoke_not_wired"),
            retryable: Some(false),
            retry_after_seconds: None,
            details: None,
        }
    }

    pub(crate) fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, error.to_string())
    }

    pub(crate) fn service_unavailable_code(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: error.into(),
            code: Some(code),
            error_type: Some("unavailable_error"),
            retryable: Some(true),
            retry_after_seconds: Some(1),
            details: None,
        }
    }

    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!("internal api error: {error}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }

    pub(crate) fn not_found(error: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, error.into())
    }

    pub(crate) fn proxy(error: proxy::ProxyError) -> Self {
        let code = error.error_code();
        let error_type = error.error_type();
        let retryable = error.retryable();
        let message = error.client_message().to_string();
        let retry_after_seconds = error.retry_after_seconds();
        Self {
            status: error.status,
            message,
            code: Some(code),
            error_type: Some(error_type),
            retryable: Some(retryable),
            retry_after_seconds,
            details: None,
        }
    }

    pub(crate) fn with_retry_after_ms(mut self, retry_after_ms: Option<i64>) -> Self {
        self.retry_after_seconds = retry_after_ms.map(|milliseconds| {
            u64::try_from(milliseconds.max(0))
                .unwrap_or(u64::MAX)
                .saturating_add(999)
                / 1_000
        });
        self
    }

    pub(crate) fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    pub(crate) fn with_details(mut self, details: Value) -> Self {
        self.details = Some(Box::new(details));
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after_seconds = self.retry_after_seconds;
        let mut response = (
            self.status,
            Json(ErrorResponse {
                ok: false,
                error: self.message,
                code: self.code,
                error_type: self.error_type,
                status: Some(self.status.as_u16()),
                retryable: self.retryable,
                retry_after_seconds,
                details: self.details.map(|details| *details),
            }),
        )
            .into_response();
        if let Some(seconds) = retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        response
    }
}

pub(crate) fn map_email_auth_error(error: EmailAuthError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

pub(crate) fn map_web_auth_error(error: crate::domain::web_auth::WebAuthError) -> ApiError {
    let message = error.to_string();
    if message.contains("invalid password")
        || message.contains("invalid current password")
        || message.contains("not configured")
        || message.contains("not found")
        || message.contains("expired")
        || message.contains("too many")
    {
        ApiError::unauthorized(message)
    } else {
        ApiError::bad_request(message)
    }
}

pub(crate) fn map_share_patch_error(
    error: crate::domain::sharing::shares::SharePatchError,
) -> ApiError {
    match error {
        crate::domain::sharing::shares::SharePatchError::NotFound => {
            ApiError::not_found("share not found")
        }
        crate::domain::sharing::shares::SharePatchError::BindingImmutable => {
            ApiError::conflict_code(
                "cc_switch_share_binding_immutable",
                "share binding is immutable in ordinary upsert/import; pause the Share and use the binding endpoint",
            )
        }
        crate::domain::sharing::shares::SharePatchError::PolicyDivergent(message) => {
            ApiError::conflict_code("cc_switch_share_policy_divergent", message)
        }
        crate::domain::sharing::shares::SharePatchError::RevisionConflict {
            expected,
            current,
        } => ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            format!(
                "managed grant expected config revision {expected}, current revision is {current}"
            ),
        )
        .with_retryable(true),
        crate::domain::sharing::shares::SharePatchError::GrantRevisionConflict {
            email,
            expected,
            current,
        } => ApiError::conflict_code(
            "cc_switch_share_user_grant_revision_conflict",
            format!(
                "user grant {email} changed since this editor was opened (expected revision {expected}, current revision {current})"
            ),
        )
        .with_retryable(true),
        crate::domain::sharing::shares::SharePatchError::UsageTargetBelowObserved {
            email,
            target,
            observed,
        } => ApiError::conflict_code(
            "cc_switch_share_usage_target_below_observed",
            format!(
                "usage target for {email} ({target}) cannot be below observed usage ({observed})"
            ),
        ),
        crate::domain::sharing::shares::SharePatchError::ManagedGrantReadOnly(email) => {
            ApiError::conflict_code(
                "cc_switch_share_market_grant_read_only",
                format!("Share Market managed user {email} is read-only"),
            )
        }
        crate::domain::sharing::shares::SharePatchError::Invalid(message) => {
            ApiError::bad_request(message)
        }
    }
}

pub(crate) fn map_subscription_binding_error(
    error: crate::domain::sharing::subscription_identity::SubscriptionBindingError,
) -> ApiError {
    if matches!(
        error,
        crate::domain::sharing::subscription_identity::SubscriptionBindingError::UnverifiedIdentity { .. }
    ) {
        ApiError::unprocessable_code(error.code(), error.to_string())
    } else {
        ApiError::conflict_code(error.code(), error.to_string())
    }
}

pub(crate) fn map_account_write_error(error: anyhow::Error) -> ApiError {
    if let Some(binding) = error
        .downcast_ref::<crate::domain::sharing::subscription_identity::SubscriptionBindingError>(
    ) {
        return map_subscription_binding_error(binding.clone());
    }
    ApiError::internal(error)
}

pub(crate) fn map_codex_active_account_selection_error(
    error: crate::state::CodexActiveAccountSelectionError,
) -> ApiError {
    let code = error.code();
    let message = error.to_string();
    let mut api_error = ApiError::not_found(message);
    api_error.code = Some(code);
    api_error
}

#[cfg(test)]
mod retry_response_tests {
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn rate_limit_response_exposes_matching_header_and_json_delay() {
        let response = super::ApiError::proxy(crate::proxy::ProxyError::rate_limited(
            "account is cooling down",
            7,
        ))
        .into_response();

        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[axum::http::header::RETRY_AFTER], "7");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["retryAfterSeconds"], 7);
        assert_eq!(json["error"], "account is cooling down");
        assert_eq!(json["retryable"], true);
    }

    #[tokio::test]
    async fn capacity_shed_response_has_stable_code_and_short_retry_delay() {
        let response = super::ApiError::proxy(crate::proxy::ProxyError::upstream_capacity_shed(1))
            .into_response();

        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(response.headers()[axum::http::header::RETRY_AFTER], "1");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "cc_switch_upstream_capacity_shed");
        assert_eq!(json["retryAfterSeconds"], 1);
        assert_eq!(json["retryable"], true);
        assert_eq!(
            json["error"],
            "OpenAI Codex upstream is temporarily overloaded; retry shortly"
        );
        assert!(!json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("CC_UPSTREAM_CAPACITY_SHED"));
    }
}

pub(crate) fn map_codex_workspace_rebind_error(
    error: crate::domain::sharing::subscription_identity::CodexWorkspaceRebindError,
) -> ApiError {
    use crate::domain::sharing::subscription_identity::CodexWorkspaceRebindError;

    match error {
        CodexWorkspaceRebindError::ShareNotFound(_)
        | CodexWorkspaceRebindError::AccountNotFound(_) => ApiError::not_found(error.to_string()),
        CodexWorkspaceRebindError::InvalidWorkspace(_) => {
            ApiError::unprocessable_code(error.code(), error.to_string())
        }
        CodexWorkspaceRebindError::SubscriptionBinding(binding) => {
            map_subscription_binding_error(binding)
        }
        CodexWorkspaceRebindError::RevisionConflict { .. }
        | CodexWorkspaceRebindError::MustBePaused
        | CodexWorkspaceRebindError::AccountBindingMismatch
        | CodexWorkspaceRebindError::AccountInUse { .. } => {
            ApiError::conflict_code(error.code(), error.to_string())
        }
    }
}

pub(crate) fn map_copilot_device_error(error: CopilotDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

pub(crate) fn map_kiro_device_error(error: KiroDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

pub(crate) fn map_codex_device_error(error: CodexDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

pub(crate) fn map_grok_device_error(error: GrokDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

pub(crate) fn map_kimi_device_error(error: KimiDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

pub(crate) fn map_qoder_client_error(
    error: crate::clients::oauth::qoder::QoderClientError,
) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        super::providers::redact_provider_test_error(&error.message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn proxy_api_error_response_includes_stable_code_and_type() {
        let response = ApiError::proxy(crate::proxy::ProxyError::bad_gateway("connection refused"))
            .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = json_body(response).await;

        assert_eq!(body["ok"].as_bool(), Some(false));
        assert_eq!(body["code"].as_str(), Some("cc_switch_forward_failed"));
        assert_eq!(body["type"].as_str(), Some("upstream_error"));
        assert_eq!(body["status"].as_u64(), Some(502));
        assert_eq!(body["retryable"].as_bool(), Some(true));
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("connection refused"));
    }

    #[tokio::test]
    async fn cursor_session_loss_has_a_stable_conflict_code() {
        let response = ApiError::proxy(crate::proxy::ProxyError::cursor_session_lost(
            "Cursor session is unavailable",
        ))
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = json_body(response).await;
        assert_eq!(body["code"], "cursor_session_lost");
        assert_eq!(body["error"], "Cursor session is unavailable");
        assert_eq!(body["retryable"], false);
    }

    #[tokio::test]
    async fn revision_conflict_error_preserves_authoritative_details() {
        let response = ApiError::conflict_code(
            "cc_switch_share_revision_conflict",
            "managed grant expected config revision 3, current revision is 4",
        )
        .with_retryable(true)
        .with_details(serde_json::json!({
            "currentConfigRevision": 4,
            "currentShare": { "shareId": "share-a", "configRevision": 4 },
        }))
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = json_body(response).await;
        assert_eq!(body["code"], "cc_switch_share_revision_conflict");
        assert_eq!(body["retryable"], true);
        assert_eq!(body["details"]["currentConfigRevision"], 4);
        assert_eq!(body["details"]["currentShare"]["shareId"], "share-a");
        assert_eq!(body["details"]["currentShare"]["configRevision"], 4);
    }

    #[tokio::test]
    async fn previous_response_context_error_uses_stable_openai_shape() {
        let response = InferenceApiError::proxy(
            InferenceSurface::OpenAi,
            Some("request-previous-context".to_string()),
            crate::proxy::ProxyError::response_context_unavailable(),
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = json_body(response).await;
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "response_context_unavailable");
        assert_eq!(body["error"]["param"], "previous_response_id");
        assert_eq!(
            body["error"]["message"],
            "Required previous response tool context is unavailable"
        );
    }

    #[tokio::test]
    async fn cursor_continuation_in_progress_is_retryable_on_anthropic_surface() {
        let response = InferenceApiError::proxy(
            InferenceSurface::Anthropic,
            Some("request-cursor-continuation".to_string()),
            crate::proxy::ProxyError::cursor_continuation_in_progress(
                "Cursor continuation is already being resumed by another request",
            ),
        )
        .into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response
                .headers()
                .get("x-cc-switch-error-code")
                .and_then(|value| value.to_str().ok()),
            Some("cursor_continuation_in_progress")
        );
        assert_eq!(
            response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("5")
        );
        assert_eq!(
            response
                .headers()
                .get("x-should-retry")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
        let body = json_body(response).await;
        assert_eq!(body["error"]["code"], "cursor_continuation_in_progress");
        assert_eq!(body["error"]["details"]["retryable"], true);
        assert_eq!(
            body["error"]["message"],
            "Cursor continuation is already being resumed by another request"
        );
    }

    #[tokio::test]
    async fn inference_concurrency_errors_use_surface_native_bodies_and_headers() {
        for (surface, expected_path, expected_value) in [
            (
                InferenceSurface::OpenAi,
                "/error/code",
                "cc_switch_user_concurrency_limit_exceeded",
            ),
            (
                InferenceSurface::Anthropic,
                "/error/code",
                "cc_switch_user_concurrency_limit_exceeded",
            ),
            (InferenceSurface::Gemini, "/error/status", "ABORTED"),
        ] {
            let response = InferenceApiError::proxy(
                surface,
                Some("request-123".to_string()),
                crate::proxy::ProxyError::concurrency_limited(
                    crate::proxy::ProxyConcurrencyScope::User,
                    2,
                    2,
                    "Your Share user concurrency limit has been reached (2/2).",
                ),
            )
            .into_response();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(
                response
                    .headers()
                    .get("x-cc-switch-error-code")
                    .and_then(|value| value.to_str().ok()),
                Some("cc_switch_user_concurrency_limit_exceeded")
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-cc-switch-error-scope")
                    .and_then(|value| value.to_str().ok()),
                Some("user")
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some("request-123")
            );
            assert!(!response.headers().contains_key(RETRY_AFTER));
            if surface == InferenceSurface::Anthropic {
                assert_eq!(
                    response
                        .headers()
                        .get("x-should-retry")
                        .and_then(|value| value.to_str().ok()),
                    Some("false")
                );
            }
            let body = json_body(response).await;
            assert_eq!(
                body.pointer(expected_path).and_then(Value::as_str),
                Some(expected_value)
            );
            if surface != InferenceSurface::Gemini {
                assert_eq!(body["error"]["details"]["current"], 2);
                assert_eq!(body["error"]["details"]["limit"], 2);
            }
        }
    }

    #[tokio::test]
    async fn kiro_tool_json_error_exposes_terminal_code_without_internal_prefix() {
        let error = crate::proxy::kiro::KiroToolJsonError::Incomplete {
            tool_use_id: "toolu_1".to_string(),
            name: "Read".to_string(),
            bytes: 17,
        };
        let response =
            ApiError::proxy(crate::proxy::ProxyError::kiro_tool_json(error)).into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = json_body(response).await;

        assert_eq!(body["code"].as_str(), Some("TOOL_JSON_INCOMPLETE"));
        assert_eq!(body["type"].as_str(), Some("upstream_tool_json_error"));
        assert_eq!(body["retryable"].as_bool(), Some(false));
        assert!(!body["error"].as_str().unwrap().starts_with('['));

        let response = ApiError::proxy(crate::proxy::ProxyError::kiro_tool_json(
            crate::proxy::kiro::KiroToolJsonError::Limit {
                tool_use_id: "toolu_2".to_string(),
                name: "Write".to_string(),
                bytes: 9,
                max: 8,
            },
        ))
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = json_body(response).await;
        assert_eq!(body["code"].as_str(), Some("TOOL_JSON_LIMIT"));
        assert_eq!(body["type"].as_str(), Some("upstream_tool_json_error"));
        assert_eq!(body["retryable"].as_bool(), Some(false));
    }

    #[test]
    fn device_error_mappers_redact_provider_diagnostics() {
        let copilot = map_copilot_device_error(CopilotDeviceError {
            status: reqwest::StatusCode::BAD_GATEWAY,
            message: "request https://client:password@example.com/token?code=private failed"
                .to_string(),
            endpoint_validation: false,
        });
        assert!(!copilot.message.contains("client"));
        assert!(!copilot.message.contains("password"));
        assert!(!copilot.message.contains("private"));

        let kiro = map_kiro_device_error(KiroDeviceError {
            status: reqwest::StatusCode::UNAUTHORIZED,
            message: "provider rejected ksk_abcdefghijklmnop".to_string(),
        });
        assert!(!kiro.message.contains("abcdefghijklmnop"));
        assert!(!kiro.message.contains("mnop"));
        assert!(kiro.message.contains("[REDACTED_KIRO_API_KEY]"));

        let grok = map_grok_device_error(GrokDeviceError {
            status: reqwest::StatusCode::BAD_REQUEST,
            message: "access_token=secret-provider-detail".to_string(),
        });
        assert!(!grok.message.contains("secret-provider-detail"));
        assert!(grok.message.contains("[REDACTED]"));

        let codex = map_codex_device_error(CodexDeviceError {
            status: reqwest::StatusCode::BAD_GATEWAY,
            message: "x".repeat(2_000),
        });
        assert_eq!(codex.message.chars().count(), 800);
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }
}
