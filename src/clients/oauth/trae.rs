use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::domain::accounts::oauth::{merge_account_refresh_raw, OAuthErrorKind};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;
use crate::domain::trae::{
    random_trae_identity, TraeAccountProfile, TRAE_APP_ID, TRAE_AUTHORIZATION_PATH,
    TRAE_DEVICE_BRAND, TRAE_EXCHANGE_TOKEN_PATH, TRAE_IDE_VERSION, TRAE_IDE_VERSION_CODE,
    TRAE_OS_VERSION, TRAE_PLUGIN_VERSION, TRAE_QUOTA_PATH, TRAE_USER_INFO_PATH,
};

mod device_proof;

const FLOW_TTL_SECS: u64 = 10 * 60;
const MAX_RESPONSE_BODY_BYTES: usize = 256 * 1024;
const MAX_RUNTIME_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const IDE_CLIENT_ID: &str = "ono9krqynydwx5";
const CODE_EXCHANGE_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
const MAX_CALLBACK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct TraeOAuthEndpoints {
    oauth_origin: Url,
    code_exchange_origin: Url,
    protocol: crate::domain::trae::TraeProtocol,
}

impl TraeOAuthEndpoints {
    fn production_for(provider_type: ProviderType) -> Result<Self, TraeClientError> {
        let protocol = crate::domain::trae::TraeProtocol::for_provider(provider_type)
            .map_err(TraeClientError::bad_request)?;
        Ok(Self {
            oauth_origin: Url::parse(protocol.oauth_origin)
                .map_err(|error| TraeClientError::protocol(error.to_string()))?,
            code_exchange_origin: Url::parse(if protocol.intl {
                protocol.oauth_origin
            } else {
                "https://api.trae.cn"
            })
            .map_err(|error| TraeClientError::protocol(error.to_string()))?,
            protocol,
        })
    }

    #[cfg(test)]
    fn for_test(origin: &str) -> Self {
        Self {
            oauth_origin: Url::parse(origin).unwrap(),
            code_exchange_origin: Url::parse(origin).unwrap(),
            protocol: crate::domain::trae::TraeProtocol::for_provider(ProviderType::TraeSolo)
                .unwrap(),
        }
    }

    #[cfg(not(test))]
    fn for_account(account: &Account) -> Result<Self, TraeClientError> {
        Self::production_for(account.provider_type)
    }

    #[cfg(test)]
    fn for_account(account: &Account) -> Result<Self, TraeClientError> {
        if let Some(origin) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testTraeOAuthOrigin"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let mut endpoints = Self::for_test(origin);
            endpoints.protocol =
                crate::domain::trae::TraeProtocol::for_provider(account.provider_type)
                    .map_err(TraeClientError::bad_request)?;
            return Ok(endpoints);
        }
        Self::production_for(account.provider_type)
    }

    fn url(&self, path: &str) -> Result<Url, TraeClientError> {
        self.oauth_origin.join(path).map_err(|error| {
            TraeClientError::protocol(format!("invalid Trae OAuth path {path}: {error}"))
        })
    }

    fn login_version(&self) -> &'static str {
        // SOLO inference is a separate channel; browser login uses the current
        // IDE client, including for SOLO, rather than its legacy refresh client.
        if self.protocol.raw {
            self.protocol.version
        } else {
            "3.4.3"
        }
    }
}

#[derive(Debug, Clone)]
struct TraeRuntimeEndpoints {
    agent_origin: Url,
    billing_origin: Url,
}

impl TraeRuntimeEndpoints {
    fn production_for(provider_type: ProviderType) -> Result<Self, TraeClientError> {
        let protocol = crate::domain::trae::TraeProtocol::for_provider(provider_type)
            .map_err(TraeClientError::bad_request)?;
        Ok(Self {
            agent_origin: Url::parse(protocol.agent_origin)
                .map_err(|error| TraeClientError::protocol(error.to_string()))?,
            billing_origin: Url::parse(protocol.billing_origin)
                .map_err(|error| TraeClientError::protocol(error.to_string()))?,
        })
    }

    #[cfg(not(test))]
    fn for_account(account: &Account) -> Result<Self, TraeClientError> {
        Self::production_for(account.provider_type)
    }

    #[cfg(test)]
    fn for_account(account: &Account) -> Result<Self, TraeClientError> {
        let mut endpoints = Self::production_for(account.provider_type)?;
        if let Some(origin) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testTraeAgentOrigin"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            endpoints.agent_origin = Url::parse(origin).map_err(|error| {
                TraeClientError::protocol(format!("invalid test Trae Agent origin: {error}"))
            })?;
        }
        if let Some(origin) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testTraeBillingOrigin"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            endpoints.billing_origin = Url::parse(origin).map_err(|error| {
                TraeClientError::protocol(format!("invalid test Trae Billing origin: {error}"))
            })?;
        }
        Ok(endpoints)
    }

    fn agent_url(&self, path: &str) -> Result<Url, TraeClientError> {
        self.agent_origin.join(path).map_err(|error| {
            TraeClientError::protocol(format!("invalid Trae Agent path {path}: {error}"))
        })
    }

    fn billing_url(&self, path: &str) -> Result<Url, TraeClientError> {
        self.billing_origin.join(path).map_err(|error| {
            TraeClientError::protocol(format!("invalid Trae Billing path {path}: {error}"))
        })
    }

    fn origins(&self) -> (String, String) {
        (
            self.agent_origin.as_str().trim_end_matches('/').to_string(),
            self.billing_origin
                .as_str()
                .trim_end_matches('/')
                .to_string(),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraeLoginStart {
    pub flow_id: String,
    pub auth_url: String,
    pub expires_at: i64,
    pub expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[derive(Clone)]
pub struct PendingTraeLoginFlow {
    expires_at_ms: i64,
    state_digest: [u8; 32],
    callback_base_url: String,
    code_verifier: Zeroizing<String>,
    device_private_key_pem: Zeroizing<String>,
    machine_id: String,
    device_id: String,
    expected_account_id: Option<String>,
    expected_uid: Option<String>,
    expected_enterprise_id: Option<String>,
    endpoints: TraeOAuthEndpoints,
}

impl fmt::Debug for PendingTraeLoginFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingTraeLoginFlow")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("provider_type", &self.endpoints.protocol.provider_type)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
struct TraeLoginFlowEntry {
    flow: PendingTraeLoginFlow,
    state: TraeLoginFlowState,
}

#[derive(Clone)]
enum TraeLoginFlowState {
    Pending,
    Completing,
    Completed(Box<UpsertAccountInput>),
}

#[derive(Clone)]
pub enum TraeLoginStatus {
    Pending,
    Completing,
    Completed(Box<UpsertAccountInput>),
}

impl fmt::Debug for TraeLoginFlowState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Pending => "Pending",
            Self::Completing => "Completing",
            Self::Completed(_) => "Completed(<redacted>)",
        })
    }
}

impl fmt::Debug for TraeLoginStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Pending => "Pending",
            Self::Completing => "Completing",
            Self::Completed(_) => "Completed(<redacted>)",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraeLoginFlowError {
    NotFound,
    Expired,
    StateMismatch,
    CallbackMismatch,
    AlreadyConsumed,
    InvalidTransition,
}

impl fmt::Display for TraeLoginFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "Trae login flow was not found",
            Self::Expired => "Trae login flow expired; restart login",
            Self::StateMismatch => "Trae callback state does not match the login flow",
            Self::CallbackMismatch => "Trae callback address does not match the login flow",
            Self::AlreadyConsumed => "Trae login callback was already consumed",
            Self::InvalidTransition => "Trae login flow is not ready for this transition",
        })
    }
}

impl std::error::Error for TraeLoginFlowError {}

