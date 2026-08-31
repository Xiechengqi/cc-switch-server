use std::collections::BTreeMap;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::{AccountQuota, AccountQuotaTier, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;

pub(crate) const DEFAULT_REGION: &str = crate::domain::providers::kiro::DEFAULT_REGION;
pub(crate) const DEFAULT_START_URL: &str = "https://view.awsapps.com/start";
pub(crate) const KIRO_CLIENT_NAME: &str = "kiro-oauth-client";
pub(crate) const KIRO_CLIENT_TYPE: &str = "public";
pub(crate) const KIRO_ISSUER_URL: &str =
    "https://identitycenter.amazonaws.com/ssoins-722374e8c3c8e6c6";
pub(crate) const KIRO_AUTH_METHOD_BUILDER_ID: &str = "builder-id";
pub(crate) const KIRO_AUTH_METHOD_IDC: &str = "idc";
pub(crate) const KIRO_AUTH_METHOD_API_KEY: &str = "api_key";
pub(crate) const KIRO_AUTH_METHOD_SOCIAL: &str = "social";
const KIRO_SOCIAL_CLIENT_ID: &str = "kiro-cli";
pub(crate) use crate::domain::providers::kiro::{BUILDER_ID_PROFILE_ARN, SOCIAL_PROFILE_ARN};

#[derive(Debug, Clone, Default)]
pub struct KiroDeviceFlowStore {
    pending: BTreeMap<String, PendingKiroDeviceFlow>,
    pending_social: BTreeMap<String, PendingKiroSocialDeviceFlow>,
}

impl KiroDeviceFlowStore {
    pub fn insert(&mut self, device_code: String, flow: PendingKiroDeviceFlow, now_ms: i64) {
        self.pending.retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending.insert(device_code, flow);
    }

    pub fn get(&mut self, device_code: &str, now_ms: i64) -> Option<PendingKiroDeviceFlow> {
        self.pending.retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending.get(device_code).cloned()
    }

    pub fn remove(&mut self, device_code: &str) {
        self.pending.remove(device_code);
    }

    pub fn insert_social(
        &mut self,
        device_code: String,
        flow: PendingKiroSocialDeviceFlow,
        now_ms: i64,
    ) {
        self.pending_social
            .retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending_social.insert(device_code, flow);
    }

    pub fn get_social(
        &mut self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<PendingKiroSocialDeviceFlow> {
        self.pending_social
            .retain(|_, flow| flow.expires_at_ms > now_ms);
        self.pending_social.get(device_code).cloned()
    }

    pub fn remove_social(&mut self, device_code: &str) {
        self.pending_social.remove(device_code);
    }
}

#[derive(Debug, Clone)]
pub struct PendingKiroDeviceFlow {
    client_id: String,
    client_secret: String,
    client_secret_expires_at: Option<i64>,
    region: String,
    start_url: String,
    issuer_url: String,
    auth_method: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct PendingKiroSocialDeviceFlow {
    provider: String,
    auth_region: String,
    client_id: String,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroDeviceCodeResponse {
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
pub struct KiroDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Clone)]
pub struct KiroDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl KiroDeviceError {
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
            message: crate::logging::mask_kiro_api_keys(&message.into()),
        }
    }
}

impl fmt::Display for KiroDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for KiroDeviceError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegisterClientResponse {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    #[serde(default)]
    pub(crate) client_secret_expires_at: Option<i64>,
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
struct SocialDeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    #[serde(default)]
    verification_uri: Option<String>,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    expires_in_milliseconds: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    interval_in_milliseconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuilderIdTokenResponse {
    #[serde(default, alias = "access_token")]
    access_token: Option<String>,
    #[serde(default, alias = "refresh_token")]
    refresh_token: Option<String>,
    #[serde(default, alias = "expires_in")]
    expires_in: Option<i64>,
    #[serde(default, alias = "profile_arn")]
    profile_arn: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, alias = "error_description")]
    error_description: Option<String>,
    #[serde(flatten)]
    extra: Value,
}

impl BuilderIdTokenResponse {
    fn first_email(&self) -> Option<String> {
        first_email([
            self.extra
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_string),
            self.extra
                .get("accountEmail")
                .and_then(Value::as_str)
                .map(str::to_string),
            self.extra
                .get("userEmail")
                .and_then(Value::as_str)
                .map(str::to_string),
            self.extra
                .get("id_token")
                .and_then(Value::as_str)
                .and_then(email_from_jwt),
            self.access_token.as_deref().and_then(email_from_jwt),
            self.refresh_token.as_deref().and_then(email_from_jwt),
        ])
    }
}

pub async fn start_device_flow(
    http: &reqwest::Client,
    region: Option<&str>,
    start_url: Option<&str>,
    issuer_url: Option<&str>,
    now_ms: i64,
) -> Result<(KiroDeviceCodeResponse, PendingKiroDeviceFlow), KiroDeviceError> {
    let region = normalize_region(region.unwrap_or(DEFAULT_REGION))?;
    let start_url = normalize_start_url(start_url.unwrap_or(DEFAULT_START_URL))?;
    let issuer_url = normalize_issuer_url(issuer_url.unwrap_or(KIRO_ISSUER_URL))?;
    let auth_method = if issuer_url == KIRO_ISSUER_URL && start_url == DEFAULT_START_URL {
        KIRO_AUTH_METHOD_BUILDER_ID
    } else {
        KIRO_AUTH_METHOD_IDC
    };
    let client = register_client_with_issuer(http, &region, &issuer_url).await?;
    let device = request_device_authorization(http, &region, &client, &start_url).await?;
    let expires_at_ms = now_ms.saturating_add((device.expires_in as i64).saturating_mul(1000));
    let flow = PendingKiroDeviceFlow {
        client_id: client.client_id,
        client_secret: client.client_secret,
        client_secret_expires_at: client.client_secret_expires_at,
        region: region.clone(),
        start_url: start_url.clone(),
        issuer_url,
        auth_method: auth_method.to_string(),
        expires_at_ms,
    };
    let response = KiroDeviceCodeResponse {
        device_code: device.device_code,
        user_code: device.user_code,
        verification_uri: device.verification_uri,
        verification_uri_complete: device.verification_uri_complete,
        expires_in: device.expires_in,
        interval: device.interval.unwrap_or(5),
        region,
        start_url,
    };
    Ok((response, flow))
}

