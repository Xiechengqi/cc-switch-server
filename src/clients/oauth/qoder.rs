use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::header::{
    HeaderMap, ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::accounts::store::{Account, AccountRefreshUpdate};
use crate::domain::providers::model::ProviderType;
use crate::domain::qoder::{
    machine_token_from_raw, qoder_account_id, qoder_decode, qoder_encode, random_qoder_hex,
    random_qoder_machine, random_qoder_token, QoderAccountProfile, QoderCosySession,
    QoderCredentialRail, QoderIdentity, QoderMachineIdentity, QoderSite, QoderSiteProfile,
    QODER_CN_CLIENT_IP_ENV, QODER_MODEL_LIST_PATH, QODER_MODEL_LIST_SIGNATURE_PATH,
    QODER_QUOTA_PATH, QODER_QUOTA_SIGNATURE_PATH, QODER_REFRESH_MODE_COSY,
    QODER_REFRESH_MODE_QODER_CN20,
};

pub(crate) const DEVICE_POLL_PATH: &str = "/api/v1/deviceToken/poll";
pub(crate) const DEVICE_REFRESH_PATH: &str = "/api/v1/deviceToken/refresh";
pub(crate) const USER_INFO_PATH: &str = "/api/v1/userinfo";
const ORGANIZATION_TAGS_PREFIX: &str = "/api/v1/organizations/";
pub(crate) const AUTH_STATUS_ACTUAL_PATH: &str = "/algo/api/v3/user/status";
const QODER_APP_CODE: &str = "cosy";
const QODER_APP_SECRET: &str = "d2FyLCB3YXIgbmV2ZXIgY2hhbmdlcw==";
pub const QODER_CLI_VERSION: &str = "1.1.32";
pub(crate) const QODER_OPENAPI_USER_AGENT: &str = "qoder/1.1.32";
pub(crate) const DEFAULT_POLL_INTERVAL_SECS: u64 = 1;
pub(crate) const FLOW_TTL_SECS: u64 = 5 * 60;
const MAX_QODER_DEVICE_FLOWS: usize = 64;
const COMPLETED_QODER_DEVICE_FLOW_TTL_MS: i64 = 60 * 1_000;
const MAX_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct QoderEndpoints {
    pub site: QoderSite,
    pub device_authorization_url: String,
    pub openapi_base_url: String,
    pub center_base_url: Option<String>,
    pub gateway_base_url: String,
    pub job_gateway_base_url: String,
    pub client_version: String,
    pub oauth_client_id: String,
    pub client_type: String,
}

impl QoderEndpoints {
    pub fn for_site(site: QoderSite) -> Self {
        Self::from_profile(site.profile())
    }

    fn from_profile(profile: QoderSiteProfile) -> Self {
        Self {
            site: profile.site,
            device_authorization_url: profile.device_authorization_url.to_string(),
            openapi_base_url: profile.openapi_base_url.to_string(),
            center_base_url: profile.center_base_url.map(str::to_string),
            gateway_base_url: profile.gateway_base_url.to_string(),
            job_gateway_base_url: profile.job_gateway_base_url.to_string(),
            client_version: profile.client_version.to_string(),
            oauth_client_id: profile.oauth_client_id.to_string(),
            client_type: profile.client_type.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_account(account: &Account, site: QoderSite) -> Self {
        let mut endpoints = Self::for_site(site);
        let Some(overrides) = account
            .raw
            .as_ref()
            .and_then(|raw| raw.get("testQoderEndpoints"))
        else {
            return endpoints;
        };
        for (field, target) in [
            ("openapiBaseUrl", &mut endpoints.openapi_base_url),
            ("gatewayBaseUrl", &mut endpoints.gateway_base_url),
            ("jobGatewayBaseUrl", &mut endpoints.job_gateway_base_url),
        ] {
            if let Some(value) = overrides
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                *target = value.trim_end_matches('/').to_string();
            }
        }
        if let Some(value) = overrides
            .get("centerBaseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            endpoints.center_base_url = Some(value.trim_end_matches('/').to_string());
        }
        endpoints
    }

    #[cfg(not(test))]
    pub(crate) fn from_account(_account: &Account, site: QoderSite) -> Self {
        Self::for_site(site)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderDeviceCodeResponse {
    pub device_code: String,
    pub state: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
    pub site: QoderSite,
}

#[derive(Debug, Clone)]
pub struct PendingQoderDeviceFlow {
    pub expires_at_ms: i64,
    pub interval: u64,
    pub state: String,
    pub nonce: String,
    pub code_verifier: String,
    pub machine: QoderMachineIdentity,
    pub endpoints: QoderEndpoints,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
struct QoderDeviceFlowEntry {
    flow: PendingQoderDeviceFlow,
    created_at_ms: i64,
    state: QoderDeviceFlowState,
}

#[derive(Debug, Clone)]
enum QoderDeviceFlowState {
    Pending {
        next_poll_at_ms: i64,
    },
    Polling,
    Completed {
        result: Box<QoderDevicePollResult>,
        expires_at_ms: i64,
    },
}

#[derive(Debug, Clone)]
pub enum QoderDevicePollLease {
    Ready(Box<PendingQoderDeviceFlow>),
    Wait(u64),
    InProgress,
    Completed(Box<QoderDevicePollResult>),
}

#[derive(Debug, Clone, Default)]
pub struct QoderDeviceFlowStore {
    pending: BTreeMap<String, QoderDeviceFlowEntry>,
}

impl QoderDeviceFlowStore {
    pub fn insert(
        &mut self,
        device_code: String,
        flow: PendingQoderDeviceFlow,
        now_ms: i64,
    ) -> Vec<String> {
        self.cleanup(now_ms);
        let mut evicted = Vec::new();
        while self.pending.len() >= MAX_QODER_DEVICE_FLOWS {
            let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, entry)| entry.created_at_ms)
                .map(|(code, _)| code.clone())
            else {
                break;
            };
            self.pending.remove(&oldest);
            evicted.push(oldest);
        }
        self.pending.insert(
            device_code,
            QoderDeviceFlowEntry {
                flow,
                created_at_ms: now_ms,
                state: QoderDeviceFlowState::Pending {
                    next_poll_at_ms: now_ms,
                },
            },
        );
        evicted
    }

    pub fn begin_poll(&mut self, device_code: &str, now_ms: i64) -> Option<QoderDevicePollLease> {
        self.cleanup(now_ms);
        let entry = self.pending.get_mut(device_code)?;
        match &entry.state {
            QoderDeviceFlowState::Pending { next_poll_at_ms } if now_ms < *next_poll_at_ms => {
                Some(QoderDevicePollLease::Wait(
                    u64::try_from(next_poll_at_ms.saturating_sub(now_ms))
                        .unwrap_or(u64::MAX)
                        .saturating_add(999)
                        / 1_000,
                ))
            }
            QoderDeviceFlowState::Pending { .. } => {
                entry.state = QoderDeviceFlowState::Polling;
                Some(QoderDevicePollLease::Ready(Box::new(entry.flow.clone())))
            }
            QoderDeviceFlowState::Polling => Some(QoderDevicePollLease::InProgress),
            QoderDeviceFlowState::Completed { result, .. } => {
                Some(QoderDevicePollLease::Completed(result.clone()))
            }
        }
    }

    pub fn state_matches(&mut self, device_code: &str, state: &str, now_ms: i64) -> bool {
        self.cleanup(now_ms);
        let state = state.trim();
        !state.is_empty()
            && self
                .pending
                .get(device_code)
                .is_some_and(|entry| entry.flow.state == state)
    }

    pub fn finish_poll(
        &mut self,
        device_code: &str,
        result: QoderDevicePollResult,
        now_ms: i64,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(device_code) else {
            return false;
        };
        if !matches!(entry.state, QoderDeviceFlowState::Polling) {
            return false;
        }
        entry.state = if result.pending {
            let delay = result
                .retry_after_secs
                .unwrap_or(entry.flow.interval)
                .max(1);
            QoderDeviceFlowState::Pending {
                next_poll_at_ms: now_ms.saturating_add((delay as i64).saturating_mul(1_000)),
            }
        } else {
            QoderDeviceFlowState::Completed {
                result: Box::new(result),
                expires_at_ms: now_ms.saturating_add(COMPLETED_QODER_DEVICE_FLOW_TTL_MS),
            }
        };
        true
    }

    pub fn fail_poll(&mut self, device_code: &str, terminal: bool, now_ms: i64) {
        if terminal {
            self.pending.remove(device_code);
        } else if let Some(entry) = self.pending.get_mut(device_code) {
            entry.state = QoderDeviceFlowState::Pending {
                next_poll_at_ms: now_ms
                    .saturating_add((entry.flow.interval as i64).saturating_mul(1_000)),
            };
        }
    }

    pub fn cancel(&mut self, device_code: &str) -> bool {
        self.pending.remove(device_code).is_some()
    }

    pub fn active_codes(&mut self, now_ms: i64) -> Vec<String> {
        self.cleanup(now_ms);
        self.pending.keys().cloned().collect()
    }

    fn cleanup(&mut self, now_ms: i64) {
        self.pending.retain(|_, entry| match &entry.state {
            QoderDeviceFlowState::Completed { expires_at_ms, .. } => *expires_at_ms > now_ms,
            _ => entry.flow.expires_at_ms > now_ms,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QoderErrorKind {
    InvalidGrant,
    Authentication,
    Permission,
    RateLimited,
    Temporary,
    Protocol,
}

impl QoderErrorKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidGrant => "invalid_grant",
            Self::Authentication => "authentication",
            Self::Permission => "permission",
            Self::RateLimited => "rate_limited",
            Self::Temporary => "temporary",
            Self::Protocol => "protocol",
        }
    }
}

#[derive(Debug, Clone)]
pub struct QoderClientError {
    pub status: StatusCode,
    pub upstream_status: Option<StatusCode>,
    pub kind: QoderErrorKind,
    pub terminal: bool,
    pub outcome_unknown: bool,
    pub retry_after_ms: Option<i64>,
    pub message: String,
}

impl QoderClientError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            upstream_status: None,
            kind: QoderErrorKind::Protocol,
            terminal: true,
            outcome_unknown: false,
            retry_after_ms: None,
            message: message.into(),
        }
    }

    fn upstream(
        status: StatusCode,
        headers: Option<&HeaderMap>,
        operation: &str,
        body: &[u8],
    ) -> Self {
        let kind = classify_qoder_upstream_error(status, body);
        let terminal = matches!(
            kind,
            QoderErrorKind::InvalidGrant
                | QoderErrorKind::Authentication
                | QoderErrorKind::Permission
                | QoderErrorKind::Protocol
        );
        let downstream_status = match kind {
            QoderErrorKind::InvalidGrant | QoderErrorKind::Authentication => {
                StatusCode::UNAUTHORIZED
            }
            QoderErrorKind::Permission => StatusCode::FORBIDDEN,
            QoderErrorKind::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            QoderErrorKind::Temporary | QoderErrorKind::Protocol => StatusCode::BAD_GATEWAY,
        };
        let error = Self {
            status: downstream_status,
            upstream_status: Some(status),
            kind,
            terminal,
            outcome_unknown: false,
            retry_after_ms: headers.and_then(parse_retry_after_ms),
            message: format!(
                "Qoder {operation} failed with upstream HTTP {}: {}",
                status.as_u16(),
                public_error_body(body)
            ),
        };
        crate::metrics::record_qoder_error(error.kind.as_str());
        error
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        crate::metrics::record_qoder_error("protocol");
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            kind: QoderErrorKind::Protocol,
            terminal: false,
            outcome_unknown: false,
            retry_after_ms: None,
            message: message.into(),
        }
    }

    fn temporary(message: impl Into<String>) -> Self {
        crate::metrics::record_qoder_error("temporary");
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            kind: QoderErrorKind::Temporary,
            terminal: false,
            outcome_unknown: false,
            retry_after_ms: None,
            message: message.into(),
        }
    }

    fn refresh_outcome_unknown(message: impl Into<String>) -> Self {
        crate::metrics::record_qoder_error("outcome_unknown");
        Self {
            status: StatusCode::BAD_GATEWAY,
            upstream_status: None,
            kind: QoderErrorKind::Temporary,
            terminal: false,
            outcome_unknown: true,
            retry_after_ms: None,
            message: message.into(),
        }
    }

    fn mark_outcome_unknown(mut self) -> Self {
        self.outcome_unknown = true;
        self
    }
}