#[derive(Debug, Clone, Default)]
pub struct TraeLoginFlowStore {
    pending: BTreeMap<String, TraeLoginFlowEntry>,
}

impl TraeLoginFlowStore {
    pub fn insert(&mut self, flow_id: String, flow: PendingTraeLoginFlow, now_ms: i64) {
        self.cleanup(now_ms);
        self.pending.insert(
            flow_id,
            TraeLoginFlowEntry {
                flow,
                state: TraeLoginFlowState::Pending,
            },
        );
    }

    pub fn begin_completion(
        &mut self,
        flow_id: &str,
        callback: &TraeCallbackPayload,
        now_ms: i64,
    ) -> Result<PendingTraeLoginFlow, TraeLoginFlowError> {
        self.cleanup(now_ms);
        let entry = self
            .pending
            .get_mut(flow_id)
            .ok_or(TraeLoginFlowError::NotFound)?;
        if entry.flow.expires_at_ms <= now_ms {
            self.pending.remove(flow_id);
            return Err(TraeLoginFlowError::Expired);
        }
        let candidate = capability_digest(&callback.flow_id);
        if !constant_time_eq(&candidate, &entry.flow.state_digest) {
            return Err(TraeLoginFlowError::StateMismatch);
        }
        if callback.callback_base_url != entry.flow.callback_base_url {
            return Err(TraeLoginFlowError::CallbackMismatch);
        }
        match entry.state {
            TraeLoginFlowState::Pending => {
                entry.state = TraeLoginFlowState::Completing;
                Ok(entry.flow.clone())
            }
            TraeLoginFlowState::Completing | TraeLoginFlowState::Completed(_) => {
                Err(TraeLoginFlowError::AlreadyConsumed)
            }
        }
    }

    pub fn finish_completion(
        &mut self,
        flow_id: &str,
        account_input: UpsertAccountInput,
        now_ms: i64,
    ) -> Result<(), TraeLoginFlowError> {
        self.cleanup(now_ms);
        let entry = self
            .pending
            .get_mut(flow_id)
            .ok_or(TraeLoginFlowError::NotFound)?;
        if !matches!(entry.state, TraeLoginFlowState::Completing) {
            return Err(TraeLoginFlowError::InvalidTransition);
        }
        entry.state = TraeLoginFlowState::Completed(Box::new(account_input));
        Ok(())
    }

    pub fn fail_completion(&mut self, flow_id: &str) -> bool {
        self.pending.remove(flow_id).is_some()
    }

    pub fn status(&mut self, flow_id: &str, now_ms: i64) -> Option<TraeLoginStatus> {
        self.cleanup(now_ms);
        self.pending.get(flow_id).map(|entry| match &entry.state {
            TraeLoginFlowState::Pending => TraeLoginStatus::Pending,
            TraeLoginFlowState::Completing => TraeLoginStatus::Completing,
            TraeLoginFlowState::Completed(account) => TraeLoginStatus::Completed(account.clone()),
        })
    }

    pub fn cancel(&mut self, flow_id: &str) -> bool {
        self.pending.remove(flow_id).is_some()
    }

    fn cleanup(&mut self, now_ms: i64) {
        self.pending
            .retain(|_, entry| entry.flow.expires_at_ms > now_ms);
    }
}

#[derive(Clone)]
pub struct TraeCallbackPayload {
    pub flow_id: String,
    callback_base_url: String,
    auth_code: Zeroizing<String>,
    callback_uid: Option<String>,
    callback_name: Option<String>,
    callback_enterprise_id: Option<String>,
}

impl fmt::Debug for TraeCallbackPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TraeCallbackPayload")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct TraeClientError {
    pub status: StatusCode,
    pub upstream_status: Option<StatusCode>,
    pub terminal: bool,
    pub message: String,
    pub business_code: Option<String>,
}

impl TraeClientError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            upstream_status: None,
            terminal: true,
            message: message.into(),
            business_code: None,
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            upstream_status: None,
            terminal: true,
            message: message.into(),
            business_code: None,
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            terminal: false,
            message: message.into(),
            business_code: None,
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            terminal: true,
            message: message.into(),
            business_code: None,
        }
    }

    fn upstream(status: StatusCode, operation: &str, body: &[u8]) -> Self {
        let terminal = !(status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error());
        let api_status = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self {
            status: api_status,
            upstream_status: Some(status),
            terminal,
            message: format!(
                "Trae {operation} failed with upstream HTTP {}: {}",
                status.as_u16(),
                public_error_body(body)
            ),
            business_code: extract_business_code(body),
        }
    }

    fn business(operation: &str, code: String, message: &str) -> Self {
        let status = match code.as_str() {
            "1001" => StatusCode::UNAUTHORIZED,
            "1005" | "4008" | "4011" => StatusCode::TOO_MANY_REQUESTS,
            "4001" => StatusCode::BAD_REQUEST,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self {
            status,
            upstream_status: None,
            terminal: true,
            message: format!(
                "Trae {operation} failed with business code {code}: {}",
                sanitized_message(message)
            ),
            business_code: Some(code),
        }
    }

    pub(crate) fn is_authentication_failure(&self) -> bool {
        self.status == StatusCode::UNAUTHORIZED
            || matches!(
                self.upstream_status,
                Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            )
            || self.business_code.as_deref() == Some("1001")
    }

    pub(crate) fn is_transient(&self) -> bool {
        !self.terminal
            || matches!(
                self.upstream_status,
                Some(status) if status == StatusCode::REQUEST_TIMEOUT
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            )
    }
}

impl fmt::Display for TraeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TraeClientError {}

pub fn start_login(
    callback_base_url: &str,
    expected_account: Option<&Account>,
    now_ms: i64,
) -> Result<(TraeLoginStart, PendingTraeLoginFlow), TraeClientError> {
    start_login_for_provider(
        callback_base_url,
        expected_account.map_or(ProviderType::TraeSolo, |account| account.provider_type),
        expected_account,
        now_ms,
    )
}

pub(crate) fn login_http_client() -> Result<reqwest::Client, TraeClientError> {
    crate::infra::http::outbound_client_builder()
        .map_err(|_| TraeClientError::protocol("could not configure Trae login HTTP client"))?
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| TraeClientError::protocol("could not build Trae login HTTP client"))
}

pub fn start_login_for_provider(
    callback_base_url: &str,
    provider_type: ProviderType,
    expected_account: Option<&Account>,
    now_ms: i64,
) -> Result<(TraeLoginStart, PendingTraeLoginFlow), TraeClientError> {
    if !provider_type.is_trae() {
        return Err(TraeClientError::bad_request("expected a Trae ProviderType"));
    }
    let (machine_id, device_id, expected_account_id, expected_uid) =
        if let Some(account) = expected_account {
            if account.provider_type != provider_type {
                return Err(TraeClientError::bad_request(format!(
                    "expected account type {}, got {}",
                    provider_type.as_str(),
                    account.provider_type.as_str()
                )));
            }
            let profile = TraeAccountProfile::parse(account.profile.as_ref())
                .map_err(TraeClientError::bad_request)?;
            (
                profile.machine_id,
                profile.device_id,
                Some(account.id.clone()),
                Some(profile.uid),
            )
        } else {
            let (machine_id, device_id) = random_trae_identity();
            (machine_id, device_id, None, None)
        };
    let (start, mut flow) = start_login_with_endpoints(
        callback_base_url,
        machine_id,
        device_id,
        expected_account_id,
        expected_uid,
        TraeOAuthEndpoints::production_for(provider_type)?,
        now_ms,
    )?;
    if let Some(account) = expected_account {
        let profile = TraeAccountProfile::parse(account.profile.as_ref())
            .map_err(TraeClientError::bad_request)?;
        flow.expected_enterprise_id = Some(profile.enterprise_id);
        if let Some(key) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("devicePrivateKeyPem"))
        {
            let key = key
                .as_str()
                .ok_or_else(|| TraeClientError::bad_request("Trae device key must be PEM text"))?;
            device_proof::public_key_pem(key)?;
            flow.device_private_key_pem = Zeroizing::new(key.to_string());
        }
    }
    Ok((start, flow))
}