pub async fn start_social_device_flow(
    http: &reqwest::Client,
    provider: &str,
    auth_region: Option<&str>,
    now_ms: i64,
) -> Result<(KiroDeviceCodeResponse, PendingKiroSocialDeviceFlow), KiroDeviceError> {
    let provider = normalize_social_provider(provider)?;
    let auth_region = normalize_region(auth_region.unwrap_or(DEFAULT_REGION))?;
    let url =
        format!("https://prod.{auth_region}.auth.desktop.kiro.dev/oauth/device/authorization");
    let response = http
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "clientId": KIRO_SOCIAL_CLIENT_ID,
            "loginProvider": social_provider_label(&provider),
        }))
        .send()
        .await
        .map_err(|error| {
            KiroDeviceError::bad_gateway(format!(
                "kiro social device authorization failed: {error}"
            ))
        })?;
    let device: SocialDeviceAuthorizationResponse =
        handle_json_response(response, "kiro social device authorization").await?;
    let expires_in = device
        .expires_in
        .or_else(|| device.expires_in_milliseconds.map(|value| value / 1000))
        .unwrap_or(300)
        .max(1);
    let interval = device
        .interval
        .or_else(|| device.interval_in_milliseconds.map(|value| value / 1000))
        .unwrap_or(5)
        .max(1);
    let verification_uri = device
        .verification_uri_complete
        .clone()
        .or(device.verification_uri.clone())
        .ok_or_else(|| {
            KiroDeviceError::bad_gateway("kiro social device authorization lacks verification URI")
        })?;
    let expires_at_ms = now_ms.saturating_add((expires_in as i64).saturating_mul(1000));
    let flow = PendingKiroSocialDeviceFlow {
        provider: provider.clone(),
        auth_region: auth_region.clone(),
        client_id: KIRO_SOCIAL_CLIENT_ID.to_string(),
        expires_at_ms,
    };
    Ok((
        KiroDeviceCodeResponse {
            device_code: device.device_code,
            user_code: device.user_code,
            verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_in,
            interval,
            region: auth_region,
            start_url: format!("social:{provider}"),
        },
        flow,
    ))
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: PendingKiroDeviceFlow,
    now_ms: i64,
) -> Result<KiroDevicePollResult, KiroDeviceError> {
    let device_code = device_code.trim();
    if device_code.is_empty() {
        return Err(KiroDeviceError::bad_request("deviceCode is required"));
    }
    if flow.expires_at_ms <= now_ms {
        return Err(KiroDeviceError::unauthorized("device code expired"));
    }
    let token = poll_builder_id_token(http, &flow, device_code).await?;
    if let Some(error) = token.error.as_deref() {
        return Ok(KiroDevicePollResult {
            pending: true,
            message: if error == "slow_down" {
                "authorization pending; slow down polling"
            } else {
                "authorization pending"
            }
            .to_string(),
            retry_after_secs: Some(if error == "slow_down" { 10 } else { 5 }),
            account_input: None,
        });
    }
    let Some(access_token) = token.access_token.clone() else {
        return Ok(KiroDevicePollResult {
            pending: true,
            message: "authorization pending".to_string(),
            retry_after_secs: Some(5),
            account_input: None,
        });
    };
    let account_input = account_input_from_token(http, token, flow, access_token, now_ms).await?;
    Ok(KiroDevicePollResult {
        pending: false,
        message: "kiro device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

pub async fn poll_social_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    flow: PendingKiroSocialDeviceFlow,
    now_ms: i64,
) -> Result<KiroDevicePollResult, KiroDeviceError> {
    let device_code = device_code.trim();
    if device_code.is_empty() {
        return Err(KiroDeviceError::bad_request("deviceCode is required"));
    }
    if flow.expires_at_ms <= now_ms {
        return Err(KiroDeviceError::unauthorized("device code expired"));
    }
    let url = format!(
        "https://prod.{}.auth.desktop.kiro.dev/oauth/device/poll",
        flow.auth_region
    );
    let response = http
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "deviceCode": device_code,
            "clientId": flow.client_id,
        }))
        .send()
        .await
        .map_err(|error| {
            KiroDeviceError::bad_gateway(format!("kiro social device poll failed: {error}"))
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| format!("HTTP {status}"));
    let token: Value = serde_json::from_str(&body).map_err(|error| {
        KiroDeviceError::remote(
            status,
            format!("parse kiro social device response failed: {error}"),
        )
    })?;
    let error = string_at(&token, &["/error"]);
    if matches!(
        error.as_deref(),
        Some("authorization_pending" | "slow_down")
    ) {
        return Ok(KiroDevicePollResult {
            pending: true,
            message: "authorization pending".to_string(),
            retry_after_secs: Some(if error.as_deref() == Some("slow_down") {
                10
            } else {
                5
            }),
            account_input: None,
        });
    }
    if !status.is_success() || error.is_some() {
        let message = string_at(
            &token,
            &["/errorDescription", "/error_description", "/message"],
        )
        .or(error)
        .unwrap_or(body);
        let status = if status.is_success() {
            StatusCode::UNAUTHORIZED
        } else {
            status
        };
        return Err(KiroDeviceError::remote(status, message));
    }
    let access_token = string_at(&token, &["/accessToken", "/access_token"])
        .ok_or_else(|| KiroDeviceError::bad_gateway("kiro social token lacks accessToken"))?;
    let refresh_token = string_at(&token, &["/refreshToken", "/refresh_token"])
        .ok_or_else(|| KiroDeviceError::bad_gateway("kiro social token lacks refreshToken"))?;
    let expires_in = token
        .pointer("/expiresIn")
        .or_else(|| token.pointer("/expires_in"))
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    let token_profile_arn = string_at(&token, &["/profileArn", "/profile_arn"])
        .map(|profile_arn| {
            crate::domain::providers::kiro::parse_profile_arn(&profile_arn)
                .map(|profile| profile.arn)
                .ok_or_else(|| {
                    KiroDeviceError::bad_gateway("kiro social token returned invalid profile ARN")
                })
        })
        .transpose()?;
    let discovered_profile = if token_profile_arn.is_none() {
        discover_profile_arn(http, Some(&flow.auth_region), &access_token, None)
            .await?
            .map(|(profile_arn, _)| profile_arn)
    } else {
        None
    };
    let (profile_arn, profile_provenance) = if let Some(profile_arn) = token_profile_arn {
        (Some(profile_arn), Some("token_response"))
    } else if let Some(profile_arn) = discovered_profile {
        (Some(profile_arn), Some("list_available_profiles"))
    } else {
        (None, None)
    };
    let mut input = crate::clients::oauth::kiro::import_credentials_json(
        json!({
            "accessToken": access_token,
            "refreshToken": refresh_token,
            "profileArn": profile_arn,
            "profileProvenance": profile_provenance,
            "expiresAt": now_ms.saturating_add(expires_in.saturating_mul(1000)),
            "authMethod": KIRO_AUTH_METHOD_SOCIAL,
            "provider": social_provider_label(&flow.provider),
            "authRegion": flow.auth_region,
        }),
        now_ms,
    )
    .map_err(|error| {
        KiroDeviceError::remote(
            StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            error.message,
        )
    })?;
    if let Some(profile_provenance) = profile_provenance {
        for value in [&mut input.profile, &mut input.raw]
            .into_iter()
            .filter_map(Option::as_mut)
            .filter_map(Value::as_object_mut)
        {
            value.insert("profileProvenance".to_string(), json!(profile_provenance));
        }
    }
    Ok(KiroDevicePollResult {
        pending: false,
        message: "kiro social device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(input),
    })
}

