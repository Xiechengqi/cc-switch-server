use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use url::Url;

use crate::domain::accounts::oauth::{merge_account_refresh_raw, OAuthErrorKind};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, UpsertAccountInput};
use crate::domain::codebuddy::{
    codebuddy_access_token_subject, codebuddy_account_id, CodeBuddyAccountProfile, CodeBuddySite,
    CODEBUDDY_ACCOUNTS_PATH, CODEBUDDY_AUTH_STATE_PATH, CODEBUDDY_AUTH_TOKEN_PATH,
    CODEBUDDY_CLIENT_VERSION, CODEBUDDY_CONFIG_PATH, CODEBUDDY_LOGIN_ACCOUNT_PATH,
    CODEBUDDY_PLATFORM, CODEBUDDY_REFRESH_PATH, CODEBUDDY_RESOURCE_PATH,
};
use crate::domain::providers::model::ProviderType;

const FLOW_TTL_SECS: u64 = 10 * 60;
const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;
const MAX_RESPONSE_BODY_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TOKEN_PENDING_CODE: i64 = 11_217;
const ACCOUNT_PENDING_CODE: i64 = 12_151;

#[derive(Debug, Clone)]
pub struct CodeBuddyEndpoints {
    pub site: CodeBuddySite,
    pub base_url: Url,
}

impl CodeBuddyEndpoints {
    pub fn for_site(site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        let base_url = Url::parse(site.profile().endpoint).map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!(
                "invalid reviewed CodeBuddy endpoint: {error}"
            ))
        })?;
        Ok(Self { site, base_url })
    }

    #[cfg(test)]
    fn for_test(site: CodeBuddySite, base_url: &str) -> Self {
        Self {
            site,
            base_url: Url::parse(base_url).unwrap(),
        }
    }

    #[cfg(not(test))]
    fn for_account(_account: &Account, site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        Self::for_site(site)
    }

    #[cfg(test)]
    fn for_account(account: &Account, site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        let mut endpoints = Self::for_site(site)?;
        if let Some(base_url) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testCodeBuddyBaseUrl"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            endpoints.base_url = Url::parse(base_url).map_err(|error| {
                CodeBuddyClientError::bad_gateway(format!(
                    "invalid test CodeBuddy endpoint: {error}"
                ))
            })?;
        }
        Ok(endpoints)
    }

    fn url(&self, path: &str) -> Result<Url, CodeBuddyClientError> {
        self.base_url.join(path).map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!(
                "invalid CodeBuddy endpoint path {path}: {error}"
            ))
        })
    }

    pub(crate) fn base_url(&self) -> String {
        self.base_url.as_str().trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone)]
struct CodeBuddyBillingEndpoints {
    base_url: Url,
}

impl CodeBuddyBillingEndpoints {
    fn for_site(site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        let base_url = Url::parse(site.profile().billing_endpoint).map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!(
                "invalid reviewed CodeBuddy billing endpoint: {error}"
            ))
        })?;
        Ok(Self { base_url })
    }

    #[cfg(not(test))]
    fn for_account(_account: &Account, site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        Self::for_site(site)
    }

    #[cfg(test)]
    fn for_account(account: &Account, site: CodeBuddySite) -> Result<Self, CodeBuddyClientError> {
        let mut endpoints = Self::for_site(site)?;
        // Billing is a separate authority for CN. Its loopback override must
        // never inherit the config/chat override implicitly.
        if let Some(base_url) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testCodeBuddyBillingBaseUrl"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            endpoints.base_url = Url::parse(base_url).map_err(|error| {
                CodeBuddyClientError::bad_gateway(format!(
                    "invalid test CodeBuddy billing endpoint: {error}"
                ))
            })?;
        }
        Ok(endpoints)
    }

    fn url(&self, path: &str) -> Result<Url, CodeBuddyClientError> {
        self.base_url.join(path).map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!(
                "invalid CodeBuddy billing endpoint path {path}: {error}"
            ))
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBuddyLoginStart {
    pub flow_id: String,
    pub auth_url: String,
    pub expires_at: i64,
    pub expires_in: u64,
    pub interval: u64,
    pub site: CodeBuddySite,
}

#[derive(Debug, Clone)]
pub struct PendingCodeBuddyLoginFlow {
    pub expires_at_ms: i64,
    pub interval: u64,
    pub upstream_state: String,
    pub endpoints: CodeBuddyEndpoints,
    client: reqwest::Client,
    token: Option<CodeBuddyLoginToken>,
}

#[derive(Debug, Clone)]
struct CodeBuddyLoginToken {
    access_token: String,
    refresh_token: String,
    token_type: String,
    domain: String,
    scopes: Vec<String>,
    expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBuddyLoginPollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(skip)]
    pub account_input: Option<UpsertAccountInput>,
    #[serde(skip)]
    token: Option<CodeBuddyLoginToken>,
}

#[derive(Debug, Clone)]
struct CodeBuddyLoginFlowEntry {
    flow: PendingCodeBuddyLoginFlow,
    state: CodeBuddyLoginFlowState,
}

#[derive(Debug, Clone)]
enum CodeBuddyLoginFlowState {
    Pending { next_poll_at_ms: i64 },
    Polling,
    Completed(Box<CodeBuddyLoginPollResult>),
}

#[derive(Debug, Clone)]
pub enum CodeBuddyLoginPollLease {
    Ready(Box<PendingCodeBuddyLoginFlow>),
    Wait(u64),
    InProgress,
    Completed(Box<CodeBuddyLoginPollResult>),
}

#[derive(Debug, Clone, Default)]
pub struct CodeBuddyLoginFlowStore {
    pending: BTreeMap<String, CodeBuddyLoginFlowEntry>,
}

impl CodeBuddyLoginFlowStore {
    pub fn insert(&mut self, flow_id: String, flow: PendingCodeBuddyLoginFlow, now_ms: i64) {
        self.cleanup(now_ms);
        self.pending.insert(
            flow_id,
            CodeBuddyLoginFlowEntry {
                flow,
                state: CodeBuddyLoginFlowState::Pending {
                    next_poll_at_ms: now_ms,
                },
            },
        );
    }