#[cfg(test)]
pub(crate) fn start_test_login(
    callback_base_url: &str,
    provider_type: ProviderType,
    origin: &str,
    now_ms: i64,
) -> (TraeLoginStart, PendingTraeLoginFlow) {
    let mut endpoints = TraeOAuthEndpoints::for_test(origin);
    endpoints.protocol = crate::domain::trae::TraeProtocol::for_provider(provider_type).unwrap();
    start_login_with_endpoints(
        callback_base_url,
        "test-machine".into(),
        "test-device".into(),
        None,
        None,
        endpoints,
        now_ms,
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn start_login_with_endpoints(
    callback_base_url: &str,
    machine_id: String,
    device_id: String,
    expected_account_id: Option<String>,
    expected_uid: Option<String>,
    endpoints: TraeOAuthEndpoints,
    now_ms: i64,
) -> Result<(TraeLoginStart, PendingTraeLoginFlow), TraeClientError> {
    // The website echoes login_trace_id as loginTraceID. It is our 256-bit,
    // one-use OAuth state; no parameters may be preloaded into its callback URL.
    let flow_id = random_hex(32);
    validate_callback_base_url(callback_base_url)?;
    let code_verifier = Zeroizing::new(random_secret());
    let auth_url = build_login_url(
        &machine_id,
        &device_id,
        callback_base_url,
        &flow_id,
        &code_verifier,
        &endpoints,
    )?;
    let expires_at_ms = now_ms.saturating_add((FLOW_TTL_SECS as i64).saturating_mul(1_000));
    let flow = PendingTraeLoginFlow {
        expires_at_ms,
        state_digest: capability_digest(&flow_id),
        callback_base_url: callback_base_url.to_string(),
        code_verifier,
        device_private_key_pem: device_proof::generate_key_pem()?,
        machine_id,
        device_id,
        expected_account_id: expected_account_id.clone(),
        expected_uid,
        expected_enterprise_id: None,
        endpoints,
    };
    Ok((
        TraeLoginStart {
            flow_id,
            auth_url,
            expires_at: expires_at_ms,
            expires_in: FLOW_TTL_SECS,
            account_id: expected_account_id,
        },
        flow,
    ))
}

pub fn parse_callback_url(raw_url: &str) -> Result<TraeCallbackPayload, TraeClientError> {
    if raw_url.len() > MAX_CALLBACK_BYTES {
        return Err(TraeClientError::bad_request(
            "Trae callback URL exceeds 64 KiB",
        ));
    }
    let raw_url = raw_url.trim();
    let raw_callback_base_url = raw_url.split('?').next().unwrap_or_default();
    // Browsers omit the default HTTP port in the final address. The URL sent
    // to the website still always contains the explicit port required there.
    let callback_base_url = if raw_callback_base_url == "http://127.0.0.1/authorize" {
        "http://127.0.0.1:80/authorize"
    } else {
        raw_callback_base_url
    };
    validate_callback_base_url(callback_base_url)?;
    let parsed = Url::parse(raw_url.trim())
        .map_err(|_| TraeClientError::bad_request("Trae callback URL is invalid"))?;
    if parsed.fragment().is_some() {
        return Err(TraeClientError::bad_request(
            "Trae callback URL must not contain a fragment",
        ));
    }
    let flow_id = unique_query_value(&parsed, &["loginTraceID", "login_trace_id"])?
        .ok_or_else(|| TraeClientError::bad_request("Trae callback is missing loginTraceID"))?;
    if flow_id.len() != 64 || !flow_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TraeClientError::bad_request(
            "Trae callback loginTraceID is invalid",
        ));
    }
    if unique_query_value(&parsed, &["scope"])?.as_deref() != Some("trae") {
        return Err(TraeClientError::bad_request(
            "Trae callback must use the trae IDE scope",
        ));
    }
    let code_info =
        unique_query_value(&parsed, &["authCodeInfo", "auth_code_info"])?.ok_or_else(|| {
            TraeClientError::bad_request(
                "Trae callback is missing authCodeInfo; restart login with PKCE",
            )
        })?;
    #[derive(serde::Deserialize)]
    struct AuthCodeInfo {
        #[serde(rename = "AuthCode")]
        auth_code: String,
    }
    let info: AuthCodeInfo = serde_json::from_str(&code_info)
        .map_err(|_| TraeClientError::bad_request("Trae callback authCodeInfo is invalid"))?;
    if info.auth_code.trim().is_empty()
        || info.auth_code.len() > 8192
        || info.auth_code.chars().any(char::is_control)
    {
        return Err(TraeClientError::bad_request(
            "Trae callback authorization code is invalid",
        ));
    }
    // A PKCE flow must not silently downgrade to refresh/access-token import.
    if parsed.query_pairs().any(|(key, _)| {
        matches!(
            key.as_ref(),
            "refreshToken"
                | "refresh_token"
                | "userJwt"
                | "user_jwt"
                | "accessToken"
                | "code"
                | "authCode"
                | "auth_code"
                | "flowId"
                | "flow_id"
                | "capability"
        )
    }) {
        return Err(TraeClientError::bad_request(
            "Trae PKCE callback contains conflicting credential fields",
        ));
    }
    let mut callback_uid = None;
    let mut callback_name = None;
    let mut callback_enterprise_id = None;

    if let Some(user_info) = unique_query_value(&parsed, &["userInfo", "user_info"])? {
        let value: Value = serde_json::from_str(&user_info).map_err(|_| {
            TraeClientError::bad_request("Trae callback userInfo is not valid JSON")
        })?;
        callback_uid = string_at(&value, &["/UserID", "/uid", "/userId"]);
        callback_name = string_at(&value, &["/ScreenName", "/nickname", "/name"]);
        callback_enterprise_id =
            string_at_including_empty(&value, &["/TenantID", "/EnterpriseID", "/enterpriseId"]);
    }
    Ok(TraeCallbackPayload {
        flow_id,
        callback_base_url: callback_base_url.to_string(),
        auth_code: Zeroizing::new(info.auth_code),
        callback_uid,
        callback_name,
        callback_enterprise_id,
    })
}