fn normalize_social_provider(provider: &str) -> Result<String, KiroDeviceError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "google" => Ok("google".to_string()),
        "github" => Ok("github".to_string()),
        _ => Err(KiroDeviceError::bad_request(
            "Kiro social loginProvider must be google or github",
        )),
    }
}

fn social_provider_label(provider: &str) -> &'static str {
    if provider.eq_ignore_ascii_case("github") {
        "Github"
    } else {
        "Google"
    }
}

pub(crate) async fn register_client(
    http: &reqwest::Client,
    region: &str,
) -> Result<RegisterClientResponse, KiroDeviceError> {
    register_client_with_issuer(http, region, KIRO_ISSUER_URL).await
}

pub(crate) async fn register_client_with_issuer(
    http: &reqwest::Client,
    region: &str,
    issuer_url: &str,
) -> Result<RegisterClientResponse, KiroDeviceError> {
    let region = normalize_region(region)?;
    let response = http
        .post(format!(
            "https://oidc.{region}.amazonaws.com/client/register"
        ))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "clientName": KIRO_CLIENT_NAME,
            "clientType": KIRO_CLIENT_TYPE,
            "scopes": [
                "codewhisperer:completions",
                "codewhisperer:analysis",
                "codewhisperer:conversations"
            ],
            "grantTypes": [
                "urn:ietf:params:oauth:grant-type:device_code",
                "refresh_token"
            ],
            "issuerUrl": issuer_url
        }))
        .send()
        .await
        .map_err(|error| {
            KiroDeviceError::bad_gateway(format!("kiro client registration failed: {error}"))
        })?;
    handle_json_response(response, "kiro client registration").await
}

async fn request_device_authorization(
    http: &reqwest::Client,
    region: &str,
    client: &RegisterClientResponse,
    start_url: &str,
) -> Result<DeviceAuthorizationResponse, KiroDeviceError> {
    let response = http
        .post(format!(
            "https://oidc.{region}.amazonaws.com/device_authorization"
        ))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "clientId": client.client_id,
            "clientSecret": client.client_secret,
            "startUrl": start_url
        }))
        .send()
        .await
        .map_err(|error| {
            KiroDeviceError::bad_gateway(format!("kiro device authorization failed: {error}"))
        })?;
    handle_json_response(response, "kiro device authorization").await
}

async fn poll_builder_id_token(
    http: &reqwest::Client,
    flow: &PendingKiroDeviceFlow,
    device_code: &str,
) -> Result<BuilderIdTokenResponse, KiroDeviceError> {
    let response = http
        .post(format!("https://oidc.{}.amazonaws.com/token", flow.region))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({
            "clientId": flow.client_id,
            "clientSecret": flow.client_secret,
            "deviceCode": device_code,
            "grantType": "urn:ietf:params:oauth:grant-type:device_code"
        }))
        .send()
        .await
        .map_err(|error| {
            KiroDeviceError::bad_gateway(format!("kiro token poll failed: {error}"))
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| format!("HTTP {status}"));
    let token: BuilderIdTokenResponse = serde_json::from_str(&body).map_err(|error| {
        if status.is_success() {
            KiroDeviceError::bad_gateway(format!("parse kiro token response failed: {error}"))
        } else {
            KiroDeviceError::remote(status, body.clone())
        }
    })?;
    if let Some(error) = token.error.as_deref() {
        return match error {
            "authorization_pending" | "slow_down" => Ok(BuilderIdTokenResponse {
                error: Some(error.to_string()),
                ..token
            }),
            "expired_token" => Err(KiroDeviceError::unauthorized("device code expired")),
            "access_denied" => Err(KiroDeviceError::unauthorized("access denied")),
            other => Err(KiroDeviceError::bad_gateway(format!(
                "{}: {}",
                other,
                token.error_description.unwrap_or_default()
            ))),
        };
    }
    if !status.is_success() {
        return Err(KiroDeviceError::remote(status, body));
    }
    Ok(token)
}