impl fmt::Display for QoderClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for QoderClientError {}

pub fn start_device_flow(
    site: QoderSite,
    now_ms: i64,
) -> Result<(QoderDeviceCodeResponse, PendingQoderDeviceFlow), QoderClientError> {
    start_device_flow_with_endpoints(QoderEndpoints::for_site(site), now_ms)
}

fn start_device_flow_with_endpoints(
    endpoints: QoderEndpoints,
    now_ms: i64,
) -> Result<(QoderDeviceCodeResponse, PendingQoderDeviceFlow), QoderClientError> {
    let code_verifier = random_qoder_token(32);
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    let nonce = crate::domain::qoder::random_qoder_uuid();
    let state = random_qoder_token(32);
    let device_code = random_qoder_hex(16);
    let machine = random_qoder_machine(endpoints.site);
    let mut authorization_url =
        Url::parse(&endpoints.device_authorization_url).map_err(|error| {
            QoderClientError::bad_gateway(format!("invalid Qoder auth URL: {error}"))
        })?;
    authorization_url.query_pairs_mut().extend_pairs([
        ("nonce", nonce.as_str()),
        ("challenge", code_challenge.as_str()),
        ("challenge_method", "S256"),
        ("client_id", endpoints.oauth_client_id.as_str()),
        ("machine_id", machine.machine_id.as_str()),
    ]);
    let expires_at_ms = now_ms.saturating_add((FLOW_TTL_SECS as i64).saturating_mul(1_000));
    let flow = PendingQoderDeviceFlow {
        expires_at_ms,
        interval: DEFAULT_POLL_INTERVAL_SECS,
        state: state.clone(),
        nonce,
        code_verifier,
        machine,
        endpoints: endpoints.clone(),
    };
    Ok((
        QoderDeviceCodeResponse {
            device_code,
            state,
            verification_uri: endpoints.device_authorization_url,
            verification_uri_complete: authorization_url.to_string(),
            expires_in: FLOW_TTL_SECS,
            interval: DEFAULT_POLL_INTERVAL_SECS,
            site: endpoints.site,
        },
        flow,
    ))
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    flow: &PendingQoderDeviceFlow,
    supplied_state: &str,
    now_ms: i64,
) -> Result<QoderDevicePollResult, QoderClientError> {
    if flow.expires_at_ms <= now_ms {
        return Err(QoderClientError::bad_request(
            "Qoder device flow expired; restart login",
        ));
    }
    if supplied_state.trim().is_empty() || supplied_state.trim() != flow.state {
        return Err(QoderClientError::bad_request(
            "Qoder device flow state does not match",
        ));
    }
    let poll_url = qoder_device_poll_url(flow)?;
    let mut response = http
        .get(poll_url)
        .header(ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder device poll failed: {error}"))
        })?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(pending_result(flow.interval));
    }
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "device poll").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "device poll",
            &body,
        ));
    }
    let token = parse_token_response(&body, now_ms)?;
    let Some(access_token) = token.access_token() else {
        return Ok(pending_result(flow.interval));
    };
    let identity =
        complete_identity(http, &flow.endpoints, &flow.machine, &token, access_token).await?;
    let rail = if flow.endpoints.site == QoderSite::Cn {
        QoderCredentialRail::CnOauth
    } else {
        QoderCredentialRail::GlobalOauth
    };
    let refresh_token = token.refresh_token.trim();
    if refresh_token.is_empty() {
        return Err(QoderClientError::bad_request(
            "Qoder OAuth response is missing refresh_token",
        ));
    }
    let account_input = account_input(QoderAccountDraft {
        endpoints: &flow.endpoints,
        rail,
        machine: &flow.machine,
        identity,
        access_token: Some(access_token.to_string()),
        refresh_token: Some(refresh_token.to_string()),
        api_key: None,
        expires_at: token.expires_at_ms,
        login_method: "device",
        now_ms,
    })?;
    Ok(QoderDevicePollResult {
        pending: false,
        message: "Qoder device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

pub async fn import_pat(
    http: &reqwest::Client,
    pat: &str,
    now_ms: i64,
) -> Result<UpsertAccountInput, QoderClientError> {
    import_pat_with_endpoints(
        http,
        pat,
        QoderEndpoints::for_site(QoderSite::Global),
        now_ms,
    )
    .await
}

async fn import_pat_with_endpoints(
    http: &reqwest::Client,
    pat: &str,
    endpoints: QoderEndpoints,
    now_ms: i64,
) -> Result<UpsertAccountInput, QoderClientError> {
    let pat = pat.trim();
    if endpoints.site != QoderSite::Global || !pat.starts_with("pt-") {
        return Err(QoderClientError::bad_request(
            "Qoder PAT import requires a Global pt-* personal token",
        ));
    }
    let exchanged = exchange_pat_job_token(http, &endpoints, pat, now_ms).await?;
    let access_token = exchanged.access_token().ok_or_else(|| {
        QoderClientError::bad_gateway("Qoder PAT exchange response is missing job token")
    })?;
    let user = fetch_user_info(http, &endpoints, access_token).await?;
    let uid = user.uid().ok_or_else(|| {
        QoderClientError::bad_gateway("Qoder PAT userinfo response is missing user id")
    })?;
    let mut identity = QoderIdentity {
        name: user.name(),
        aid: user.aid().unwrap_or(uid).to_string(),
        uid: uid.to_string(),
        organization_id: user.organization_id(),
        organization_name: user.organization_name(),
        user_type: user.user_type(),
        security_oauth_token: access_token.to_string(),
        refresh_token: exchanged.refresh_token.clone(),
    };
    enrich_organization(http, &endpoints, access_token, &mut identity).await;
    let machine = random_qoder_machine(QoderSite::Global);
    account_input(QoderAccountDraft {
        endpoints: &endpoints,
        rail: QoderCredentialRail::PatJobToken,
        machine: &machine,
        identity,
        access_token: None,
        refresh_token: None,
        api_key: Some(pat.to_string()),
        expires_at: None,
        login_method: "pat",
        now_ms,
    })
}

pub async fn exchange_pat_job_token(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    pat: &str,
    now_ms: i64,
) -> Result<QoderTokenResponse, QoderClientError> {
    let url = endpoint_url(
        &endpoints.openapi_base_url,
        crate::domain::qoder::QODER_PAT_EXCHANGE_PATH,
    )?;
    let mut response = http
        .post(url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, qoder_openapi_user_agent())
        .header("cosy-version", &endpoints.client_version)
        .header("cosy-clienttype", "5")
        .json(&json!({"personal_token": pat.trim()}))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder PAT exchange failed: {error}"))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "PAT exchange").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "PAT exchange",
            &body,
        ));
    }
    parse_token_response(&body, now_ms)
}

pub async fn fetch_model_catalog(
    http: &reqwest::Client,
    gateway_base_url: &str,
    session: &QoderCosySession,
    timeout: Duration,
) -> Result<Value, QoderClientError> {
    let url = endpoint_url(gateway_base_url, QODER_MODEL_LIST_PATH)?;
    let client_ip = qoder_client_ip(session.site, &url).await?;
    let request_id = crate::domain::qoder::random_qoder_uuid();
    let headers = session
        .signed_headers(
            &[],
            QODER_MODEL_LIST_SIGNATURE_PATH,
            chrono::Utc::now().timestamp(),
            &request_id,
            &crate::domain::qoder::qoder_machine_os(),
            &client_ip,
        )
        .map_err(QoderClientError::bad_gateway)?;
    let mut request = http.get(url).timeout(timeout);
    for (name, value) in headers {
        if matches!(name.as_str(), "accept" | "content-type" | "cache-control") {
            continue;
        }
        request = request.header(name, value);
    }
    let mut response = request
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder model catalog failed: {error}"))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "model catalog").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "model catalog",
            &body,
        ));
    }
    let body = unwrap_qoder_json_response(&body)?;
    serde_json::from_slice(&body).map_err(|error| {
        QoderClientError::bad_gateway(format!("Qoder model catalog is not valid JSON: {error}"))
    })
}