pub async fn complete_login(
    http: &reqwest::Client,
    flow: &PendingTraeLoginFlow,
    callback: &TraeCallbackPayload,
    now_ms: i64,
) -> Result<UpsertAccountInput, TraeClientError> {
    if !constant_time_eq(&flow.state_digest, &capability_digest(&callback.flow_id))
        || flow.callback_base_url != callback.callback_base_url
    {
        return Err(TraeClientError::bad_request(
            "Trae callback does not match the login flow",
        ));
    }
    let receipt = exchange_auth_code(http, flow, &callback.auth_code, now_ms).await?;
    let identity = fetch_user_info(
        http,
        &flow.endpoints,
        &receipt.access_token,
        flow.endpoints.login_version(),
    )
    .await?;
    if let Some(callback_uid) = callback.callback_uid.as_deref() {
        if callback_uid != identity.uid {
            return Err(TraeClientError::conflict(
                "Trae callback identity disagrees with GetUserInfo",
            ));
        }
    }
    if let Some(expected_uid) = flow.expected_uid.as_deref() {
        if expected_uid != identity.uid {
            return Err(TraeClientError::conflict(
                "Trae login returned a different uid; start a new account login",
            ));
        }
    }
    if flow
        .expected_enterprise_id
        .as_deref()
        .is_some_and(|expected| expected != identity.enterprise_id)
    {
        return Err(TraeClientError::conflict(
            "Trae login returned a different enterprise identity",
        ));
    }
    if let Some(callback_enterprise_id) = callback.callback_enterprise_id.as_deref() {
        if callback_enterprise_id != identity.enterprise_id {
            return Err(TraeClientError::conflict(
                "Trae callback enterprise identity disagrees with GetUserInfo",
            ));
        }
    }
    let account_id = crate::domain::trae::trae_account_id_for(
        flow.endpoints.protocol.provider_type,
        &identity.uid,
    )
    .map_err(|error| TraeClientError::protocol(error.to_string()))?;
    if flow
        .expected_account_id
        .as_deref()
        .is_some_and(|expected| expected != account_id)
    {
        return Err(TraeClientError::conflict(
            "Trae login does not match the target account",
        ));
    }
    let name = (!identity.name.is_empty())
        .then_some(identity.name.clone())
        .or_else(|| callback.callback_name.clone())
        .unwrap_or_default();
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: flow.endpoints.protocol.provider_type,
        email: None,
        access_token: Some(receipt.access_token),
        refresh_token: Some(receipt.refresh_token),
        id_token: None,
        token_type: Some(flow.endpoints.protocol.token_type().to_string()),
        api_key: None,
        extra_headers: None,
        scopes: Vec::new(),
        profile: Some(json!({
            "uid": identity.uid,
            "enterpriseId": identity.enterprise_id,
            "name": name,
            "email": "",
            "machineId": flow.machine_id,
            "deviceId": flow.device_id,
        })),
        raw: Some(json!({
            "source": "trae_oauth_login",
            "oauthClientId": IDE_CLIENT_ID,
            "devicePrivateKeyPem": flow.device_private_key_pem.as_str(),
            "refreshExpiresAtMs": receipt.refresh_expires_at_ms,
            "observedAtMs": now_ms,
        })),
        subscription_level: None,
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at: Some(receipt.expires_at_ms),
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

pub async fn refresh_trae_account<F>(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    receipt_hook: &mut F,
) -> Result<AccountRefreshUpdate, crate::clients::oauth::refresh::AccountRefreshFailure>
where
    F: FnMut(
        &AccountRefreshUpdate,
    ) -> Result<(), crate::clients::oauth::refresh::AccountRefreshFailure>,
{
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    if !account.provider_type.is_trae() {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected a Trae account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = TraeAccountProfile::parse(account.profile.as_ref())
        .map_err(AccountRefreshFailure::bad_request)?;
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("Trae refresh token is required"))?;
    let endpoints =
        TraeOAuthEndpoints::for_account(account).map_err(trae_refresh_client_failure)?;
    let client_id =
        refresh_client_id(account, endpoints.protocol).map_err(trae_refresh_client_failure)?;
    let receipt = exchange_token(
        http,
        &endpoints,
        client_id,
        refresh_token,
        &profile.uid,
        device_proof::device_proof(account, &profile.uid, client_id, refresh_token, now_ms)
            .map_err(trae_refresh_client_failure)?,
        now_ms,
    )
    .await
    .map_err(trae_refresh_exchange_failure)?;
    let refresh_update = AccountRefreshUpdate {
        access_token: Some(receipt.access_token.clone()),
        refresh_token: Some(receipt.refresh_token.clone()),
        token_type: Some(endpoints.protocol.token_type().to_string()),
        expires_at: Some(receipt.expires_at_ms),
        raw: Some(json!({
            "traeRefreshReceipt": {
                "refreshExpiresAtMs": receipt.refresh_expires_at_ms,
                "receivedAtMs": now_ms,
            }
        })),
        ..AccountRefreshUpdate::default()
    };
    // ExchangeToken may rotate the refresh token. Persist the complete receipt
    // before the identity closure request so a crash never retries the old token.
    receipt_hook(&refresh_update)?;
    let version = if client_id == IDE_CLIENT_ID {
        endpoints.login_version()
    } else {
        endpoints.protocol.version
    };
    let identity = fetch_user_info(http, &endpoints, &receipt.access_token, version)
        .await
        .map_err(trae_refresh_client_failure)?;
    if identity.uid != profile.uid || identity.enterprise_id != profile.enterprise_id {
        return Err(trae_refresh_identity_failure(
            "Trae refresh returned a different uid or enterprise identity",
        ));
    }
    let refreshed_profile = TraeAccountProfile {
        uid: profile.uid,
        enterprise_id: profile.enterprise_id,
        name: if identity.name.is_empty() {
            profile.name
        } else {
            identity.name
        },
        email: profile.email,
        machine_id: profile.machine_id,
        device_id: profile.device_id,
    };
    let mut update = AccountRefreshUpdate {
        profile: Some(serde_json::to_value(refreshed_profile).map_err(|error| {
            AccountRefreshFailure::parse(format!("encode Trae refresh profile: {error}"))
        })?),
        ..refresh_update
    };
    if let Some(raw) = update.raw.take() {
        update.raw = Some(merge_account_refresh_raw(account.raw.as_ref(), raw));
    }
    Ok(update)
}

pub(crate) fn runtime_origins(account: &Account) -> Result<(String, String), TraeClientError> {
    validate_runtime_account(account)?;
    TraeRuntimeEndpoints::for_account(account).map(|endpoints| endpoints.origins())
}

pub(crate) async fn fetch_model_detail(
    http: &reqwest::Client,
    account: &Account,
    request_timeout: Duration,
) -> Result<Value, TraeClientError> {
    let profile = validate_runtime_account(account)?;
    let access_token = required_access_token(account)?;
    let endpoints = TraeRuntimeEndpoints::for_account(account)?;
    let protocol = crate::domain::trae::TraeProtocol::for_provider(account.provider_type)
        .map_err(TraeClientError::bad_request)?;
    let path = protocol.catalog_path.ok_or_else(|| {
        TraeClientError::bad_request(
            "Trae Intl live model discovery is unavailable; configure an explicit upstream model",
        )
    })?;
    let url = endpoints.agent_url(path)?;
    if protocol.raw {
        let mut request = http.post(url).json(&json!({"type":"chat"}));
        for (key, value) in protocol.headers(&profile, access_token, false) {
            request = request.header(key, value);
        }
        let (status, body) = execute_bounded_with(
            request,
            "raw model list",
            request_timeout,
            MAX_RUNTIME_RESPONSE_BODY_BYTES,
        )
        .await?;
        if !status.is_success() {
            return Err(TraeClientError::upstream(status, "raw model list", &body));
        }
        let value = serde_json::from_slice(&body)
            .map_err(|error| TraeClientError::protocol(error.to_string()))?;
        ensure_success_envelope(&value, "raw model list")?;
        return Ok(value);
    }
    let request = solo_request(
        http.request(Method::POST, url).json(&json!({
            "function": crate::domain::trae::TRAE_FUNCTION,
            "config_names": Value::Null,
            "need_prompt": false,
            "current_config_info": Value::Null,
            "poly_prompt": true,
            "mode_type": Value::Null,
            "agent_type": Value::Null,
        })),
        &profile,
        access_token,
        false,
    );
    let (status, body) = execute_bounded_with(
        request,
        "model detail",
        request_timeout,
        MAX_RUNTIME_RESPONSE_BODY_BYTES,
    )
    .await?;
    if !status.is_success() {
        return Err(TraeClientError::upstream(status, "model detail", &body));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        TraeClientError::protocol(format!(
            "Trae model detail response is not valid JSON: {error}"
        ))
    })?;
    ensure_success_envelope(&value, "model detail")?;
    Ok(value)
}