async fn account_input_from_token(
    http: &reqwest::Client,
    token: BuilderIdTokenResponse,
    flow: PendingKiroDeviceFlow,
    access_token: String,
    now_ms: i64,
) -> Result<UpsertAccountInput, KiroDeviceError> {
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| KiroDeviceError::bad_gateway("kiro token response lacks refresh_token"))?;
    let account_id = format!("kiro_{}", &sha256_hex(&refresh_token)[..24]);
    let machine_id = machine_id_from_refresh_token(&refresh_token);
    let explicit_profile_arn = token
        .profile_arn
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|profile_arn| {
            crate::domain::providers::kiro::parse_profile_arn(profile_arn)
                .map(|parsed| parsed.arn)
                .ok_or_else(|| {
                    KiroDeviceError::bad_gateway("kiro token returned invalid profile ARN")
                })
        })
        .transpose()?;
    let discovered_profile = if explicit_profile_arn.is_none() {
        discover_profile_arn(http, Some(&flow.region), &access_token, None).await?
    } else {
        None
    };
    let (profile_arn, profile_provenance) = if let Some(profile_arn) = explicit_profile_arn {
        (profile_arn, "token_response")
    } else if let Some((profile_arn, _)) = discovered_profile {
        (profile_arn, "list_available_profiles")
    } else if let Some(profile_arn) =
        default_profile_arn(&json!({ "authMethod": flow.auth_method }))
    {
        (profile_arn, "auth_method_default")
    } else {
        return Err(KiroDeviceError::bad_gateway(
            "kiro IdC login completed but no organization profile ARN was discovered",
        ));
    };
    if flow.auth_method == KIRO_AUTH_METHOD_IDC
        && crate::domain::providers::kiro::is_legacy_enterprise_fallback_profile(&profile_arn)
    {
        return Err(KiroDeviceError::bad_gateway(
            "kiro IdC login returned a legacy shared fallback profile ARN instead of an organization profile",
        ));
    }
    let runtime =
        crate::domain::providers::kiro::resolve_runtime_identity(Some(&profile_arn), None, None)
            .map_err(|_| KiroDeviceError::bad_gateway("kiro login resolved invalid profile ARN"))?;
    let usage = fetch_usage_limits(
        http,
        &runtime.runtime_region,
        Some(&profile_arn),
        &machine_id,
        &access_token,
        None,
    )
    .await
    .ok();
    let email = token.first_email().or_else(|| {
        usage
            .as_ref()
            .and_then(find_email_in_value)
            .map(str::to_string)
    });
    let subscription_level = usage
        .as_ref()
        .and_then(|value| {
            string_at(
                value,
                &[
                    "/subscriptionInfo/subscriptionTitle",
                    "/subscription_info/subscription_title",
                ],
            )
        })
        .or_else(|| Some("Kiro OAuth".to_string()));
    let quota = usage
        .as_ref()
        .map(|value| quota_from_usage_limits(value.clone(), subscription_level.clone(), now_ms));
    let quota_percent = quota.as_ref().and_then(agentic_quota_percent);
    let expires_at = token
        .expires_in
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)));
    let profile_auth_region = flow.region.clone();
    let raw_auth_region = flow.region.clone();
    let profile_runtime_region = runtime.runtime_region.clone();
    let raw_runtime_region = runtime.runtime_region.clone();
    let profile_start_url = flow.start_url.clone();
    let raw_start_url = flow.start_url.clone();
    let profile_issuer_url = flow.issuer_url.clone();
    let raw_issuer_url = flow.issuer_url.clone();
    let profile_auth_method = flow.auth_method.clone();
    let raw_auth_method = flow.auth_method.clone();
    let provider = if flow.auth_method == KIRO_AUTH_METHOD_IDC {
        "IdC"
    } else {
        "BuilderId"
    };
    let client_id = flow.client_id.clone();
    let client_secret = flow.client_secret.clone();
    let client_secret_expires_at = flow.client_secret_expires_at;
    let usage_fetched = usage.is_some();
    let profile_machine_id = machine_id.clone();
    let raw_machine_id = machine_id.clone();
    let profile_profile_arn = profile_arn.clone();
    let raw_profile_arn = profile_arn.clone();
    let profile_account_id = account_id.clone();
    let profile = json!({
        "accountId": profile_account_id,
        "email": email.as_deref(),
        "profileArn": profile_profile_arn,
        "authRegion": profile_auth_region,
        "runtimeRegion": profile_runtime_region,
        "apiRegion": runtime.runtime_region,
        "profileProvenance": profile_provenance,
        "machineId": profile_machine_id,
        "startUrl": profile_start_url,
        "issuerUrl": profile_issuer_url,
        "authMethod": profile_auth_method,
        "provider": provider,
    });
    let raw = json!({
        "provider": provider,
        "authMethod": raw_auth_method,
        "clientId": client_id,
        "clientSecret": client_secret,
        "clientSecretExpiresAt": client_secret_expires_at,
        "startUrl": raw_start_url,
        "issuerUrl": raw_issuer_url,
        "authRegion": raw_auth_region,
        "runtimeRegion": raw_runtime_region,
        "apiRegion": runtime.runtime_region,
        "profileArn": token.profile_arn,
        "resolvedProfileArn": raw_profile_arn,
        "profileProvenance": profile_provenance,
        "machineId": raw_machine_id,
        "tokenResponse": token.extra,
        "kiroUsageLimits": usage.as_ref(),
        "importedBy": "kiro_device_flow",
        "importedAtMs": now_ms,
    });
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::KiroOAuth,
        email,
        access_token: Some(access_token),
        refresh_token: Some(refresh_token),
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key: None,
        extra_headers: None,
        scopes: vec![
            "codewhisperer:completions".to_string(),
            "codewhisperer:analysis".to_string(),
            "codewhisperer:conversations".to_string(),
        ],
        profile: Some(profile),
        raw: Some(raw),
        subscription_level,
        entitlement_status: None,
        quota_percent,
        quota,
        quota_refreshed_at: usage_fetched.then_some(now_ms),
        quota_next_refresh_at: usage_fetched.then_some(now_ms.saturating_add(
            crate::domain::settings::ui_settings::default_oauth_quota_refresh_interval_ms(),
        )),
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

