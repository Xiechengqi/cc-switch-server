use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::clients::oauth::refresh::AccountRefreshFailure;
use crate::domain::accounts::oauth::OAuthErrorKind;
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;

pub const AMAZON_Q_OIDC_REGION: &str = "us-east-1";
pub const AMAZON_Q_START_URL: &str = "https://view.awsapps.com/start";
pub const AMAZON_Q_CLIENT_NAME: &str = "Amazon Q Developer for command line";
pub const AMAZON_Q_CLIENT_TYPE: &str = "public";
pub const AMAZON_Q_DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
pub const AMAZON_Q_REFRESH_GRANT: &str = "refresh_token";
pub const AMAZON_Q_SCOPES: &[&str] = &[
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
];

const MAX_OIDC_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct AmazonQDeviceFlowStore {
    pending: BTreeMap<String, PendingAmazonQDeviceFlow>,
}

impl AmazonQDeviceFlowStore {
    pub fn insert(&mut self, device_code: String, flow: PendingAmazonQDeviceFlow, now_ms: i64) {
        self.pending.retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending.insert(device_code, flow);
    }

    pub fn get(&mut self, device_code: &str, now_ms: i64) -> Option<PendingAmazonQDeviceFlow> {
        self.pending.retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending.get(device_code).cloned()
    }

    pub fn remove(&mut self, device_code: &str) {
        self.pending.remove(device_code);
    }
}

#[derive(Debug, Clone)]
pub struct PendingAmazonQDeviceFlow {
    client_id: String,
    client_secret: String,
    client_secret_expires_at: Option<i64>,
    region: String,
    start_url: String,
    expires_at_ms: i64,
    #[cfg(test)]
    oidc_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmazonQDeviceCodeResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_uri: String,
    #[serde(
        default,
        alias = "verification_uri_complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub verification_uri_complete: Option<String>,
    #[serde(alias = "expires_in")]
    pub expires_in: u64,
    pub interval: u64,
    pub region: String,
    pub start_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmazonQDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
pub struct AmazonQDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl AmazonQDeviceError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

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

    fn remote(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: sanitize_remote_message(&message.into()),
        }
    }
}

impl fmt::Display for AmazonQDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AmazonQDeviceError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientResponse {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    client_secret_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(alias = "expires_in")]
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    #[serde(default, alias = "access_token")]
    access_token: Option<String>,
    #[serde(default, alias = "refresh_token")]
    refresh_token: Option<String>,
    #[serde(default, alias = "expires_in")]
    expires_in: Option<i64>,
    #[serde(default, alias = "token_type")]
    token_type: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, alias = "error_description")]
    error_description: Option<String>,
    #[serde(flatten)]
    extra: Value,
}

pub async fn start_device_flow(
    http: &reqwest::Client,
    now_ms: i64,
) -> Result<(AmazonQDeviceCodeResponse, PendingAmazonQDeviceFlow), AmazonQDeviceError> {
    start_device_flow_with_base(http, now_ms, None).await
}