    pub fn begin_poll(&mut self, flow_id: &str, now_ms: i64) -> Option<CodeBuddyLoginPollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(flow_id)?;
        match &entry.state {
            CodeBuddyLoginFlowState::Pending { next_poll_at_ms } if now_ms < *next_poll_at_ms => {
                Some(CodeBuddyLoginPollLease::Wait(
                    u64::try_from(next_poll_at_ms.saturating_sub(now_ms))
                        .unwrap_or(u64::MAX)
                        .saturating_add(999)
                        / 1_000,
                ))
            }
            CodeBuddyLoginFlowState::Pending { .. } => {
                entry.state = CodeBuddyLoginFlowState::Polling;
                Some(CodeBuddyLoginPollLease::Ready(Box::new(entry.flow.clone())))
            }
            CodeBuddyLoginFlowState::Polling => Some(CodeBuddyLoginPollLease::InProgress),
            CodeBuddyLoginFlowState::Completed(result) => {
                Some(CodeBuddyLoginPollLease::Completed(result.clone()))
            }
        }
    }

    pub fn finish_poll(
        &mut self,
        flow_id: &str,
        mut result: CodeBuddyLoginPollResult,
        now_ms: i64,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(flow_id) else {
            return false;
        };
        if !matches!(entry.state, CodeBuddyLoginFlowState::Polling) {
            return false;
        }
        if result.pending {
            if let Some(token) = result.token.take() {
                entry.flow.token = Some(token);
            }
            let delay = result
                .retry_after_secs
                .unwrap_or(entry.flow.interval)
                .max(1);
            entry.state = CodeBuddyLoginFlowState::Pending {
                next_poll_at_ms: now_ms.saturating_add((delay as i64).saturating_mul(1_000)),
            };
        } else {
            result.token = None;
            entry.state = CodeBuddyLoginFlowState::Completed(Box::new(result));
        }
        true
    }

    pub fn fail_poll(&mut self, flow_id: &str, terminal: bool, now_ms: i64) {
        if terminal {
            self.pending.remove(flow_id);
        } else if let Some(entry) = self.pending.get_mut(flow_id) {
            entry.state = CodeBuddyLoginFlowState::Pending {
                next_poll_at_ms: now_ms
                    .saturating_add((entry.flow.interval as i64).saturating_mul(1_000)),
            };
        }
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
pub struct CodeBuddyClientError {
    pub status: StatusCode,
    pub upstream_status: Option<StatusCode>,
    pub terminal: bool,
    pub message: String,
}

impl CodeBuddyClientError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            upstream_status: None,
            terminal: true,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            terminal: false,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            terminal: true,
            message: message.into(),
        }
    }

    fn upstream(status: StatusCode, operation: &str, body: &[u8]) -> Self {
        let terminal = !matches!(
            status,
            StatusCode::REQUEST_TIMEOUT
                | StatusCode::TOO_MANY_REQUESTS
                | StatusCode::INTERNAL_SERVER_ERROR
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        );
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
                "CodeBuddy {operation} failed with upstream HTTP {}: {}",
                status.as_u16(),
                public_error_body(body)
            ),
        }
    }

    fn business(operation: &str, code: i64, message: &str) -> Self {
        let status = if matches!(code, 12_005 | 11_212 | 11_216 | 12_153) {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::BAD_GATEWAY
        };
        Self {
            status,
            upstream_status: None,
            terminal: true,
            message: format!(
                "CodeBuddy {operation} failed with business code {code}: {}",
                sanitized_message(message)
            ),
        }
    }

    pub(crate) fn is_authentication_failure(&self) -> bool {
        self.status == StatusCode::UNAUTHORIZED
            || matches!(
                self.upstream_status,
                Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            )
    }

    pub(crate) fn is_transient(&self) -> bool {
        self.upstream_status.is_none() && !self.terminal
            || matches!(
                self.upstream_status,
                Some(
                    StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                        | StatusCode::INTERNAL_SERVER_ERROR
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT
                )
            )
    }
}

impl fmt::Display for CodeBuddyClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodeBuddyClientError {}

pub async fn start_login(
    site: CodeBuddySite,
    now_ms: i64,
) -> Result<(CodeBuddyLoginStart, PendingCodeBuddyLoginFlow), CodeBuddyClientError> {
    start_login_with_endpoints(CodeBuddyEndpoints::for_site(site)?, now_ms).await
}

async fn start_login_with_endpoints(
    endpoints: CodeBuddyEndpoints,
    now_ms: i64,
) -> Result<(CodeBuddyLoginStart, PendingCodeBuddyLoginFlow), CodeBuddyClientError> {
    let client = crate::infra::http::outbound_client_builder()
        .map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!("build CodeBuddy login client: {error}"))
        })?
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            CodeBuddyClientError::bad_gateway(format!("build CodeBuddy cookie client: {error}"))
        })?;
    let url = endpoints.url(CODEBUDDY_AUTH_STATE_PATH)?;
    let request = unauthenticated_request(
        client
            .request(Method::POST, url)
            .header(CONTENT_TYPE, "application/json")
            .body("{}"),
    );
    let (status, body) = execute_bounded(request, "auth state").await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(status, "auth state", &body));
    }
    let (code, message, data) = parse_envelope(&body, "auth state")?;
    if code != 0 {
        return Err(CodeBuddyClientError::business("auth state", code, &message));
    }
    let upstream_state = string_at(&data, &["/state", "/oauthState"])
        .ok_or_else(|| CodeBuddyClientError::protocol("CodeBuddy auth state is missing state"))?;
    let auth_url = string_at(
        &data,
        &["/authUrl", "/authorizationUrl", "/authorizeUrl", "/url"],
    )
    .ok_or_else(|| CodeBuddyClientError::protocol("CodeBuddy auth state is missing authUrl"))?;
    validate_auth_url(&auth_url, &endpoints)?;
    let mut flow_id = random_flow_id();
    if flow_id == upstream_state {
        flow_id = random_flow_id();
    }
    let expires_at_ms = now_ms.saturating_add((FLOW_TTL_SECS as i64).saturating_mul(1_000));
    let flow = PendingCodeBuddyLoginFlow {
        expires_at_ms,
        interval: DEFAULT_POLL_INTERVAL_SECS,
        upstream_state,
        endpoints: endpoints.clone(),
        client,
        token: None,
    };
    Ok((
        CodeBuddyLoginStart {
            flow_id,
            auth_url,
            expires_at: expires_at_ms,
            expires_in: FLOW_TTL_SECS,
            interval: DEFAULT_POLL_INTERVAL_SECS,
            site: endpoints.site,
        },
        flow,
    ))
}

pub async fn poll_login(
    flow: &PendingCodeBuddyLoginFlow,
    now_ms: i64,
) -> Result<CodeBuddyLoginPollResult, CodeBuddyClientError> {
    if flow.expires_at_ms <= now_ms {
        return Err(CodeBuddyClientError::bad_request(
            "CodeBuddy login flow expired; restart login",
        ));
    }

    let token = match flow.token.clone() {
        Some(token) => token,
        None => match poll_token(flow, now_ms).await? {
            Some(token) => token,
            None => return Ok(pending_result("authorization_pending", flow.interval, None)),
        },
    };
    let Some(identity) = poll_account(flow, &token).await? else {
        return Ok(pending_result(
            "account_pending",
            flow.interval,
            Some(token),
        ));
    };
    let enterprise_id = fetch_enterprise_identity(flow, &token, &identity.uid).await?;
    let account_id = codebuddy_account_id(
        flow.endpoints.site,
        &token.domain,
        &identity.uid,
        &enterprise_id,
    )
    .map_err(CodeBuddyClientError::protocol)?;
    let profile = json!({
        "site": flow.endpoints.site,
        "domain": token.domain,
        "uid": identity.uid,
        "enterpriseId": enterprise_id,
        "name": identity.name,
        "email": identity.email,
        "nickname": identity.nickname,
        "accountType": identity.account_type,
        "clientVersion": CODEBUDDY_CLIENT_VERSION,
        "productPlatform": CODEBUDDY_PLATFORM,
    });
    let account_input = UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::CodeBuddyOAuth,
        email: non_empty(identity.email),
        access_token: Some(token.access_token),
        refresh_token: Some(token.refresh_token),
        id_token: None,
        token_type: Some(token.token_type),
        api_key: None,
        extra_headers: None,
        scopes: token.scopes,
        profile: Some(profile),
        raw: Some(json!({
            "source": "codebuddy_oauth_login",
            "site": flow.endpoints.site.as_str(),
            "domain": token.domain,
            "observedAtMs": now_ms,
        })),
        subscription_level: None,
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at: token.expires_at_ms,
        rate_limited_until: None,
        last_refresh_error: None,
    };
    Ok(CodeBuddyLoginPollResult {
        pending: false,
        message: "CodeBuddy authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
        token: None,
    })
}