pub(crate) async fn fetch_usage_limits(
    http: &reqwest::Client,
    region: &str,
    profile_arn: Option<&str>,
    machine_id: &str,
    access_token: &str,
    token_type: Option<&str>,
) -> Result<Value, KiroDeviceError> {
    let region = normalize_region(region)?;
    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/macos lang/js md/nodejs#22.22.0 api/codewhispererruntime#1.0.0 m/N,E KiroIDE-2.3.0-{machine_id}"
    );
    let amz_user_agent = format!("aws-sdk-js/1.0.0 KiroIDE-2.3.0-{machine_id}");
    let q_host = format!("q.{region}.amazonaws.com");
    let mut urls = Vec::with_capacity(2);
    if profile_arn.is_some() {
        urls.push(usage_limits_url(&q_host, profile_arn));
    }
    urls.push(usage_limits_url(&q_host, None));
    for url in urls {
        match send_usage_limits_get(
            http,
            &url,
            &q_host,
            &amz_user_agent,
            &user_agent,
            access_token,
            token_type,
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(error) if error.status == StatusCode::UNAUTHORIZED => return Err(error),
            Err(_) => {}
        }
    }
    let cw_host = format!("codewhisperer.{region}.amazonaws.com");
    send_usage_limits_get(
        http,
        &usage_limits_url(&cw_host, None),
        &cw_host,
        &amz_user_agent,
        &user_agent,
        access_token,
        token_type,
    )
    .await
}

async fn send_usage_limits_get(
    http: &reqwest::Client,
    url: &str,
    host: &str,
    amz_user_agent: &str,
    user_agent: &str,
    access_token: &str,
    token_type: Option<&str>,
) -> Result<Value, KiroDeviceError> {
    let mut request = http
        .get(url)
        .header("x-amz-user-agent", amz_user_agent)
        .header("user-agent", user_agent)
        .header("host", host)
        .header("Accept", "application/json")
        .header("amz-sdk-invocation-id", random_hex(16))
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Connection", "close");
    if let Some(token_type) = token_type {
        request = request.header("tokentype", token_type);
    }
    let response = request.send().await.map_err(|error| {
        KiroDeviceError::bad_gateway(format!("kiro usage limits request failed: {error}"))
    })?;
    handle_json_response(response, "kiro usage limits").await
}