async fn start_device_flow_with_base(
    http: &reqwest::Client,
    now_ms: i64,
    oidc_base_url: Option<&str>,
) -> Result<(AmazonQDeviceCodeResponse, PendingAmazonQDeviceFlow), AmazonQDeviceError> {
    let base = oidc_base_url_for(AMAZON_Q_OIDC_REGION, oidc_base_url)?;
    let client: RegisterClientResponse = post_json(
        http,
        &format!("{base}/client/register"),
        json!({
            "clientName": AMAZON_Q_CLIENT_NAME,
            "clientType": AMAZON_Q_CLIENT_TYPE,
            "scopes": AMAZON_Q_SCOPES,
        }),
        "Amazon Q client registration",
    )
    .await?;
    validate_client_registration(&client)?;
    let device: DeviceAuthorizationResponse = post_json(
        http,
        &format!("{base}/device_authorization"),
        json!({
            "clientId": client.client_id,
            "clientSecret": client.client_secret,
            "startUrl": AMAZON_Q_START_URL,
        }),
        "Amazon Q device authorization",
    )
    .await?;
    validate_device_authorization(&device)?;
    let expires_at_ms = now_ms.saturating_add(
        i64::try_from(device.expires_in)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000),
    );
    let flow = PendingAmazonQDeviceFlow {
        client_id: client.client_id,
        client_secret: client.client_secret,
        client_secret_expires_at: client.client_secret_expires_at,
        region: AMAZON_Q_OIDC_REGION.to_string(),
        start_url: AMAZON_Q_START_URL.to_string(),
        expires_at_ms,
        #[cfg(test)]
        oidc_base_url: oidc_base_url.map(str::to_string),
    };
    let response = AmazonQDeviceCodeResponse {
        device_code: device.device_code,
        user_code: device.user_code,
        verification_uri: device.verification_uri,
        verification_uri_complete: device.verification_uri_complete,
        expires_in: device.expires_in,
        interval: device.interval.unwrap_or(5).max(1),
        region: AMAZON_Q_OIDC_REGION.to_string(),
        start_url: AMAZON_Q_START_URL.to_string(),
    };
    Ok((response, flow))
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: PendingAmazonQDeviceFlow,
    now_ms: i64,
) -> Result<AmazonQDevicePollResult, AmazonQDeviceError> {
    let device_code = token_only(device_code, "deviceCode")?;
    if flow.expires_at_ms <= now_ms {
        return Err(AmazonQDeviceError::unauthorized(
            "Amazon Q device code expired",
        ));
    }
    #[cfg(test)]
    let override_base = flow.oidc_base_url.as_deref();
    #[cfg(not(test))]
    let override_base = None;
    let base = oidc_base_url_for(&flow.region, override_base)?;
    let response = http
        .post(format!("{base}/token"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .json(&json!({
            "clientId": flow.client_id,
            "clientSecret": flow.client_secret,
            "deviceCode": device_code,
            "grantType": AMAZON_Q_DEVICE_GRANT,
        }))
        .send()
        .await
        .map_err(|error| {
            AmazonQDeviceError::bad_gateway(format!("Amazon Q token poll failed: {error}"))
        })?;
    let (status, token) = parse_token_response(response, "Amazon Q token poll").await?;
    if let Some(error) = token.error.as_deref() {
        return match error {
            "authorization_pending" | "slow_down" => Ok(AmazonQDevicePollResult {
                pending: true,
                message: if error == "slow_down" {
                    "authorization pending; slow down polling"
                } else {
                    "authorization pending"
                }
                .to_string(),
                retry_after_secs: Some(if error == "slow_down" { 10 } else { 5 }),
                account_input: None,
            }),
            "expired_token" => Err(AmazonQDeviceError::unauthorized(
                "Amazon Q device code expired",
            )),
            "access_denied" => Err(AmazonQDeviceError::unauthorized(
                "Amazon Q device authorization was denied",
            )),
            _ => Err(AmazonQDeviceError::remote(
                status,
                format!(
                    "{}: {}",
                    error,
                    token.error_description.as_deref().unwrap_or_default()
                ),
            )),
        };
    }
    if !status.is_success() {
        return Err(AmazonQDeviceError::remote(
            status,
            "Amazon Q token endpoint rejected the device flow",
        ));
    }
    let access_token = required_token(token.access_token.as_deref(), "accessToken")?;
    let refresh_token = required_token(token.refresh_token.as_deref(), "refreshToken")?;
    let account_input =
        account_input_from_token(&flow, &token, access_token, refresh_token, now_ms);
    Ok(AmazonQDevicePollResult {
        pending: false,
        message: "Amazon Q Developer device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

pub async fn refresh_amazon_q_account(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    if account.provider_type != ProviderType::AmazonQOAuth {
        return Err(AccountRefreshFailure::bad_request(format!(
            "expected amazon_q_oauth account, got {}",
            account.provider_type.as_str()
        )));
    }
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AccountRefreshFailure::bad_request("Amazon Q refresh token is required"))?;
    let client_id = account_string(account, &["/clientId", "/client_id"])
        .ok_or_else(|| refresh_relogin("Amazon Q account is missing its OIDC clientId"))?;
    let client_secret = account_string(account, &["/clientSecret", "/client_secret"])
        .ok_or_else(|| refresh_relogin("Amazon Q account is missing its OIDC clientSecret"))?;
    if account_i64(
        account,
        &["/clientSecretExpiresAt", "/client_secret_expires_at"],
    )
    .is_some_and(|expires_at| expires_at.saturating_mul(1000) <= now_ms)
    {
        return Err(refresh_relogin(
            "Amazon Q OIDC client registration expired; sign in again",
        ));
    }
    let region = account_string(account, &["/authRegion", "/auth_region"])
        .unwrap_or_else(|| AMAZON_Q_OIDC_REGION.to_string());
    if region != AMAZON_Q_OIDC_REGION {
        return Err(AccountRefreshFailure::bad_request(
            "Amazon Q Builder ID OIDC region must be us-east-1",
        ));
    }
    #[cfg(test)]
    let override_base = account_string(account, &["/testAmazonQOidcBaseUrl"]);
    #[cfg(not(test))]
    let override_base: Option<String> = None;
    let base = oidc_base_url_for(&region, override_base.as_deref())
        .map_err(|error| AccountRefreshFailure::bad_request(error.message))?;
    let mut response = http
        .post(format!("{base}/token"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .json(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "refreshToken": refresh_token,
            "grantType": AMAZON_Q_REFRESH_GRANT,
        }))
        .send()
        .await
        .map_err(|error| {
            AccountRefreshFailure::bad_gateway(format!("Amazon Q refresh request failed: {error}"))
        })?;
    let status = response.status();
    let body =
        crate::infra::http::read_response_body_limited(&mut response, MAX_OIDC_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                AccountRefreshFailure::bad_gateway(format!(
                    "Amazon Q refresh response failed: {error}"
                ))
            })?;
    let token: TokenResponse = serde_json::from_slice(&body).map_err(|error| {
        AccountRefreshFailure::parse(format!("parse Amazon Q refresh response: {error}"))
    })?;
    if let Some(error) = token.error.as_deref() {
        return Err(refresh_remote_error(
            status,
            error,
            token.error_description.as_deref(),
        ));
    }
    if !status.is_success() {
        return Err(refresh_remote_error(status, "provider_rejected", None));
    }
    let access_token = required_token(token.access_token.as_deref(), "accessToken")
        .map_err(|error| AccountRefreshFailure::parse(error.message))?;
    let returned_refresh_token = token
        .refresh_token
        .as_deref()
        .and_then(|value| token_only(value, "refreshToken").ok())
        .unwrap_or(refresh_token);
    let mut raw = account.raw.clone().unwrap_or_else(|| json!({}));
    if !raw.is_object() {
        raw = json!({});
    }
    if let Some(object) = raw.as_object_mut() {
        object.insert("lastRefreshSource".to_string(), json!("amazon_q_sso_oidc"));
        object.insert("lastRefreshAtMs".to_string(), json!(now_ms));
    }
    Ok(AccountRefreshUpdate {
        access_token: Some(access_token.to_string()),
        refresh_token: Some(returned_refresh_token.to_string()),
        token_type: Some(
            token
                .token_type
                .as_deref()
                .filter(|value| value.eq_ignore_ascii_case("bearer"))
                .unwrap_or("Bearer")
                .to_string(),
        ),
        scopes: Some(
            AMAZON_Q_SCOPES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ),
        profile: account.profile.clone(),
        raw: Some(raw),
        expires_at: token
            .expires_in
            .map(|seconds| now_ms.saturating_add(seconds.max(1).saturating_mul(1000))),
        last_refresh_error: None,
        ..AccountRefreshUpdate::default()
    })
}

fn account_input_from_token(
    flow: &PendingAmazonQDeviceFlow,
    token: &TokenResponse,
    access_token: &str,
    refresh_token: &str,
    now_ms: i64,
) -> UpsertAccountInput {
    let account_id = format!("amazon_q_{}", &sha256_hex(refresh_token)[..24]);
    let expires_at = token
        .expires_in
        .map(|seconds| now_ms.saturating_add(seconds.max(1).saturating_mul(1000)));
    let profile = json!({
        "accountId": account_id,
        "authRegion": flow.region,
        "runtimeRegion": "us-east-1",
        "startUrl": flow.start_url,
        "authMethod": "builder_id",
        "provider": "AmazonQDeveloper",
        "endpoint": "amazon_q_cli",
    });
    let raw = json!({
        "clientId": flow.client_id,
        "clientSecret": flow.client_secret,
        "clientSecretExpiresAt": flow.client_secret_expires_at,
        "authRegion": flow.region,
        "runtimeRegion": "us-east-1",
        "startUrl": flow.start_url,
        "authMethod": "builder_id",
        "provider": "AmazonQDeveloper",
        "endpoint": "amazon_q_cli",
        "tokenMetadata": token.extra,
        "importedBy": "amazon_q_device_flow",
        "importedAtMs": now_ms,
    });
    UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::AmazonQOAuth,
        email: None,
        access_token: Some(access_token.to_string()),
        refresh_token: Some(refresh_token.to_string()),
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key: None,
        extra_headers: None,
        scopes: AMAZON_Q_SCOPES
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        profile: Some(profile),
        raw: Some(raw),
        subscription_level: Some("Amazon Q Developer".to_string()),
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    }
}

async fn post_json<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    url: &str,
    body: Value,
    context: &str,
) -> Result<T, AmazonQDeviceError> {
    let mut response = http
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| AmazonQDeviceError::bad_gateway(format!("{context} failed: {error}")))?;
    let status = response.status();
    let bytes =
        crate::infra::http::read_response_body_limited(&mut response, MAX_OIDC_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                AmazonQDeviceError::bad_gateway(format!("read {context} response failed: {error}"))
            })?;
    if !status.is_success() {
        return Err(AmazonQDeviceError::remote(
            status,
            remote_error_message(&bytes).unwrap_or_else(|| format!("{context} rejected")),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| AmazonQDeviceError::bad_gateway(format!("parse {context}: {error}")))
}

async fn parse_token_response(
    mut response: reqwest::Response,
    context: &str,
) -> Result<(StatusCode, TokenResponse), AmazonQDeviceError> {
    let status = response.status();
    let bytes =
        crate::infra::http::read_response_body_limited(&mut response, MAX_OIDC_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                AmazonQDeviceError::bad_gateway(format!("read {context} response failed: {error}"))
            })?;
    let token = serde_json::from_slice(&bytes).map_err(|error| {
        if status.is_success() {
            AmazonQDeviceError::bad_gateway(format!("parse {context} response: {error}"))
        } else {
            AmazonQDeviceError::remote(
                status,
                remote_error_message(&bytes).unwrap_or_else(|| format!("{context} rejected")),
            )
        }
    })?;
    Ok((status, token))
}