pub async fn refresh_codebuddy_account<F>(
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

    if account.provider_type != ProviderType::CodeBuddyOAuth {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected codebuddy_oauth account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = CodeBuddyAccountProfile::parse(account.profile.as_ref())
        .map_err(AccountRefreshFailure::bad_request)?;
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("CodeBuddy access token is required"))?;
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("CodeBuddy refresh token is required"))?;
    let endpoints = CodeBuddyEndpoints::for_account(account, profile.site)
        .map_err(codebuddy_refresh_client_failure)?;
    let url = endpoints
        .url(CODEBUDDY_REFRESH_PATH)
        .map_err(codebuddy_refresh_client_failure)?;
    let request = authenticated_request(
        http.post(url)
            .header(CONTENT_TYPE, "application/json")
            .header("X-Refresh-Token", refresh_token)
            .header("X-Auth-Refresh-Source", "plugin")
            .body("{}"),
        access_token,
        &profile.uid,
        Some(&profile.enterprise_id),
        &profile.domain,
    );
    let (status, body) = execute_bounded(request, "token refresh")
        .await
        .map_err(codebuddy_refresh_client_failure)?;
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        if status.is_success() {
            AccountRefreshFailure::outcome_unknown(format!(
                "CodeBuddy refresh response is not valid JSON after a successful exchange: {error}"
            ))
        } else {
            AccountRefreshFailure::parse(format!(
                "CodeBuddy refresh error response is not valid JSON: {error}"
            ))
        }
    })?;
    let business_code = recursive_business_code(&value, 0);
    let business_message = string_at(
        &value,
        &["/msg", "/message", "/error/message", "/data/message"],
    )
    .unwrap_or_default();
    if !status.is_success() || business_code.is_some_and(|code| code != 0) {
        return Err(codebuddy_refresh_rejection(
            status,
            business_code,
            &business_message,
            &body,
        ));
    }
    let token_value = value.get("data").unwrap_or(&value);
    let token_value = token_value.pointer("/token").unwrap_or(token_value);
    let new_access_token =
        string_at(token_value, &["/accessToken", "/access_token"]).ok_or_else(|| {
            AccountRefreshFailure::outcome_unknown(
                "CodeBuddy refresh receipt has no accessToken after a successful exchange",
            )
        })?;
    let new_refresh_token = string_at(token_value, &["/refreshToken", "/refresh_token"])
        .unwrap_or_else(|| refresh_token.to_string());
    let expires_at = i64_at(token_value, &["/expiresIn", "/expires_in"])
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)));
    let receipt = AccountRefreshUpdate {
        access_token: Some(new_access_token),
        refresh_token: Some(new_refresh_token),
        token_type: Some(
            string_at(token_value, &["/tokenType", "/token_type"])
                .unwrap_or_else(|| "Bearer".to_string()),
        ),
        scopes: scopes_option_at(token_value),
        expires_at,
        raw: Some(json!({
            "codeBuddyRefreshReceipt": {
                "site": profile.site.as_str(),
                "domain": string_at(token_value, &["/domain"])
                    .unwrap_or_else(|| profile.domain.clone()),
                "receivedAtMs": now_ms
            }
        })),
        ..AccountRefreshUpdate::default()
    };

    // The endpoint rotates refresh tokens. Journal the complete credential
    // receipt before identity validation so the old refresh token is never
    // retried after a successful exchange with an interrupted validation.
    receipt_hook(&receipt)?;
    let mut update = complete_codebuddy_refresh_receipt(http, account, receipt).await?;
    if let Some(raw) = update.raw.take() {
        update.raw = Some(merge_account_refresh_raw(account.raw.as_ref(), raw));
    }
    Ok(update)
}