pub async fn fetch_quota_usage(
    http: &reqwest::Client,
    account: &Account,
    timeout: Duration,
) -> Result<Value, QoderClientError> {
    if account.provider_type != ProviderType::QoderCosy {
        return Err(QoderClientError::bad_request(format!(
            "expected qoder_cosy account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = QoderAccountProfile::parse(account.profile.as_ref())
        .map_err(QoderClientError::bad_request)?;
    let endpoints = QoderEndpoints::from_account(account, profile.site);
    let machine_token = machine_token_from_raw(account.raw.as_ref()).unwrap_or_default();
    let machine = profile.machine(&machine_token);
    machine
        .validate(profile.site)
        .map_err(QoderClientError::bad_request)?;

    let attempts = if profile.credential_rail == QoderCredentialRail::PatJobToken {
        2
    } else {
        1
    };
    let mut last_error = None;
    for attempt in 0..attempts {
        let (access_token, refresh_token, gateway_base_url) = match profile.credential_rail {
            QoderCredentialRail::PatJobToken => {
                let pat = account
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| value.starts_with("pt-") && value.len() > 3)
                    .ok_or_else(|| {
                        QoderClientError::bad_request(
                            "Qoder PAT quota requires a Global pt-* credential",
                        )
                    })?;
                let exchanged = exchange_pat_job_token(
                    http,
                    &endpoints,
                    pat,
                    crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
                )
                .await?;
                let access_token = exchanged.access_token().ok_or_else(|| {
                    QoderClientError::bad_gateway(
                        "Qoder PAT quota exchange returned no transient job token",
                    )
                })?;
                (
                    access_token.to_string(),
                    exchanged.refresh_token().unwrap_or_default().to_string(),
                    endpoints.job_gateway_base_url.as_str(),
                )
            }
            QoderCredentialRail::GlobalOauth | QoderCredentialRail::CnOauth => {
                let access_token = account
                    .access_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        QoderClientError::bad_request("Qoder quota requires an access token")
                    })?;
                (
                    access_token.to_string(),
                    account
                        .refresh_token
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string(),
                    endpoints.gateway_base_url.as_str(),
                )
            }
        };
        let session = QoderCosySession::new(
            profile.site,
            profile.identity(&access_token, &refresh_token),
            machine.clone(),
        )
        .map_err(QoderClientError::bad_request)?;
        match fetch_signed_quota_usage(http, gateway_base_url, &session, timeout).await {
            Ok(value) => return Ok(value),
            Err(error)
                if attempt + 1 < attempts
                    && matches!(
                        error.upstream_status,
                        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                    ) =>
            {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        QoderClientError::bad_gateway("Qoder quota request failed without an upstream response")
    }))
}

async fn fetch_signed_quota_usage(
    http: &reqwest::Client,
    gateway_base_url: &str,
    session: &QoderCosySession,
    timeout: Duration,
) -> Result<Value, QoderClientError> {
    let mut url = endpoint_url(gateway_base_url, QODER_QUOTA_PATH)?;
    if session.site == QoderSite::Cn && !session.identity.organization_id.trim().is_empty() {
        url.query_pairs_mut()
            .append_pair("orgId", session.identity.organization_id.trim());
    }
    let request_id = crate::domain::qoder::random_qoder_uuid();
    let client_ip = qoder_client_ip(session.site, &url).await?;
    let headers = session
        .signed_headers(
            &[],
            QODER_QUOTA_SIGNATURE_PATH,
            chrono::Utc::now().timestamp(),
            &request_id,
            &crate::domain::qoder::qoder_machine_os(),
            &client_ip,
        )
        .map_err(QoderClientError::bad_gateway)?;
    let http_date = httpdate::fmt_http_date(SystemTime::now());
    let center_signature =
        md5_hex(format!("{QODER_APP_CODE}&{QODER_APP_SECRET}&{http_date}").as_bytes());
    let mut request = http.get(url).timeout(timeout);
    for (name, value) in headers {
        if matches!(name.as_str(), "accept" | "content-type" | "cache-control") {
            continue;
        }
        request = request.header(name, value);
    }
    let mut response = request
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, crate::domain::qoder::QODER_COSY_USER_AGENT)
        .header("date", http_date)
        .header("signature", center_signature)
        .header("appcode", QODER_APP_CODE)
        .send()
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder quota request failed: {error}"))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "quota").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "quota",
            &body,
        ));
    }
    let body = unwrap_qoder_json_response(&body)?;
    serde_json::from_slice(&body).map_err(|error| {
        QoderClientError::bad_gateway(format!("Qoder quota response is not valid JSON: {error}"))
    })
}

pub async fn refresh_qoder_account<F>(
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

    if account.provider_type != ProviderType::QoderCosy {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected qoder_cosy account, got {}",
            account.provider_type.as_str()
        )));
    }
    let profile = QoderAccountProfile::parse(account.profile.as_ref())
        .map_err(AccountRefreshFailure::bad_request)?;
    if profile.credential_rail == QoderCredentialRail::PatJobToken {
        return Err(AccountRefreshFailure::bad_request(
            "Qoder PAT accounts exchange a transient job token and do not use native refresh",
        ));
    }
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("Qoder refresh token is required"))?;
    let machine_token = machine_token_from_raw(account.raw.as_ref()).unwrap_or_default();
    let machine = profile.machine(&machine_token);
    machine
        .validate(profile.site)
        .map_err(AccountRefreshFailure::bad_request)?;
    let endpoints = QoderEndpoints::from_account(account, profile.site);

    let token = match profile.credential_rail {
        QoderCredentialRail::GlobalOauth | QoderCredentialRail::CnOauth => {
            refresh_device_token(http, &endpoints, refresh_token, now_ms).await
        }
        QoderCredentialRail::PatJobToken => unreachable!("PAT was rejected above"),
    }
    .map_err(qoder_refresh_failure)?;

    let new_access_token = token.access_token().ok_or_else(|| {
        AccountRefreshFailure::outcome_unknown("Qoder refresh receipt has no access token")
    })?;
    let effective_refresh_token = first_non_empty([token.refresh_token.as_str(), refresh_token])
        .ok_or_else(|| {
            AccountRefreshFailure::outcome_unknown("Qoder refresh receipt has no refresh token")
        })?;
    let receipt = AccountRefreshUpdate {
        access_token: Some(new_access_token.to_string()),
        refresh_token: Some(effective_refresh_token.to_string()),
        token_type: Some("Bearer".to_string()),
        expires_at: token.expires_at_ms,
        raw: Some(json!({
            "qoderRefreshReceipt": {
                "site": profile.site.as_str(),
                "credentialRail": profile.credential_rail.as_str(),
                "receivedAtMs": now_ms
            }
        })),
        ..AccountRefreshUpdate::default()
    };

    // The token endpoint may rotate the refresh token. Persist the complete
    // credential receipt before any userinfo/auth-status validation.
    receipt_hook(&receipt)?;
    let mut update = complete_qoder_refresh_receipt(http, account, receipt).await?;
    if crate::domain::accounts::store::account_refresh_replaces_auth_identity(account, &update) {
        return Err(AccountRefreshFailure {
            status_code: 409,
            upstream_status: None,
            message: "Qoder OAuth refresh returned a different subscription identity; re-login as a new account"
                .to_string(),
            kind: crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        });
    }
    if let Some(raw) = update.raw.take() {
        update.raw = Some(crate::domain::accounts::oauth::merge_account_refresh_raw(
            account.raw.as_ref(),
            raw,
        ));
    }
    Ok(update)
}

pub async fn complete_qoder_refresh_receipt(
    http: &reqwest::Client,
    account: &Account,
    mut update: AccountRefreshUpdate,
) -> Result<AccountRefreshUpdate, crate::clients::oauth::refresh::AccountRefreshFailure> {
    use crate::clients::oauth::refresh::AccountRefreshFailure;

    let profile = QoderAccountProfile::parse(account.profile.as_ref())
        .map_err(AccountRefreshFailure::parse)?;
    if profile.credential_rail == QoderCredentialRail::PatJobToken {
        return Err(AccountRefreshFailure::bad_request(
            "Qoder PAT receipts are transient and cannot enter the refresh journal",
        ));
    }
    let access_token = update
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::parse("Qoder refresh receipt has no access token"))?;
    let refresh_token = update
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AccountRefreshFailure::parse("Qoder refresh receipt has no refresh token")
        })?;
    let endpoints = QoderEndpoints::from_account(account, profile.site);
    let machine_token = machine_token_from_raw(account.raw.as_ref()).unwrap_or_default();
    let machine = profile.machine(&machine_token);
    let token = QoderTokenResponse {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at_ms: update.expires_at,
        ..QoderTokenResponse::default()
    };
    let identity = complete_identity(http, &endpoints, &machine, &token, access_token)
        .await
        .map_err(qoder_refresh_failure)?;

    let mut candidate_profile = profile.clone();
    candidate_profile.uid = identity.uid.clone();
    candidate_profile.aid = identity.aid.clone();
    candidate_profile.name = identity.name.clone();
    candidate_profile.organization_id = identity.organization_id.clone();
    candidate_profile.organization_name = identity.organization_name.clone();
    candidate_profile.user_type = identity.user_type.clone();
    candidate_profile
        .validate()
        .map_err(AccountRefreshFailure::parse)?;
    update.profile =
        Some(serde_json::to_value(candidate_profile).map_err(|error| {
            AccountRefreshFailure::parse(format!("encode Qoder profile: {error}"))
        })?);
    Ok(update)
}

async fn refresh_device_token(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    refresh_token: &str,
    now_ms: i64,
) -> Result<QoderTokenResponse, QoderClientError> {
    let url = endpoint_url(&endpoints.openapi_base_url, DEVICE_REFRESH_PATH)?;
    let mut response = http
        .post(url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, qoder_openapi_user_agent())
        .json(&json!({"refresh_token": refresh_token}))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            QoderClientError::refresh_outcome_unknown(format!(
                "Qoder {} device refresh failed: {error}",
                endpoints.site.as_str()
            ))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let operation = format!("{} device refresh", endpoints.site.as_str());
    let body = read_body(&mut response, &operation)
        .await
        .map_err(QoderClientError::mark_outcome_unknown)?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            &operation,
            &body,
        ));
    }
    parse_token_response(&body, now_ms).map_err(QoderClientError::mark_outcome_unknown)
}