pub(crate) async fn fetch_available_profile_arn(
    http: &reqwest::Client,
    region: &str,
    access_token: &str,
    token_type: Option<&str>,
) -> Result<Option<String>, KiroDeviceError> {
    let region = normalize_region(region)?;
    let host = format!("q.{region}.amazonaws.com");
    let mut request = http
        .post(format!("https://{host}/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            "AmazonCodeWhispererService.ListAvailableProfiles",
        )
        .header("accept", "application/json")
        .header("host", &host)
        .header("amz-sdk-invocation-id", random_hex(16))
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("authorization", format!("Bearer {access_token}"))
        .header("connection", "close")
        .timeout(std::time::Duration::from_secs(10))
        .body(r#"{"maxResults":10}"#);
    if let Some(token_type) = token_type {
        request = request.header("tokentype", token_type);
    }
    let response = request.send().await.map_err(|error| {
        KiroDeviceError::bad_gateway(format!("kiro profile discovery failed: {error}"))
    })?;
    let body: Value = handle_json_response(response, "kiro profile discovery").await?;
    let profiles = body
        .get("profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let matching = profiles
        .iter()
        .filter_map(|profile| string_at(profile, &["/arn", "/profileArn"]))
        .filter_map(|arn| crate::domain::providers::kiro::parse_profile_arn(&arn))
        .find(|profile| profile.region == region)
        .map(|profile| profile.arn);
    Ok(matching.or_else(|| {
        profiles
            .iter()
            .find_map(|profile| string_at(profile, &["/arn", "/profileArn"]))
            .and_then(|arn| crate::domain::providers::kiro::parse_profile_arn(&arn))
            .map(|profile| profile.arn)
    }))
}

pub(crate) async fn discover_profile_arn(
    http: &reqwest::Client,
    auth_region: Option<&str>,
    access_token: &str,
    token_type: Option<&str>,
) -> Result<Option<(String, String)>, KiroDeviceError> {
    let regions = crate::domain::providers::kiro::profile_discovery_regions(auth_region)
        .ok_or_else(|| KiroDeviceError::bad_request("invalid Kiro auth region"))?;
    for region in regions {
        match fetch_available_profile_arn(http, &region, access_token, token_type).await {
            Ok(Some(profile_arn)) => return Ok(Some((profile_arn, region))),
            Ok(None) => {}
            Err(error) if error.status == StatusCode::UNAUTHORIZED => return Err(error),
            Err(_) => {}
        }
    }
    Ok(None)
}

async fn handle_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    context: &str,
) -> Result<T, KiroDeviceError> {
    if response.status().is_success() {
        return response.json::<T>().await.map_err(|error| {
            KiroDeviceError::bad_gateway(format!("parse {context} response failed: {error}"))
        });
    }
    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| format!("HTTP {status}"));
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| value.get("error_description").and_then(Value::as_str))
                .or_else(|| value.get("error").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or(text);
    Err(KiroDeviceError::remote(status, message))
}

pub(crate) fn quota_from_usage_limits(
    body: Value,
    plan: Option<String>,
    now_ms: i64,
) -> AccountQuota {
    let overage_enabled = bool_at(
        &body,
        &[
            "/overageConfiguration/overageEnabled",
            "/overage_configuration/overage_enabled",
        ],
    );
    let breakdowns = usage_breakdowns(&body);
    let mut resources = Vec::with_capacity(breakdowns.len());
    let mut tiers = Vec::with_capacity(breakdowns.len());
    let mut used_names = std::collections::BTreeSet::new();
    for (index, breakdown) in breakdowns.iter().enumerate() {
        let resource = parse_usage_breakdown(breakdown, index, &body);
        let normalized_resource = normalize_resource_name(&resource.resource_type);
        let mut tier_name = if normalized_resource == "agentic_request" {
            "kiro_agentic_requests".to_string()
        } else {
            format!("kiro_{normalized_resource}")
        };
        if !used_names.insert(tier_name.clone()) {
            tier_name.push('_');
            tier_name.push_str(&(index + 1).to_string());
            used_names.insert(tier_name.clone());
        }
        tiers.push(AccountQuotaTier {
            name: tier_name,
            label: Some(resource.resource_type.clone()),
            utilization: (resource.limit > 0.0)
                .then_some((resource.used / resource.limit).clamp(0.0, 1.0)),
            used: Some(resource.used),
            limit: Some(resource.limit),
            unit: Some("credits".to_string()),
            resets_at: resource.resets_at,
            ..Default::default()
        });
        resources.push(json!({
            "resourceType": resource.resource_type,
            "used": resource.used,
            "limit": resource.limit,
            "baseUsed": resource.base_used,
            "baseLimit": resource.base_limit,
            "freeTrialUsed": resource.free_trial_used,
            "freeTrialLimit": resource.free_trial_limit,
            "bonusUsed": resource.bonus_used,
            "bonusLimit": resource.bonus_limit,
            "resetsAt": resource.resets_at,
        }));
    }
    let availability = if tiers.is_empty() {
        "no_usage_breakdown"
    } else {
        "observed"
    };
    AccountQuota {
        success: true,
        credential_message: plan.or_else(|| Some("Kiro OAuth".to_string())),
        tiers,
        extra_usage: Some(json!({
            "raw": body,
            "source": "kiro_device_flow",
            "availability": availability,
            "resources": resources,
            "overageEnabled": overage_enabled,
            "queriedAt": now_ms,
        })),
    }
}

pub(crate) fn agentic_quota_percent(quota: &AccountQuota) -> Option<f64> {
    quota
        .tiers
        .iter()
        .find(|tier| tier.name == "kiro_agentic_requests")
        .and_then(|tier| tier.utilization)
        .filter(|value| value.is_finite())
        .map(|value| (value * 100.0).clamp(0.0, 10_000.0))
}

#[derive(Debug)]
struct KiroUsageResource {
    resource_type: String,
    base_used: f64,
    base_limit: f64,
    free_trial_used: f64,
    free_trial_limit: f64,
    bonus_used: f64,
    bonus_limit: f64,
    used: f64,
    limit: f64,
    resets_at: Option<i64>,
}

fn usage_breakdowns(body: &Value) -> Vec<&Value> {
    [
        "/usageBreakdownList",
        "/usage_breakdown_list",
        "/usageBreakdown",
        "/usage_breakdown",
    ]
    .iter()
    .find_map(|pointer| body.pointer(pointer).and_then(Value::as_array))
    .map(|breakdowns| breakdowns.iter().collect())
    .unwrap_or_default()
}

fn parse_usage_breakdown(breakdown: &Value, index: usize, body: &Value) -> KiroUsageResource {
    let resource_type =
        string_at(breakdown, &["/resourceType", "/resource_type"]).unwrap_or_else(|| {
            if index == 0 {
                "AGENTIC_REQUEST".to_string()
            } else {
                format!("UNKNOWN_{}", index + 1)
            }
        });
    let base_used = number_at(
        breakdown,
        &[
            "/currentUsageWithPrecision",
            "/current_usage_with_precision",
            "/currentUsage",
            "/current_usage",
        ],
    )
    .unwrap_or(0.0);
    let base_limit = number_at(
        breakdown,
        &[
            "/usageLimitWithPrecision",
            "/usage_limit_with_precision",
            "/usageLimit",
            "/usage_limit",
        ],
    )
    .unwrap_or(0.0);
    let free_trial_used = number_at(
        breakdown,
        &[
            "/freeTrialInfo/currentUsageWithPrecision",
            "/free_trial_info/current_usage_with_precision",
            "/freeTrialInfo/currentUsage",
            "/free_trial_info/current_usage",
        ],
    )
    .filter(|_| {
        usage_component_is_active(
            breakdown,
            &[
                "/freeTrialInfo/freeTrialStatus",
                "/free_trial_info/free_trial_status",
            ],
        )
    })
    .unwrap_or(0.0);
    let free_trial_limit = number_at(
        breakdown,
        &[
            "/freeTrialInfo/usageLimitWithPrecision",
            "/free_trial_info/usage_limit_with_precision",
            "/freeTrialInfo/usageLimit",
            "/free_trial_info/usage_limit",
        ],
    )
    .filter(|_| {
        usage_component_is_active(
            breakdown,
            &[
                "/freeTrialInfo/freeTrialStatus",
                "/free_trial_info/free_trial_status",
            ],
        )
    })
    .unwrap_or(0.0);
    let bonus_used = bonuses_total(Some(breakdown), &["currentUsage", "current_usage"]);
    let bonus_limit = bonuses_total(Some(breakdown), &["usageLimit", "usage_limit"]);
    let resets_at = number_at(
        breakdown,
        &[
            "/nextResetTimestamp",
            "/next_reset_timestamp",
            "/nextDateReset",
            "/next_date_reset",
        ],
    )
    .or_else(|| {
        number_at(
            body,
            &[
                "/nextDateReset",
                "/next_date_reset",
                "/resetDate",
                "/reset_date",
            ],
        )
    })
    .and_then(timestamp_number_to_unix_ms)
    .or_else(|| {
        timestamp_string_at(
            breakdown,
            &[
                "/nextResetTimestamp",
                "/next_reset_timestamp",
                "/nextDateReset",
                "/next_date_reset",
            ],
        )
    })
    .or_else(|| {
        timestamp_string_at(
            body,
            &[
                "/nextDateReset",
                "/next_date_reset",
                "/resetDate",
                "/reset_date",
            ],
        )
    });
    KiroUsageResource {
        resource_type,
        base_used,
        base_limit,
        free_trial_used,
        free_trial_limit,
        bonus_used,
        bonus_limit,
        used: base_used + free_trial_used + bonus_used,
        limit: base_limit + free_trial_limit + bonus_limit,
        resets_at,
    }
}

fn normalize_resource_name(resource_type: &str) -> String {
    let mut output = String::with_capacity(resource_type.len().min(64));
    let mut previous_separator = false;
    for byte in resource_type.bytes().take(64) {
        let normalized = if byte.is_ascii_alphanumeric() {
            previous_separator = false;
            byte.to_ascii_lowercase() as char
        } else if !previous_separator {
            previous_separator = true;
            '_'
        } else {
            continue;
        };
        output.push(normalized);
    }
    let normalized = output.trim_matches('_');
    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized.to_string()
    }
}