pub(crate) async fn fetch_model_config(
    http: &reqwest::Client,
    account: &Account,
    request_timeout: Duration,
) -> Result<(Value, String), CodeBuddyClientError> {
    if account.provider_type != ProviderType::CodeBuddyOAuth {
        return Err(CodeBuddyClientError::bad_request(format!(
            "expected codebuddy_oauth account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = CodeBuddyAccountProfile::parse(account.profile.as_ref())
        .map_err(CodeBuddyClientError::bad_request)?;
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CodeBuddyClientError::bad_request("CodeBuddy access token is required"))?;
    let endpoints = CodeBuddyEndpoints::for_account(account, profile.site)?;
    let url = endpoints.url(CODEBUDDY_CONFIG_PATH)?;
    let request = authenticated_request(
        http.get(url),
        access_token,
        &profile.uid,
        Some(&profile.enterprise_id),
        &profile.domain,
    );
    let (status, body) = execute_bounded_with_timeout(
        request,
        "model config",
        request_timeout.min(REQUEST_TIMEOUT),
    )
    .await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(
            status,
            "model config",
            &body,
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        CodeBuddyClientError::protocol(format!(
            "CodeBuddy model config response is not valid JSON: {error}"
        ))
    })?;
    if let Some(code) = recursive_business_code(&value, 0) {
        if code != 0 {
            let message =
                string_at(&value, &["/msg", "/message", "/error/message"]).unwrap_or_default();
            return Err(CodeBuddyClientError::business(
                "model config",
                code,
                &message,
            ));
        }
    }
    Ok((value, endpoints.base_url()))
}

pub(crate) async fn fetch_billing_resource(
    http: &reqwest::Client,
    account: &Account,
    request_timeout: Duration,
) -> Result<Value, CodeBuddyClientError> {
    if account.provider_type != ProviderType::CodeBuddyOAuth {
        return Err(CodeBuddyClientError::bad_request(format!(
            "expected codebuddy_oauth account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = CodeBuddyAccountProfile::parse(account.profile.as_ref())
        .map_err(CodeBuddyClientError::bad_request)?;
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CodeBuddyClientError::bad_request("CodeBuddy access token is required"))?;
    let endpoints = CodeBuddyBillingEndpoints::for_account(account, profile.site)?;
    let url = endpoints.url(CODEBUDDY_RESOURCE_PATH)?;
    let now = chrono::Utc::now();
    let range_end = now
        .checked_add_signed(chrono::Duration::days(365 * 100))
        .ok_or_else(|| {
            CodeBuddyClientError::bad_gateway("CodeBuddy billing time range overflow")
        })?;
    let body = serde_json::to_vec(&json!({
        "PageNumber": 1,
        "PageSize": 100,
        "ProductCode": "p_tcaca",
        "Status": [0, 3],
        "PackageEndTimeRangeBegin": now.format("%Y-%m-%d %H:%M:%S").to_string(),
        "PackageEndTimeRangeEnd": range_end.format("%Y-%m-%d %H:%M:%S").to_string(),
    }))
    .map_err(|error| {
        CodeBuddyClientError::bad_gateway(format!(
            "encode CodeBuddy billing resource request: {error}"
        ))
    })?;
    let request = authenticated_request(
        http.post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body),
        access_token,
        &profile.uid,
        Some(&profile.enterprise_id),
        &profile.domain,
    );
    let (status, body) = execute_bounded_with_timeout(
        request,
        "billing resource",
        request_timeout.min(REQUEST_TIMEOUT),
    )
    .await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(
            status,
            "billing resource",
            &body,
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        CodeBuddyClientError::protocol(format!(
            "CodeBuddy billing resource response is not valid JSON: {error}"
        ))
    })?;
    if let Some(code) = recursive_business_code(&value, 0) {
        if code != 0 {
            let message =
                string_at(&value, &["/msg", "/message", "/error/message"]).unwrap_or_default();
            return Err(CodeBuddyClientError::business(
                "billing resource",
                code,
                &message,
            ));
        }
    }
    Ok(value)
}

pub(crate) fn runtime_base_url(account: &Account) -> Result<String, CodeBuddyClientError> {
    let profile = CodeBuddyAccountProfile::parse(account.profile.as_ref())
        .map_err(CodeBuddyClientError::bad_request)?;
    CodeBuddyEndpoints::for_account(account, profile.site).map(|endpoints| endpoints.base_url())
}

pub async fn complete_codebuddy_refresh_receipt(
    http: &reqwest::Client,
    account: &Account,
    mut update: AccountRefreshUpdate,
) -> Result<AccountRefreshUpdate, crate::clients::oauth::refresh::AccountRefreshFailure> {
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    let profile = CodeBuddyAccountProfile::parse(account.profile.as_ref())
        .map_err(AccountRefreshFailure::parse)?;
    let access_token = update
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AccountRefreshFailure::parse("CodeBuddy refresh receipt has no access token")
        })?;
    if let Some(domain) = update
        .raw
        .as_ref()
        .and_then(|raw| raw.pointer("/codeBuddyRefreshReceipt/domain"))
        .and_then(Value::as_str)
    {
        let domain = profile.site.canonical_token_domain(domain).ok_or_else(|| {
            codebuddy_refresh_identity_failure(format!(
                "CodeBuddy OAuth refresh returned a domain outside the bound {} site",
                profile.site.as_str()
            ))
        })?;
        if domain != profile.domain {
            return Err(codebuddy_refresh_identity_failure(
                "CodeBuddy OAuth refresh returned a different domain identity; re-login as a new account"
                    .to_string(),
            ));
        }
    }
    // A JWT payload is unverified metadata and cannot close account identity on
    // its own. Always re-read both the stable account UID and /v3/config's
    // enterprise identity with the new credential before committing it.
    let jwt_subject = codebuddy_access_token_subject(access_token);
    let refreshed = fetch_current_account_identity(http, account, &profile, access_token).await?;
    if jwt_subject
        .as_deref()
        .is_some_and(|subject| subject != refreshed.uid)
    {
        return Err(codebuddy_refresh_identity_failure(
            "CodeBuddy OAuth refresh JWT subject disagrees with the account endpoint".to_string(),
        ));
    }
    if refreshed.uid != profile.uid {
        return Err(AccountRefreshFailure {
            status_code: StatusCode::CONFLICT.as_u16(),
            upstream_status: None,
            message: "CodeBuddy OAuth refresh returned a different uid; re-login as a new account"
                .to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        });
    }
    if refreshed.enterprise_id != profile.enterprise_id {
        return Err(codebuddy_refresh_identity_failure(
            "CodeBuddy OAuth refresh returned a different enterprise identity; re-login as a new account"
                .to_string(),
        ));
    }
    update.profile = Some(serde_json::to_value(profile).map_err(|error| {
        AccountRefreshFailure::parse(format!("encode CodeBuddy profile: {error}"))
    })?);
    Ok(update)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeBuddyRefreshIdentity {
    uid: String,
    enterprise_id: String,
}

async fn fetch_current_account_identity(
    http: &reqwest::Client,
    account: &Account,
    profile: &CodeBuddyAccountProfile,
    access_token: &str,
) -> Result<CodeBuddyRefreshIdentity, crate::clients::oauth::refresh::AccountRefreshFailure> {
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    let endpoints = CodeBuddyEndpoints::for_account(account, profile.site)
        .map_err(codebuddy_refresh_client_failure)?;
    let url = endpoints
        .url(CODEBUDDY_ACCOUNTS_PATH)
        .map_err(codebuddy_refresh_client_failure)?;
    let request = authenticated_request(
        http.get(url),
        access_token,
        &profile.uid,
        Some(&profile.enterprise_id),
        &profile.domain,
    );
    let (status, body) = execute_bounded(request, "refresh identity validation")
        .await
        .map_err(codebuddy_refresh_client_failure)?;
    if !status.is_success() {
        return Err(codebuddy_refresh_rejection(status, None, "", &body));
    }
    let (code, message, data) = parse_envelope(&body, "refresh identity validation")
        .map_err(codebuddy_refresh_client_failure)?;
    if code != 0 {
        return Err(codebuddy_refresh_rejection(
            status,
            Some(code),
            &message,
            &body,
        ));
    }
    let identity = identity_value(&data);
    let uid = string_at(identity, &["/uid", "/userId", "/user_id", "/sub"]).ok_or_else(|| {
        AccountRefreshFailure::parse("CodeBuddy refresh identity response is missing stable uid")
    })?;
    let config_url = endpoints
        .url(CODEBUDDY_CONFIG_PATH)
        .map_err(codebuddy_refresh_client_failure)?;
    let config_request = authenticated_request(
        http.get(config_url),
        access_token,
        &uid,
        Some(&profile.enterprise_id),
        &profile.domain,
    );
    let (config_status, config_body) = execute_bounded(config_request, "refresh config identity")
        .await
        .map_err(codebuddy_refresh_client_failure)?;
    if !config_status.is_success() {
        return Err(codebuddy_refresh_rejection(
            config_status,
            None,
            "",
            &config_body,
        ));
    }
    let config: Value = serde_json::from_slice(&config_body).map_err(|error| {
        AccountRefreshFailure::parse(format!(
            "CodeBuddy refresh config response is not valid JSON: {error}"
        ))
    })?;
    if let Some(code) = recursive_business_code(&config, 0) {
        if code != 0 {
            let message =
                string_at(&config, &["/msg", "/message", "/error/message"]).unwrap_or_default();
            return Err(codebuddy_refresh_rejection(
                config_status,
                Some(code),
                &message,
                &config_body,
            ));
        }
    }
    let enterprise_id = string_at_including_empty(
        &config,
        &[
            "/data/enterpriseId",
            "/data/enterprise_id",
            "/enterpriseId",
            "/enterprise_id",
        ],
    )
    .ok_or_else(|| {
        AccountRefreshFailure::parse(
            "CodeBuddy refresh config response is missing enterpriseId identity closure",
        )
    })?;
    Ok(CodeBuddyRefreshIdentity { uid, enterprise_id })
}

fn codebuddy_refresh_identity_failure(
    message: String,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    use crate::clients::oauth::refresh::AccountRefreshFailure;
    AccountRefreshFailure {
        status_code: StatusCode::CONFLICT.as_u16(),
        upstream_status: None,
        message,
        kind: OAuthErrorKind::InvalidGrant,
        retryable: false,
        retry_after_ms: None,
        immediate_relogin: true,
        outcome_unknown: true,
        endpoint_fallback_safe: false,
    }
}

fn codebuddy_refresh_client_failure(
    error: CodeBuddyClientError,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    AccountRefreshFailure {
        status_code: error.status.as_u16(),
        upstream_status: error.upstream_status.map(|status| status.as_u16()),
        message: error.message,
        kind: if error.terminal {
            OAuthErrorKind::ProviderRejected
        } else {
            OAuthErrorKind::Network
        },
        retryable: !error.terminal,
        retry_after_ms: None,
        immediate_relogin: false,
        outcome_unknown: false,
        endpoint_fallback_safe: false,
    }
}

fn codebuddy_refresh_rejection(
    status: StatusCode,
    business_code: Option<i64>,
    message: &str,
    body: &[u8],
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    let invalid_grant = business_code == Some(12_153)
        || message.to_ascii_lowercase().contains("invalid_grant")
        || message
            .to_ascii_lowercase()
            .contains("session doesn't have required client");
    let retryable = status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error();
    let kind = if invalid_grant {
        OAuthErrorKind::InvalidGrant
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        OAuthErrorKind::RateLimited
    } else if retryable {
        OAuthErrorKind::Network
    } else {
        OAuthErrorKind::ProviderRejected
    };
    let detail = if message.trim().is_empty() {
        public_error_body(body)
    } else {
        sanitized_message(message)
    };
    AccountRefreshFailure {
        status_code: if invalid_grant {
            StatusCode::UNAUTHORIZED.as_u16()
        } else if status.is_success() {
            StatusCode::BAD_GATEWAY.as_u16()
        } else {
            status.as_u16()
        },
        upstream_status: (!status.is_success()).then_some(status.as_u16()),
        message: format!("CodeBuddy token refresh rejected: {detail}"),
        kind,
        retryable,
        retry_after_ms: None,
        immediate_relogin: invalid_grant,
        outcome_unknown: false,
        endpoint_fallback_safe: false,
    }
}

fn recursive_business_code(value: &Value, depth: usize) -> Option<i64> {
    if depth > 6 {
        return None;
    }
    let own_code = i64_at(value, &["/code"]);
    if own_code.is_some_and(|code| code >= 1_000) {
        return own_code;
    }
    match value {
        Value::Object(object) => {
            for key in ["data", "error", "details", "message"] {
                let Some(child) = object.get(key) else {
                    continue;
                };
                if let Some(code) = recursive_business_code(child, depth + 1) {
                    return Some(code);
                }
                if let Some(text) = child.as_str() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        if let Some(code) = recursive_business_code(&parsed, depth + 1) {
                            return Some(code);
                        }
                    }
                }
            }
            own_code.filter(|code| *code == 0)
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| recursive_business_code(value, depth + 1)),
        _ => own_code.filter(|code| *code == 0),
    }
}