pub(crate) async fn fetch_entitlement_usage(
    http: &reqwest::Client,
    account: &Account,
    request_timeout: Duration,
) -> Result<Value, TraeClientError> {
    let profile = validate_runtime_account(account)?;
    let access_token = required_access_token(account)?;
    let endpoints = TraeRuntimeEndpoints::for_account(account)?;
    let protocol = crate::domain::trae::TraeProtocol::for_provider(account.provider_type)
        .map_err(TraeClientError::bad_request)?;
    let url = endpoints.billing_url(if protocol.intl {
        "/trae/api/v1/pay/ide_user_ent_usage"
    } else {
        TRAE_QUOTA_PATH
    })?;
    let request = http
        .request(Method::POST, url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .header(
            USER_AGENT,
            if protocol.raw {
                if protocol.intl {
                    format!("Trae/{}", protocol.version)
                } else {
                    format!("Trae CN/{}", protocol.version)
                }
            } else {
                format!("Trae/{TRAE_IDE_VERSION}")
            },
        )
        .header("Authorization", format!("Cloud-IDE-JWT {access_token}"))
        .header("X-User-Region", if protocol.intl { "US" } else { "CN" })
        .header("X-Device-Id", &profile.device_id)
        .json(&json!({}));
    let (status, body) = execute_bounded_with(
        request,
        "entitlement usage",
        request_timeout,
        MAX_RUNTIME_RESPONSE_BODY_BYTES,
    )
    .await?;
    if !status.is_success() {
        return Err(TraeClientError::upstream(
            status,
            "entitlement usage",
            &body,
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        TraeClientError::protocol(format!(
            "Trae entitlement usage response is not valid JSON: {error}"
        ))
    })?;
    ensure_success_envelope(&value, "entitlement usage")?;
    Ok(value)
}

fn validate_runtime_account(account: &Account) -> Result<TraeAccountProfile, TraeClientError> {
    if !account.provider_type.is_trae() {
        return Err(TraeClientError::bad_request(format!(
            "expected a Trae account, got {}",
            account.provider_type.as_str()
        )));
    }
    if account.needs_relogin {
        return Err(TraeClientError::business(
            "runtime preparation",
            "1001".to_string(),
            "account requires login",
        ));
    }
    TraeAccountProfile::parse(account.profile.as_ref()).map_err(TraeClientError::bad_request)
}

fn required_access_token(account: &Account) -> Result<&str, TraeClientError> {
    account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TraeClientError::bad_request("Trae access token is required"))
}

fn solo_request(
    request: reqwest::RequestBuilder,
    profile: &TraeAccountProfile,
    access_token: &str,
    stream: bool,
) -> reqwest::RequestBuilder {
    request
        .header(
            ACCEPT,
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, format!("Trae/{TRAE_IDE_VERSION}"))
        .header("Authorization", format!("Cloud-IDE-JWT {access_token}"))
        .header("X-Cloudide-Token", access_token)
        .header("X-Ide-Token", access_token)
        .header("X-Uid", &profile.uid)
        .header("X-App-Id", TRAE_APP_ID)
        .header("X-App-Version", "default")
        .header("X-Ide-Version", TRAE_IDE_VERSION)
        .header("X-Ide-Version-Code", TRAE_IDE_VERSION_CODE)
        .header("X-App-Version-Code", TRAE_IDE_VERSION_CODE)
        .header("X-Ide-Version-Type", "stable")
        .header("X-Device-Type", "windows")
        .header("X-OS-Version", TRAE_OS_VERSION)
        .header("X-Device-Brand", TRAE_DEVICE_BRAND)
        .header("Request-Traffic-Type", "prod")
        .header("X-Machine-Id", &profile.machine_id)
        .header("X-Device-Id", &profile.device_id)
}

#[derive(Debug, Clone)]
struct TraeTokenReceipt {
    access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
    refresh_expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct TraeUserIdentity {
    uid: String,
    name: String,
    enterprise_id: String,
}

fn refresh_client_id(
    account: &Account,
    protocol: crate::domain::trae::TraeProtocol,
) -> Result<&'static str, TraeClientError> {
    match account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("oauthClientId"))
    {
        None | Some(Value::Null) => Ok(protocol.client_id),
        Some(Value::String(value)) if value == IDE_CLIENT_ID => Ok(IDE_CLIENT_ID),
        Some(Value::String(value)) if value == protocol.client_id => Ok(protocol.client_id),
        _ => Err(TraeClientError::bad_request(
            "Trae account has an unsupported OAuth client binding",
        )),
    }
}

async fn exchange_auth_code(
    http: &reqwest::Client,
    flow: &PendingTraeLoginFlow,
    auth_code: &str,
    now_ms: i64,
) -> Result<TraeTokenReceipt, TraeClientError> {
    let endpoints = &flow.endpoints;
    let url = endpoints
        .code_exchange_origin
        .join(CODE_EXCHANGE_PATH)
        .map_err(|_| TraeClientError::protocol("invalid Trae code exchange URL"))?;
    let public_key = device_proof::public_key_pem(&flow.device_private_key_pem)?;
    let request = http
        .post(url)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, format!("Trae/{}", endpoints.login_version()))
        .json(&json!({
            "ClientID": IDE_CLIENT_ID,
            "AuthCode": auth_code,
            "CodeVerifier": flow.code_verifier.as_str(),
            "IDEVersion": endpoints.login_version(),
            "DeviceInfo": {
                "DeviceID": flow.device_id,
                "MachineID": flow.machine_id,
                "PlatformCode": "IDE_PC",
                "DeviceType": "PC",
                "DeviceName": "cc-switch-server",
                "DeviceModel": "",
                "ClientVersion": endpoints.login_version(),
                "DeviceBrand": "PC",
                "DeviceCPU": "",
                "OSInfo": std::env::consts::OS,
                "OSVersion": "1.0",
                "DevicePublicKey": public_key,
            }
        }));
    // Single-use codes are exchanged exactly once at a reviewed origin. Neither
    // callback host hints nor an error response can redirect credentials.
    let (status, body) = execute_bounded(request, "authorization code exchange").await?;
    if !status.is_success() {
        let mut error = TraeClientError::upstream(status, "authorization code exchange", &body);
        // Upstream diagnostics may reflect the request's code or verifier.
        error.message = format!(
            "Trae authorization code exchange failed with upstream HTTP {}",
            status.as_u16()
        );
        return Err(error);
    }
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| TraeClientError::protocol("Trae code exchange response is not valid JSON"))?;
    let receipt = parse_token_receipt(&value, None, now_ms).map_err(|mut error| {
        if error.business_code.is_some() {
            error.message = "Trae authorization code exchange was rejected".to_string();
        }
        error
    })?;
    if receipt.expires_at_ms <= now_ms {
        return Err(TraeClientError::protocol(
            "Trae code exchange returned an expired token",
        ));
    }
    Ok(receipt)
}

async fn exchange_token(
    http: &reqwest::Client,
    endpoints: &TraeOAuthEndpoints,
    client_id: &str,
    refresh_token: &str,
    uid: &str,
    proof: Option<Value>,
    now_ms: i64,
) -> Result<TraeTokenReceipt, TraeClientError> {
    if refresh_token.trim().is_empty() {
        return Err(TraeClientError::bad_request(
            "Trae refresh token is required",
        ));
    }
    let mut payload = json!({"ClientID": client_id, "RefreshToken": refresh_token, "ClientSecret":"-", "UserID":if client_id == IDE_CLIENT_ID { uid } else { "" }});
    if let Some(Value::Object(proof)) = proof {
        payload.as_object_mut().unwrap().extend(proof);
    }
    let url = endpoints.url(TRAE_EXCHANGE_TOKEN_PATH)?;
    let request = oauth_request(
        http.request(Method::POST, url)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload),
    );
    let version = if client_id == IDE_CLIENT_ID {
        endpoints.login_version()
    } else {
        endpoints.protocol.version
    };
    let request = request.header(USER_AGENT, format!("Trae/{version}"));
    let (status, body) = execute_bounded(request, "token exchange").await?;
    if !status.is_success() {
        return Err(TraeClientError::upstream(status, "token exchange", &body));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        TraeClientError::protocol(format!("Trae token response is not valid JSON: {error}"))
    })?;
    parse_token_receipt(&value, Some(refresh_token), now_ms)
}