fn qoder_refresh_failure(
    error: QoderClientError,
) -> crate::clients::oauth::refresh::AccountRefreshFailure {
    use crate::clients::oauth::refresh::AccountRefreshFailure;
    use crate::domain::accounts::oauth::OAuthErrorKind;

    let upstream_status = error.upstream_status.map(|status| status.as_u16());
    let invalid = matches!(
        error.kind,
        QoderErrorKind::InvalidGrant | QoderErrorKind::Authentication
    );
    let rate_limited = matches!(error.kind, QoderErrorKind::RateLimited);
    let outcome_unknown = error.outcome_unknown;
    AccountRefreshFailure {
        status_code: if rate_limited {
            StatusCode::TOO_MANY_REQUESTS.as_u16()
        } else {
            error.status.as_u16()
        },
        upstream_status,
        message: error.message,
        kind: match (&error.kind, outcome_unknown) {
            (_, true) => OAuthErrorKind::Unknown,
            (QoderErrorKind::InvalidGrant | QoderErrorKind::Authentication, false) => {
                OAuthErrorKind::InvalidGrant
            }
            (QoderErrorKind::Permission | QoderErrorKind::Protocol, false) => {
                OAuthErrorKind::ProviderRejected
            }
            (QoderErrorKind::RateLimited, false) => OAuthErrorKind::RateLimited,
            (QoderErrorKind::Temporary, false) => OAuthErrorKind::Network,
        },
        retryable: matches!(
            error.kind,
            QoderErrorKind::RateLimited | QoderErrorKind::Temporary
        ) && !outcome_unknown,
        retry_after_ms: error.retry_after_ms,
        immediate_relogin: invalid || outcome_unknown,
        outcome_unknown,
        endpoint_fallback_safe: false,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QoderTokenResponse {
    #[serde(default)]
    device_token: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    expires_at: Value,
    #[serde(default)]
    expires_in: Value,
    #[serde(skip)]
    expires_at_ms: Option<i64>,
}

impl QoderTokenResponse {
    pub fn access_token(&self) -> Option<&str> {
        first_non_empty([
            self.device_token.as_str(),
            self.token.as_str(),
            self.access_token.as_str(),
        ])
    }

    pub fn refresh_token(&self) -> Option<&str> {
        first_non_empty([self.refresh_token.as_str()])
    }

    pub fn expires_at_ms(&self) -> Option<i64> {
        self.expires_at_ms
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct QoderUserInfo {
    #[serde(default)]
    id: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    user_name: String,
    #[serde(default, alias = "user_type")]
    user_type: String,
    #[serde(default, alias = "organizationId")]
    organization_id: String,
    #[serde(default, alias = "organizationName")]
    organization_name: String,
}

impl QoderUserInfo {
    fn uid(&self) -> Option<&str> {
        first_non_empty([self.user_id.as_str(), self.id.as_str()])
    }

    fn aid(&self) -> Option<&str> {
        first_non_empty([
            self.account_id.as_str(),
            self.id.as_str(),
            self.user_id.as_str(),
        ])
    }

    fn name(&self) -> String {
        first_non_empty([self.name.as_str(), self.user_name.as_str()])
            .unwrap_or_default()
            .to_string()
    }

    fn user_type(&self) -> String {
        first_non_empty([self.user_type.as_str()])
            .unwrap_or("personal_standard")
            .to_string()
    }

    fn organization_id(&self) -> String {
        self.organization_id.trim().to_string()
    }

    fn organization_name(&self) -> String {
        self.organization_name.trim().to_string()
    }
}

async fn complete_identity(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    machine: &QoderMachineIdentity,
    token: &QoderTokenResponse,
    access_token: &str,
) -> Result<QoderIdentity, QoderClientError> {
    let user = match fetch_user_info(http, endpoints, access_token).await {
        Ok(user) => user,
        Err(error) if endpoints.site == QoderSite::Global && !token.user_id.trim().is_empty() => {
            tracing::warn!(%error, "Qoder userinfo failed; using device token identity");
            QoderUserInfo {
                user_id: token.user_id.clone(),
                user_name: token.user_name.clone(),
                ..QoderUserInfo::default()
            }
        }
        Err(error) => return Err(error),
    };
    let provisional_uid = first_non_empty([
        token.user_id.as_str(),
        user.user_id.as_str(),
        user.id.as_str(),
    ])
    .ok_or_else(|| QoderClientError::bad_gateway("Qoder identity is missing user id"))?;
    let mut identity = QoderIdentity {
        name: user.name(),
        aid: user.aid().unwrap_or(provisional_uid).to_string(),
        uid: provisional_uid.to_string(),
        organization_id: user.organization_id(),
        organization_name: user.organization_name(),
        user_type: user.user_type(),
        security_oauth_token: access_token.to_string(),
        refresh_token: token.refresh_token.trim().to_string(),
    };
    if endpoints.site == QoderSite::Cn {
        identity = complete_cn_auth_status(http, endpoints, machine, &identity).await?;
        identity.security_oauth_token = access_token.to_string();
        identity.refresh_token = token.refresh_token.trim().to_string();
    }
    enrich_organization(http, endpoints, access_token, &mut identity).await;
    identity.validate().map_err(QoderClientError::bad_gateway)?;
    Ok(identity)
}

async fn fetch_user_info(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    token: &str,
) -> Result<QoderUserInfo, QoderClientError> {
    let url = endpoint_url(&endpoints.openapi_base_url, USER_INFO_PATH)?;
    let mut response = http
        .get(url)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {}", token.trim()))
        .header(USER_AGENT, qoder_openapi_user_agent())
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| QoderClientError::temporary(format!("Qoder userinfo failed: {error}")))?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "userinfo").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "userinfo",
            &body,
        ));
    }
    serde_json::from_slice(&body).map_err(|error| {
        QoderClientError::bad_gateway(format!("Qoder userinfo is not valid JSON: {error}"))
    })
}

async fn enrich_organization(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    token: &str,
    identity: &mut QoderIdentity,
) {
    if !identity.organization_id.trim().is_empty() || identity.uid.trim().is_empty() {
        return;
    }
    let Ok(mut url) = endpoint_url(&endpoints.openapi_base_url, ORGANIZATION_TAGS_PREFIX) else {
        return;
    };
    {
        let mut segments = match url.path_segments_mut() {
            Ok(segments) => segments,
            Err(()) => return,
        };
        segments.pop_if_empty().push(&identity.uid).push("tags");
    }
    let response = http
        .get(url)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {}", token.trim()))
        .header(USER_AGENT, qoder_openapi_user_agent())
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await;
    let Ok(mut response) = response else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(body) = read_body(&mut response, "organization tags").await else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return;
    };
    identity.organization_id =
        string_at(&value, &["/organization_id", "/organizationId", "/id"]).unwrap_or_default();
    identity.organization_name = string_at(
        &value,
        &["/organization_name", "/organizationName", "/name"],
    )
    .unwrap_or_default();
}

async fn complete_cn_auth_status(
    http: &reqwest::Client,
    endpoints: &QoderEndpoints,
    machine: &QoderMachineIdentity,
    provisional: &QoderIdentity,
) -> Result<QoderIdentity, QoderClientError> {
    let params = json!({
        "accessKey": "",
        "secretKey": "",
        "securityToken": "",
        "userId": provisional.uid,
        "orgId": "",
        "token": "",
        "personalToken": "",
        "securityOauthToken": provisional.security_oauth_token,
        "refreshToken": provisional.refresh_token,
        "needRefresh": false,
        "authInfo": {}
    });
    let envelope = serde_json::to_vec(&json!({
        "payload": serde_json::to_string(&params).map_err(|error| QoderClientError::bad_gateway(error.to_string()))?,
        "encodeVersion": "1"
    }))
    .map_err(|error| QoderClientError::bad_gateway(error.to_string()))?;
    let encoded = qoder_encode(&envelope);
    let mut url = endpoint_url(&endpoints.gateway_base_url, AUTH_STATUS_ACTUAL_PATH)?;
    url.query_pairs_mut().append_pair("Encode", "1");
    let client_ip = qoder_client_ip(QoderSite::Cn, &url).await?;
    let date = httpdate::fmt_http_date(SystemTime::now());
    let signature = md5_hex(format!("{QODER_APP_CODE}&{QODER_APP_SECRET}&{date}").as_bytes());
    let mut response = http
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .header(ACCEPT_ENCODING, "identity")
        .header(USER_AGENT, crate::domain::qoder::QODER_COSY_USER_AGENT)
        .header("date", date)
        .header("signature", signature)
        .header("appcode", QODER_APP_CODE)
        .header("login-version", "v2")
        .header("cosy-version", &endpoints.client_version)
        .header("cosy-clienttype", &endpoints.client_type)
        .header("cosy-clientip", client_ip)
        .header("cosy-machineos", machine_os())
        .header("cosy-machineid", &machine.machine_id)
        .header("cosy-machinetype", "")
        .header("cosy-machinetoken", "")
        .header("cosy-machinecode", "")
        .body(encoded)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder CN auth status failed: {error}"))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = read_body(&mut response, "CN auth status").await?;
    if !status.is_success() {
        return Err(QoderClientError::upstream(
            status,
            Some(&response_headers),
            "CN auth status",
            &body,
        ));
    }
    let body = unwrap_qoder_json_response(&body)?;
    let value: Value = serde_json::from_slice(&body).map_err(|error| {
        QoderClientError::bad_gateway(format!("Qoder CN auth status is not valid JSON: {error}"))
    })?;
    let uid = string_at(&value, &["/id", "/accountId"]).unwrap_or_else(|| provisional.uid.clone());
    let identity = QoderIdentity {
        name: string_at(&value, &["/name"]).unwrap_or_else(|| provisional.name.clone()),
        aid: string_at(&value, &["/accountId", "/id"]).unwrap_or_else(|| provisional.aid.clone()),
        uid,
        organization_id: string_at(&value, &["/orgId", "/organizationId"])
            .unwrap_or_else(|| provisional.organization_id.clone()),
        organization_name: string_at(&value, &["/orgName", "/organizationName"])
            .unwrap_or_else(|| provisional.organization_name.clone()),
        user_type: string_at(&value, &["/userType"])
            .unwrap_or_else(|| provisional.user_type.clone()),
        security_oauth_token: provisional.security_oauth_token.clone(),
        refresh_token: provisional.refresh_token.clone(),
    };
    identity.validate().map_err(QoderClientError::bad_gateway)?;
    Ok(identity)
}

struct QoderAccountDraft<'a> {
    endpoints: &'a QoderEndpoints,
    rail: QoderCredentialRail,
    machine: &'a QoderMachineIdentity,
    identity: QoderIdentity,
    access_token: Option<String>,
    refresh_token: Option<String>,
    api_key: Option<String>,
    expires_at: Option<i64>,
    login_method: &'a str,
    now_ms: i64,
}

fn account_input(draft: QoderAccountDraft<'_>) -> Result<UpsertAccountInput, QoderClientError> {
    let QoderAccountDraft {
        endpoints,
        rail,
        machine,
        identity,
        access_token,
        refresh_token,
        api_key,
        expires_at,
        login_method,
        now_ms,
    } = draft;
    let refresh_mode = if endpoints.site == QoderSite::Cn {
        QODER_REFRESH_MODE_QODER_CN20
    } else {
        QODER_REFRESH_MODE_COSY
    };
    let profile = QoderAccountProfile {
        site: endpoints.site,
        credential_rail: rail,
        refresh_mode: refresh_mode.to_string(),
        uid: identity.uid.clone(),
        aid: identity.aid.clone(),
        name: identity.name.clone(),
        email: String::new(),
        organization_id: identity.organization_id.clone(),
        organization_name: identity.organization_name.clone(),
        user_type: identity.user_type.clone(),
        machine_id: machine.machine_id.clone(),
        machine_type: machine.machine_type.clone(),
    };
    profile.validate().map_err(QoderClientError::bad_gateway)?;
    let id = qoder_account_id(endpoints.site, rail, &identity.uid)
        .map_err(QoderClientError::bad_gateway)?;
    Ok(UpsertAccountInput {
        id: Some(id),
        provider_type: ProviderType::QoderCosy,
        email: None,
        access_token,
        refresh_token,
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key,
        extra_headers: None,
        scopes: Vec::new(),
        profile: Some(serde_json::to_value(profile).map_err(|error| {
            QoderClientError::bad_gateway(format!("encode Qoder profile: {error}"))
        })?),
        raw: Some(json!({
            "loginMethod": login_method,
            "importedAtMs": now_ms,
            "qoderSecrets": {"machineToken": machine.machine_token}
        })),
        subscription_level: Some("qoder".to_string()),
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

fn parse_token_response(body: &[u8], now_ms: i64) -> Result<QoderTokenResponse, QoderClientError> {
    let mut token: QoderTokenResponse = serde_json::from_slice(body).map_err(|error| {
        QoderClientError::bad_gateway(format!("Qoder token response is not valid JSON: {error}"))
    })?;
    token.expires_at_ms = parse_expiry_ms(&token.expires_at, &token.expires_in, now_ms);
    Ok(token)
}

fn parse_expiry_ms(expires_at: &Value, expires_in: &Value, now_ms: i64) -> Option<i64> {
    let absolute = flexible_i64(expires_at).and_then(|value| match value {
        value if value >= 1_000_000_000_000 => Some(value),
        value if value >= 1_000_000_000 => Some(value.saturating_mul(1_000)),
        value if value > 0 => Some(now_ms.saturating_add(value.saturating_mul(1_000))),
        _ => None,
    });
    if absolute.is_some() {
        return absolute;
    }
    if let Some(value) = expires_at
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(value) = DateTime::parse_from_rfc3339(value) {
            return Some(value.timestamp_millis());
        }
        if let Ok(value) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f") {
            return Some(Utc.from_utc_datetime(&value).timestamp_millis());
        }
        if let Ok(value) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            return value
                .and_hms_opt(0, 0, 0)
                .map(|value| Utc.from_utc_datetime(&value).timestamp_millis());
        }
    }
    flexible_i64(expires_in)
        .filter(|value| *value > 0)
        .map(|value| now_ms.saturating_add(value.saturating_mul(1_000)))
}