async fn poll_token(
    flow: &PendingCodeBuddyLoginFlow,
    now_ms: i64,
) -> Result<Option<CodeBuddyLoginToken>, CodeBuddyClientError> {
    let mut url = flow.endpoints.url(CODEBUDDY_AUTH_TOKEN_PATH)?;
    url.query_pairs_mut()
        .append_pair("state", &flow.upstream_state);
    let request = unauthenticated_request(flow.client.get(url));
    let (status, body) = execute_bounded(request, "auth token poll").await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(
            status,
            "auth token poll",
            &body,
        ));
    }
    let (code, message, data) = parse_envelope(&body, "auth token poll")?;
    if code == TOKEN_PENDING_CODE {
        return Ok(None);
    }
    if code != 0 {
        return Err(CodeBuddyClientError::business(
            "auth token poll",
            code,
            &message,
        ));
    }
    let token_data = data.pointer("/token").unwrap_or(&data);
    let access_token =
        string_at(token_data, &["/accessToken", "/access_token"]).ok_or_else(|| {
            CodeBuddyClientError::protocol("CodeBuddy token response is missing accessToken")
        })?;
    let refresh_token =
        string_at(token_data, &["/refreshToken", "/refresh_token"]).ok_or_else(|| {
            CodeBuddyClientError::protocol("CodeBuddy token response is missing refreshToken")
        })?;
    let expires_in = i64_at(token_data, &["/expiresIn", "/expires_in"]);
    let domain = string_at(token_data, &["/domain"])
        .unwrap_or_else(|| flow.endpoints.site.profile().domain.to_string());
    let domain = flow
        .endpoints
        .site
        .canonical_token_domain(&domain)
        .ok_or_else(|| {
            CodeBuddyClientError::protocol(format!(
                "CodeBuddy token domain is outside the selected {} site",
                flow.endpoints.site.as_str()
            ))
        })?;
    Ok(Some(CodeBuddyLoginToken {
        access_token,
        refresh_token,
        token_type: string_at(token_data, &["/tokenType", "/token_type"])
            .unwrap_or_else(|| "Bearer".to_string()),
        domain,
        scopes: scopes_at(token_data),
        expires_at_ms: expires_in
            .filter(|seconds| *seconds > 0)
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000))),
    }))
}

#[derive(Debug)]
struct CodeBuddyLoginIdentity {
    uid: String,
    name: String,
    email: String,
    nickname: String,
    account_type: String,
}

async fn poll_account(
    flow: &PendingCodeBuddyLoginFlow,
    token: &CodeBuddyLoginToken,
) -> Result<Option<CodeBuddyLoginIdentity>, CodeBuddyClientError> {
    let mut url = flow.endpoints.url(CODEBUDDY_LOGIN_ACCOUNT_PATH)?;
    url.query_pairs_mut()
        .append_pair("state", &flow.upstream_state);
    let request = unauthenticated_request(
        flow.client
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {}", token.access_token)),
    );
    let (status, body) = execute_bounded(request, "login account poll").await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(
            status,
            "login account poll",
            &body,
        ));
    }
    let (code, message, data) = parse_envelope(&body, "login account poll")?;
    if code == ACCOUNT_PENDING_CODE {
        return Ok(None);
    }
    if code != 0 {
        return Err(CodeBuddyClientError::business(
            "login account poll",
            code,
            &message,
        ));
    }
    let identity = identity_value(&data);
    let uid = string_at(identity, &["/uid", "/userId", "/user_id", "/sub"]).ok_or_else(|| {
        CodeBuddyClientError::protocol("CodeBuddy login account response is missing stable uid")
    })?;
    Ok(Some(CodeBuddyLoginIdentity {
        uid,
        name: string_at(identity, &["/name", "/userName", "/user_name"]).unwrap_or_default(),
        email: string_at(identity, &["/email"]).unwrap_or_default(),
        nickname: string_at(identity, &["/nickname", "/nickName"]).unwrap_or_default(),
        account_type: string_at(identity, &["/accountType", "/account_type", "/type"])
            .unwrap_or_default(),
    }))
}

async fn fetch_enterprise_identity(
    flow: &PendingCodeBuddyLoginFlow,
    token: &CodeBuddyLoginToken,
    uid: &str,
) -> Result<String, CodeBuddyClientError> {
    let url = flow.endpoints.url(CODEBUDDY_CONFIG_PATH)?;
    let request = authenticated_request(
        flow.client.get(url),
        &token.access_token,
        uid,
        None,
        &token.domain,
    );
    let (status, body) = execute_bounded(request, "config identity closure").await?;
    if !status.is_success() {
        return Err(CodeBuddyClientError::upstream(
            status,
            "config identity closure",
            &body,
        ));
    }
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        CodeBuddyClientError::protocol(format!(
            "CodeBuddy config response is not valid JSON: {error}"
        ))
    })?;
    if let Some(code) = i64_at(&value, &["/code"]) {
        if code != 0 {
            let message = string_at(&value, &["/msg", "/message"]).unwrap_or_default();
            return Err(CodeBuddyClientError::business(
                "config identity closure",
                code,
                &message,
            ));
        }
    }
    string_at_including_empty(
        &value,
        &[
            "/data/enterpriseId",
            "/data/enterprise_id",
            "/enterpriseId",
            "/enterprise_id",
        ],
    )
    .ok_or_else(|| {
        CodeBuddyClientError::protocol(
            "CodeBuddy config response is missing enterpriseId identity closure",
        )
    })
}

fn pending_result(
    message: &str,
    interval: u64,
    token: Option<CodeBuddyLoginToken>,
) -> CodeBuddyLoginPollResult {
    CodeBuddyLoginPollResult {
        pending: true,
        message: message.to_string(),
        retry_after_secs: Some(interval.max(1)),
        account_input: None,
        token,
    }
}