pub fn machine_id_from_refresh_token(refresh_token: &str) -> String {
    sha256_hex(&format!("KotlinNativeAPI/{refresh_token}"))
}

pub fn normalize_region(raw: &str) -> Result<String, KiroDeviceError> {
    crate::domain::providers::kiro::normalize_region(raw)
        .ok_or_else(|| KiroDeviceError::bad_request("invalid Kiro region"))
}

fn normalize_start_url(raw: &str) -> Result<String, KiroDeviceError> {
    let url = raw.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(KiroDeviceError::bad_request(
            "Kiro startUrl must be an http(s) URL",
        ));
    }
    if url.contains('@') || url.len() > 512 {
        return Err(KiroDeviceError::bad_request("invalid Kiro startUrl"));
    }
    Ok(url.to_string())
}

fn normalize_issuer_url(raw: &str) -> Result<String, KiroDeviceError> {
    let url = raw.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(KiroDeviceError::bad_request(
            "Kiro issuerUrl must be an http(s) URL",
        ));
    }
    if url.contains('@') || url.len() > 512 {
        return Err(KiroDeviceError::bad_request("invalid Kiro issuerUrl"));
    }
    Ok(url.to_string())
}

fn usage_limits_url(host: &str, profile_arn: Option<&str>) -> String {
    let mut url = format!(
        "https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST&isEmailRequired=true"
    );
    if let Some(profile_arn) = profile_arn.filter(|value| !value.trim().is_empty()) {
        url.push_str("&profileArn=");
        url.push_str(&percent_encode(profile_arn));
    }
    url
}

pub(crate) fn default_profile_arn(token_extra: &Value) -> Option<String> {
    let kind = string_at(
        token_extra,
        &[
            "/authMethod",
            "/auth_method",
            "/provider",
            "/identityProvider",
            "/identity_provider",
        ],
    )
    .unwrap_or_default()
    .to_ascii_lowercase();
    if matches!(kind.as_str(), "social" | "google" | "github") {
        return Some(SOCIAL_PROFILE_ARN.to_string());
    }
    if crate::domain::providers::kiro::is_enterprise_auth_method(&kind) {
        return None;
    }
    Some(BUILDER_ID_PROFILE_ARN.to_string())
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0_u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn first_email(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(|value| value.trim().to_string())
        .find_map(|value| valid_email(&value).map(str::to_string))
}

fn email_from_jwt(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    first_email_claim(&claims, &["email", "preferred_username", "username", "upn"]).or_else(|| {
        claims
            .get("identities")
            .and_then(Value::as_array)
            .and_then(|identities| {
                identities.iter().find_map(|identity| {
                    first_email_claim(
                        identity,
                        &[
                            "email",
                            "userId",
                            "user_id",
                            "providerName",
                            "provider_user_id",
                        ],
                    )
                })
            })
    })
}

fn first_email_claim(claims: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        claims
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .and_then(valid_email)
            .map(str::to_string)
    })
}

fn valid_email(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.len() < 3 || trimmed.contains(char::is_whitespace) {
        return None;
    }
    let (local, domain) = trimmed.split_once('@')?;
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return None;
    }
    Some(trimmed)
}

pub(crate) fn find_email_in_value(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => valid_email(value),
        Value::Array(values) => values.iter().find_map(find_email_in_value),
        Value::Object(map) => {
            for key in [
                "email",
                "accountEmail",
                "userEmail",
                "account_email",
                "user_email",
            ] {
                if let Some(value) = map.get(key).and_then(Value::as_str).and_then(valid_email) {
                    return Some(value);
                }
            }
            map.values().find_map(find_email_in_value)
        }
        _ => None,
    }
}

pub(crate) fn string_at(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn number_at(value: &Value, pointers: &[&str]) -> Option<f64> {
    pointers.iter().find_map(|pointer| {
        value.pointer(pointer).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|number| number as f64))
                .or_else(|| value.as_u64().map(|number| number as f64))
                .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        })
    })
}

fn bool_at(value: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_bool))
}

fn bonuses_total(breakdown: Option<&Value>, field_names: &[&str]) -> f64 {
    let Some(breakdown) = breakdown else {
        return 0.0;
    };
    breakdown
        .get("bonuses")
        .or_else(|| breakdown.get("bonusList"))
        .or_else(|| breakdown.get("bonus_list"))
        .and_then(Value::as_array)
        .map(|bonuses| {
            bonuses
                .iter()
                .filter(|bonus| {
                    bonus
                        .get("status")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|status| !status.is_empty())
                        .is_none_or(|status| {
                            matches!(status.to_ascii_uppercase().as_str(), "ACTIVE" | "ENABLED")
                        })
                })
                .filter_map(|bonus| {
                    field_names.iter().find_map(|name| {
                        bonus.get(*name).and_then(|value| {
                            value.as_f64().or_else(|| value.as_i64().map(|n| n as f64))
                        })
                    })
                })
                .sum()
        })
        .unwrap_or(0.0)
}

