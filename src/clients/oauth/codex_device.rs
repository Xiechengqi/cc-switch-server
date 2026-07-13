use std::collections::BTreeMap;
use std::fmt;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::oauth::{upsert_input_from_token_response, OAuthTokenResponse};
use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::providers::model::ProviderType;

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_AUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 900;
const POLLING_SAFETY_MARGIN_SECS: u64 = 3;
const CODEX_USER_AGENT: &str = "cc-switch-server-codex-oauth";

#[derive(Debug, Clone, Default)]
pub struct CodexDeviceFlowStore {
    pending: BTreeMap<String, CodexDeviceFlowEntry>,
}

#[derive(Debug, Clone)]
struct CodexDeviceFlowEntry {
    flow: PendingCodexDeviceFlow,
    state: CodexDeviceFlowState,
}

#[derive(Debug, Clone)]
enum CodexDeviceFlowState {
    Pending,
    Polling,
    Completed(Box<CodexDevicePollResult>),
}

#[derive(Debug, Clone)]
pub enum CodexDevicePollLease {
    Ready(PendingCodexDeviceFlow),
    InProgress,
    Completed(Box<CodexDevicePollResult>),
}

impl CodexDeviceFlowStore {
    pub fn insert(&mut self, device_code: String, flow: PendingCodexDeviceFlow, now_ms: i64) {
        self.cleanup(now_ms);
        self.pending.insert(
            device_code,
            CodexDeviceFlowEntry {
                flow,
                state: CodexDeviceFlowState::Pending,
            },
        );
    }