fn unauthenticated_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(ACCEPT, "application/json, text/plain, */*")
        .header(USER_AGENT, codebuddy_user_agent())
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-No-Authorization", "true")
        .header("X-No-User-Id", "true")
        .header("X-No-Enterprise-Id", "true")
        .header("X-No-Department-Info", "true")
}

pub(crate) fn authenticated_request(
    request: reqwest::RequestBuilder,
    access_token: &str,
    uid: &str,
    enterprise_id: Option<&str>,
    domain: &str,
) -> reqwest::RequestBuilder {
    let mut request = request
        .header(ACCEPT, "application/json, text/plain, */*")
        .header(USER_AGENT, codebuddy_user_agent())
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("X-User-Id", uid)
        .header("X-Domain", domain)
        .header("X-Product", "SaaS")
        .header("X-Request-ID", random_request_id())
        .header("X-Requested-With", "XMLHttpRequest");
    if let Some(enterprise_id) = enterprise_id.filter(|value| !value.trim().is_empty()) {
        request = request.header("X-Enterprise-Id", enterprise_id);
    } else {
        request = request.header("X-No-Enterprise-Id", "true");
    }
    request
}

async fn execute_bounded(
    request: reqwest::RequestBuilder,
    operation: &str,
) -> Result<(StatusCode, Vec<u8>), CodeBuddyClientError> {
    execute_bounded_with_timeout(request, operation, REQUEST_TIMEOUT).await
}

async fn execute_bounded_with_timeout(
    request: reqwest::RequestBuilder,
    operation: &str,
    timeout: Duration,
) -> Result<(StatusCode, Vec<u8>), CodeBuddyClientError> {
    let mut response = request.timeout(timeout).send().await.map_err(|error| {
        CodeBuddyClientError::bad_gateway(format!("CodeBuddy {operation} request failed: {error}"))
    })?;
    let status = response.status();
    let body =
        crate::infra::http::read_response_body_limited(&mut response, MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|error| match error {
                crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                    CodeBuddyClientError::protocol(format!(
                        "CodeBuddy {operation} response exceeds {MAX_RESPONSE_BODY_BYTES} bytes"
                    ))
                }
                crate::infra::http::BoundedResponseBodyError::Request(error) => {
                    CodeBuddyClientError::bad_gateway(format!(
                        "CodeBuddy {operation} response read failed: {error}"
                    ))
                }
            })?;
    Ok((status, body.to_vec()))
}

fn parse_envelope(
    body: &[u8],
    operation: &str,
) -> Result<(i64, String, Value), CodeBuddyClientError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        CodeBuddyClientError::protocol(format!(
            "CodeBuddy {operation} response is not valid JSON: {error}"
        ))
    })?;
    let code = i64_at(&value, &["/code"]).ok_or_else(|| {
        CodeBuddyClientError::protocol(format!(
            "CodeBuddy {operation} response is missing business code"
        ))
    })?;
    let message = string_at(&value, &["/msg", "/message", "/error/message"]).unwrap_or_default();
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    Ok((code, message, data))
}

