use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use chrono::DateTime;
use rand::RngCore;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::domain::settings::config::{RouterIdentity, ServerConfig};

const EMAIL_AUTH_FILE_NAME: &str = "email-auth.json";
const LOGIN_PURPOSE: &str = "login";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAuthState {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_expires_at: Option<i64>,
    #[serde(default)]
    pub verified_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAuthStatus {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailCodeRequestResponse {
    pub ok: bool,
    pub cooldown_secs: i64,
    pub masked_destination: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAuthUser {
    pub id: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterVerifyEmailCodeResponse {
    pub user: EmailAuthUser,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub refresh_expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindOwnerEmailResponse {
    pub ok: bool,
    pub owner_email: String,
    pub already_bound: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOwnerEmailResponse {
    pub ok: bool,
    pub old_email: String,
    pub new_email: String,
    pub updated_shares: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSessionMeResponse {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<EmailAuthUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_owner_email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RouterErrorResponse {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthRequestCodePayload<'a> {
    email: &'a str,
    purpose: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BindOwnerEmailSignaturePayload<'a> {
    email: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_token: Option<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeOwnerEmailSignaturePayload<'a> {
    old_email: &'a str,
    new_email: &'a str,
}

#[derive(Debug, Clone)]
pub struct EmailAuthError {
    pub status: StatusCode,
    pub message: String,
}

impl EmailAuthError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn remote(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl fmt::Display for EmailAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for EmailAuthError {}

pub fn email_auth_path(config_dir: &Path) -> PathBuf {
    config_dir.join(EMAIL_AUTH_FILE_NAME)
}

pub fn load_state(config_dir: &Path) -> anyhow::Result<Option<EmailAuthState>> {
    let path = email_auth_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parse {}", path.display()))
        .map(Some)
}

pub fn save_state(config_dir: &Path, state: &EmailAuthState) -> anyhow::Result<()> {
    crate::infra::storage::write_json_pretty(&email_auth_path(config_dir), state)
}

pub fn clear_state(config_dir: &Path) -> anyhow::Result<()> {
    let path = email_auth_path(config_dir);
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))
}

pub fn get_status(config_dir: &Path) -> anyhow::Result<EmailAuthStatus> {
    let Some(state) = load_state(config_dir)? else {
        return Ok(EmailAuthStatus {
            authenticated: false,
            email: None,
            expires_at: None,
            router_domain: None,
        });
    };
    Ok(EmailAuthStatus {
        authenticated: !state.email.trim().is_empty(),
        email: Some(state.email),
        expires_at: state.expires_at,
        router_domain: state.router_domain,
    })
}

pub fn session_me(
    config_dir: &Path,
    config: &ServerConfig,
) -> anyhow::Result<EmailSessionMeResponse> {
    let Some(state) = load_state(config_dir)? else {
        return Ok(EmailSessionMeResponse {
            authenticated: false,
            user: None,
            expires_at: None,
            installation_owner_email: config.owner.email.clone(),
        });
    };
    Ok(EmailSessionMeResponse {
        authenticated: !state.email.trim().is_empty(),
        user: Some(EmailAuthUser {
            id: state.email.clone(),
            email: state.email,
        }),
        expires_at: state.expires_at.map(|value| value.to_string()),
        installation_owner_email: config.owner.email.clone(),
    })
}

pub async fn request_code(
    http: &reqwest::Client,
    config: &ServerConfig,
    email: &str,
) -> Result<EmailCodeRequestResponse, EmailAuthError> {
    let email = normalize_email(email)?;
    let identity = router_identity(config)?;
    let payload = AuthRequestCodePayload {
        email: &email,
        purpose: LOGIN_PURPOSE,
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = crate::clients::router::client::sign_payload(
        identity,
        "auth_request_code",
        &payload,
        timestamp_ms,
        &nonce,
    )
    .map_err(|error| EmailAuthError::internal(error.to_string()))?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/auth/email/request-code"))
        .json(&json!({
            "email": email,
            "installationId": identity.installation_id.as_str(),
            "timestampMs": timestamp_ms,
            "nonce": nonce,
            "signature": signature,
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("request email code failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub async fn verify_client_web_code(
    http: &reqwest::Client,
    config: &ServerConfig,
    email: &str,
    code: &str,
) -> Result<RouterVerifyEmailCodeResponse, EmailAuthError> {
    let email = normalize_email(email)?;
    let code = code.trim();
    if code.is_empty() {
        return Err(EmailAuthError::bad_request("code is required"));
    }
    let identity = router_identity(config)?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/client-web/auth/email/verify-code"))
        .json(&json!({
            "email": email,
            "code": code,
            "installationId": identity.installation_id.as_str(),
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("verify email code failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub async fn refresh_session(
    http: &reqwest::Client,
    config: &ServerConfig,
    refresh_token: &str,
) -> Result<RouterVerifyEmailCodeResponse, EmailAuthError> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err(EmailAuthError::bad_request("refreshToken is required"));
    }
    let identity = router_identity(config)?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/auth/session/refresh"))
        .json(&json!({
            "refreshToken": refresh_token,
            "installationId": identity.installation_id.as_str(),
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("refresh email auth session failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub async fn bind_owner_email(
    http: &reqwest::Client,
    config: &ServerConfig,
    email: &str,
    access_token: &str,
) -> Result<BindOwnerEmailResponse, EmailAuthError> {
    let email = normalize_email(email)?;
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err(EmailAuthError::bad_request("access token is required"));
    }
    let identity = router_identity(config)?;
    let payload = BindOwnerEmailSignaturePayload {
        email: &email,
        verification_token: None,
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = crate::clients::router::client::sign_payload(
        identity,
        "bind_installation_owner_email",
        &payload,
        timestamp_ms,
        &nonce,
    )
    .map_err(|error| EmailAuthError::internal(error.to_string()))?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/installations/bind-owner-email"))
        .bearer_auth(access_token)
        .json(&json!({
            "installationId": identity.installation_id.as_str(),
            "email": email,
            "verificationToken": null,
            "timestampMs": timestamp_ms,
            "nonce": nonce,
            "signature": signature,
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("bind installation owner email failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub async fn bind_owner_email_at_setup(
    http: &reqwest::Client,
    config: &ServerConfig,
    email: &str,
) -> Result<BindOwnerEmailResponse, EmailAuthError> {
    let email = normalize_email(email)?;
    let identity = router_identity(config)?;
    let payload = BindOwnerEmailSignaturePayload {
        email: &email,
        verification_token: None,
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = crate::clients::router::client::sign_payload(
        identity,
        "bind_installation_owner_email",
        &payload,
        timestamp_ms,
        &nonce,
    )
    .map_err(|error| EmailAuthError::internal(error.to_string()))?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/installations/bind-owner-email"))
        .json(&json!({
            "installationId": identity.installation_id.as_str(),
            "email": email,
            "verificationToken": null,
            "timestampMs": timestamp_ms,
            "nonce": nonce,
            "signature": signature,
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("bind installation owner email failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub async fn change_owner_email(
    http: &reqwest::Client,
    config: &ServerConfig,
    old_email: &str,
    new_email: &str,
    access_token: &str,
) -> Result<ChangeOwnerEmailResponse, EmailAuthError> {
    let old_email = normalize_email(old_email)?;
    let new_email = normalize_email(new_email)?;
    if old_email == new_email {
        return Err(EmailAuthError::bad_request(
            "new owner email must be different from current owner email",
        ));
    }
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err(EmailAuthError::bad_request("access token is required"));
    }
    let identity = router_identity(config)?;
    let payload = ChangeOwnerEmailSignaturePayload {
        old_email: &old_email,
        new_email: &new_email,
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = crate::clients::router::client::sign_payload(
        identity,
        "change_installation_owner_email",
        &payload,
        timestamp_ms,
        &nonce,
    )
    .map_err(|error| EmailAuthError::internal(error.to_string()))?;
    let api_base = router_api_base(config)?;
    let response = http
        .post(format!("{api_base}/v1/installations/change-owner-email"))
        .bearer_auth(access_token)
        .json(&json!({
            "installationId": identity.installation_id.as_str(),
            "oldEmail": old_email,
            "newEmail": new_email,
            "timestampMs": timestamp_ms,
            "nonce": nonce,
            "signature": signature,
        }))
        .send()
        .await
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("change owner email failed: {error}"))
        })?;
    handle_json_response(response).await
}

pub fn state_from_router_session(
    config: &ServerConfig,
    response: &RouterVerifyEmailCodeResponse,
) -> Result<EmailAuthState, EmailAuthError> {
    Ok(EmailAuthState {
        email: normalize_email(&response.user.email)?,
        router_domain: config
            .router
            .domain
            .clone()
            .or_else(|| config.router_api_base().map(str::to_string)),
        access_token: Some(response.access_token.clone()),
        refresh_token: Some(response.refresh_token.clone()),
        expires_at: Some(parse_rfc3339_timestamp(&response.expires_at)?),
        refresh_expires_at: Some(parse_rfc3339_timestamp(&response.refresh_expires_at)?),
        verified_at: now_secs(),
    })
}

pub fn normalize_email(email: &str) -> Result<String, EmailAuthError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Err(EmailAuthError::bad_request("email is required"));
    }
    Ok(email)
}

pub fn humanize_remote_owner_binding_error(message: &str) -> String {
    let normalized = message.trim();
    if normalized.is_empty() {
        return "bind installation owner email failed; request and verify a fresh email code"
            .to_string();
    }
    if normalized.contains("verification token is required")
        || normalized.contains("verification token expired or not found")
        || normalized.contains("redeem verification token failed")
        || normalized.contains("verification token does not match")
    {
        return "email verification session expired; request and verify a fresh code".to_string();
    }
    if normalized.contains("this installation is locked to a different owner email") {
        return "this installation is locked to a different owner email".to_string();
    }
    if normalized.contains("installation owner email binding is required") {
        return "installation owner email binding is required; verify email first".to_string();
    }
    if normalized.contains("installation not found")
        || normalized.contains("signature verification failed")
    {
        return "router installation identity is invalid; re-register the router identity and verify email again"
            .to_string();
    }
    if let Some(detail) = normalized.strip_prefix("bind installation owner email failed: ") {
        return humanize_remote_owner_binding_error(detail);
    }
    normalized.to_string()
}

fn router_identity(config: &ServerConfig) -> Result<&RouterIdentity, EmailAuthError> {
    config
        .router
        .identity
        .as_ref()
        .filter(|identity| {
            !identity.installation_id.trim().is_empty() && !identity.private_key.trim().is_empty()
        })
        .ok_or_else(|| {
            EmailAuthError::bad_request("router installation identity is not registered")
        })
}

fn router_api_base(config: &ServerConfig) -> Result<String, EmailAuthError> {
    config
        .router_api_base()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .ok_or_else(|| EmailAuthError::bad_request("router api base is not configured"))
}

fn parse_rfc3339_timestamp(value: &str) -> Result<i64, EmailAuthError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.timestamp())
        .map_err(|error| {
            EmailAuthError::bad_gateway(format!("parse auth timestamp failed: {error}"))
        })
}

async fn handle_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, EmailAuthError> {
    if response.status().is_success() {
        return response.json::<T>().await.map_err(|error| {
            EmailAuthError::bad_gateway(format!("parse email auth response failed: {error}"))
        });
    }
    let status = response.status();
    let text = response
        .text()
        .await
        .unwrap_or_else(|_| format!("HTTP {status}"));
    let message = serde_json::from_str::<RouterErrorResponse>(&text)
        .ok()
        .and_then(|body| {
            let RouterErrorResponse { message, error } = body;
            message.or_else(|| error.and_then(|value| value.as_str().map(ToOwned::to_owned)))
        })
        .unwrap_or(text);
    Err(EmailAuthError::remote(status, message))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_email_trims_and_lowercases() {
        assert_eq!(
            normalize_email(" OWNER@Example.COM ").unwrap(),
            "owner@example.com"
        );
    }

    #[test]
    fn state_path_uses_dedicated_json_store() {
        assert!(email_auth_path(Path::new("/tmp/ccs")).ends_with("email-auth.json"));
    }
}
