use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::oauth::OAuthTokenResponse;
use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::kimi_cli::{
    account_record_id, enrich_profile, extract_user_id, KimiDeviceIdentity, KIMI_CLIENT_ID,
    KIMI_DEVICE_AUTHORIZATION_URL, KIMI_TOKEN_URL,
};
use crate::domain::providers::model::ProviderType;

const KIMI_DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_INTERVAL_SECS: u64 = 5;
const MAX_INTERVAL_SECS: u64 = 5 * 60;
const DEFAULT_EXPIRES_IN_SECS: u64 = 15 * 60;
const MAX_EXPIRES_IN_SECS: u64 = 30 * 60;
const MAX_DEVICE_RESPONSE_BODY_BYTES: usize = 256 * 1024;
const DEVICE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct KimiDeviceFlowStore {
    pending: BTreeMap<String, KimiDeviceFlowEntry>,
}

#[derive(Debug, Clone)]
struct KimiDeviceFlowEntry {
    flow: PendingKimiDeviceFlow,
    state: KimiDeviceFlowState,
}

#[derive(Debug, Clone)]
enum KimiDeviceFlowState {
    Pending { next_poll_at_ms: i64 },
    Polling,
    Completed(Box<KimiDevicePollResult>),
}

#[derive(Debug, Clone)]
pub enum KimiDevicePollLease {
    Ready(PendingKimiDeviceFlow),
    Wait(u64),
    InProgress,
    Completed(Box<KimiDevicePollResult>),
}

impl KimiDeviceFlowStore {
    pub fn insert(&mut self, device_code: String, flow: PendingKimiDeviceFlow, now_ms: i64) {
        self.cleanup(now_ms);
        self.pending.insert(
            device_code,
            KimiDeviceFlowEntry {
                flow,
                state: KimiDeviceFlowState::Pending {
                    next_poll_at_ms: now_ms,
                },
            },
        );
    }