fn flexible_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn pending_result(interval: u64) -> QoderDevicePollResult {
    QoderDevicePollResult {
        pending: true,
        message: "authorization_pending".to_string(),
        retry_after_secs: Some(interval.max(1)),
        account_input: None,
    }
}

pub(crate) fn qoder_openapi_user_agent() -> &'static str {
    QODER_OPENAPI_USER_AGENT
}

pub(crate) fn qoder_device_poll_url(
    flow: &PendingQoderDeviceFlow,
) -> Result<Url, QoderClientError> {
    let mut poll_url = endpoint_url(&flow.endpoints.openapi_base_url, DEVICE_POLL_PATH)?;
    poll_url.query_pairs_mut().extend_pairs([
        ("nonce", flow.nonce.as_str()),
        ("verifier", flow.code_verifier.as_str()),
        ("challenge_method", "S256"),
    ]);
    Ok(poll_url)
}

fn endpoint_url(base: &str, path: &str) -> Result<Url, QoderClientError> {
    let base = format!("{}/", base.trim_end_matches('/'));
    Url::parse(&base)
        .and_then(|base| base.join(path.trim_start_matches('/')))
        .map_err(|error| QoderClientError::bad_gateway(format!("invalid Qoder endpoint: {error}")))
}

async fn qoder_client_ip(site: QoderSite, target: &Url) -> Result<String, QoderClientError> {
    if site != QoderSite::Cn {
        return Ok(String::new());
    }
    let configured = std::env::var(QODER_CN_CLIENT_IP_ENV).ok();
    let resolved =
        crate::infra::network_identity::resolve_outbound_ipv4(target, configured.as_deref())
            .await
            .map_err(|error| {
                QoderClientError::bad_gateway(format!(
            "resolve Qoder CN client IP (set {QODER_CN_CLIENT_IP_ENV} to override): {error}"
        ))
            })?;
    crate::metrics::record_qoder_client_ip_source(resolved.source);
    Ok(resolved.address.to_string())
}

async fn read_body(
    response: &mut reqwest::Response,
    operation: &str,
) -> Result<bytes::Bytes, QoderClientError> {
    crate::infra::http::read_response_body_limited(response, MAX_RESPONSE_BODY_BYTES)
        .await
        .map_err(|error| {
            QoderClientError::temporary(format!("Qoder {operation} response read failed: {error}"))
        })
}

pub(crate) fn unwrap_qoder_json_response(body: &[u8]) -> Result<Vec<u8>, QoderClientError> {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Ok(body.to_vec()),
    };
    let Some(inner) = value.get("body") else {
        return Ok(body.to_vec());
    };
    let status = value
        .get("statusCodeValue")
        .and_then(Value::as_u64)
        .or_else(|| {
            value.get("statusCode").and_then(|status| {
                status.as_u64().or_else(|| {
                    let status = status.as_str()?.trim();
                    status
                        .parse()
                        .ok()
                        .or_else(|| match status.to_ascii_uppercase().as_str() {
                            "BAD_REQUEST" => Some(400),
                            "UNAUTHORIZED" => Some(401),
                            "FORBIDDEN" => Some(403),
                            "NOT_FOUND" => Some(404),
                            "TOO_MANY_REQUESTS" => Some(429),
                            "INTERNAL_SERVER_ERROR" => Some(500),
                            "BAD_GATEWAY" => Some(502),
                            "SERVICE_UNAVAILABLE" => Some(503),
                            "GATEWAY_TIMEOUT" => Some(504),
                            _ => None,
                        })
                })
            })
        })
        .unwrap_or(200);
    let bytes = match inner {
        Value::String(value) if serde_json::from_str::<Value>(value).is_ok() => {
            value.as_bytes().to_vec()
        }
        Value::String(value) => qoder_decode(value).map_err(QoderClientError::bad_gateway)?,
        value => serde_json::to_vec(value).map_err(|error| {
            QoderClientError::bad_gateway(format!("encode Qoder response body: {error}"))
        })?,
    };
    if status >= 400 {
        return Err(QoderClientError::upstream(
            StatusCode::from_u16(status as u16).unwrap_or(StatusCode::BAD_GATEWAY),
            None,
            "gateway request",
            &bytes,
        ));
    }
    Ok(bytes)
}

fn public_error_body(body: &[u8]) -> String {
    let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let message = string_at(
        &value,
        &["/message", "/error/message", "/error_description"],
    )
    .unwrap_or_else(|| String::from_utf8_lossy(body).to_string());
    crate::logging::redact_sensitive_text(&message)
        .chars()
        .take(320)
        .collect()
}

fn classify_qoder_upstream_error(status: StatusCode, body: &[u8]) -> QoderErrorKind {
    let evidence = String::from_utf8_lossy(body).to_ascii_lowercase();
    let invalid_grant = [
        "invalid_grant",
        "expired_token",
        "invalid refresh token",
        "refresh token is invalid",
        "refresh_token is invalid",
        "refresh token expired",
        "refresh_token expired",
    ]
    .iter()
    .any(|marker| evidence.contains(marker));
    if invalid_grant {
        return QoderErrorKind::InvalidGrant;
    }
    match status {
        StatusCode::UNAUTHORIZED => QoderErrorKind::Authentication,
        StatusCode::FORBIDDEN => QoderErrorKind::Permission,
        StatusCode::TOO_MANY_REQUESTS => QoderErrorKind::RateLimited,
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::TOO_EARLY
        | StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => QoderErrorKind::Temporary,
        _ => QoderErrorKind::Protocol,
    }
}

fn parse_retry_after_ms(headers: &HeaderMap) -> Option<i64> {
    const MAX_RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1_000;
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
    {
        return Some(value.clamp(0, MAX_RETRY_AFTER_MS));
    }
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds.saturating_mul(1_000).clamp(0, MAX_RETRY_AFTER_MS));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .as_millis()
            .min(MAX_RETRY_AFTER_MS as u128) as i64,
    )
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