fn identity_value(data: &Value) -> &Value {
    if string_at(data, &["/uid", "/userId", "/user_id", "/sub"]).is_some() {
        return data;
    }
    for pointer in ["/account", "/user", "/accounts/0", "/users/0"] {
        if let Some(value) = data.pointer(pointer) {
            if string_at(value, &["/uid", "/userId", "/user_id", "/sub"]).is_some() {
                return value;
            }
        }
    }
    data
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

fn scopes_at(value: &Value) -> Vec<String> {
    match value.get("scope").or_else(|| value.get("scopes")) {
        Some(Value::String(scope)) => scope
            .split(|character: char| character.is_ascii_whitespace() || character == ',')
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::Array(scopes)) => scopes
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn scopes_option_at(value: &Value) -> Option<Vec<String>> {
    value
        .get("scope")
        .or_else(|| value.get("scopes"))
        .map(|_| scopes_at(value))
}

fn validate_auth_url(
    auth_url: &str,
    endpoints: &CodeBuddyEndpoints,
) -> Result<(), CodeBuddyClientError> {
    let parsed = Url::parse(auth_url).map_err(|error| {
        CodeBuddyClientError::protocol(format!("CodeBuddy authUrl is invalid: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CodeBuddyClientError::protocol(
            "CodeBuddy authUrl must use HTTP(S)",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CodeBuddyClientError::protocol(
            "CodeBuddy authUrl must not contain URL credentials",
        ));
    }
    if endpoints.base_url.scheme() == "https" && parsed.scheme() != "https" {
        return Err(CodeBuddyClientError::protocol(
            "CodeBuddy production authUrl must use HTTPS",
        ));
    }
    let auth_host = parsed
        .host_str()
        .ok_or_else(|| CodeBuddyClientError::protocol("CodeBuddy authUrl is missing a host"))?;
    if endpoints.base_url.scheme() == "https" {
        if parsed.port_or_known_default() != Some(443)
            || !endpoints.site.allows_browser_auth_host(auth_host)
        {
            return Err(CodeBuddyClientError::protocol(format!(
                "CodeBuddy authUrl host is outside the selected {} site allowlist",
                endpoints.site.as_str()
            )));
        }
    } else if parsed.scheme() != endpoints.base_url.scheme()
        || parsed.host_str() != endpoints.base_url.host_str()
        || parsed.port_or_known_default() != endpoints.base_url.port_or_known_default()
    {
        return Err(CodeBuddyClientError::protocol(
            "CodeBuddy test authUrl must remain on the configured test origin",
        ));
    }
    Ok(())
}

fn random_flow_id() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_request_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn codebuddy_user_agent() -> String {
    format!("CLI/{CODEBUDDY_CLIENT_VERSION} CodeBuddy/{CODEBUDDY_CLIENT_VERSION}")
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::store::AccountStore;
    use axum::body::Body;
    use axum::http::{HeaderMap, Response};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct Observation {
        path: String,
        headers: HeaderMap,
    }

    async fn serve_login_fixture() -> (
        String,
        Arc<Mutex<Vec<Observation>>>,
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let base_url = format!("http://{address}");
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_route = Arc::clone(&observations);
        let account_polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let account_polls_for_route = Arc::clone(&account_polls);
        let token_polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_polls_for_route = Arc::clone(&token_polls);
        let auth_url = format!("{base_url}/browser/authorize");
        let app = axum::Router::new().fallback(axum::routing::any(
            move |uri: axum::http::Uri, headers: HeaderMap| {
                let observations = Arc::clone(&observations_for_route);
                let account_polls = Arc::clone(&account_polls_for_route);
                let token_polls = Arc::clone(&token_polls_for_route);
                let auth_url = auth_url.clone();
                async move {
                    observations.lock().unwrap().push(Observation {
                        path: uri.path().to_string(),
                        headers: headers.clone(),
                    });
                    let json_response = |value: Value| {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "application/json")
                            .body(Body::from(value.to_string()))
                            .unwrap()
                    };
                    match uri.path() {
                        "/v2/plugin/auth/state" => Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "application/json")
                            .header(
                                "set-cookie",
                                "codebuddy_flow=cookie-bound; Path=/; HttpOnly",
                            )
                            .body(Body::from(
                                json!({
                                    "code": 0,
                                    "data": {"state": "upstream-state", "authUrl": auth_url}
                                })
                                .to_string(),
                            ))
                            .unwrap(),
                        "/v2/plugin/auth/token" => {
                            token_polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            json_response(json!({
                                "code": 0,
                                "data": {
                                    "accessToken": "access-secret",
                                    "refreshToken": "refresh-secret",
                                    "expiresIn": 3600,
                                    "tokenType": "Bearer",
                                    "domain": "www.codebuddy.ai",
                                    "scope": "chat models"
                                }
                            }))
                        }
                        "/v2/plugin/login/account" => {
                            let attempt =
                                account_polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            if attempt == 0 {
                                json_response(json!({"code": ACCOUNT_PENDING_CODE, "msg": "wait"}))
                            } else {
                                json_response(json!({
                                    "code": 0,
                                    "data": {
                                        "uid": "uid-1",
                                        "email": "user@example.com",
                                        "nickname": "Fixture",
                                        "type": "personal"
                                    }
                                }))
                            }
                        }
                        "/v3/config" => json_response(json!({
                            "data": {"enterpriseId": "", "models": []}
                        })),
                        _ => Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::empty())
                            .unwrap(),
                    }
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base_url, observations, token_polls, server)
    }

    #[test]
    fn flow_store_serializes_poll_and_expires_cookie_client() {
        let endpoints = CodeBuddyEndpoints::for_site(CodeBuddySite::Intl).unwrap();
        let flow = PendingCodeBuddyLoginFlow {
            expires_at_ms: 10,
            interval: 2,
            upstream_state: "state".to_string(),
            endpoints,
            client: reqwest::Client::new(),
            token: None,
        };
        let mut store = CodeBuddyLoginFlowStore::default();
        store.insert("flow".to_string(), flow, 0);
        assert!(matches!(
            store.begin_poll("flow", 0),
            Some(CodeBuddyLoginPollLease::Ready(_))
        ));
        assert!(matches!(
            store.begin_poll("flow", 0),
            Some(CodeBuddyLoginPollLease::InProgress)
        ));
        store.fail_poll("flow", false, 1);
        assert!(matches!(
            store.begin_poll("flow", 1),
            Some(CodeBuddyLoginPollLease::Wait(2))
        ));
        assert!(store.begin_poll("flow", 10).is_none());
    }

    #[tokio::test]
    async fn login_reuses_cookie_jar_preserves_token_phase_and_closes_enterprise_identity() {
        let (base_url, observations, token_polls, server) = serve_login_fixture().await;
        let endpoints = CodeBuddyEndpoints::for_test(CodeBuddySite::Intl, &base_url);
        let (start, flow) = start_login_with_endpoints(endpoints, 1_000).await.unwrap();
        assert_ne!(start.flow_id, flow.upstream_state);
        assert_eq!(start.expires_at, 601_000);

        let mut store = CodeBuddyLoginFlowStore::default();
        store.insert(start.flow_id.clone(), flow, 1_000);
        let CodeBuddyLoginPollLease::Ready(flow) = store.begin_poll(&start.flow_id, 1_000).unwrap()
        else {
            panic!("first poll must acquire the flow")
        };
        let pending = poll_login(&flow, 1_000).await.unwrap();
        assert!(pending.pending);
        assert_eq!(pending.message, "account_pending");
        assert!(store.finish_poll(&start.flow_id, pending, 1_001));

        let CodeBuddyLoginPollLease::Ready(flow) = store.begin_poll(&start.flow_id, 3_001).unwrap()
        else {
            panic!("second poll must acquire the flow")
        };
        let completed = poll_login(&flow, 3_001).await.unwrap();
        assert!(!completed.pending);
        let input = completed.account_input.as_ref().unwrap();
        assert_eq!(input.provider_type, ProviderType::CodeBuddyOAuth);
        assert_eq!(input.profile.as_ref().unwrap()["enterpriseId"], "");
        assert_eq!(input.profile.as_ref().unwrap()["site"], "intl");
        assert_eq!(input.access_token.as_deref(), Some("access-secret"));
        assert_eq!(
            token_polls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "account-pending must not cause a second token exchange"
        );

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 5);
        for observation in observations.iter().take(4) {
            for header in [
                "x-no-authorization",
                "x-no-user-id",
                "x-no-enterprise-id",
                "x-no-department-info",
            ] {
                assert_eq!(observation.headers[header], "true");
            }
        }
        for observation in observations.iter().skip(1) {
            assert_eq!(
                observation.headers["cookie"], "codebuddy_flow=cookie-bound",
                "{} must use the per-flow cookie jar",
                observation.path
            );
        }
        let config = observations.last().unwrap();
        assert_eq!(config.path, "/v3/config");
        assert_eq!(config.headers["authorization"], "Bearer access-secret");
        assert_eq!(config.headers["x-user-id"], "uid-1");
        assert_eq!(config.headers["x-domain"], "www.codebuddy.ai");
        server.abort();
    }

    #[tokio::test]
    async fn config_must_explicitly_close_enterprise_identity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(axum::routing::any(
            move |uri: axum::http::Uri| async move {
                let value = match uri.path() {
                    "/v2/plugin/auth/state" => json!({
                        "code": 0,
                        "data": {
                            "state": "state",
                            "authUrl": format!("http://{address}/authorize")
                        }
                    }),
                    "/v2/plugin/auth/token" => json!({
                        "code": 0,
                        "data": {"accessToken": "a", "refreshToken": "r"}
                    }),
                    "/v2/plugin/login/account" => {
                        json!({"code": 0, "data": {"uid": "uid"}})
                    }
                    "/v3/config" => json!({"data": {"models": []}}),
                    _ => json!({"code": 404}),
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(value.to_string()))
                    .unwrap()
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let endpoints =
            CodeBuddyEndpoints::for_test(CodeBuddySite::Intl, &format!("http://{address}"));
        let (_, flow) = start_login_with_endpoints(endpoints, 0).await.unwrap();
        let error = poll_login(&flow, 1).await.unwrap_err();
        assert!(error.terminal);
        assert!(error.message.contains("missing enterpriseId"));
        server.abort();
    }

    fn jwt_for_subject(subject: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"sub": subject})).unwrap());
        format!("header.{payload}.signature")
    }

    fn refresh_account(base_url: &str, uid: &str) -> Account {
        AccountStore::default().upsert(UpsertAccountInput {
            id: Some("codebuddy-refresh-account".to_string()),
            provider_type: ProviderType::CodeBuddyOAuth,
            email: Some("user@example.com".to_string()),
            access_token: Some(jwt_for_subject(uid)),
            refresh_token: Some("refresh-old".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: vec!["old-scope".to_string()],
            profile: Some(json!({
                "site": "intl",
                "domain": "www.codebuddy.ai",
                "uid": uid,
                "enterpriseId": "enterprise-1",
                "name": "Fixture",
                "email": "user@example.com",
                "clientVersion": CODEBUDDY_CLIENT_VERSION,
                "productPlatform": CODEBUDDY_PLATFORM
            })),
            raw: Some(json!({
                "source": "fixture",
                "testCodeBuddyBaseUrl": base_url
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

    async fn serve_refresh_fixture(
        status: StatusCode,
        response: Value,
        refreshed_uid: &str,
        enterprise_id: &str,
    ) -> (
        String,
        Arc<Mutex<Vec<Observation>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_route = Arc::clone(&observations);
        let refreshed_uid = refreshed_uid.to_string();
        let enterprise_id = enterprise_id.to_string();
        let app = axum::Router::new().fallback(axum::routing::any(
            move |uri: axum::http::Uri, headers: HeaderMap| {
                let observations = Arc::clone(&observations_for_route);
                let response = response.clone();
                let refreshed_uid = refreshed_uid.clone();
                let enterprise_id = enterprise_id.clone();
                async move {
                    observations.lock().unwrap().push(Observation {
                        path: uri.path().to_string(),
                        headers,
                    });
                    let (status, response) = match uri.path() {
                        CODEBUDDY_REFRESH_PATH => (status, response),
                        CODEBUDDY_ACCOUNTS_PATH => (
                            StatusCode::OK,
                            json!({"code": 0, "data": {"uid": refreshed_uid}}),
                        ),
                        CODEBUDDY_CONFIG_PATH => (
                            StatusCode::OK,
                            json!({"code": 0, "data": {"enterpriseId": enterprise_id, "models": []}}),
                        ),
                        _ => (StatusCode::NOT_FOUND, json!({"code": 404})),
                    };
                    Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(Body::from(response.to_string()))
                        .unwrap()
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), observations, server)
    }

    #[tokio::test]
    async fn refresh_sends_complete_identity_and_journals_rotated_token_before_validation() {
        let refreshed_access = jwt_for_subject("uid-1");
        let (base_url, observations, server) = serve_refresh_fixture(
            StatusCode::OK,
            json!({
                "accessToken": refreshed_access,
                "refreshToken": "refresh-rotated",
                "expiresIn": 3600,
                "tokenType": "Bearer",
                "scope": "chat models",
                "domain": "www.codebuddy.ai"
            }),
            "uid-1",
            "enterprise-1",
        )
        .await;
        let account = refresh_account(&base_url, "uid-1");
        let journaled = Arc::new(Mutex::new(None::<AccountRefreshUpdate>));
        let journaled_for_hook = Arc::clone(&journaled);
        let mut hook = move |receipt: &AccountRefreshUpdate| {
            *journaled_for_hook.lock().unwrap() = Some(receipt.clone());
            Ok(())
        };
        let update = refresh_codebuddy_account(&reqwest::Client::new(), &account, 1_000, &mut hook)
            .await
            .unwrap();
        assert_eq!(update.refresh_token.as_deref(), Some("refresh-rotated"));
        assert_eq!(update.expires_at, Some(3_601_000));
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
        assert_eq!(
            update.raw.as_ref().unwrap()["testCodeBuddyBaseUrl"],
            base_url
        );

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 3);
        let request = &observations[0];
        assert_eq!(request.path, CODEBUDDY_REFRESH_PATH);
        assert!(request.headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer header."));
        assert_eq!(request.headers["x-refresh-token"], "refresh-old");
        assert_eq!(request.headers["x-auth-refresh-source"], "plugin");
        assert_eq!(request.headers["x-user-id"], "uid-1");
        assert_eq!(request.headers["x-enterprise-id"], "enterprise-1");
        assert_eq!(request.headers["x-domain"], "www.codebuddy.ai");
        assert_eq!(request.headers["x-product"], "SaaS");
        assert_eq!(observations[1].path, CODEBUDDY_ACCOUNTS_PATH);
        assert_eq!(observations[2].path, CODEBUDDY_CONFIG_PATH);
        assert_eq!(observations[2].headers["x-enterprise-id"], "enterprise-1");
        server.abort();
    }

    #[test]
    fn billing_endpoint_override_is_independent_from_config_and_chat() {
        let mut account = refresh_account("http://127.0.0.1:3210", "uid-1");
        account.raw.as_mut().unwrap()["testCodeBuddyBillingBaseUrl"] =
            json!("http://127.0.0.1:6543");
        let runtime = CodeBuddyEndpoints::for_account(&account, CodeBuddySite::Intl).unwrap();
        let billing =
            CodeBuddyBillingEndpoints::for_account(&account, CodeBuddySite::Intl).unwrap();
        assert_eq!(runtime.base_url.as_str(), "http://127.0.0.1:3210/");
        assert_eq!(billing.base_url.as_str(), "http://127.0.0.1:6543/");

        account
            .raw
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("testCodeBuddyBillingBaseUrl");
        let billing =
            CodeBuddyBillingEndpoints::for_account(&account, CodeBuddySite::Intl).unwrap();
        assert_eq!(
            billing.base_url.as_str(),
            CodeBuddySite::Intl.profile().billing_endpoint.to_string() + "/"
        );
        assert_ne!(
            CodeBuddySite::Cn.profile().endpoint,
            CodeBuddySite::Cn.profile().billing_endpoint
        );
    }

    #[tokio::test]
    async fn refresh_identity_mismatch_is_rejected_after_receipt_is_journaled() {
        let (base_url, _, server) = serve_refresh_fixture(
            StatusCode::OK,
            json!({
                "code": 0,
                "data": {
                    "accessToken": jwt_for_subject("uid-other"),
                    "refreshToken": "refresh-rotated"
                }
            }),
            "uid-other",
            "enterprise-1",
        )
        .await;
        let account = refresh_account(&base_url, "uid-1");
        let journaled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let journaled_for_hook = Arc::clone(&journaled);
        let mut hook = move |_: &AccountRefreshUpdate| {
            journaled_for_hook.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        };
        let error = refresh_codebuddy_account(&reqwest::Client::new(), &account, 1_000, &mut hook)
            .await
            .unwrap_err();
        assert!(journaled.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(error.status_code, StatusCode::CONFLICT.as_u16());
        assert!(error.immediate_relogin);
        assert!(error.outcome_unknown);
        server.abort();
    }

    #[tokio::test]
    async fn only_session_dead_invalid_grant_marks_refresh_as_relogin() {
        let (base_url, _, server) = serve_refresh_fixture(
            StatusCode::BAD_REQUEST,
            json!({
                "code": 12153,
                "message": "invalid_grant: Session doesn't have required client"
            }),
            "uid-1",
            "enterprise-1",
        )
        .await;
        let account = refresh_account(&base_url, "uid-1");
        let mut hook = |_: &AccountRefreshUpdate| Ok(());
        let error = refresh_codebuddy_account(&reqwest::Client::new(), &account, 1_000, &mut hook)
            .await
            .unwrap_err();
        assert_eq!(error.kind, OAuthErrorKind::InvalidGrant);
        assert!(error.immediate_relogin);
        assert!(!error.retryable);
        server.abort();
    }

    #[test]
    fn nested_nonzero_business_code_is_not_hidden_by_root_success_code() {
        assert_eq!(
            recursive_business_code(&json!({"code": 0, "data": {"error": {"code": 12153}}}), 0),
            Some(12153)
        );
        assert_eq!(recursive_business_code(&json!({"code": 0}), 0), Some(0));
    }

    #[test]
    fn production_auth_url_is_site_scoped_and_test_url_is_same_origin() {
        let intl = CodeBuddyEndpoints::for_site(CodeBuddySite::Intl).unwrap();
        assert!(validate_auth_url("https://www.codebuddy.ai/login", &intl).is_ok());
        assert!(validate_auth_url("https://www.workbuddy.ai/login", &intl).is_ok());
        assert!(validate_auth_url("https://www.codebuddy.cn/login", &intl).is_err());
        assert!(validate_auth_url("https://www.codebuddy.ai.attacker.test/login", &intl).is_err());
        let test = CodeBuddyEndpoints::for_test(CodeBuddySite::Intl, "http://127.0.0.1:3210");
        assert!(validate_auth_url("http://127.0.0.1:3210/login", &test).is_ok());
        assert!(validate_auth_url("http://127.0.0.1:3211/login", &test).is_err());
    }
}