fn validate_client_registration(client: &RegisterClientResponse) -> Result<(), AmazonQDeviceError> {
    token_only(&client.client_id, "clientId")?;
    token_only(&client.client_secret, "clientSecret")?;
    Ok(())
}

fn validate_device_authorization(
    device: &DeviceAuthorizationResponse,
) -> Result<(), AmazonQDeviceError> {
    token_only(&device.device_code, "deviceCode")?;
    token_only(&device.user_code, "userCode")?;
    let verification = url::Url::parse(&device.verification_uri).map_err(|_| {
        AmazonQDeviceError::bad_gateway("Amazon Q verificationUri is not a valid URL")
    })?;
    if verification.scheme() != "https" {
        return Err(AmazonQDeviceError::bad_gateway(
            "Amazon Q verificationUri must use HTTPS",
        ));
    }
    if device.expires_in == 0 {
        return Err(AmazonQDeviceError::bad_gateway(
            "Amazon Q device authorization returned expiresIn=0",
        ));
    }
    Ok(())
}

fn oidc_base_url_for(
    region: &str,
    override_base: Option<&str>,
) -> Result<String, AmazonQDeviceError> {
    if region != AMAZON_Q_OIDC_REGION {
        return Err(AmazonQDeviceError::bad_request(
            "Amazon Q Builder ID OIDC region must be us-east-1",
        ));
    }
    #[cfg(test)]
    if let Some(value) = override_base {
        let parsed = url::Url::parse(value).map_err(|_| {
            AmazonQDeviceError::bad_request("Amazon Q test OIDC base URL is invalid")
        })?;
        let loopback = parsed.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if parsed.scheme() != "http"
            || !loopback
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AmazonQDeviceError::bad_request(
                "Amazon Q test OIDC base URL must be a credential-free loopback HTTP origin",
            ));
        }
        return Ok(value.trim_end_matches('/').to_string());
    }
    #[cfg(not(test))]
    let _ = override_base;
    Ok(format!("https://oidc.{region}.amazonaws.com"))
}