fn usage_component_is_active(value: &Value, status_pointers: &[&str]) -> bool {
    status_pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .is_none_or(|status| matches!(status.to_ascii_uppercase().as_str(), "ACTIVE" | "ENABLED"))
}

fn timestamp_string_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value.trim()).ok())
            .map(|value| value.timestamp_millis())
    })
}

fn timestamp_number_to_unix_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    if value > 1_000_000_000_000.0 {
        Some(value as i64)
    } else {
        Some((value * 1000.0) as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiro_region_validation_is_conservative() {
        assert_eq!(normalize_region("US-EAST-1").unwrap(), "us-east-1");
        assert!(normalize_region("../us-east-1").is_err());
    }

    #[tokio::test]
    async fn network_helpers_reject_unsafe_region_before_building_hosts() {
        let http = reqwest::Client::new();
        let invalid = "us-east-1.attacker.invalid";

        let register_error = register_client_with_issuer(&http, invalid, KIRO_ISSUER_URL)
            .await
            .unwrap_err();
        assert_eq!(register_error.status, StatusCode::BAD_REQUEST);

        let usage_error = fetch_usage_limits(
            &http,
            invalid,
            Some(BUILDER_ID_PROFILE_ARN),
            "machine",
            "access",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(usage_error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn machine_id_matches_kiro_prefix_input() {
        assert_eq!(
            machine_id_from_refresh_token("rt"),
            sha256_hex("KotlinNativeAPI/rt")
        );
    }

    #[test]
    fn social_provider_and_enterprise_profile_fallback_are_normalized() {
        assert_eq!(normalize_social_provider("Google").unwrap(), "google");
        assert_eq!(normalize_social_provider("github").unwrap(), "github");
        assert!(normalize_social_provider("amazon").is_err());
        assert_eq!(
            default_profile_arn(&json!({ "authMethod": "social" })).as_deref(),
            Some(SOCIAL_PROFILE_ARN)
        );
        assert_eq!(default_profile_arn(&json!({ "authMethod": "idc" })), None);
    }

    #[test]
    fn quota_preserves_multiple_resources_and_only_active_credit_components() {
        let quota = quota_from_usage_limits(
            json!({
                "subscriptionInfo": {"subscriptionTitle": "Kiro Pro"},
                "resetDate": "2026-09-01T00:00:00Z",
                "usageBreakdownList": [
                    {
                        "resourceType": "AGENTIC_REQUEST",
                        "currentUsageWithPrecision": 10.0,
                        "usageLimitWithPrecision": 100.0,
                        "freeTrialInfo": {
                            "freeTrialStatus": "ACTIVE",
                            "currentUsageWithPrecision": 2.0,
                            "usageLimitWithPrecision": 20.0
                        },
                        "bonuses": [
                            {"status": "ACTIVE", "currentUsage": 3.0, "usageLimit": 30.0},
                            {"status": "EXPIRED", "currentUsage": 90.0, "usageLimit": 900.0}
                        ]
                    },
                    {
                        "resourceType": "CODE_REVIEW",
                        "currentUsageWithPrecision": 4.0,
                        "usageLimitWithPrecision": 40.0,
                        "freeTrialInfo": {
                            "freeTrialStatus": "EXPIRED",
                            "currentUsageWithPrecision": 8.0,
                            "usageLimitWithPrecision": 80.0
                        }
                    }
                ]
            }),
            Some("Kiro Pro".to_string()),
            1_000,
        );

        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 2);
        assert_eq!(quota.tiers[0].name, "kiro_agentic_requests");
        assert_eq!(quota.tiers[0].label.as_deref(), Some("AGENTIC_REQUEST"));
        assert_eq!(quota.tiers[0].used, Some(15.0));
        assert_eq!(quota.tiers[0].limit, Some(150.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.1));
        assert_eq!(quota.tiers[0].resets_at, Some(1_788_220_800_000));
        assert_eq!(quota.tiers[1].name, "kiro_code_review");
        assert_eq!(quota.tiers[1].used, Some(4.0));
        assert_eq!(quota.tiers[1].limit, Some(40.0));
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.pointer("/resources/0/bonusUsed"))
                .and_then(Value::as_f64),
            Some(3.0)
        );
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.pointer("/availability"))
                .and_then(Value::as_str),
            Some("observed")
        );
    }

    #[test]
    fn empty_usage_breakdown_is_connected_but_not_fake_zero_quota() {
        let quota = quota_from_usage_limits(
            json!({
                "subscriptionInfo": {"subscriptionTitle": "Kiro Builder ID"},
                "usageBreakdownList": []
            }),
            Some("Kiro Builder ID".to_string()),
            1_000,
        );

        assert!(quota.success);
        assert!(quota.tiers.is_empty());
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.pointer("/availability"))
                .and_then(Value::as_str),
            Some("no_usage_breakdown")
        );
    }

    #[test]
    fn quota_identifies_agentic_requests_by_resource_type_not_array_position() {
        let quota = quota_from_usage_limits(
            json!({
                "usageBreakdownList": [
                    {
                        "resourceType": "CODE_REVIEW",
                        "currentUsage": 2,
                        "usageLimit": 2
                    },
                    {
                        "resourceType": "AGENTIC_REQUEST",
                        "currentUsage": 3,
                        "usageLimit": 10
                    }
                ]
            }),
            Some("Kiro Pro".to_string()),
            1_000,
        );

        assert_eq!(quota.tiers[0].name, "kiro_code_review");
        assert_eq!(quota.tiers[1].name, "kiro_agentic_requests");
        assert_eq!(quota.tiers[0].utilization, Some(1.0));
        assert_eq!(quota.tiers[1].utilization, Some(0.3));
        assert_eq!(agentic_quota_percent(&quota), Some(30.0));
    }
}
