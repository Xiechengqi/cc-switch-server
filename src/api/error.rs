use axum::http::{header::RETRY_AFTER, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::clients::oauth::codex_device::CodexDeviceError;
use crate::clients::oauth::copilot_device::CopilotDeviceError;
use crate::clients::oauth::grok_device::GrokDeviceError;
use crate::clients::oauth::kiro_device::KiroDeviceError;
use crate::clients::router::email_auth::EmailAuthError;
use crate::proxy;

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
}

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
    pub(crate) code: Option<&'static str>,
    pub(crate) error_type: Option<&'static str>,
    pub(crate) retryable: Option<bool>,
    pub(crate) retry_after_seconds: Option<u64>,
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
        }
    }

    pub(crate) fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, error.to_string())
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
    match error {
        crate::state::CodexActiveAccountSelectionError::AccountNotFound(_) => {
            let mut api_error = ApiError::not_found(message);
            api_error.code = Some(code);
            api_error
        }
        crate::state::CodexActiveAccountSelectionError::ShareConflict { .. } => {
            ApiError::conflict_code(code, message)
        }
    }
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
    }

    #[test]
    fn device_error_mappers_redact_provider_diagnostics() {
        let copilot = map_copilot_device_error(CopilotDeviceError {
            status: reqwest::StatusCode::BAD_GATEWAY,
            message: "request https://client:password@example.com/token?code=private failed"
                .to_string(),
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
