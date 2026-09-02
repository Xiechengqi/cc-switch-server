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

use crate::domain::accounts::oauth::{merge_account_refresh_raw, OAuthErrorKind};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;
use crate::domain::trae::{
    random_trae_identity, trae_account_id, TraeAccountProfile, TRAE_AGENT_ORIGIN, TRAE_APP_ID,
    TRAE_AUTHORIZATION_PATH, TRAE_BILLING_ORIGIN, TRAE_CLIENT_ID, TRAE_CONSOLE_ORIGIN,
    TRAE_DEVICE_BRAND, TRAE_EXCHANGE_TOKEN_PATH, TRAE_IDE_VERSION, TRAE_IDE_VERSION_CODE,
    TRAE_MODEL_DETAIL_PATH, TRAE_OAUTH_ORIGIN, TRAE_OS_VERSION, TRAE_PLUGIN_VERSION,
    TRAE_QUOTA_PATH, TRAE_USER_INFO_PATH,
};

const FLOW_TTL_SECS: u64 = 10 * 60;
const MAX_RESPONSE_BODY_BYTES: usize = 256 * 1024;
const MAX_RUNTIME_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
struct TraeOAuthEndpoints {
    oauth_origin: Url,
}

impl TraeOAuthEndpoints {
    fn production() -> Result<Self, TraeClientError> {
        let oauth_origin = Url::parse(TRAE_OAUTH_ORIGIN).map_err(|error| {
            TraeClientError::protocol(format!("invalid reviewed Trae OAuth origin: {error}"))
        })?;
        Ok(Self { oauth_origin })
    }

    #[cfg(test)]
    fn for_test(origin: &str) -> Self {
        Self {
            oauth_origin: Url::parse(origin).unwrap(),
        }
    }

    #[cfg(not(test))]
    fn for_account(_account: &Account) -> Result<Self, TraeClientError> {
        Self::production()
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
            return Ok(Self::for_test(origin));
        }
        Self::production()
    }

    fn url(&self, path: &str) -> Result<Url, TraeClientError> {
        self.oauth_origin.join(path).map_err(|error| {
            TraeClientError::protocol(format!("invalid Trae OAuth path {path}: {error}"))
        })
    }
}

#[derive(Debug, Clone)]
struct TraeRuntimeEndpoints {
    agent_origin: Url,
    billing_origin: Url,
}

impl TraeRuntimeEndpoints {
    fn production() -> Result<Self, TraeClientError> {
        let agent_origin = Url::parse(TRAE_AGENT_ORIGIN).map_err(|error| {
            TraeClientError::protocol(format!("invalid reviewed Trae Agent origin: {error}"))
        })?;
        let billing_origin = Url::parse(TRAE_BILLING_ORIGIN).map_err(|error| {
            TraeClientError::protocol(format!("invalid reviewed Trae Billing origin: {error}"))
        })?;
        Ok(Self {
            agent_origin,
            billing_origin,
        })
    }

    #[cfg(not(test))]
    fn for_account(_account: &Account) -> Result<Self, TraeClientError> {
        Self::production()
    }

