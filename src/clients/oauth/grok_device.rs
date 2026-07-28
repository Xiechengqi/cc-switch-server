use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::oauth::{
    upsert_input_from_verified_grok_token_response, OAuthTokenResponse,
};
use crate::domain::accounts::store::UpsertAccountInput;

const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const XAI_CLI_SCOPE: &str =
    "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write";
const XAI_USER_AGENT: &str = "cc-switch-server-grok-oauth";
const DEFAULT_INTERVAL_SECS: u64 = 5;
const MAX_INTERVAL_SECS: u64 = 5 * 60;
const MAX_EXPIRES_IN_SECS: u64 = 30 * 60;
const MAX_DEVICE_RESPONSE_BODY_BYTES: usize = 256 * 1024;
const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    Pending { next_poll_at_ms: i64 },
    Polling,
    Completed(Box<GrokDevicePollResult>),
}

#[derive(Debug, Clone)]
pub enum GrokDevicePollLease {
    Ready(PendingGrokDeviceFlow),
    Wait(u64),
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
                state: GrokDeviceFlowState::Pending {
                    next_poll_at_ms: now_ms,
                },
            },
        );
    }

    pub fn begin_poll(&mut self, device_code: &str, now_ms: i64) -> Option<GrokDevicePollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(device_code)?;
        match &entry.state {
            GrokDeviceFlowState::Pending { next_poll_at_ms } if now_ms < *next_poll_at_ms => {
                let remaining_ms = next_poll_at_ms.saturating_sub(now_ms);
                Some(GrokDevicePollLease::Wait(
                    u64::try_from(remaining_ms)
                        .unwrap_or(u64::MAX)
                        .saturating_add(999)
                        / 1_000,
                ))
            }
            GrokDeviceFlowState::Pending { .. } => {
                entry.state = GrokDeviceFlowState::Polling;
                Some(GrokDevicePollLease::Ready(entry.flow.clone()))
            }
            GrokDeviceFlowState::Polling => Some(GrokDevicePollLease::InProgress),
            GrokDeviceFlowState::Completed(result) => {
                Some(GrokDevicePollLease::Completed(result.clone()))
            }
        }
    }

    pub fn finish_poll(
        &mut self,
        device_code: &str,
        mut result: GrokDevicePollResult,
        now_ms: i64,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(device_code) else {
            return false;
        };
        let GrokDeviceFlowState::Polling = entry.state else {
            return false;
        };
        entry.state = if result.pending {
            let delay = result
                .retry_after_secs
                .unwrap_or(entry.flow.interval)
                .max(entry.flow.interval)
                .clamp(1, MAX_INTERVAL_SECS);
            if result.message == "slow_down" {
                entry.flow.interval = delay;
            }
            result.retry_after_secs = Some(delay);
            GrokDeviceFlowState::Pending {
                next_poll_at_ms: now_ms.saturating_add((delay as i64).saturating_mul(1_000)),
            }
        } else {
            GrokDeviceFlowState::Completed(Box::new(result))
        };
        true
    }

    pub fn fail_poll(&mut self, device_code: &str, terminal: bool, now_ms: i64) {
        if terminal {
            self.pending.remove(device_code);
        } else if let Some(entry) = self.pending.get_mut(device_code) {
            entry.state = GrokDeviceFlowState::Pending {
                next_poll_at_ms: now_ms.saturating_add(
                    (bounded_poll_interval(entry.flow.interval) as i64).saturating_mul(1_000),
                ),
            };
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
    let mut response = grok_device_post(http, XAI_DEVICE_CODE_URL)
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

    let status = response.status();
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_DEVICE_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok device code response read failed: {error}"))
    })?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body);
        return Err(GrokDeviceError::bad_gateway(format!(
            "grok device code request failed: {status} - {text}"
        )));
    }

    let device: DeviceCodeResponse = serde_json::from_slice(&body).map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok device code response parse failed: {error}"))
    })?;
    normalize_device_code_response(device, now_ms)
}