    pub fn begin_poll(&mut self, device_code: &str, now_ms: i64) -> Option<KimiDevicePollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(device_code)?;
        match &entry.state {
            KimiDeviceFlowState::Pending { next_poll_at_ms } if now_ms < *next_poll_at_ms => {
                let remaining_ms = next_poll_at_ms.saturating_sub(now_ms);
                Some(KimiDevicePollLease::Wait(
                    u64::try_from(remaining_ms)
                        .unwrap_or(u64::MAX)
                        .saturating_add(999)
                        / 1_000,
                ))
            }
            KimiDeviceFlowState::Pending { .. } => {
                entry.state = KimiDeviceFlowState::Polling;
                Some(KimiDevicePollLease::Ready(entry.flow.clone()))
            }
            KimiDeviceFlowState::Polling => Some(KimiDevicePollLease::InProgress),
            KimiDeviceFlowState::Completed(result) => {
                Some(KimiDevicePollLease::Completed(result.clone()))
            }
        }
    }

    pub fn finish_poll(
        &mut self,
        device_code: &str,
        mut result: KimiDevicePollResult,
        now_ms: i64,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(device_code) else {
            return false;
        };
        if !matches!(entry.state, KimiDeviceFlowState::Polling) {
            return false;
        }
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
            KimiDeviceFlowState::Pending {
                next_poll_at_ms: now_ms.saturating_add((delay as i64).saturating_mul(1_000)),
            }
        } else {
            KimiDeviceFlowState::Completed(Box::new(result))
        };
        true
    }

    pub fn fail_poll(&mut self, device_code: &str, terminal: bool, now_ms: i64) {
        if terminal {
            self.pending.remove(device_code);
        } else if let Some(entry) = self.pending.get_mut(device_code) {
            entry.state = KimiDeviceFlowState::Pending {
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
pub struct PendingKimiDeviceFlow {
    pub expires_at_ms: i64,
    pub interval: u64,
    pub device_identity: KimiDeviceIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiDeviceCodeResponse {
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
pub struct KimiDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
pub struct KimiDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl KimiDeviceError {
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

impl fmt::Display for KimiDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for KimiDeviceError {}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    #[serde(default)]
    verification_uri: Option<String>,
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
) -> Result<(KimiDeviceCodeResponse, PendingKimiDeviceFlow), KimiDeviceError> {
    let identity = KimiDeviceIdentity::random();
    let mut response = kimi_device_post(http, KIMI_DEVICE_AUTHORIZATION_URL, &identity)
        .form(&[("client_id", KIMI_CLIENT_ID)])
        .send()
        .await
        .map_err(|error| {
            KimiDeviceError::bad_gateway(format!(
                "Kimi device authorization request failed: {error}"
            ))
        })?;
    let status = response.status();
    let body = read_device_body(&mut response, "Kimi device authorization response").await?;
    if !status.is_success() {
        tracing::warn!(
            upstream_status = status.as_u16(),
            "Kimi device authorization rejected"
        );
        return Err(KimiDeviceError::bad_gateway(
            "Kimi device authorization was rejected by the upstream service",
        ));
    }
    let device: DeviceCodeResponse = serde_json::from_slice(&body).map_err(|error| {
        KimiDeviceError::bad_gateway(format!(
            "Kimi device authorization response parse failed: {error}"
        ))
    })?;
    normalize_device_code_response(device, identity, now_ms)
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: &PendingKimiDeviceFlow,
    now_ms: i64,
) -> Result<KimiDevicePollResult, KimiDeviceError> {
    if flow.expires_at_ms <= now_ms {
        return Err(KimiDeviceError::unauthorized(
            "Kimi device code expired; restart login",
        ));
    }
    let mut response = kimi_device_post(http, KIMI_TOKEN_URL, &flow.device_identity)
        .form(&[
            ("client_id", KIMI_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", KIMI_DEVICE_GRANT),
        ])
        .send()
        .await
        .map_err(|error| {
            KimiDeviceError::bad_gateway(format!("Kimi device poll request failed: {error}"))
        })?;
    let status = response.status();
    let body = read_device_body(&mut response, "Kimi device poll response").await?;
    if !status.is_success() {
        if let Some(pending) = pending_result_from_error(status, &body, flow.interval) {
            return Ok(pending);
        }
        let error = serde_json::from_slice::<DeviceErrorResponse>(&body).ok();
        let terminal = error
            .as_ref()
            .and_then(|error| error.error.as_deref())
            .is_some_and(|error| matches!(error, "expired_token" | "access_denied"));
        tracing::warn!(
            upstream_status = status.as_u16(),
            terminal,
            "Kimi device poll rejected"
        );
        return Err(
            if terminal || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                KimiDeviceError::unauthorized("Kimi device authorization expired or was denied")
            } else {
                KimiDeviceError::bad_gateway(
                    "Kimi device token request was rejected by the upstream service",
                )
            },
        );
    }
    let raw: Value = serde_json::from_slice(&body).map_err(|error| {
        KimiDeviceError::bad_gateway(format!("Kimi OAuth token response parse failed: {error}"))
    })?;
    let tokens: OAuthTokenResponse = serde_json::from_value(raw.clone()).map_err(|error| {
        KimiDeviceError::bad_gateway(format!("Kimi OAuth token response missing fields: {error}"))
    })?;
    let account_input = account_input_from_tokens(&tokens, raw, &flow.device_identity, now_ms)?;
    Ok(KimiDevicePollResult {
        pending: false,
        message: "Kimi OAuth device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

fn normalize_device_code_response(
    device: DeviceCodeResponse,
    identity: KimiDeviceIdentity,
    now_ms: i64,
) -> Result<(KimiDeviceCodeResponse, PendingKimiDeviceFlow), KimiDeviceError> {
    let device_code = device.device_code.trim().to_string();
    let user_code = device.user_code.trim().to_string();
    let verification_uri_complete = device
        .verification_uri_complete
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let verification_uri = device
        .verification_uri
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| verification_uri_complete.clone())
        .ok_or_else(|| {
            KimiDeviceError::bad_gateway(
                "Kimi device authorization response is missing verification URI",
            )
        })?;
    if device_code.is_empty() || user_code.is_empty() {
        return Err(KimiDeviceError::bad_gateway(
            "Kimi device authorization response is missing required fields",
        ));
    }
    let interval = bounded_poll_interval(
        device
            .interval
            .filter(|interval| *interval > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS),
    );
    let expires_in = device
        .expires_in
        .filter(|expires_in| *expires_in > 0)
        .unwrap_or(DEFAULT_EXPIRES_IN_SECS)
        .min(MAX_EXPIRES_IN_SECS);
    Ok((
        KimiDeviceCodeResponse {
            device_code,
            user_code,
            verification_uri,
            verification_uri_complete,
            expires_in,
            interval,
        },
        PendingKimiDeviceFlow {
            expires_at_ms: now_ms.saturating_add((expires_in as i64).saturating_mul(1_000)),
            interval,
            device_identity: identity,
        },
    ))
}

fn pending_result_from_error(
    status: StatusCode,
    body: &[u8],
    interval: u64,
) -> Option<KimiDevicePollResult> {
    if !matches!(status, StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN) {
        return None;
    }
    let parsed = serde_json::from_slice::<DeviceErrorResponse>(body).ok()?;
    let error = parsed.error.as_deref()?.trim();
    match error {
        "authorization_pending" => Some(KimiDevicePollResult {
            pending: true,
            message: "authorization_pending".to_string(),
            retry_after_secs: Some(bounded_poll_interval(interval)),
            account_input: None,
        }),
        "slow_down" => Some(KimiDevicePollResult {
            pending: true,
            message: "slow_down".to_string(),
            retry_after_secs: Some(bounded_poll_interval(interval.saturating_add(5))),
            account_input: None,
        }),
        _ if parsed
            .error_description
            .as_deref()
            .is_some_and(|description| description.contains("authorization_pending")) =>
        {
            Some(KimiDevicePollResult {
                pending: true,
                message: "authorization_pending".to_string(),
                retry_after_secs: Some(bounded_poll_interval(interval)),
                account_input: None,
            })
        }
        _ => None,
    }
}

fn account_input_from_tokens(
    tokens: &OAuthTokenResponse,
    mut raw: Value,
    identity: &KimiDeviceIdentity,
    now_ms: i64,
) -> Result<UpsertAccountInput, KimiDeviceError> {
    let user_id = extract_user_id(&tokens.access_token).ok_or_else(|| {
        KimiDeviceError::unauthorized(
            "Kimi access token has no stable userId identity claim; restart login",
        )
    })?;
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            KimiDeviceError::unauthorized(
                "Kimi token response is missing refresh_token; restart login",
            )
        })?;
    if let Some(object) = raw.as_object_mut() {
        object.insert("importedBy".to_string(), json!("kimi_oauth_device_flow"));
        object.insert("importedAtMs".to_string(), json!(now_ms));
        object.insert("loginMethod".to_string(), json!("device"));
    }
    let account_id = account_record_id(&user_id);
    let mut profile = None;
    enrich_profile(&mut profile, Some(&user_id), identity);
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::KimiCode,
        email: None,
        access_token: Some(tokens.access_token.clone()),
        refresh_token: Some(refresh_token.to_string()),
        id_token: tokens.id_token.clone(),
        token_type: tokens
            .token_type
            .clone()
            .or_else(|| Some("Bearer".to_string())),
        api_key: None,
        extra_headers: None,
        scopes: tokens
            .scope
            .as_deref()
            .map(|scope| scope.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
        profile,
        raw: Some(raw),
        subscription_level: Some("kimi_code".to_string()),
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at: tokens
            .expires_in
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000))),
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

fn bounded_poll_interval(interval: u64) -> u64 {
    interval.clamp(1, MAX_INTERVAL_SECS)
}

fn kimi_device_post(
    http: &reqwest::Client,
    url: &str,
    identity: &KimiDeviceIdentity,
) -> reqwest::RequestBuilder {
    let mut request = http
        .post(url)
        .timeout(DEVICE_REQUEST_TIMEOUT)
        .header("Content-Type", "application/x-www-form-urlencoded");
    for (name, value) in identity.headers() {
        request = request.header(name, value);
    }
    request
}

async fn read_device_body(
    response: &mut reqwest::Response,
    context: &str,
) -> Result<bytes::Bytes, KimiDeviceError> {
    crate::infra::http::read_response_body_limited(response, MAX_DEVICE_RESPONSE_BODY_BYTES)
        .await
        .map_err(|error| KimiDeviceError::bad_gateway(format!("{context} read failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(message: &str, retry_after_secs: u64) -> KimiDevicePollResult {
        KimiDevicePollResult {
            pending: true,
            message: message.to_string(),
            retry_after_secs: Some(retry_after_secs),
            account_input: None,
        }
    }

    #[test]
    fn flow_store_serializes_polling_and_honors_slow_down() {
        let mut store = KimiDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingKimiDeviceFlow {
                expires_at_ms: 60_000,
                interval: 5,
                device_identity: KimiDeviceIdentity::stable_for_account("fixture"),
            },
            0,
        );
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(KimiDevicePollLease::Ready(_))
        ));
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(KimiDevicePollLease::InProgress)
        ));
        assert!(store.finish_poll("device", pending("slow_down", 10), 0));
        assert!(matches!(
            store.begin_poll("device", 9_000),
            Some(KimiDevicePollLease::Wait(1))
        ));
        assert!(matches!(
            store.begin_poll("device", 10_000),
            Some(KimiDevicePollLease::Ready(PendingKimiDeviceFlow {
                interval: 10,
                ..
            }))
        ));
    }

    #[test]
    fn completed_login_keeps_flow_device_identity_in_account_profile() {
        let identity = KimiDeviceIdentity::stable_for_account("fixture");
        let tokens = OAuthTokenResponse {
            access_token: "e30.eyJ1c2VyX2lkIjoia2ltaS11c2VyIn0.signature".to_string(),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            scope: Some("openid coding".to_string()),
            expires_in: Some(900),
            extra: Value::Null,
        };
        let input = account_input_from_tokens(&tokens, json!({}), &identity, 1_000).unwrap();
        assert_eq!(input.provider_type, ProviderType::KimiCode);
        assert_eq!(
            crate::domain::kimi_cli::device_identity_from_profile(input.profile.as_ref()),
            Some(identity)
        );
        assert_eq!(
            crate::domain::kimi_cli::user_id_from_profile(input.profile.as_ref()).as_deref(),
            Some("kimi-user")
        );
    }

    #[test]
    fn pending_errors_are_sanitized_and_poll_interval_is_bounded() {
        let result = pending_result_from_error(
            StatusCode::BAD_REQUEST,
            br#"{"error":"authorization_pending","error_description":"access_token=secret"}"#,
            5,
        )
        .unwrap();
        assert_eq!(result.message, "authorization_pending");
        assert!(!result.message.contains("secret"));
        assert_eq!(bounded_poll_interval(u64::MAX), MAX_INTERVAL_SECS);
    }
}