    #[cfg(test)]
    fn for_account(account: &Account) -> Result<Self, TraeClientError> {
        let mut endpoints = Self::production()?;
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

#[derive(Debug, Clone)]
pub struct PendingTraeLoginFlow {
    expires_at_ms: i64,
    capability_digest: [u8; 32],
    machine_id: String,
    device_id: String,
    expected_account_id: Option<String>,
    expected_uid: Option<String>,
    endpoints: TraeOAuthEndpoints,
}

#[derive(Debug, Clone)]
struct TraeLoginFlowEntry {
    flow: PendingTraeLoginFlow,
    state: TraeLoginFlowState,
}

#[derive(Debug, Clone)]
enum TraeLoginFlowState {
    Pending,
    Completing,
    Completed(Box<UpsertAccountInput>),
}

#[derive(Debug, Clone)]
pub enum TraeLoginStatus {
    Pending,
    Completing,
    Completed(Box<UpsertAccountInput>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraeLoginFlowError {
    NotFound,
    Expired,
    CapabilityMismatch,
    AlreadyConsumed,
    InvalidTransition,
}

impl fmt::Display for TraeLoginFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "Trae login flow was not found",
            Self::Expired => "Trae login flow expired; restart login",
            Self::CapabilityMismatch => "Trae callback capability does not match the login flow",
            Self::AlreadyConsumed => "Trae callback capability was already consumed",
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
        capability: &str,
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
        let candidate = capability_digest(capability);
        if !constant_time_eq(&candidate, &entry.flow.capability_digest) {
            return Err(TraeLoginFlowError::CapabilityMismatch);
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

#[derive(Debug, Clone)]
pub struct TraeCallbackPayload {
    pub flow_id: String,
    pub capability: String,
    refresh_token: String,
    callback_uid: Option<String>,
    callback_name: Option<String>,
    callback_enterprise_id: Option<String>,
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
    let (machine_id, device_id, expected_account_id, expected_uid) =
        if let Some(account) = expected_account {
            if account.provider_type != ProviderType::TraeSolo {
                return Err(TraeClientError::bad_request(format!(
                    "expected trae_solo account, got {}",
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
    start_login_with_endpoints(
        callback_base_url,
        machine_id,
        device_id,
        expected_account_id,
        expected_uid,
        TraeOAuthEndpoints::production()?,
        now_ms,
    )
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
    let flow_id = random_secret();
    let capability = random_secret();
    let trace = random_hex(16);
    let callback_url = build_callback_url(callback_base_url, &flow_id, &capability)?;
    let auth_url = build_login_url(&machine_id, &device_id, callback_url.as_str(), &trace)?;
    let expires_at_ms = now_ms.saturating_add((FLOW_TTL_SECS as i64).saturating_mul(1_000));
    let flow = PendingTraeLoginFlow {
        expires_at_ms,
        capability_digest: capability_digest(&capability),
        machine_id,
        device_id,
        expected_account_id: expected_account_id.clone(),
        expected_uid,
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
    let parsed = Url::parse(raw_url.trim())
        .map_err(|_| TraeClientError::bad_request("Trae callback URL is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(TraeClientError::bad_request(
            "Trae callback URL must use HTTP(S)",
        ));
    }
    let flow_id = unique_query_value(&parsed, &["flowId", "flow_id"])?
        .ok_or_else(|| TraeClientError::bad_request("Trae callback is missing flowId"))?;
    let capability = unique_query_value(&parsed, &["capability"])?
        .ok_or_else(|| TraeClientError::bad_request("Trae callback is missing capability"))?;
    let mut refresh_token = unique_query_value(&parsed, &["refreshToken", "refresh_token"])?;
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
    if let Some(user_jwt) = unique_query_value(&parsed, &["userJwt", "user_jwt"])? {
        if user_jwt.trim_start().starts_with('{') {
            let value: Value = serde_json::from_str(&user_jwt).map_err(|_| {
                TraeClientError::bad_request("Trae callback userJwt is not valid JSON")
            })?;
            let nested_refresh = string_at(
                &value,
                &["/RefreshToken", "/refreshToken", "/refresh_token"],
            );
            match (refresh_token.as_deref(), nested_refresh.as_deref()) {
                (Some(existing), Some(nested)) if existing != nested => {
                    return Err(TraeClientError::bad_request(
                        "Trae callback contains conflicting refresh tokens",
                    ));
                }
                (None, Some(nested)) => refresh_token = Some(nested.to_string()),
                _ => {}
            }
        }
    }
    let refresh_token = refresh_token
        .ok_or_else(|| TraeClientError::bad_request("Trae callback is missing refreshToken"))?;
    if refresh_token.len() > 64 * 1024 {
        return Err(TraeClientError::bad_request(
            "Trae callback refreshToken is too large",
        ));
    }
    Ok(TraeCallbackPayload {
        flow_id,
        capability,
        refresh_token,
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
    let receipt = exchange_token(http, &flow.endpoints, &callback.refresh_token, now_ms).await?;
    let identity = fetch_user_info(http, &flow.endpoints, &receipt.access_token).await?;
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
    if let Some(callback_enterprise_id) = callback.callback_enterprise_id.as_deref() {
        if callback_enterprise_id != identity.enterprise_id {
            return Err(TraeClientError::conflict(
                "Trae callback enterprise identity disagrees with GetUserInfo",
            ));
        }
    }
    let account_id = trae_account_id(&identity.uid).map_err(TraeClientError::protocol)?;
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
        provider_type: ProviderType::TraeSolo,
        email: None,
        access_token: Some(receipt.access_token),
        refresh_token: Some(receipt.refresh_token),
        id_token: None,
        token_type: Some("Cloud-IDE-JWT".to_string()),
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

    if account.provider_type != ProviderType::TraeSolo {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected trae_solo account, got {}",
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
    let receipt = exchange_token(http, &endpoints, refresh_token, now_ms)
        .await
        .map_err(trae_refresh_exchange_failure)?;
    let refresh_update = AccountRefreshUpdate {
        access_token: Some(receipt.access_token.clone()),
        refresh_token: Some(receipt.refresh_token.clone()),
        token_type: Some("Cloud-IDE-JWT".to_string()),
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
    let identity = fetch_user_info(http, &endpoints, &receipt.access_token)
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
    let url = endpoints.agent_url(TRAE_MODEL_DETAIL_PATH)?;
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
    let url = endpoints.billing_url(TRAE_QUOTA_PATH)?;
    let request = http
        .request(Method::POST, url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, format!("Trae/{TRAE_IDE_VERSION}"))
        .header("Authorization", format!("Cloud-IDE-JWT {access_token}"))
        .header("X-User-Region", "CN")
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
    if account.provider_type != ProviderType::TraeSolo {
        return Err(TraeClientError::bad_request(format!(
            "expected trae_solo account, got {}",
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

async fn exchange_token(
    http: &reqwest::Client,
    endpoints: &TraeOAuthEndpoints,
    refresh_token: &str,
    now_ms: i64,
) -> Result<TraeTokenReceipt, TraeClientError> {
    if refresh_token.trim().is_empty() {
        return Err(TraeClientError::bad_request(
            "Trae refresh token is required",
        ));
    }
    let url = endpoints.url(TRAE_EXCHANGE_TOKEN_PATH)?;
    let request = oauth_request(
        http.request(Method::POST, url)
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({
                "ClientID": TRAE_CLIENT_ID,
                "RefreshToken": refresh_token,
                "ClientSecret": "-",
                "UserID": "",
            })),
    );
    let (status, body) = execute_bounded(request, "token exchange").await?;
    if !status.is_success() {
        return Err(TraeClientError::upstream(status, "token exchange", &body));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        TraeClientError::protocol(format!("Trae token response is not valid JSON: {error}"))
    })?;
    ensure_success_envelope(&value, "token exchange")?;
    let result = value
        .get("Result")
        .or_else(|| value.get("result"))
        .or_else(|| value.get("data"))
        .unwrap_or(&value);
    let access_token = string_at(result, &["/Token", "/token", "/accessToken"])
        .ok_or_else(|| TraeClientError::protocol("Trae token response is missing Token"))?;
    let rotated_refresh = string_at(
        result,
        &["/RefreshToken", "/refreshToken", "/refresh_token"],
    )
    .unwrap_or_else(|| refresh_token.to_string());
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
) -> Result<TraeUserIdentity, TraeClientError> {
    let url = endpoints.url(TRAE_USER_INFO_PATH)?;
    let request = oauth_request(
        http.request(Method::POST, url)
            .header(CONTENT_TYPE, "application/json")
            .header("X-Cloudide-Token", access_token)
            .json(&json!({"ReqSource": "IDE", "IDEVersion": TRAE_IDE_VERSION})),
    );
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

fn build_callback_url(
    callback_base_url: &str,
    flow_id: &str,
    capability: &str,
) -> Result<Url, TraeClientError> {
    let mut callback = Url::parse(callback_base_url)
        .map_err(|_| TraeClientError::bad_request("Trae callback base URL is not a valid URL"))?;
    if callback.scheme() != "http"
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.fragment().is_some()
        || callback.query().is_some()
    {
        return Err(TraeClientError::bad_request(
            "Trae callback base URL must be a plain HTTP loopback URL",
        ));
    }
    let host = callback.host_str().unwrap_or_default();
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Err(TraeClientError::bad_request(
            "Trae callback base URL must use a loopback host",
        ));
    }
    callback
        .query_pairs_mut()
        .append_pair("flowId", flow_id)
        .append_pair("capability", capability);
    Ok(callback)
}

fn build_login_url(
    machine_id: &str,
    device_id: &str,
    callback_url: &str,
    trace: &str,
) -> Result<String, TraeClientError> {
    let mut url = Url::parse(TRAE_CONSOLE_ORIGIN)
        .and_then(|origin| origin.join(TRAE_AUTHORIZATION_PATH))
        .map_err(|error| {
            TraeClientError::protocol(format!("invalid reviewed Trae browser URL: {error}"))
        })?;
    url.query_pairs_mut()
        .append_pair("login_version", "1")
        .append_pair("auth_from", "solo")
        .append_pair("login_channel", "native_ide")
        .append_pair("plugin_version", TRAE_PLUGIN_VERSION)
        .append_pair("auth_type", "local")
        .append_pair("client_id", TRAE_CLIENT_ID)
        .append_pair("redirect", "0")
        .append_pair("login_trace_id", trace)
        .append_pair("auth_callback_url", callback_url)
        .append_pair("machine_id", machine_id)
        .append_pair("device_id", device_id)
        .append_pair("x_device_id", device_id)
        .append_pair("x_machine_id", machine_id)
        .append_pair("x_device_brand", "PC")
        .append_pair("x_device_type", "PC")
        .append_pair("x_os_version", "1.0")
        .append_pair("x_app_version", TRAE_IDE_VERSION)
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
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.len() > 1 {
        return Err(TraeClientError::bad_request(format!(
            "Trae callback contains duplicate {} fields",
            names[0]
        )));
    }
    Ok(values.into_iter().next())
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
    use super::*;
    use crate::domain::accounts::store::AccountStore;
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
                    TRAE_EXCHANGE_TOKEN_PATH => json!({
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

    fn callback_from_start(start: &TraeLoginStart, refresh_token: &str) -> String {
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
            .append_pair("refreshToken", refresh_token)
            .append_pair(
                "userInfo",
                r#"{"UserID":"uid-1","TenantID":"enterprise-1"}"#,
            );
        callback.into()
    }

    #[test]
    fn flow_capability_is_exact_once_only_and_expires() {
        let endpoints = TraeOAuthEndpoints::for_test("http://127.0.0.1:9");
        let (start, flow) = start_login_with_endpoints(
            "http://localhost:9911/api/accounts/trae/login/callback",
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
        assert_eq!(
            store
                .begin_completion(&start.flow_id, "wrong-capability", 1_001)
                .unwrap_err(),
            TraeLoginFlowError::CapabilityMismatch
        );
        store
            .begin_completion(&start.flow_id, &callback.capability, 1_001)
            .unwrap();
        assert_eq!(
            store
                .begin_completion(&start.flow_id, &callback.capability, 1_001)
                .unwrap_err(),
            TraeLoginFlowError::AlreadyConsumed
        );

        let (expired, flow) = start_login_with_endpoints(
            "http://localhost:9911/api/accounts/trae/login/callback",
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
                .begin_completion(&expired.flow_id, &callback.capability, 600_000)
                .unwrap_err(),
            TraeLoginFlowError::NotFound
        );
    }

    #[test]
    fn callback_parser_rejects_missing_or_ambiguous_capability_and_token() {
        assert!(parse_callback_url(
            "http://localhost/callback?flowId=f&capability=c&refreshToken=r"
        )
        .is_ok());
        for callback in [
            "http://localhost/callback?flowId=f&refreshToken=r",
            "http://localhost/callback?flowId=f&capability=c",
            "http://localhost/callback?flowId=f&flowId=g&capability=c&refreshToken=r",
            "http://localhost/callback?flowId=f&capability=c&capability=d&refreshToken=r",
        ] {
            assert!(parse_callback_url(callback).is_err(), "{callback}");
        }
    }

    #[tokio::test]
    async fn login_uses_fixed_exchange_shape_and_closes_trusted_identity() {
        let (origin, observations, server) = serve_oauth_fixture("uid-1", "enterprise-1").await;
        let (start, flow) = start_login_with_endpoints(
            "http://localhost:9911/api/accounts/trae/login/callback",
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
        assert_eq!(observations[0].path, TRAE_EXCHANGE_TOKEN_PATH);
        assert_eq!(observations[0].body["ClientID"], TRAE_CLIENT_ID);
        assert_eq!(observations[0].body["RefreshToken"], "refresh-old");
        assert_eq!(observations[1].path, TRAE_USER_INFO_PATH);
        assert_eq!(observations[1].headers["x-cloudide-token"], "access-new");
        assert_eq!(observations[1].body["IDEVersion"], TRAE_IDE_VERSION);
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
            "http://localhost:9911/api/accounts/trae/login/callback",
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
        assert_eq!(callback.host_str(), Some("localhost"));
        assert!(callback.query_pairs().any(|(key, _)| key == "capability"));
    }
}