fn parse_token_receipt(
    value: &Value,
    previous_refresh: Option<&str>,
    now_ms: i64,
) -> Result<TraeTokenReceipt, TraeClientError> {
    ensure_success_envelope(value, "token exchange")?;
    let result = value
        .get("Result")
        .or_else(|| value.get("result"))
        .or_else(|| value.get("data"))
        .unwrap_or(value);
    let access_token = string_at(result, &["/Token", "/token", "/accessToken"])
        .ok_or_else(|| TraeClientError::protocol("Trae token response is missing Token"))?;
    let rotated_refresh = string_at(
        result,
        &["/RefreshToken", "/refreshToken", "/refresh_token"],
    )
    .or_else(|| previous_refresh.map(str::to_string))
    .ok_or_else(|| {
        TraeClientError::protocol("Trae code exchange response is missing RefreshToken")
    })?;
    let expires_at_ms = timestamp_ms_at(result, &["/TokenExpireAt", "/tokenExpireAt"])
        .or_else(|| {
            i64_at(
                result,
                &["/TokenExpireDuration", "/tokenExpireDuration", "/expiresIn"],
            )
            .filter(|seconds| *seconds > 0)
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)))
        })
        .ok_or_else(|| TraeClientError::protocol("Trae token response is missing token expiry"))?;
    let refresh_expires_at_ms = timestamp_ms_at(
        result,
        &["/RefreshExpireAt", "/refreshExpireAt", "/refreshExpiresAt"],
    );
    Ok(TraeTokenReceipt {
        access_token,
        refresh_token: rotated_refresh,
        expires_at_ms,
        refresh_expires_at_ms,
    })
}

async fn fetch_user_info(
    http: &reqwest::Client,
    endpoints: &TraeOAuthEndpoints,
    access_token: &str,
    version: &str,
) -> Result<TraeUserIdentity, TraeClientError> {
    let url = endpoints.url(TRAE_USER_INFO_PATH)?;
    let request = oauth_request(
        http.request(Method::POST, url)
            .header(CONTENT_TYPE, "application/json")
            .header("X-Cloudide-Token", access_token)
            .json(&json!({"ReqSource": "IDE", "IDEVersion": version})),
    );
    let request = request.header(USER_AGENT, format!("Trae/{version}"));
    let (status, body) = execute_bounded(request, "user info").await?;
    if !status.is_success() {
        return Err(TraeClientError::upstream(status, "user info", &body));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        TraeClientError::protocol(format!(
            "Trae user info response is not valid JSON: {error}"
        ))
    })?;
    ensure_success_envelope(&value, "user info")?;
    let result = value
        .get("Result")
        .or_else(|| value.get("result"))
        .or_else(|| value.get("data"))
        .unwrap_or(&value);
    let uid = string_at(result, &["/UserID", "/userId", "/uid"])
        .ok_or_else(|| TraeClientError::protocol("Trae user info is missing UserID"))?;
    Ok(TraeUserIdentity {
        uid,
        name: string_at(result, &["/ScreenName", "/screenName", "/nickname"]).unwrap_or_default(),
        enterprise_id: string_at_including_empty(
            result,
            &["/EnterpriseID", "/enterpriseId", "/TenantID"],
        )
        .unwrap_or_default(),
    })
}

fn validate_callback_base_url(callback_base_url: &str) -> Result<(), TraeClientError> {
    let port = callback_base_url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|suffix| suffix.strip_suffix("/authorize"));
    if !port.is_some_and(|port| {
        !port.is_empty()
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port != 0)
    }) {
        return Err(TraeClientError::bad_request(
            "Trae callback must be http://127.0.0.1:<port>/authorize without query parameters",
        ));
    }
    Ok(())
}

fn build_login_url(
    machine_id: &str,
    device_id: &str,
    callback_url: &str,
    trace: &str,
    code_verifier: &str,
    endpoints: &TraeOAuthEndpoints,
) -> Result<String, TraeClientError> {
    let protocol = endpoints.protocol;
    let mut url = Url::parse(protocol.console_origin)
        .and_then(|origin| origin.join(TRAE_AUTHORIZATION_PATH))
        .map_err(|error| {
            TraeClientError::protocol(format!("invalid reviewed Trae browser URL: {error}"))
        })?;
    url.query_pairs_mut()
        .append_pair("login_version", "1")
        .append_pair("auth_from", "trae")
        .append_pair("login_channel", "native_ide")
        .append_pair("plugin_version", TRAE_PLUGIN_VERSION)
        .append_pair("auth_type", "local")
        .append_pair("client_id", IDE_CLIENT_ID)
        .append_pair("redirect", "0")
        .append_pair("login_trace_id", trace)
        .append_pair(
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes())),
        )
        .append_pair("code_challenge_method", "S256")
        .append_pair("auth_callback_url", callback_url)
        .append_pair("machine_id", machine_id)
        .append_pair("device_id", device_id)
        .append_pair("x_device_id", device_id)
        .append_pair("x_machine_id", machine_id)
        .append_pair("x_device_brand", "PC")
        .append_pair("x_device_type", "PC")
        .append_pair("x_os_version", "1.0")
        .append_pair("x_app_version", endpoints.login_version())
        .append_pair("x_app_type", "stable");
    Ok(url.into())
}

fn oauth_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, format!("Trae/{TRAE_IDE_VERSION}"))
}

async fn execute_bounded(
    request: reqwest::RequestBuilder,
    operation: &str,
) -> Result<(StatusCode, Vec<u8>), TraeClientError> {
    execute_bounded_with(request, operation, REQUEST_TIMEOUT, MAX_RESPONSE_BODY_BYTES).await
}

async fn execute_bounded_with(
    request: reqwest::RequestBuilder,
    operation: &str,
    request_timeout: Duration,
    max_response_body_bytes: usize,
) -> Result<(StatusCode, Vec<u8>), TraeClientError> {
    let mut response = request
        .timeout(
            request_timeout
                .max(Duration::from_secs(1))
                .min(Duration::from_secs(120)),
        )
        .send()
        .await
        .map_err(|error| {
            TraeClientError::bad_gateway(format!("Trae {operation} request failed: {error}"))
        })?;
    let status = response.status();
    let body =
        crate::infra::http::read_response_body_limited(&mut response, max_response_body_bytes)
            .await
            .map_err(|error| match error {
                crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                    TraeClientError::protocol(format!(
                        "Trae {operation} response exceeds {max_response_body_bytes} bytes"
                    ))
                }
                crate::infra::http::BoundedResponseBodyError::Request(error) => {
                    TraeClientError::bad_gateway(format!(
                        "Trae {operation} response read failed: {error}"
                    ))
                }
            })?;
    Ok((status, body.to_vec()))
}

fn ensure_success_envelope(value: &Value, operation: &str) -> Result<(), TraeClientError> {
    let code = value
        .get("Code")
        .or_else(|| value.get("code"))
        .and_then(value_code);
    if let Some(code) = code.filter(|code| code != "0") {
        let message =
            string_at(value, &["/Message", "/message", "/Msg", "/msg"]).unwrap_or_default();
        return Err(TraeClientError::business(operation, code, &message));
    }
    Ok(())
}

