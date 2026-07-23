use std::collections::BTreeMap;
use std::fmt;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::oauth::{upsert_input_from_token_response, OAuthTokenResponse};
use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::providers::model::ProviderType;

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const XAI_DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const XAI_CLI_SCOPE: &str =
    "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write";
const XAI_USER_AGENT: &str = "cc-switch-server-grok-oauth";
const DEFAULT_INTERVAL_SECS: u64 = 5;
const MAX_EXPIRES_IN_SECS: u64 = 30 * 60;

#[derive(Debug, Clone, Default)]
pub struct GrokDeviceFlowStore {
    pending: BTreeMap<String, GrokDeviceFlowEntry>,
}

#[derive(Debug, Clone)]
struct GrokDeviceFlowEntry {
    flow: PendingGrokDeviceFlow,
    state: GrokDeviceFlowState,
}

#[derive(Debug, Clone)]
enum GrokDeviceFlowState {
    Pending,
    Polling,
    Completed(Box<GrokDevicePollResult>),
}

#[derive(Debug, Clone)]
pub enum GrokDevicePollLease {
    Ready(PendingGrokDeviceFlow),
    InProgress,
    Completed(Box<GrokDevicePollResult>),
}

impl GrokDeviceFlowStore {
    pub fn insert(&mut self, device_code: String, flow: PendingGrokDeviceFlow, now_ms: i64) {
        self.cleanup(now_ms);
        self.pending.insert(
            device_code,
            GrokDeviceFlowEntry {
                flow,
                state: GrokDeviceFlowState::Pending,
            },
        );
    }

    pub fn begin_poll(&mut self, device_code: &str, now_ms: i64) -> Option<GrokDevicePollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(device_code)?;
        match &entry.state {
            GrokDeviceFlowState::Pending => {
                entry.state = GrokDeviceFlowState::Polling;
                Some(GrokDevicePollLease::Ready(entry.flow.clone()))
            }
            GrokDeviceFlowState::Polling => Some(GrokDevicePollLease::InProgress),
            GrokDeviceFlowState::Completed(result) => {
                Some(GrokDevicePollLease::Completed(result.clone()))
            }
        }
    }

    pub fn finish_poll(&mut self, device_code: &str, result: GrokDevicePollResult) -> bool {
        let Some(entry) = self.pending.get_mut(device_code) else {
            return false;
        };
        if !matches!(entry.state, GrokDeviceFlowState::Polling) {
            return false;
        }
        entry.state = if result.pending {
            GrokDeviceFlowState::Pending
        } else {
            GrokDeviceFlowState::Completed(Box::new(result))
        };
        true
    }

    pub fn fail_poll(&mut self, device_code: &str, terminal: bool) {
        if terminal {
            self.pending.remove(device_code);
        } else if let Some(entry) = self.pending.get_mut(device_code) {
            entry.state = GrokDeviceFlowState::Pending;
        }
    }

    pub fn cancel(&mut self, device_code: &str) -> bool {
        self.pending.remove(device_code).is_some()
    }

    fn cleanup(&mut self, now_ms: i64) {
        self.pending
            .retain(|_, entry| entry.flow.expires_at_ms > now_ms);
    }
}