fn first_non_empty<const N: usize>(values: [&str; N]) -> Option<&str> {
    values
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn md5_hex(value: &[u8]) -> String {
    format!("{:x}", md5::compute(value))
}

pub(crate) fn machine_os() -> String {
    crate::domain::qoder::qoder_machine_os()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::store::AccountStore;
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct QoderHttpObservation {
        method: axum::http::Method,
        path: String,
        query: Option<String>,
        headers: axum::http::HeaderMap,
        body: Vec<u8>,
    }

    async fn serve_qoder_quota(
        pat_reauth: bool,
    ) -> (
        String,
        Arc<Mutex<Vec<QoderHttpObservation>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_route = Arc::clone(&observations);
        let exchange_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let exchange_count_for_route = Arc::clone(&exchange_count);
        let quota_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let quota_count_for_route = Arc::clone(&quota_count);
        let app = axum::Router::new().fallback(axum::routing::any(
            move |method: axum::http::Method,
                  uri: axum::http::Uri,
                  headers: axum::http::HeaderMap,
                  body: axum::body::Bytes| {
                let observations = Arc::clone(&observations_for_route);
                let exchange_count = Arc::clone(&exchange_count_for_route);
                let quota_count = Arc::clone(&quota_count_for_route);
                async move {
                    observations.lock().unwrap().push(QoderHttpObservation {
                        method: method.clone(),
                        path: uri.path().to_string(),
                        query: uri.query().map(str::to_string),
                        headers,
                        body: body.to_vec(),
                    });
                    if method == axum::http::Method::POST
                        && uri.path() == crate::domain::qoder::QODER_PAT_EXCHANGE_PATH
                    {
                        let attempt =
                            exchange_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        return axum::http::Response::builder()
                            .status(axum::http::StatusCode::OK)
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from(
                                json!({
                                    "token": format!("jt-quota-{attempt}"),
                                    "expires_in": 3600
                                })
                                .to_string(),
                            ))
                            .unwrap();
                    }
                    if method == axum::http::Method::GET
                        && uri.path() == crate::domain::qoder::QODER_QUOTA_PATH
                    {
                        let attempt =
                            quota_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        if pat_reauth && attempt == 1 {
                            return axum::http::Response::builder()
                                .status(axum::http::StatusCode::UNAUTHORIZED)
                                .header("content-type", "application/json")
                                .body(axum::body::Body::from(
                                    json!({"message": "expired transient job token"}).to_string(),
                                ))
                                .unwrap();
                        }
                        return axum::http::Response::builder()
                            .status(axum::http::StatusCode::OK)
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from(
                                json!({
                                    "userType": "teams",
                                    "expiresAt": 1_800_000_000_000_i64,
                                    "userQuota": {
                                        "total": 100,
                                        "used": 20,
                                        "remaining": 80
                                    }
                                })
                                .to_string(),
                            ))
                            .unwrap();
                    }
                    axum::http::Response::builder()
                        .status(axum::http::StatusCode::NOT_FOUND)
                        .body(axum::body::Body::empty())
                        .unwrap()
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), observations, server)
    }

    struct QoderLifecycleServer {
        base_url: String,
        observations: Arc<Mutex<Vec<QoderHttpObservation>>>,
        refresh_status: Arc<AtomicU16>,
        user_uid: Arc<Mutex<String>>,
        unexpected_count: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }

    fn qoder_json_response(
        status: axum::http::StatusCode,
        body: Value,
    ) -> axum::http::Response<axum::body::Body> {
        axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    async fn serve_qoder_lifecycle(site: QoderSite) -> QoderLifecycleServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_route = Arc::clone(&observations);
        let poll_count = Arc::new(AtomicUsize::new(0));
        let poll_count_for_route = Arc::clone(&poll_count);
        let refresh_status = Arc::new(AtomicU16::new(200));
        let refresh_status_for_route = Arc::clone(&refresh_status);
        let user_uid = Arc::new(Mutex::new("qoder-uid".to_string()));
        let user_uid_for_route = Arc::clone(&user_uid);
        let unexpected_count = Arc::new(AtomicUsize::new(0));
        let unexpected_count_for_route = Arc::clone(&unexpected_count);
        let app = axum::Router::new().fallback(axum::routing::any(
            move |method: axum::http::Method,
                  uri: axum::http::Uri,
                  headers: axum::http::HeaderMap,
                  body: axum::body::Bytes| {
                let observations = Arc::clone(&observations_for_route);
                let poll_count = Arc::clone(&poll_count_for_route);
                let refresh_status = Arc::clone(&refresh_status_for_route);
                let user_uid = Arc::clone(&user_uid_for_route);
                let unexpected_count = Arc::clone(&unexpected_count_for_route);
                async move {
                    observations.lock().unwrap().push(QoderHttpObservation {
                        method: method.clone(),
                        path: uri.path().to_string(),
                        query: uri.query().map(str::to_string),
                        headers,
                        body: body.to_vec(),
                    });
                    if method == axum::http::Method::GET && uri.path() == DEVICE_POLL_PATH {
                        if poll_count.fetch_add(1, Ordering::SeqCst) == 0 {
                            return axum::http::Response::builder()
                                .status(axum::http::StatusCode::NOT_FOUND)
                                .body(axum::body::Body::empty())
                                .unwrap();
                        }
                        return qoder_json_response(
                            axum::http::StatusCode::OK,
                            json!({
                                "device_token": "poll-access",
                                "refresh_token": "poll-refresh",
                                "expires_at": 1_800_000_000_i64
                            }),
                        );
                    }
                    if method == axum::http::Method::POST && uri.path() == DEVICE_REFRESH_PATH {
                        let status =
                            axum::http::StatusCode::from_u16(refresh_status.load(Ordering::SeqCst))
                                .unwrap();
                        if !status.is_success() {
                            return qoder_json_response(
                                status,
                                json!({"message": format!("refresh status {}", status.as_u16())}),
                            );
                        }
                        return qoder_json_response(
                            status,
                            json!({
                                "device_token": "rotated-access",
                                "refresh_token": "rotated-refresh",
                                "expires_at": 1_800_000_000_i64,
                                "refresh_token_expires_at": 1_900_000_000_i64
                            }),
                        );
                    }
                    let uid = user_uid.lock().unwrap().clone();
                    if method == axum::http::Method::GET && uri.path() == USER_INFO_PATH {
                        return qoder_json_response(
                            axum::http::StatusCode::OK,
                            json!({
                                "id": uid,
                                "user_id": uid,
                                "account_id": format!("{uid}-aid"),
                                "name": "Qoder User",
                                "userType": "teams",
                                "organizationId": "qoder-org",
                                "organizationName": "Qoder Org"
                            }),
                        );
                    }
                    if site == QoderSite::Cn
                        && method == axum::http::Method::POST
                        && uri.path() == AUTH_STATUS_ACTUAL_PATH
                    {
                        return qoder_json_response(
                            axum::http::StatusCode::OK,
                            json!({
                                "id": uid,
                                "accountId": format!("{uid}-aid"),
                                "name": "Qoder User",
                                "userType": "teams",
                                "orgId": "qoder-org",
                                "orgName": "Qoder Org"
                            }),
                        );
                    }
                    unexpected_count.fetch_add(1, Ordering::SeqCst);
                    qoder_json_response(
                        axum::http::StatusCode::NOT_FOUND,
                        json!({"message": "unexpected Qoder test endpoint"}),
                    )
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        QoderLifecycleServer {
            base_url: format!("http://{address}"),
            observations,
            refresh_status,
            user_uid,
            unexpected_count,
            server,
        }
    }

    fn qoder_test_endpoints(site: QoderSite, base_url: &str) -> QoderEndpoints {
        let mut endpoints = QoderEndpoints::for_site(site);
        endpoints.device_authorization_url = format!("{base_url}/device/selectAccounts");
        endpoints.openapi_base_url = base_url.to_string();
        endpoints.center_base_url = Some(base_url.to_string());
        endpoints.gateway_base_url = base_url.to_string();
        endpoints.job_gateway_base_url = base_url.to_string();
        endpoints
    }

    fn qoder_lifecycle_account(site: QoderSite, base_url: &str) -> Account {
        let endpoints = QoderEndpoints::for_site(site);
        let machine = match site {
            QoderSite::Global => QoderMachineIdentity {
                machine_id: "0123456789abcdef0123456789abcdef0123".to_string(),
                machine_token: "qoder-machine-token".to_string(),
                machine_type: "5".to_string(),
            },
            QoderSite::Cn => QoderMachineIdentity {
                machine_id: "018f47ec-51d8-4c2a-9c2b-4f859709c9e8".to_string(),
                machine_token: String::new(),
                machine_type: String::new(),
            },
        };
        let identity = QoderIdentity {
            name: "Qoder User".to_string(),
            aid: "qoder-uid-aid".to_string(),
            uid: "qoder-uid".to_string(),
            organization_id: "qoder-org".to_string(),
            organization_name: "Qoder Org".to_string(),
            user_type: "teams".to_string(),
            security_oauth_token: "old-access".to_string(),
            refresh_token: "old-refresh".to_string(),
        };
        let mut input = account_input(QoderAccountDraft {
            endpoints: &endpoints,
            rail: if site == QoderSite::Cn {
                QoderCredentialRail::CnOauth
            } else {
                QoderCredentialRail::GlobalOauth
            },
            machine: &machine,
            identity,
            access_token: Some("old-access".to_string()),
            refresh_token: Some("old-refresh".to_string()),
            api_key: None,
            expires_at: Some(1_700_000_000_000),
            login_method: "test",
            now_ms: 1,
        })
        .unwrap();
        input.raw.as_mut().unwrap()["testQoderEndpoints"] = json!({
            "openapiBaseUrl": base_url,
            "centerBaseUrl": base_url,
            "gatewayBaseUrl": base_url,
            "jobGatewayBaseUrl": base_url
        });
        AccountStore::default().upsert(input)
    }

    fn assert_uuid_v4(value: &str, label: &str) {
        let bytes = value.as_bytes();
        assert_eq!(bytes.len(), 36, "{label} must be a UUID");
        for index in [8, 13, 18, 23] {
            assert_eq!(bytes[index], b'-', "{label} has an invalid UUID separator");
        }
        assert_eq!(bytes[14], b'4', "{label} must be UUID v4");
        assert!(matches!(
            bytes[19].to_ascii_lowercase(),
            b'8' | b'9' | b'a' | b'b'
        ));
        assert!(bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| { [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit() }));
    }

    fn qoder_oracle_fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../assets/contract/qoder-cli-oracle.json"
        ))
        .expect("Qoder CLI oracle fixture must be valid JSON")
    }

    fn oracle_oauth_login(fixture: &Value, site: QoderSite) -> &Value {
        &fixture["rails"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rail| {
                rail["site"].as_str() == Some(site.as_str())
                    && rail["credentialRail"].as_str() != Some("pat_job_token")
            })
            .unwrap_or_else(|| panic!("Qoder oracle is missing the {} OAuth rail", site.as_str()))
            ["login"]
    }

    fn header_value<'a>(observation: &'a QoderHttpObservation, name: &str) -> Option<&'a str> {
        observation
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn assert_no_protocol_auth_headers(observation: &QoderHttpObservation) {
        assert!(header_value(observation, "authorization").is_none());
        assert!(observation
            .headers
            .keys()
            .all(|name| !name.as_str().starts_with("cosy-")));
    }

    fn qoder_quota_account(site: QoderSite, rail: QoderCredentialRail, base_url: &str) -> Account {
        let endpoints = QoderEndpoints::for_site(site);
        let machine = match site {
            QoderSite::Global => QoderMachineIdentity {
                machine_id: "0123456789abcdef0123456789abcdef0123".to_string(),
                machine_token: "qoder-machine-token".to_string(),
                machine_type: "5".to_string(),
            },
            QoderSite::Cn => QoderMachineIdentity {
                machine_id: "018f47ec-51d8-4c2a-9c2b-4f859709c9e7".to_string(),
                machine_token: String::new(),
                machine_type: String::new(),
            },
        };
        let identity = QoderIdentity {
            name: "Quota User".to_string(),
            aid: "qoder-aid".to_string(),
            uid: "qoder-uid".to_string(),
            organization_id: if site == QoderSite::Cn {
                "qoder-org-cn"
            } else {
                ""
            }
            .to_string(),
            organization_name: String::new(),
            user_type: "teams".to_string(),
            security_oauth_token: "qoder-access".to_string(),
            refresh_token: "qoder-refresh".to_string(),
        };
        let (access_token, refresh_token, api_key) = match rail {
            QoderCredentialRail::PatJobToken => (None, None, Some("pt-qoder-secret".to_string())),
            _ => (
                Some("qoder-access".to_string()),
                Some("qoder-refresh".to_string()),
                None,
            ),
        };
        let mut input = account_input(QoderAccountDraft {
            endpoints: &endpoints,
            rail,
            machine: &machine,
            identity,
            access_token,
            refresh_token,
            api_key,
            expires_at: Some(1_800_000_000_000),
            login_method: "test",
            now_ms: 1,
        })
        .unwrap();
        input.raw.as_mut().unwrap()["testQoderEndpoints"] = json!({
            "openapiBaseUrl": base_url,
            "gatewayBaseUrl": base_url,
            "jobGatewayBaseUrl": base_url
        });
        AccountStore::default().upsert(input)
    }

    #[test]
    fn device_flow_freezes_site_machine_state_nonce_and_verifier() {
        let oracle = qoder_oracle_fixture();
        for site in [QoderSite::Global, QoderSite::Cn] {
            let endpoints = qoder_test_endpoints(site, "http://127.0.0.1:12345");
            let (device, flow) = start_device_flow_with_endpoints(endpoints, 1_000).unwrap();
            assert_eq!(device.site, site);
            assert_eq!(device.state, flow.state);
            assert_eq!(device.expires_in, 300);
            assert_eq!(device.interval, 1);
            assert_eq!(flow.expires_at_ms, 301_000);
            assert_eq!(flow.interval, 1);
            assert_eq!(flow.endpoints.site, site);
            assert_uuid_v4(&flow.nonce, "device nonce");
            match site {
                QoderSite::Global => {
                    assert_eq!(flow.machine.machine_id.len(), 36);
                    assert!(flow
                        .machine
                        .machine_id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit()));
                }
                QoderSite::Cn => assert_uuid_v4(&flow.machine.machine_id, "CN machine id"),
            }
            assert!(!flow.code_verifier.is_empty());
            let url = Url::parse(&device.verification_uri_complete).unwrap();
            let query = url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
            assert_eq!(query.get("nonce"), Some(&flow.nonce));
            assert_eq!(query.get("machine_id"), Some(&flow.machine.machine_id));
            assert_eq!(
                query.get("challenge_method").map(String::as_str),
                Some("S256")
            );
            let expected_challenge =
                URL_SAFE_NO_PAD.encode(Sha256::digest(flow.code_verifier.as_bytes()));
            assert_eq!(
                query.get("challenge").map(String::as_str),
                Some(expected_challenge.as_str())
            );

            let poll_url = qoder_device_poll_url(&flow).unwrap();
            let poll_query = poll_url
                .query_pairs()
                .map(|(name, _)| name.into_owned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let authorization_query = query.keys().cloned().collect::<Vec<_>>();
            let mut expected_login = json!({
                "authorizationPath": url.path(),
                "authorizationQueryRequired": authorization_query,
                "machineIdFormat": if site == QoderSite::Global { "lower_hex_36" } else { "uuid_v4" },
                "nonceFormat": "uuid_v4",
                "pollForbiddenHeaders": ["authorization", "cosy-*", "user-agent"],
                "pollHeaders": {"accept": "application/json"},
                "pollIntervalSeconds": DEFAULT_POLL_INTERVAL_SECS,
                "pollMethod": "GET",
                "pollOrigin": "openapi",
                "pollPath": poll_url.path(),
                "pollPendingHttpStatus": 404,
                "pollQueryRequired": poll_query,
                "pollTimeoutSeconds": FLOW_TTL_SECS,
                "pkceMethod": query.get("challenge_method").unwrap(),
                "refreshForbiddenHeaders": [
                    "authorization",
                    "cosy-*",
                    "proxy-authorization",
                    "x-qoder-account"
                ],
                "refreshHeaders": {
                    "accept": "application/json",
                    "content-type": "application/json",
                    "user-agent": qoder_openapi_user_agent(),
                },
                "refreshMethod": "POST",
                "refreshOrigin": "openapi",
                "refreshPath": DEVICE_REFRESH_PATH,
                "refreshRequestBody": {"refresh_token": "<redacted>"},
                "refreshResponseOptionalFields": ["refresh_token_expires_at"],
                "refreshResponseRequiredFields": ["device_token", "expires_at", "refresh_token"],
                "refreshTokenRotates": true,
                "stateBound": true,
                "userinfoPath": USER_INFO_PATH,
            });
            if site == QoderSite::Cn {
                expected_login["authStatusPath"] =
                    json!(format!("{AUTH_STATUS_ACTUAL_PATH}?Encode=1"));
            }
            assert_eq!(
                oracle_oauth_login(&oracle, site),
                &expected_login,
                "{} OAuth lifecycle drifted from the frozen CLI oracle",
                site.as_str()
            );
        }
    }

    #[tokio::test]
    async fn device_poll_http_contract_is_state_bound_and_site_isolated() {
        for site in [QoderSite::Global, QoderSite::Cn] {
            let server = serve_qoder_lifecycle(site).await;
            let endpoints = qoder_test_endpoints(site, &server.base_url);
            let (device, flow) = start_device_flow_with_endpoints(endpoints, 1_000).unwrap();
            assert_eq!(
                flow.endpoints.oauth_client_id,
                site.profile().oauth_client_id
            );
            assert_eq!(flow.endpoints.client_type, site.profile().client_type);

            let error = poll_device_flow(&reqwest::Client::new(), &flow, "wrong-state", 2_000)
                .await
                .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(server.observations.lock().unwrap().is_empty());

            let pending = poll_device_flow(&reqwest::Client::new(), &flow, &device.state, 2_000)
                .await
                .unwrap();
            assert!(pending.pending);
            assert_eq!(pending.retry_after_secs, Some(1));

            let completed = poll_device_flow(&reqwest::Client::new(), &flow, &device.state, 3_000)
                .await
                .unwrap();
            assert!(!completed.pending);
            let input = completed.account_input.unwrap();
            assert_eq!(input.provider_type, ProviderType::QoderCosy);
            assert_eq!(input.access_token.as_deref(), Some("poll-access"));
            assert_eq!(input.refresh_token.as_deref(), Some("poll-refresh"));
            let profile = QoderAccountProfile::parse(input.profile.as_ref()).unwrap();
            assert_eq!(profile.site, site);
            assert_eq!(
                profile.credential_rail,
                if site == QoderSite::Cn {
                    QoderCredentialRail::CnOauth
                } else {
                    QoderCredentialRail::GlobalOauth
                }
            );

            let observations = server.observations.lock().unwrap().clone();
            let polls = observations
                .iter()
                .filter(|observation| observation.path == DEVICE_POLL_PATH)
                .collect::<Vec<_>>();
            assert_eq!(polls.len(), 2);
            for poll in polls {
                assert_eq!(poll.method, axum::http::Method::GET);
                let query = url::form_urlencoded::parse(
                    poll.query.as_deref().unwrap_or_default().as_bytes(),
                )
                .into_owned()
                .collect::<BTreeMap<_, _>>();
                assert_eq!(query.get("nonce"), Some(&flow.nonce));
                assert_eq!(query.get("verifier"), Some(&flow.code_verifier));
                assert_eq!(
                    query.get("challenge_method").map(String::as_str),
                    Some("S256")
                );
                assert_eq!(header_value(poll, "accept"), Some("application/json"));
                assert!(header_value(poll, "user-agent").is_none());
                assert_no_protocol_auth_headers(poll);
                assert!(poll.body.is_empty());
            }

            let userinfo = observations
                .iter()
                .filter(|observation| observation.path == USER_INFO_PATH)
                .collect::<Vec<_>>();
            assert_eq!(userinfo.len(), 1);
            assert_eq!(userinfo[0].method, axum::http::Method::GET);
            assert_eq!(
                header_value(userinfo[0], "authorization"),
                Some("Bearer poll-access")
            );
            assert_eq!(
                header_value(userinfo[0], "user-agent"),
                Some(QODER_OPENAPI_USER_AGENT)
            );

            let auth_status = observations
                .iter()
                .filter(|observation| observation.path == AUTH_STATUS_ACTUAL_PATH)
                .collect::<Vec<_>>();
            if site == QoderSite::Cn {
                assert_eq!(auth_status.len(), 1);
                assert_eq!(auth_status[0].method, axum::http::Method::POST);
                assert_eq!(auth_status[0].query.as_deref(), Some("Encode=1"));
                assert_eq!(
                    header_value(auth_status[0], "cosy-clienttype"),
                    Some(site.profile().client_type)
                );
                assert!(!auth_status[0].body.is_empty());
            } else {
                assert!(auth_status.is_empty());
            }
            assert_eq!(server.unexpected_count.load(Ordering::SeqCst), 0);
            server.server.abort();
        }
    }

    #[tokio::test]
    async fn device_refresh_http_contract_is_minimal_and_receipt_precedes_identity() {
        for site in [QoderSite::Global, QoderSite::Cn] {
            let server = serve_qoder_lifecycle(site).await;
            let account = qoder_lifecycle_account(site, &server.base_url);
            let observations_for_hook = Arc::clone(&server.observations);
            let mut receipt_calls = 0;
            let mut receipt_observation_counts = Vec::new();
            let mut receipt_access_tokens = Vec::new();
            let mut hook = |receipt: &AccountRefreshUpdate| {
                receipt_calls += 1;
                receipt_observation_counts.push(observations_for_hook.lock().unwrap().len());
                receipt_access_tokens.push(receipt.access_token.clone());
                Ok(())
            };
            let update = refresh_qoder_account(
                &reqwest::Client::new(),
                &account,
                1_700_000_000_000,
                &mut hook,
            )
            .await
            .unwrap();

            assert_eq!(receipt_calls, 1);
            assert_eq!(receipt_observation_counts, [1]);
            assert_eq!(receipt_access_tokens, [Some("rotated-access".to_string())]);
            assert_eq!(update.access_token.as_deref(), Some("rotated-access"));
            assert_eq!(update.refresh_token.as_deref(), Some("rotated-refresh"));
            assert_eq!(update.expires_at, Some(1_800_000_000_000));
            assert_eq!(
                update
                    .raw
                    .as_ref()
                    .and_then(|raw| raw.pointer("/qoderRefreshReceipt/site"))
                    .and_then(Value::as_str),
                Some(site.as_str())
            );

            let observations = server.observations.lock().unwrap().clone();
            assert_eq!(observations[0].method, axum::http::Method::POST);
            assert_eq!(observations[0].path, DEVICE_REFRESH_PATH);
            assert!(observations[0].query.is_none());
            assert_eq!(
                header_value(&observations[0], "content-type"),
                Some("application/json")
            );
            assert_eq!(
                header_value(&observations[0], "accept"),
                Some("application/json")
            );
            assert_eq!(
                header_value(&observations[0], "user-agent"),
                Some(QODER_OPENAPI_USER_AGENT)
            );
            assert_no_protocol_auth_headers(&observations[0]);
            assert_eq!(
                serde_json::from_slice::<Value>(&observations[0].body).unwrap(),
                json!({"refresh_token": "old-refresh"})
            );
            assert_eq!(observations[1].path, USER_INFO_PATH);
            assert_eq!(
                header_value(&observations[1], "authorization"),
                Some("Bearer rotated-access")
            );
            if site == QoderSite::Cn {
                assert_eq!(observations[2].path, AUTH_STATUS_ACTUAL_PATH);
            } else {
                assert_eq!(observations.len(), 2);
            }
            assert!(observations
                .iter()
                .all(|observation| { observation.path != "/algo/api/v3/user/jobToken" }));
            assert_eq!(server.unexpected_count.load(Ordering::SeqCst), 0);
            server.server.abort();
        }
    }

    #[tokio::test]
    async fn device_refresh_identity_drift_fails_after_rotation_receipt() {
        for site in [QoderSite::Global, QoderSite::Cn] {
            let server = serve_qoder_lifecycle(site).await;
            *server.user_uid.lock().unwrap() = "different-qoder-uid".to_string();
            let account = qoder_lifecycle_account(site, &server.base_url);
            let observations_for_hook = Arc::clone(&server.observations);
            let mut journaled = Vec::new();
            let mut hook = |receipt: &AccountRefreshUpdate| {
                journaled.push((
                    receipt.access_token.clone(),
                    receipt.refresh_token.clone(),
                    observations_for_hook.lock().unwrap().len(),
                ));
                Ok(())
            };
            let failure = refresh_qoder_account(
                &reqwest::Client::new(),
                &account,
                1_700_000_000_000,
                &mut hook,
            )
            .await
            .unwrap_err();

            assert_eq!(failure.status_code, 409);
            assert_eq!(
                failure.kind,
                crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant
            );
            assert!(failure.immediate_relogin);
            assert!(failure.outcome_unknown);
            assert!(!failure.retryable);
            assert_eq!(
                journaled,
                [(
                    Some("rotated-access".to_string()),
                    Some("rotated-refresh".to_string()),
                    1
                )]
            );
            let observations = server.observations.lock().unwrap().clone();
            assert_eq!(observations[0].path, DEVICE_REFRESH_PATH);
            assert_eq!(observations[1].path, USER_INFO_PATH);
            if site == QoderSite::Cn {
                assert_eq!(observations[2].path, AUTH_STATUS_ACTUAL_PATH);
            }
            assert_eq!(server.unexpected_count.load(Ordering::SeqCst), 0);
            server.server.abort();
        }
    }

    #[tokio::test]
    async fn device_refresh_classifies_terminal_rate_limit_server_and_unknown_outcomes() {
        let server = serve_qoder_lifecycle(QoderSite::Global).await;
        let account = qoder_lifecycle_account(QoderSite::Global, &server.base_url);
        for (status, expected_status, expected_kind, retryable, immediate_relogin) in [
            (
                400,
                502,
                crate::domain::accounts::oauth::OAuthErrorKind::ProviderRejected,
                false,
                false,
            ),
            (
                401,
                401,
                crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant,
                false,
                true,
            ),
            (
                403,
                403,
                crate::domain::accounts::oauth::OAuthErrorKind::ProviderRejected,
                false,
                false,
            ),
            (
                429,
                429,
                crate::domain::accounts::oauth::OAuthErrorKind::RateLimited,
                true,
                false,
            ),
            (
                500,
                502,
                crate::domain::accounts::oauth::OAuthErrorKind::Network,
                true,
                false,
            ),
        ] {
            server.refresh_status.store(status, Ordering::SeqCst);
            let mut receipt_calls = 0;
            let mut hook = |_: &AccountRefreshUpdate| {
                receipt_calls += 1;
                Ok(())
            };
            let failure = refresh_qoder_account(
                &reqwest::Client::new(),
                &account,
                1_700_000_000_000,
                &mut hook,
            )
            .await
            .unwrap_err();
            assert_eq!(receipt_calls, 0);
            assert_eq!(failure.status_code, expected_status);
            assert_eq!(failure.upstream_status, Some(status));
            assert_eq!(failure.kind, expected_kind);
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.immediate_relogin, immediate_relogin);
            assert!(!failure.outcome_unknown);
            assert!(!failure.endpoint_fallback_safe);
        }
        assert_eq!(server.unexpected_count.load(Ordering::SeqCst), 0);
        server.server.abort();

        let stopped = serve_qoder_lifecycle(QoderSite::Global).await;
        let account = qoder_lifecycle_account(QoderSite::Global, &stopped.base_url);
        stopped.server.abort();
        let _ = stopped.server.await;
        let mut hook = |_: &AccountRefreshUpdate| Ok(());
        let failure = refresh_qoder_account(
            &reqwest::Client::new(),
            &account,
            1_700_000_000_000,
            &mut hook,
        )
        .await
        .unwrap_err();
        assert_eq!(
            failure.kind,
            crate::domain::accounts::oauth::OAuthErrorKind::Unknown
        );
        assert!(failure.outcome_unknown);
        assert!(failure.immediate_relogin);
        assert!(!failure.retryable);
        assert!(!failure.endpoint_fallback_safe);
    }

    #[tokio::test]
    async fn pat_account_never_enters_device_refresh() {
        let server = serve_qoder_lifecycle(QoderSite::Global).await;
        let account = qoder_quota_account(
            QoderSite::Global,
            QoderCredentialRail::PatJobToken,
            &server.base_url,
        );
        let mut hook = |_: &AccountRefreshUpdate| Ok(());
        let failure = refresh_qoder_account(
            &reqwest::Client::new(),
            &account,
            1_700_000_000_000,
            &mut hook,
        )
        .await
        .unwrap_err();
        assert_eq!(failure.status_code, 400);
        assert_eq!(
            failure.kind,
            crate::domain::accounts::oauth::OAuthErrorKind::Unsupported
        );
        assert!(server.observations.lock().unwrap().is_empty());
        assert_eq!(server.unexpected_count.load(Ordering::SeqCst), 0);
        server.server.abort();
    }

    #[test]
    fn flow_store_serializes_poll_and_keeps_completed_result() {
        let (_, flow) = start_device_flow(QoderSite::Global, 0).unwrap();
        let mut store = QoderDeviceFlowStore::default();
        assert!(store.insert("device".to_string(), flow, 0).is_empty());
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(QoderDevicePollLease::Ready(_))
        ));
        assert!(matches!(
            store.begin_poll("device", 0),
            Some(QoderDevicePollLease::InProgress)
        ));
        let result = QoderDevicePollResult {
            pending: false,
            message: "done".to_string(),
            retry_after_secs: None,
            account_input: None,
        };
        assert!(store.finish_poll("device", result, 1));
        assert!(matches!(
            store.begin_poll("device", 2),
            Some(QoderDevicePollLease::Completed(_))
        ));
        assert!(store.begin_poll("device", 60_002).is_none());
    }

    #[test]
    fn flow_store_evicts_oldest_entry_at_global_capacity() {
        let (_, flow) = start_device_flow(QoderSite::Global, 0).unwrap();
        let mut store = QoderDeviceFlowStore::default();
        for index in 0..MAX_QODER_DEVICE_FLOWS {
            assert!(store
                .insert(format!("device-{index}"), flow.clone(), index as i64)
                .is_empty());
        }
        let evicted = store.insert("device-new".to_string(), flow, 100);
        assert_eq!(evicted, ["device-0"]);
        assert!(store.begin_poll("device-0", 100).is_none());
        assert!(matches!(
            store.begin_poll("device-new", 100),
            Some(QoderDevicePollLease::Ready(_))
        ));
    }

    #[test]
    fn expiry_parser_accepts_seconds_millis_and_dates() {
        assert_eq!(
            parse_expiry_ms(&json!(3600), &Value::Null, 1_000),
            Some(3_601_000)
        );
        assert_eq!(
            parse_expiry_ms(&json!(1_800_000_000_000_i64), &Value::Null, 0),
            Some(1_800_000_000_000)
        );
        assert_eq!(
            parse_expiry_ms(&json!("2026-08-13T00:00:00Z"), &Value::Null, 0),
            Some(1_786_579_200_000)
        );
        let token = parse_token_response(
            br#"{"device_token":"device","token":"token","access_token":"access"}"#,
            0,
        )
        .unwrap();
        assert_eq!(token.access_token(), Some("device"));

        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "2500".parse().unwrap());
        let limited = QoderClientError::upstream(
            StatusCode::TOO_MANY_REQUESTS,
            Some(&headers),
            "test",
            br#"{"message":"slow down"}"#,
        );
        assert_eq!(limited.kind, QoderErrorKind::RateLimited);
        assert_eq!(limited.retry_after_ms, Some(2_500));
        let plain_bad_request = QoderClientError::upstream(
            StatusCode::BAD_REQUEST,
            None,
            "test",
            br#"{"message":"invalid parameter"}"#,
        );
        assert_eq!(plain_bad_request.kind, QoderErrorKind::Protocol);
        assert!(plain_bad_request.terminal);
        let invalid_grant = QoderClientError::upstream(
            StatusCode::BAD_REQUEST,
            None,
            "test",
            br#"{"error":"invalid_grant"}"#,
        );
        assert_eq!(invalid_grant.kind, QoderErrorKind::InvalidGrant);
        assert!(invalid_grant.terminal);
    }

    #[test]
    fn account_layout_keeps_pat_and_oauth_rails_mutually_exclusive() {
        let endpoints = QoderEndpoints::for_site(QoderSite::Global);
        let machine = QoderMachineIdentity {
            machine_id: "0123456789abcdef0123456789abcdef0123".to_string(),
            machine_token: "machine-token".to_string(),
            machine_type: "5".to_string(),
        };
        let identity = QoderIdentity {
            name: String::new(),
            aid: "aid".to_string(),
            uid: "uid".to_string(),
            organization_id: String::new(),
            organization_name: String::new(),
            user_type: "personal_standard".to_string(),
            security_oauth_token: "job-token".to_string(),
            refresh_token: String::new(),
        };
        let input = account_input(QoderAccountDraft {
            endpoints: &endpoints,
            rail: QoderCredentialRail::PatJobToken,
            machine: &machine,
            identity,
            access_token: None,
            refresh_token: None,
            api_key: Some("pt-secret".to_string()),
            expires_at: None,
            login_method: "pat",
            now_ms: 1,
        })
        .unwrap();
        assert!(input.access_token.is_none());
        assert!(input.refresh_token.is_none());
        assert_eq!(input.api_key.as_deref(), Some("pt-secret"));
        assert_eq!(
            input.profile.as_ref().unwrap()["credentialRail"],
            "pat_job_token"
        );
    }

    #[test]
    fn gateway_wrapper_named_status_is_enforced() {
        let error = unwrap_qoder_json_response(
            br#"{"statusCode":"UNAUTHORIZED","body":"{\"message\":\"expired\"}"}"#,
        )
        .unwrap_err();
        assert_eq!(error.upstream_status, Some(StatusCode::UNAUTHORIZED));
        assert!(!error.message.contains("securityOauthToken"));
    }

    #[tokio::test]
    async fn global_quota_uses_actual_gateway_path_and_logical_signature_path() {
        let (base_url, observations, server) = serve_qoder_quota(false).await;
        let account = qoder_quota_account(
            QoderSite::Global,
            QoderCredentialRail::GlobalOauth,
            &base_url,
        );

        let quota = fetch_quota_usage(&reqwest::Client::new(), &account, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(quota["userQuota"]["remaining"], 80);
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        let request = &observations[0];
        assert_eq!(request.method, axum::http::Method::GET);
        assert_eq!(request.path, crate::domain::qoder::QODER_QUOTA_PATH);
        assert!(request.query.is_none());
        assert_eq!(
            request.headers["cosy-sigpath"],
            crate::domain::qoder::QODER_QUOTA_SIGNATURE_PATH
        );
        assert_eq!(request.headers["cosy-user"], "qoder-uid");
        assert_eq!(request.headers["appcode"], QODER_APP_CODE);
        assert!(request.headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer COSY."));
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn cn_quota_binds_organization_query_without_inventing_quota_key() {
        let (base_url, observations, server) = serve_qoder_quota(false).await;
        let account = qoder_quota_account(QoderSite::Cn, QoderCredentialRail::CnOauth, &base_url);

        fetch_quota_usage(&reqwest::Client::new(), &account, Duration::from_secs(5))
            .await
            .unwrap();
        let observations = observations.lock().unwrap();
        let request = &observations[0];
        let query = url::form_urlencoded::parse(request.query.as_deref().unwrap().as_bytes())
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(query.get("orgId").map(String::as_str), Some("qoder-org-cn"));
        assert!(!query.contains_key("quotaKey"));
        assert_eq!(request.headers["cosy-clienttype"], "0");
        assert_eq!(
            request.headers["cosy-sigpath"],
            crate::domain::qoder::QODER_QUOTA_SIGNATURE_PATH
        );
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn pat_quota_reexchanges_transient_job_token_once_after_auth_failure() {
        let (base_url, observations, server) = serve_qoder_quota(true).await;
        let account = qoder_quota_account(
            QoderSite::Global,
            QoderCredentialRail::PatJobToken,
            &base_url,
        );

        let quota = fetch_quota_usage(&reqwest::Client::new(), &account, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(quota["userQuota"]["remaining"], 80);
        assert_eq!(account.api_key.as_deref(), Some("pt-qoder-secret"));
        assert!(account.access_token.is_none());
        assert!(account.refresh_token.is_none());
        let observations = observations.lock().unwrap();
        assert_eq!(
            observations
                .iter()
                .map(|request| (request.method.clone(), request.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    axum::http::Method::POST,
                    crate::domain::qoder::QODER_PAT_EXCHANGE_PATH
                ),
                (
                    axum::http::Method::GET,
                    crate::domain::qoder::QODER_QUOTA_PATH
                ),
                (
                    axum::http::Method::POST,
                    crate::domain::qoder::QODER_PAT_EXCHANGE_PATH
                ),
                (
                    axum::http::Method::GET,
                    crate::domain::qoder::QODER_QUOTA_PATH
                ),
            ]
        );
        drop(observations);
        server.abort();
    }
}