fn unique_query_value(url: &Url, names: &[&str]) -> Result<Option<String>, TraeClientError> {
    let values = url
        .query_pairs()
        .filter(|(key, _)| names.iter().any(|name| key == *name))
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(TraeClientError::bad_request(format!(
            "Trae callback contains duplicate {} fields",
            names[0]
        )));
    }
    Ok(values.into_iter().next().filter(|value| !value.is_empty()))
}

fn string_at(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn string_at_including_empty(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_string)
    })
}

fn i64_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
    })
}

fn timestamp_ms_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    i64_at(value, pointers).and_then(|timestamp| {
        if timestamp <= 0 {
            None
        } else if timestamp < 10_000_000_000 {
            Some(timestamp.saturating_mul(1_000))
        } else {
            Some(timestamp)
        }
    })
}

fn value_code(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extract_business_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .get("Code")
        .or_else(|| value.get("code"))
        .and_then(value_code)
}

fn random_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    hex::encode(value)
}

fn capability_digest(capability: &str) -> [u8; 32] {
    Sha256::digest(capability.as_bytes()).into()
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn public_error_body(body: &[u8]) -> String {
    let value = String::from_utf8_lossy(body);
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "empty response".to_string();
    }
    let lowercase = trimmed.to_ascii_lowercase();
    if lowercase.contains("<html") || lowercase.contains("<!doctype") {
        return "upstream HTML error page".to_string();
    }
    sanitized_message(trimmed)
}

fn sanitized_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return "unspecified upstream error".to_string();
    }
    message.chars().take(512).collect()
}

fn trae_refresh_exchange_failure(
    error: TraeClientError,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    if error.is_authentication_failure() {
        return crate::clients::oauth::refresh::AccountRefreshFailure {
            status_code: StatusCode::UNAUTHORIZED.as_u16(),
            upstream_status: error.upstream_status.map(|status| status.as_u16()),
            message: error.message,
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        };
    }
    if error.upstream_status.is_none() && error.status == StatusCode::BAD_GATEWAY && error.terminal
    {
        return crate::clients::oauth::refresh::AccountRefreshFailure::outcome_unknown(
            error.message,
        );
    }
    trae_refresh_client_failure(error)
}

fn trae_refresh_client_failure(
    error: TraeClientError,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    let transient = error.is_transient();
    crate::clients::oauth::refresh::AccountRefreshFailure {
        status_code: error.status.as_u16(),
        upstream_status: error.upstream_status.map(|status| status.as_u16()),
        message: error.message,
        kind: if error.status == StatusCode::TOO_MANY_REQUESTS {
            OAuthErrorKind::RateLimited
        } else if transient {
            OAuthErrorKind::Network
        } else {
            OAuthErrorKind::ProviderRejected
        },
        retryable: transient,
        retry_after_ms: None,
        immediate_relogin: false,
        outcome_unknown: false,
        endpoint_fallback_safe: false,
    }
}

fn trae_refresh_identity_failure(
    message: &str,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    crate::clients::oauth::refresh::AccountRefreshFailure {
        status_code: StatusCode::CONFLICT.as_u16(),
        upstream_status: None,
        message: message.to_string(),
        kind: OAuthErrorKind::InvalidGrant,
        retryable: false,
        retry_after_ms: None,
        immediate_relogin: true,
        outcome_unknown: true,
        endpoint_fallback_safe: false,
    }
}

#[cfg(test)]
mod tests {
    include!("trae/enhancement_tests.rs");
    include!("trae/login_tests.rs");
    use super::*;
    use crate::domain::accounts::store::AccountStore;
    use crate::domain::trae::*;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{HeaderMap, Response};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct Observation {
        path: String,
        headers: HeaderMap,
        body: Value,
    }

    async fn serve_oauth_fixture(
        uid: &str,
        enterprise_id: &str,
    ) -> (
        String,
        Arc<Mutex<Vec<Observation>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let route_observations = Arc::clone(&observations);
        let uid = uid.to_string();
        let enterprise_id = enterprise_id.to_string();
        let app = axum::Router::new().fallback(axum::routing::any(move |request: Request| {
            let observations = Arc::clone(&route_observations);
            let uid = uid.clone();
            let enterprise_id = enterprise_id.clone();
            async move {
                let path = request.uri().path().to_string();
                let headers = request.headers().clone();
                let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
                    .await
                    .unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                observations.lock().unwrap().push(Observation {
                    path: path.clone(),
                    headers,
                    body,
                });
                let value = match path.as_str() {
                    TRAE_EXCHANGE_TOKEN_PATH | CODE_EXCHANGE_PATH => json!({
                        "Code": 0,
                        "Result": {
                            "Token": "access-new",
                            "RefreshToken": "refresh-rotated",
                            "TokenExpireAt": 4_102_444_800_000_i64,
                            "RefreshExpireAt": 4_133_980_800_000_i64
                        }
                    }),
                    TRAE_USER_INFO_PATH => json!({
                        "Code": 0,
                        "Result": {
                            "UserID": uid,
                            "ScreenName": "Fixture User",
                            "EnterpriseID": enterprise_id
                        }
                    }),
                    _ => json!({"Code": 404}),
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(value.to_string()))
                    .unwrap()
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), observations, server)
    }

    fn callback_from_start(start: &TraeLoginStart, auth_code: &str) -> String {
        let auth = Url::parse(&start.auth_url).unwrap();
        let callback = auth
            .query_pairs()
            .find(|(key, _)| key == "auth_callback_url")
            .unwrap()
            .1
            .to_string();
        let mut callback = Url::parse(&callback).unwrap();
        callback
            .query_pairs_mut()
            .append_pair("loginTraceID", &start.flow_id)
            .append_pair("scope", "trae")
            .append_pair("authCodeInfo", &json!({"AuthCode":auth_code}).to_string())
            .append_pair(
                "userInfo",
                r#"{"UserID":"uid-1","TenantID":"enterprise-1"}"#,
            );
        callback.into()
    }

    #[test]
    fn flow_state_is_exact_once_only_and_expires() {
        let endpoints = TraeOAuthEndpoints::for_test("http://127.0.0.1:9");
        let (start, flow) = start_login_with_endpoints(
            "http://127.0.0.1:9911/authorize",
            "machine".to_string(),
            "device".to_string(),
            None,
            None,
            endpoints,
            1_000,
        )
        .unwrap();
        let callback = parse_callback_url(&callback_from_start(&start, "refresh")).unwrap();
        let mut store = TraeLoginFlowStore::default();
        store.insert(start.flow_id.clone(), flow, 1_000);
        let mut wrong_callback = callback.clone();
        wrong_callback.flow_id = random_hex(32);
        assert_eq!(
            store
                .begin_completion(&start.flow_id, &wrong_callback, 1_001)
                .unwrap_err(),
            TraeLoginFlowError::StateMismatch
        );
        store
            .begin_completion(&start.flow_id, &callback, 1_001)
            .unwrap();
        assert_eq!(
            store
                .begin_completion(&start.flow_id, &callback, 1_001)
                .unwrap_err(),
            TraeLoginFlowError::AlreadyConsumed
        );

        let (expired, flow) = start_login_with_endpoints(
            "http://127.0.0.1:9911/authorize",
            "machine".to_string(),
            "device".to_string(),
            None,
            None,
            TraeOAuthEndpoints::for_test("http://127.0.0.1:9"),
            0,
        )
        .unwrap();
        let callback = parse_callback_url(&callback_from_start(&expired, "refresh")).unwrap();
        store.insert(expired.flow_id.clone(), flow, 0);
        assert_eq!(
            store
                .begin_completion(&expired.flow_id, &callback, 600_000)
                .unwrap_err(),
            TraeLoginFlowError::NotFound
        );
    }

    #[test]
    fn callback_parser_rejects_legacy_token_import() {
        assert!(parse_callback_url(
            "http://127.0.0.1:9911/authorize?flowId=f&capability=c&refreshToken=r"
        )
        .is_err());
    }

    #[tokio::test]
    async fn login_uses_fixed_exchange_shape_and_closes_trusted_identity() {
        let (origin, observations, server) = serve_oauth_fixture("uid-1", "enterprise-1").await;
        let (start, flow) = start_login_with_endpoints(
            "http://127.0.0.1:9911/authorize",
            "machine-1".to_string(),
            "device-1".to_string(),
            None,
            None,
            TraeOAuthEndpoints::for_test(&origin),
            1_000,
        )
        .unwrap();
        let callback = parse_callback_url(&callback_from_start(&start, "refresh-old")).unwrap();
        let input = complete_login(&reqwest::Client::new(), &flow, &callback, 1_000)
            .await
            .unwrap();
        assert_eq!(input.provider_type, ProviderType::TraeSolo);
        assert_eq!(input.access_token.as_deref(), Some("access-new"));
        assert_eq!(input.refresh_token.as_deref(), Some("refresh-rotated"));
        assert_eq!(input.profile.as_ref().unwrap()["uid"], "uid-1");
        assert_eq!(
            input.profile.as_ref().unwrap()["enterpriseId"],
            "enterprise-1"
        );
        assert_eq!(input.profile.as_ref().unwrap()["machineId"], "machine-1");

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].path, CODE_EXCHANGE_PATH);
        assert_eq!(observations[0].body["ClientID"], IDE_CLIENT_ID);
        assert_eq!(observations[0].body["AuthCode"], "refresh-old");
        assert!(observations[0].body.get("RefreshToken").is_none());
        assert_eq!(observations[1].path, TRAE_USER_INFO_PATH);
        assert_eq!(observations[1].headers["x-cloudide-token"], "access-new");
        assert_eq!(observations[1].body["IDEVersion"], "3.4.3");
        server.abort();
    }