#[derive(Debug, Clone)]
pub struct PendingGrokDeviceFlow {
    pub expires_at_ms: i64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokDeviceCodeResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_uri: String,
    #[serde(default, alias = "verification_uri_complete")]
    pub verification_uri_complete: Option<String>,
    #[serde(alias = "expires_in")]
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
pub struct GrokDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl GrokDeviceError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

impl fmt::Display for GrokDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GrokDeviceError {}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceErrorResponse {
    #[serde(default)]
    error: Option<String>,
    #[serde(default, alias = "error_description")]
    error_description: Option<String>,
}

pub async fn start_device_flow(
    http: &reqwest::Client,
    now_ms: i64,
) -> Result<(GrokDeviceCodeResponse, PendingGrokDeviceFlow), GrokDeviceError> {
    let client_id = xai_client_id();
    let response = http
        .post(XAI_DEVICE_CODE_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", XAI_USER_AGENT)
        .form(&[
            ("client_id", client_id.as_str()),
            ("scope", XAI_CLI_SCOPE),
            ("referrer", "grok-build"),
        ])
        .send()
        .await
        .map_err(|error| {
            GrokDeviceError::bad_gateway(format!("grok device code request failed: {error}"))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(GrokDeviceError::bad_gateway(format!(
            "grok device code request failed: {status} - {text}"
        )));
    }

    let device: DeviceCodeResponse = response.json().await.map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok device code response parse failed: {error}"))
    })?;
    let interval = device.interval.unwrap_or(DEFAULT_INTERVAL_SECS).max(1);
    let expires_in = device
        .expires_in
        .unwrap_or(MAX_EXPIRES_IN_SECS)
        .min(MAX_EXPIRES_IN_SECS);
    let flow = PendingGrokDeviceFlow {
        expires_at_ms: now_ms.saturating_add((expires_in as i64).saturating_mul(1000)),
        interval,
    };
    Ok((
        GrokDeviceCodeResponse {
            device_code: device.device_code,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_in,
            interval,
        },
        flow,
    ))
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: &PendingGrokDeviceFlow,
    now_ms: i64,
) -> Result<GrokDevicePollResult, GrokDeviceError> {
    if flow.expires_at_ms <= now_ms {
        return Err(GrokDeviceError::unauthorized(
            "grok device code expired; restart login",
        ));
    }

    let client_id = xai_client_id();
    let response = http
        .post(XAI_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", XAI_USER_AGENT)
        .form(&[
            ("grant_type", XAI_DEVICE_GRANT),
            ("device_code", device_code),
            ("client_id", client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|error| {
            GrokDeviceError::bad_gateway(format!("grok device poll request failed: {error}"))
        })?;

    let status = response.status();
    let text = response.text().await.map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok device poll response read failed: {error}"))
    })?;
    if !status.is_success() {
        if let Some(pending) = pending_result_from_error(status, &text, flow.interval) {
            return Ok(pending);
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST {
            return Err(GrokDeviceError::unauthorized(format!(
                "grok device poll failed: {status} - {text}"
            )));
        }
        return Err(GrokDeviceError::bad_gateway(format!(
            "grok device poll failed: {status} - {text}"
        )));
    }

    let raw: Value = serde_json::from_str(&text).map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok oauth token response parse failed: {error}"))
    })?;
    let tokens: OAuthTokenResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok oauth token response missing fields: {error}"))
    })?;
    let raw = merge_device_raw(raw, now_ms);
    let account_input =
        upsert_input_from_token_response(ProviderType::GrokOAuth, &tokens, raw, now_ms)
            .map_err(|error| GrokDeviceError::bad_gateway(error.message))?;

    Ok(GrokDevicePollResult {
        pending: false,
        message: "grok oauth device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

fn pending_result_from_error(
    status: StatusCode,
    body: &str,
    interval: u64,
) -> Option<GrokDevicePollResult> {
    if status != StatusCode::BAD_REQUEST && status != StatusCode::FORBIDDEN {
        return None;
    }
    let parsed = serde_json::from_str::<DeviceErrorResponse>(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|value| value.error.as_deref())
        .unwrap_or(body)
        .trim();
    match error {
        "authorization_pending" => Some(GrokDevicePollResult {
            pending: true,
            message: "authorization_pending".to_string(),
            retry_after_secs: Some(interval),
            account_input: None,
        }),
        "slow_down" => Some(GrokDevicePollResult {
            pending: true,
            message: "slow_down".to_string(),
            retry_after_secs: Some(interval.saturating_add(5)),
            account_input: None,
        }),
        _ => parsed
            .and_then(|value| value.error_description)
            .filter(|message| message.contains("authorization_pending"))
            .map(|_| GrokDevicePollResult {
                pending: true,
                message: "authorization_pending".to_string(),
                retry_after_secs: Some(interval),
                account_input: None,
            }),
    }
}

fn merge_device_raw(mut raw: Value, now_ms: i64) -> Value {
    if let Some(object) = raw.as_object_mut() {
        object.insert("importedBy".to_string(), json!("grok_oauth_device_flow"));
        object.insert("importedAtMs".to_string(), json!(now_ms));
        object.insert("loginMethod".to_string(), json!("device"));
        object.insert("scopeProfile".to_string(), json!("cli_build"));
    }
    raw
}

fn xai_client_id() -> String {
    std::env::var("CC_SWITCH_SERVER_XAI_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| XAI_CLIENT_ID.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_description_never_becomes_public_message() {
        let result = pending_result_from_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"temporarily_unavailable","error_description":"authorization_pending access_token=secret-provider-detail"}"#,
            5,
        )
        .expect("pending response");

        assert!(result.pending);
        assert_eq!(result.message, "authorization_pending");
        assert_eq!(result.retry_after_secs, Some(5));
        assert!(!result.message.contains("secret-provider-detail"));
    }
}