fn token_only<'a>(value: &'a str, field: &str) -> Result<&'a str, AmazonQDeviceError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 16 * 1024
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AmazonQDeviceError::bad_gateway(format!(
            "Amazon Q {field} is missing or malformed"
        )));
    }
    Ok(value)
}

fn required_token<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, AmazonQDeviceError> {
    token_only(value.unwrap_or_default(), field)
}

fn account_string(account: &Account, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        account
            .raw
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .or_else(|| {
                account
                    .profile
                    .as_ref()
                    .and_then(|value| value.pointer(pointer))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn account_i64(account: &Account, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        account
            .raw
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .or_else(|| {
                account
                    .profile
                    .as_ref()
                    .and_then(|value| value.pointer(pointer))
            })
            .and_then(Value::as_i64)
    })
}

fn refresh_relogin(message: impl Into<String>) -> AccountRefreshFailure {
    AccountRefreshFailure {
        status_code: 401,
        upstream_status: None,
        message: message.into(),
        kind: OAuthErrorKind::InvalidGrant,
        retryable: false,
        retry_after_ms: None,
        immediate_relogin: true,
        outcome_unknown: false,
        endpoint_fallback_safe: false,
    }
}

fn refresh_remote_error(
    status: StatusCode,
    code: &str,
    description: Option<&str>,
) -> AccountRefreshFailure {
    let invalid = matches!(code, "invalid_grant" | "invalid_client" | "expired_token")
        || matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        );
    let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
    AccountRefreshFailure {
        status_code: if invalid { 401 } else { status.as_u16() },
        upstream_status: Some(status.as_u16()),
        message: sanitize_remote_message(&format!(
            "Amazon Q refresh rejected ({code}): {}",
            description.unwrap_or_default()
        )),
        kind: if invalid {
            OAuthErrorKind::InvalidGrant
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            OAuthErrorKind::RateLimited
        } else {
            OAuthErrorKind::ProviderRejected
        },
        retryable,
        retry_after_ms: None,
        immediate_relogin: invalid,
        outcome_unknown: false,
        endpoint_fallback_safe: false,
    }
}