    fn refresh_account(origin: &str) -> Account {
        AccountStore::default().upsert(UpsertAccountInput {
            id: Some(trae_account_id("uid-1").unwrap()),
            provider_type: ProviderType::TraeSolo,
            email: None,
            access_token: Some("access-old".to_string()),
            refresh_token: Some("refresh-old".to_string()),
            id_token: None,
            token_type: Some("Cloud-IDE-JWT".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: Some(json!({
                "uid": "uid-1",
                "enterpriseId": "enterprise-1",
                "name": "Old Name",
                "machineId": "machine-1",
                "deviceId": "device-1"
            })),
            raw: Some(json!({
                "source": "fixture",
                "testTraeOAuthOrigin": origin
            })),
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: Some(1),
            rate_limited_until: None,
            last_refresh_error: None,
        })
    }

    #[tokio::test]
    async fn refresh_journals_rotation_before_identity_closure() {
        let (origin, _, server) = serve_oauth_fixture("uid-other", "enterprise-1").await;
        let account = refresh_account(&origin);
        let journaled = Arc::new(Mutex::new(None::<AccountRefreshUpdate>));
        let hook_value = Arc::clone(&journaled);
        let mut hook = move |receipt: &AccountRefreshUpdate| {
            *hook_value.lock().unwrap() = Some(receipt.clone());
            Ok(())
        };
        let error = refresh_trae_account(&reqwest::Client::new(), &account, 1_000, &mut hook)
            .await
            .unwrap_err();
        assert_eq!(error.status_code, StatusCode::CONFLICT.as_u16());
        assert!(error.immediate_relogin);
        assert_eq!(
            journaled
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("refresh-rotated")
        );
        server.abort();
    }

    #[tokio::test]
    async fn runtime_transports_use_fixed_planes_and_complete_cloud_ide_identity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Vec::<Observation>::new()));
        let route_observations = Arc::clone(&observations);
        let app = axum::Router::new().fallback(axum::routing::any(move |request: Request| {
            let observations = Arc::clone(&route_observations);
            async move {
                let path = request.uri().path().to_string();
                let headers = request.headers().clone();
                let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
                    .await
                    .unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                observations.lock().unwrap().push(Observation {
                    path: path.clone(),
                    headers,
                    body,
                });
                let response = if path == TRAE_MODEL_DETAIL_PATH {
                    json!({"Code": 0, "config_info_list": []})
                } else if path == TRAE_QUOTA_PATH {
                    json!({"Code": 0, "user_entitlement_pack_list": [{
                        "entitlement_base_info": {"quota": {"credits_limit": 100}},
                        "usage": {"credits_amount": 25}
                    }]})
                } else {
                    json!({"Code": 404})
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(response.to_string()))
                    .unwrap()
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let origin = format!("http://{address}");
        let mut account = refresh_account("http://127.0.0.1:9");
        account.expires_at = Some(i64::MAX / 2);
        account.raw = Some(json!({
            "source": "fixture",
            "testTraeAgentOrigin": origin.clone(),
            "testTraeBillingOrigin": origin,
        }));

        let catalog = fetch_model_detail(&reqwest::Client::new(), &account, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(catalog["config_info_list"].is_array());
        let quota =
            fetch_entitlement_usage(&reqwest::Client::new(), &account, Duration::from_secs(2))
                .await
                .unwrap();
        assert!(quota["user_entitlement_pack_list"].is_array());

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        let catalog = &observations[0];
        assert_eq!(catalog.path, TRAE_MODEL_DETAIL_PATH);
        assert_eq!(catalog.headers["authorization"], "Cloud-IDE-JWT access-old");
        assert_eq!(catalog.headers["x-cloudide-token"], "access-old");
        assert_eq!(catalog.headers["x-ide-token"], "access-old");
        assert_eq!(catalog.headers["x-uid"], "uid-1");
        assert_eq!(catalog.headers["x-machine-id"], "machine-1");
        assert_eq!(catalog.headers["x-device-id"], "device-1");
        assert_eq!(catalog.headers["x-app-id"], TRAE_APP_ID);
        assert_eq!(catalog.headers["x-ide-version"], TRAE_IDE_VERSION);
        assert_eq!(catalog.body["function"], crate::domain::trae::TRAE_FUNCTION);
        assert_eq!(catalog.body["config_names"], Value::Null);
        let quota = &observations[1];
        assert_eq!(quota.path, TRAE_QUOTA_PATH);
        assert_eq!(quota.headers["authorization"], "Cloud-IDE-JWT access-old");
        assert_eq!(quota.headers["x-user-region"], "CN");
        assert_eq!(quota.headers["x-device-id"], "device-1");
        assert!(quota.headers.get("x-cloudide-token").is_none());
        drop(observations);
        server.abort();
    }

    #[test]
    fn login_url_keeps_vendor_and_callback_origins_separate() {
        let (start, _) = start_login_with_endpoints(
            "http://127.0.0.1:9911/authorize",
            "machine".to_string(),
            "device".to_string(),
            None,
            None,
            TraeOAuthEndpoints::for_test("http://127.0.0.1:9"),
            0,
        )
        .unwrap();
        let auth = Url::parse(&start.auth_url).unwrap();
        assert_eq!(auth.origin().ascii_serialization(), TRAE_CONSOLE_ORIGIN);
        let callback = auth
            .query_pairs()
            .find(|(key, _)| key == "auth_callback_url")
            .unwrap()
            .1
            .to_string();
        let callback = Url::parse(&callback).unwrap();
        assert_eq!(callback.host_str(), Some("127.0.0.1"));
        assert_eq!(callback.path(), "/authorize");
        assert!(callback.query().is_none());
    }
}