fn normalize_device_code_response(
    device: DeviceCodeResponse,
    now_ms: i64,
) -> Result<(GrokDeviceCodeResponse, PendingGrokDeviceFlow), GrokDeviceError> {
    let device_code = device.device_code.trim().to_string();
    let user_code = device.user_code.trim().to_string();
    let verification_uri = device.verification_uri.trim().to_string();
    if device_code.is_empty() || user_code.is_empty() || verification_uri.is_empty() {
        return Err(GrokDeviceError::bad_gateway(
            "grok device code response is missing required fields",
        ));
    }
    let verification_uri_complete = device
        .verification_uri_complete
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let interval = bounded_poll_interval(
        device
            .interval
            .filter(|interval| *interval > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS),
    );
    let expires_in = device
        .expires_in
        .filter(|expires_in| *expires_in > 0)
        .unwrap_or(MAX_EXPIRES_IN_SECS)
        .min(MAX_EXPIRES_IN_SECS);
    let flow = PendingGrokDeviceFlow {
        expires_at_ms: now_ms.saturating_add((expires_in as i64).saturating_mul(1000)),
        interval,
    };
    Ok((
        GrokDeviceCodeResponse {
            device_code,
            user_code,
            verification_uri,
            verification_uri_complete,
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
    let token_url = crate::domain::accounts::oauth::grok_oauth_token_url().map_err(|error| {
        GrokDeviceError::bad_gateway(format!(
            "grok device token endpoint is invalid: {}",
            error.message
        ))
    })?;
    let mut response = grok_device_post(http, &token_url)
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
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_DEVICE_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok device poll response read failed: {error}"))
    })?;
    let text = String::from_utf8_lossy(&body);
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

    let raw: Value = serde_json::from_slice(&body).map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok oauth token response parse failed: {error}"))
    })?;
    let tokens: OAuthTokenResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        GrokDeviceError::bad_gateway(format!("grok oauth token response missing fields: {error}"))
    })?;
    let id_token = tokens.id_token.as_deref().ok_or_else(|| {
        GrokDeviceError::unauthorized("grok device token response is missing id_token")
    })?;
    let verified = crate::clients::oauth::grok_jwks::verify_grok_id_token(http, id_token, None)
        .await
        .map_err(|error| {
            tracing::warn!(error = %error, "grok device ID token verification failed");
            GrokDeviceError::unauthorized("grok device ID token verification failed; restart login")
        })?;
    let raw = merge_device_raw(raw, now_ms);
    let mut account_input =
        upsert_input_from_verified_grok_token_response(&tokens, raw, &verified.identity, now_ms)
            .map_err(|error| GrokDeviceError::bad_gateway(error.message))?;
    crate::domain::accounts::store::set_verified_grok_claims(
        &mut account_input.profile,
        Some(verified.canonical_claims),
    );

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
            retry_after_secs: Some(bounded_poll_interval(interval)),
            account_input: None,
        }),
        "slow_down" => Some(GrokDevicePollResult {
            pending: true,
            message: "slow_down".to_string(),
            retry_after_secs: Some(bounded_poll_interval(interval.saturating_add(5))),
            account_input: None,
        }),
        _ => parsed
            .and_then(|value| value.error_description)
            .filter(|message| message.contains("authorization_pending"))
            .map(|_| GrokDevicePollResult {
                pending: true,
                message: "authorization_pending".to_string(),
                retry_after_secs: Some(bounded_poll_interval(interval)),
                account_input: None,
            }),
    }
}