fn remote_error_message(bytes: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    [
        "/error_description",
        "/errorDescription",
        "/message",
        "/error",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .map(sanitize_remote_message)
}

fn sanitize_remote_message(message: &str) -> String {
    let compact = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    compact.chars().take(512).collect()
}

fn sha256_hex(value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-server:amazon-q-account:v1\0");
    digest.update(value.as_bytes());
    hex::encode(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_oidc_fixture() -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response_body in [
                json!({
                    "clientId":"amazon-q-client",
                    "clientSecret":"amazon-q-secret",
                "clientSecretExpiresAt":4_102_444_800_i64
                }),
                json!({
                    "deviceCode":"device-code",
                    "userCode":"ABCD-EFGH",
                    "verificationUri":"https://device.sso.us-east-1.amazonaws.com/",
                    "verificationUriComplete":"https://device.sso.us-east-1.amazonaws.com/?user_code=ABCD-EFGH",
                    "expiresIn":600,
                    "interval":2
                }),
                json!({
                    "accessToken":"amazon-q-access",
                    "refreshToken":"amazon-q-refresh",
                    "expiresIn":3600,
                    "tokenType":"Bearer"
                }),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(header_end) =
                        bytes.windows(4).position(|value| value == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or_default();
                        if bytes.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8_lossy(&bytes).to_string());
                let body = serde_json::to_vec(&response_body).unwrap();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(&body).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn device_flow_uses_official_amazon_q_identity_and_creates_independent_account() {
        let (base, server) = spawn_oidc_fixture().await;
        let http = reqwest::Client::new();
        let (device, flow) = start_device_flow_with_base(&http, 1_000, Some(&base))
            .await
            .unwrap();
        let result = poll_device_flow(&http, &device.device_code, flow, 2_000)
            .await
            .unwrap();
        let input = result.account_input.unwrap();
        assert_eq!(input.provider_type, ProviderType::AmazonQOAuth);
        assert!(input.id.as_deref().unwrap().starts_with("amazon_q_"));
        assert_eq!(input.profile.as_ref().unwrap()["endpoint"], "amazon_q_cli");
        assert_eq!(input.scopes, AMAZON_Q_SCOPES);

        let requests = server.await.unwrap();
        assert!(requests[0].contains("Amazon Q Developer for command line"));
        assert!(requests[0].contains("codewhisperer:conversations"));
        assert!(!requests[0].contains("kiro-oauth-client"));
        assert!(requests[2].contains(AMAZON_Q_DEVICE_GRANT));
    }

    #[test]
    fn flow_store_never_crosses_device_code_or_expiry() {
        let mut store = AmazonQDeviceFlowStore::default();
        let flow = PendingAmazonQDeviceFlow {
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            client_secret_expires_at: None,
            region: AMAZON_Q_OIDC_REGION.to_string(),
            start_url: AMAZON_Q_START_URL.to_string(),
            expires_at_ms: 2_000,
            oidc_base_url: None,
        };
        store.insert("one".to_string(), flow, 1_000);
        assert!(store.get("two", 1_500).is_none());
        assert!(store.get("one", 2_000).is_none());
    }
}
