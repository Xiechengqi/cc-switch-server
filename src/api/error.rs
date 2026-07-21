use axum::http::StatusCode;
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
}

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
    pub(crate) code: Option<&'static str>,
    pub(crate) error_type: Option<&'static str>,
    pub(crate) retryable: Option<bool>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            error_type: None,
            retryable: None,
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
        }
    }

    pub(crate) fn provider_contract_mismatch(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: error.into(),
            code: Some("cc_switch_provider_contract_mismatch"),
            error_type: Some("provider_contract_mismatch"),
            retryable: Some(false),
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
        }
    }

    pub(crate) fn web_invoke_unknown(command: impl Into<String>) -> Self {
        let command = command.into();
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: format!(
                "desktop invoke command '{command}' is not registered in cc-switch-server"
            ),
            code: Some("cc_switch_web_invoke_unknown"),
            error_type: Some("web_invoke_unknown"),
            retryable: Some(false),
        }
    }

    pub(crate) fn web_invoke_not_wired(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            message: error.into(),
            code: Some("cc_switch_web_invoke_not_wired"),
            error_type: Some("web_invoke_not_wired"),
            retryable: Some(false),
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
        Self {
            status: error.status,
            message,
            code: Some(code),
            error_type: Some(error_type),
            retryable: Some(retryable),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                ok: false,
                error: self.message,
                code: self.code,
                error_type: self.error_type,
                status: Some(self.status.as_u16()),
                retryable: self.retryable,
            }),
        )
            .into_response()
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
        crate::domain::sharing::shares::SharePatchError::Invalid(message) => {
            ApiError::bad_request(message)
        }
    }
}

pub(crate) fn map_copilot_device_error(error: CopilotDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

pub(crate) fn map_kiro_device_error(error: KiroDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

pub(crate) fn map_codex_device_error(error: CodexDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

pub(crate) fn map_grok_device_error(error: GrokDeviceError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
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

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }
}