fn bounded_poll_interval(interval: u64) -> u64 {
    interval.clamp(1, MAX_INTERVAL_SECS)
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

fn grok_device_post(http: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    http.post(url)
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", XAI_USER_AGENT)
        .header(
            "x-grok-client-version",
            crate::domain::grok_cli::grok_cli_version(),
        )
        .header("x-grok-client-surface", "ui")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct EnvGuard {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var(name).ok();
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

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

    fn pending_result(message: &str, retry_after_secs: u64) -> GrokDevicePollResult {
        GrokDevicePollResult {
            pending: true,
            message: message.to_string(),
            retry_after_secs: Some(retry_after_secs),
            account_input: None,
        }
    }

    #[test]
    fn slow_down_cumulatively_increases_future_poll_interval() {
        let mut store = GrokDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingGrokDeviceFlow {
                expires_at_ms: 60_000,
                interval: 5,
            },
            0,
        );

        assert!(matches!(
            store.begin_poll("device", 0),
            Some(GrokDevicePollLease::Ready(PendingGrokDeviceFlow {
                interval: 5,
                ..
            }))
        ));
        assert!(store.finish_poll("device", pending_result("slow_down", 10), 0));
        assert!(matches!(
            store.begin_poll("device", 9_000),
            Some(GrokDevicePollLease::Wait(1))
        ));
        assert!(matches!(
            store.begin_poll("device", 10_000),
            Some(GrokDevicePollLease::Ready(PendingGrokDeviceFlow {
                interval: 10,
                ..
            }))
        ));
        assert!(store.finish_poll("device", pending_result("slow_down", 15), 10_000));
        assert!(matches!(
            store.begin_poll("device", 24_000),
            Some(GrokDevicePollLease::Wait(1))
        ));
        assert!(matches!(
            store.begin_poll("device", 25_000),
            Some(GrokDevicePollLease::Ready(PendingGrokDeviceFlow {
                interval: 15,
                ..
            }))
        ));
    }

    #[test]
    fn device_poll_interval_is_bounded() {
        assert_eq!(bounded_poll_interval(0), 1);
        assert_eq!(bounded_poll_interval(u64::MAX), MAX_INTERVAL_SECS);
        let slow_down = pending_result_from_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"slow_down"}"#,
            u64::MAX,
        )
        .unwrap();
        assert_eq!(slow_down.retry_after_secs, Some(MAX_INTERVAL_SECS));
    }

    #[test]
    fn device_code_response_requires_identity_fields_and_normalizes_zero_timing() {
        let missing = normalize_device_code_response(
            DeviceCodeResponse {
                device_code: " ".to_string(),
                user_code: "code".to_string(),
                verification_uri: "https://auth.x.ai/activate".to_string(),
                verification_uri_complete: None,
                expires_in: Some(1_800),
                interval: Some(5),
            },
            0,
        )
        .unwrap_err();
        assert!(missing.message.contains("missing required fields"));

        let (device, flow) = normalize_device_code_response(
            DeviceCodeResponse {
                device_code: " device ".to_string(),
                user_code: " code ".to_string(),
                verification_uri: " https://auth.x.ai/activate ".to_string(),
                verification_uri_complete: Some(" ".to_string()),
                expires_in: Some(0),
                interval: Some(0),
            },
            1_000,
        )
        .unwrap();
        assert_eq!(device.device_code, "device");
        assert_eq!(device.user_code, "code");
        assert_eq!(device.verification_uri, "https://auth.x.ai/activate");
        assert_eq!(device.verification_uri_complete, None);
        assert_eq!(device.interval, DEFAULT_INTERVAL_SECS);
        assert_eq!(device.expires_in, MAX_EXPIRES_IN_SECS);
        assert_eq!(flow.interval, DEFAULT_INTERVAL_SECS);
        assert_eq!(
            flow.expires_at_ms,
            1_000 + (MAX_EXPIRES_IN_SECS as i64) * 1_000
        );
    }

    #[test]
    fn slow_device_response_does_not_shorten_the_next_poll_interval() {
        let mut store = GrokDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingGrokDeviceFlow {
                expires_at_ms: 60_000,
                interval: 5,
            },
            0,
        );
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(GrokDevicePollLease::Ready(_))
        ));
        assert!(store.finish_poll("device", pending_result("slow_down", 10), 4_000));
        assert!(matches!(
            store.begin_poll("device", 13_000),
            Some(GrokDevicePollLease::Wait(1))
        ));
        assert!(matches!(
            store.begin_poll("device", 14_000),
            Some(GrokDevicePollLease::Ready(_))
        ));
    }

    #[test]
    fn expired_device_flow_is_removed_before_polling() {
        let mut store = GrokDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingGrokDeviceFlow {
                expires_at_ms: 1_000,
                interval: 5,
            },
            0,
        );
        assert!(store.begin_poll("device", 1_000).is_none());
    }

    #[test]
    fn cancellation_during_poll_prevents_late_completion() {
        let mut store = GrokDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingGrokDeviceFlow {
                expires_at_ms: 60_000,
                interval: 5,
            },
            0,
        );
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(GrokDevicePollLease::Ready(_))
        ));
        assert!(store.cancel("device"));
        assert!(!store.finish_poll("device", pending_result("authorization_pending", 5), 0));
    }

    #[test]
    fn concurrent_poll_gets_in_progress_without_second_lease() {
        let mut store = GrokDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingGrokDeviceFlow {
                expires_at_ms: 60_000,
                interval: 5,
            },
            0,
        );
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(GrokDevicePollLease::Ready(_))
        ));
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(GrokDevicePollLease::InProgress)
        ));
    }

    #[tokio::test]
    async fn device_poll_uses_configured_token_url_and_rejects_missing_or_forged_id_token() {
        let _lock = crate::domain::grok_cli::GROK_ENV_LOCK.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in [
                r#"{"access_token":"access-without-id","expires_in":3600}"#,
                r#"{"access_token":"access-forged","id_token":"eyJhbGciOiJIUzI1NiIsImtpZCI6IngifQ.eyJleHAiOjQxMDI0NDQ4MDB9.sig","expires_in":3600}"#,
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let _token_url = EnvGuard::set(
            "CC_SWITCH_SERVER_XAI_TOKEN_URL",
            &format!("http://{address}/token"),
        );
        let flow = PendingGrokDeviceFlow {
            expires_at_ms: 60_000,
            interval: 5,
        };
        let http = reqwest::Client::new();

        let missing = poll_device_flow(&http, "device-missing", &flow, 0)
            .await
            .unwrap_err();
        assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
        assert!(missing.message.contains("missing id_token"));

        let forged = poll_device_flow(&http, "device-forged", &flow, 0)
            .await
            .unwrap_err();
        assert_eq!(forged.status, StatusCode::UNAUTHORIZED);
        assert!(forged.message.contains("verification failed"));

        let requests = server.await.unwrap();
        let version = crate::domain::grok_cli::grok_cli_version();
        for request in requests {
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("post /token http/1.1\r\n"));
            assert!(request.contains("x-grok-client-surface: ui\r\n"));
            assert!(request.contains(&format!("x-grok-client-version: {version}\r\n")));
        }
    }
}