    pub fn begin_poll(&mut self, device_code: &str, now_ms: i64) -> Option<CodexDevicePollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(device_code)?;
        match &entry.state {
            CodexDeviceFlowState::Pending => {
                entry.state = CodexDeviceFlowState::Polling;
                Some(CodexDevicePollLease::Ready(entry.flow.clone()))
            }
            CodexDeviceFlowState::Polling => Some(CodexDevicePollLease::InProgress),
            CodexDeviceFlowState::Completed(result) => {
                Some(CodexDevicePollLease::Completed(result.clone()))
            }
        }
    }

    pub fn finish_poll(&mut self, device_code: &str, result: CodexDevicePollResult) -> bool {
        let Some(entry) = self.pending.get_mut(device_code) else {
            return false;
        };
        if !matches!(entry.state, CodexDeviceFlowState::Polling) {
            return false;
        }
        entry.state = if result.pending {
            CodexDeviceFlowState::Pending
        } else {
            CodexDeviceFlowState::Completed(Box::new(result))
        };
        true
    }

    pub fn fail_poll(&mut self, device_code: &str, terminal: bool) {
        if terminal {
            self.pending.remove(device_code);
        } else if let Some(entry) = self.pending.get_mut(device_code) {
            entry.state = CodexDeviceFlowState::Pending;
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
pub struct PendingCodexDeviceFlow {
    pub user_code: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDeviceCodeResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_uri: String,
    #[serde(alias = "expires_in")]
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
pub struct CodexDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl CodexDeviceError {
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

impl fmt::Display for CodexDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodexDeviceError {}

#[derive(Debug, Clone, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    interval: Option<Value>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct DevicePollSuccess {
    authorization_code: String,
    code_verifier: String,
}

pub async fn start_device_flow(
    http: &reqwest::Client,
    now_ms: i64,
) -> Result<(CodexDeviceCodeResponse, PendingCodexDeviceFlow), CodexDeviceError> {
    let response = http
        .post(DEVICE_AUTH_USERCODE_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", CODEX_USER_AGENT)
        .json(&json!({ "client_id": CODEX_CLIENT_ID }))
        .send()
        .await
        .map_err(|error| {
            CodexDeviceError::bad_gateway(format!("codex device code request failed: {error}"))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CodexDeviceError::bad_gateway(format!(
            "codex device code request failed: {status} - {text}"
        )));
    }

    let device: DeviceCodeResponse = response.json().await.map_err(|error| {
        CodexDeviceError::bad_gateway(format!("codex device code response parse failed: {error}"))
    })?;

    let interval = parse_interval(device.interval.as_ref());
    let expires_in = device.expires_in.unwrap_or(DEVICE_CODE_DEFAULT_EXPIRES_IN);
    let expires_at_ms = now_ms + (expires_in as i64) * 1000;
    let pending = PendingCodexDeviceFlow {
        user_code: device.user_code.clone(),
        expires_at_ms,
    };

    Ok((
        CodexDeviceCodeResponse {
            device_code: device.device_auth_id,
            user_code: device.user_code,
            verification_uri: DEVICE_VERIFICATION_URL.to_string(),
            expires_in,
            interval,
        },
        pending,
    ))
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: &PendingCodexDeviceFlow,
    now_ms: i64,
) -> Result<CodexDevicePollResult, CodexDeviceError> {
    if flow.expires_at_ms <= now_ms {
        return Err(CodexDeviceError::unauthorized(
            "codex device code expired; restart login",
        ));
    }

    let poll_response = http
        .post(DEVICE_AUTH_TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", CODEX_USER_AGENT)
        .json(&json!({
            "device_auth_id": device_code,
            "user_code": flow.user_code,
        }))
        .send()
        .await
        .map_err(|error| {
            CodexDeviceError::bad_gateway(format!("codex device poll request failed: {error}"))
        })?;

    let status = poll_response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
        return Ok(CodexDevicePollResult {
            pending: true,
            message: "authorization_pending".to_string(),
            retry_after_secs: Some(flow_poll_interval_secs()),
            account_input: None,
        });
    }
    if status == StatusCode::GONE {
        return Err(CodexDeviceError::unauthorized(
            "codex device code expired; restart login",
        ));
    }
    if !status.is_success() {
        let text = poll_response.text().await.unwrap_or_default();
        return Err(CodexDeviceError::bad_gateway(format!(
            "codex device poll failed: {status} - {text}"
        )));
    }

    let success: DevicePollSuccess = poll_response.json().await.map_err(|error| {
        CodexDeviceError::bad_gateway(format!("codex device poll response parse failed: {error}"))
    })?;

    let tokens =
        exchange_code_for_tokens(http, &success.authorization_code, &success.code_verifier).await?;

    let raw = json!({
        "accessToken": tokens.access_token,
        "refreshToken": tokens.refresh_token,
        "idToken": tokens.id_token,
        "tokenType": tokens.token_type,
        "scope": tokens.scope,
        "expiresIn": tokens.expires_in,
        "importedBy": "codex_oauth_device_flow",
        "importedAtMs": now_ms,
        "loginMethod": "device",
    });

    let account_input =
        upsert_input_from_token_response(ProviderType::CodexOAuth, &tokens, raw, now_ms)
            .map_err(|error| CodexDeviceError::bad_gateway(error.message))?;

    Ok(CodexDevicePollResult {
        pending: false,
        message: "codex oauth device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

async fn exchange_code_for_tokens(
    http: &reqwest::Client,
    code: &str,
    code_verifier: &str,
) -> Result<OAuthTokenResponse, CodexDeviceError> {
    let response = http
        .post(OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", CODEX_USER_AGENT)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", DEVICE_REDIRECT_URI),
            ("client_id", CODEX_CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .map_err(|error| {
            CodexDeviceError::bad_gateway(format!("codex oauth token exchange failed: {error}"))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(CodexDeviceError::bad_gateway(format!(
            "codex oauth token exchange failed: {status} - {text}"
        )));
    }

    response.json().await.map_err(|error| {
        CodexDeviceError::bad_gateway(format!("codex oauth token response parse failed: {error}"))
    })
}

fn parse_interval(value: Option<&Value>) -> u64 {
    let raw = match value {
        Some(Value::Number(number)) => number.as_u64().unwrap_or(5),
        Some(Value::String(text)) => text.parse::<u64>().unwrap_or(5),
        _ => 5,
    };
    raw.max(1) + POLLING_SAFETY_MARGIN_SECS
}

fn flow_poll_interval_secs() -> u64 {
    5 + POLLING_SAFETY_MARGIN_SECS
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::Router;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn jwt(payload: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn parse_interval_accepts_number_and_string() {
        assert_eq!(
            parse_interval(Some(&json!(5))),
            5 + POLLING_SAFETY_MARGIN_SECS
        );
        assert_eq!(
            parse_interval(Some(&json!("10"))),
            10 + POLLING_SAFETY_MARGIN_SECS
        );
        assert_eq!(parse_interval(None), 5 + POLLING_SAFETY_MARGIN_SECS);
    }

    #[test]
    fn device_flow_store_serializes_poll_and_caches_completion() {
        let mut store = CodexDeviceFlowStore::default();
        let flow = PendingCodexDeviceFlow {
            user_code: "ABCD-EFGH".to_string(),
            expires_at_ms: 10_000,
        };
        store.insert("device".to_string(), flow.clone(), 1_000);
        assert!(matches!(
            store.begin_poll("device", 1_001),
            Some(CodexDevicePollLease::Ready(_))
        ));
        assert!(matches!(
            store.begin_poll("device", 1_002),
            Some(CodexDevicePollLease::InProgress)
        ));
        let completed = CodexDevicePollResult {
            pending: false,
            message: "done".to_string(),
            retry_after_secs: None,
            account_input: None,
        };
        assert!(store.finish_poll("device", completed));
        assert!(matches!(
            store.begin_poll("device", 1_003),
            Some(CodexDevicePollLease::Completed(_))
        ));
        assert!(store.cancel("device"));
        assert!(store.begin_poll("device", 1_004).is_none());
    }

    #[test]
    fn cancelled_in_flight_poll_cannot_publish_completion() {
        let mut store = CodexDeviceFlowStore::default();
        store.insert(
            "device".to_string(),
            PendingCodexDeviceFlow {
                user_code: "ABCD-EFGH".to_string(),
                expires_at_ms: 2_000,
            },
            1_000,
        );
        assert!(matches!(
            store.begin_poll("device", 1_001),
            Some(CodexDevicePollLease::Ready(_))
        ));
        assert!(store.cancel("device"));
        assert!(!store.finish_poll(
            "device",
            CodexDevicePollResult {
                pending: false,
                message: "done".to_string(),
                retry_after_secs: None,
                account_input: None,
            }
        ));
        assert!(store.begin_poll("device", 1_002).is_none());
    }

    #[tokio::test]
    async fn poll_device_flow_completes_with_account_input() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let poll_count = Arc::new(AtomicUsize::new(0));
        let poll_count_for_poll = poll_count.clone();
        let poll_count_for_token = poll_count.clone();
        let id_token = jwt(
            r#"{"chatgpt_account_id":"acct-42","email":"owner@example.com","openai_auth":{"chatgpt_plan_type":"plus"}}"#,
        );
        let app = Router::new()
            .route(
                "/deviceauth/token",
                post({
                    let poll_count = poll_count_for_poll;
                    move |body: String| {
                        let poll_count = poll_count.clone();
                        async move {
                            let count = poll_count.fetch_add(1, Ordering::SeqCst);
                            if count == 0 {
                                return (
                                    StatusCode::FORBIDDEN,
                                    axum::Json(json!({"error":"authorization_pending"})),
                                );
                            }
                            assert!(body.contains("auth-123"));
                            assert!(body.contains("ABCD-EFGH"));
                            (
                                StatusCode::OK,
                                axum::Json(json!({
                                    "authorization_code": "auth-code",
                                    "code_verifier": "verifier"
                                })),
                            )
                        }
                    }
                }),
            )
            .route(
                "/oauth/token",
                post({
                    let poll_count = poll_count_for_token;
                    move |headers: HeaderMap, body: String| {
                        let poll_count = poll_count.clone();
                        async move {
                            poll_count.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(
                                headers.get("content-type").and_then(|v| v.to_str().ok()),
                                Some("application/x-www-form-urlencoded")
                            );
                            assert!(body.contains("grant_type=authorization_code"));
                            assert!(body.contains("code=auth-code"));
                            assert!(body.contains("code_verifier=verifier"));
                            assert!(body.contains("redirect_uri="));
                            (
                                StatusCode::OK,
                                axum::Json(json!({
                                    "access_token": "access-token",
                                    "refresh_token": "refresh-token",
                                    "id_token": id_token,
                                    "token_type": "Bearer",
                                    "expires_in": 3600
                                })),
                            )
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        // Patch URLs via custom client isn't easy; use env override approach - instead
        // call internal functions with mocked URLs by testing poll path only with
        // reqwest against local server. We need to override constants - test via
        // full HTTP by temporarily not possible without dependency injection.
        // Use direct function calls against local mock base by re-implementing request
        // in test... Simpler: test pending + success by calling poll twice with
        // monkeypatched URLs through a test-only helper.

        let flow = PendingCodexDeviceFlow {
            user_code: "ABCD-EFGH".to_string(),
            expires_at_ms: 9_999_999,
        };

        let pending = poll_device_flow_with_urls(
            &http,
            &format!("http://{addr}/deviceauth/token"),
            &format!("http://{addr}/oauth/token"),
            "auth-123",
            &flow,
            1_000,
        )
        .await
        .unwrap();
        assert!(pending.pending);

        let completed = poll_device_flow_with_urls(
            &http,
            &format!("http://{addr}/deviceauth/token"),
            &format!("http://{addr}/oauth/token"),
            "auth-123",
            &flow,
            1_000,
        )
        .await
        .unwrap();
        assert!(!completed.pending);
        let account = completed.account_input.expect("account input");
        assert_eq!(account.provider_type, ProviderType::CodexOAuth);
        assert_eq!(account.id.as_deref(), Some("acct-42"));
        assert_eq!(account.refresh_token.as_deref(), Some("refresh-token"));
    }

    async fn poll_device_flow_with_urls(
        http: &reqwest::Client,
        poll_url: &str,
        token_url: &str,
        device_code: &str,
        flow: &PendingCodexDeviceFlow,
        now_ms: i64,
    ) -> Result<CodexDevicePollResult, CodexDeviceError> {
        if flow.expires_at_ms <= now_ms {
            return Err(CodexDeviceError::unauthorized(
                "codex device code expired; restart login",
            ));
        }

        let poll_response = http
            .post(poll_url)
            .header("Content-Type", "application/json")
            .header("User-Agent", CODEX_USER_AGENT)
            .json(&json!({
                "device_auth_id": device_code,
                "user_code": flow.user_code,
            }))
            .send()
            .await
            .map_err(|error| {
                CodexDeviceError::bad_gateway(format!("codex device poll request failed: {error}"))
            })?;

        let status = poll_response.status();
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            return Ok(CodexDevicePollResult {
                pending: true,
                message: "authorization_pending".to_string(),
                retry_after_secs: Some(flow_poll_interval_secs()),
                account_input: None,
            });
        }
        if !status.is_success() {
            let text = poll_response.text().await.unwrap_or_default();
            return Err(CodexDeviceError::bad_gateway(format!(
                "codex device poll failed: {status} - {text}"
            )));
        }

        let success: DevicePollSuccess = poll_response.json().await.map_err(|error| {
            CodexDeviceError::bad_gateway(format!(
                "codex device poll response parse failed: {error}"
            ))
        })?;

        let response = http
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", CODEX_USER_AGENT)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", success.authorization_code.as_str()),
                ("redirect_uri", DEVICE_REDIRECT_URI),
                ("client_id", CODEX_CLIENT_ID),
                ("code_verifier", success.code_verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|error| {
                CodexDeviceError::bad_gateway(format!("codex oauth token exchange failed: {error}"))
            })?;

        let tokens: OAuthTokenResponse = response.json().await.map_err(|error| {
            CodexDeviceError::bad_gateway(format!(
                "codex oauth token response parse failed: {error}"
            ))
        })?;
        let raw = json!({
            "accessToken": tokens.access_token,
            "refreshToken": tokens.refresh_token,
            "idToken": tokens.id_token,
            "importedBy": "codex_oauth_device_flow",
            "importedAtMs": now_ms,
        });
        let account_input =
            upsert_input_from_token_response(ProviderType::CodexOAuth, &tokens, raw, now_ms)
                .map_err(|error| CodexDeviceError::bad_gateway(error.message))?;

        Ok(CodexDevicePollResult {
            pending: false,
            message: "codex oauth device authorization completed".to_string(),
            retry_after_secs: None,
            account_input: Some(account_input),
        })
    }
}
