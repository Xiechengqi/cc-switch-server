use std::fmt;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::providers::model::ProviderType;

const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const GITHUB_CLIENT_ID_GHES: &str = "Ov23li8tweQw6odWQebz";
const DEFAULT_GITHUB_DOMAIN: &str = "github.com";
pub const COPILOT_EDITOR_VERSION: &str = "vscode/1.110.1";
pub const COPILOT_PLUGIN_VERSION: &str = "copilot-chat/0.38.2";
pub const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.38.2";
pub const COPILOT_API_VERSION: &str = "2025-10-01";

fn default_github_domain() -> String {
    DEFAULT_GITHUB_DOMAIN.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubDeviceCodeResponse {
    #[serde(alias = "device_code")]
    pub device_code: String,
    #[serde(alias = "user_code")]
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_uri: String,
    #[serde(alias = "expires_in")]
    pub expires_in: u64,
    pub interval: u64,
    #[serde(
        default,
        alias = "verification_uri_complete",
        skip_serializing_if = "Option::is_none"
    )]
    pub verification_uri_complete: Option<String>,
    #[serde(default = "default_github_domain", alias = "github_domain")]
    pub github_domain: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotDevicePollResult {
    pub pending: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_input: Option<UpsertAccountInput>,
}

#[derive(Debug, Deserialize)]
struct GitHubOAuthResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubUser {
    pub login: String,
    pub id: u64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "avatar_url")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotTokenResponse {
    pub token: String,
    #[serde(alias = "expires_at")]
    pub expires_at: i64,
    #[serde(default, alias = "refresh_in")]
    pub refresh_in: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotEndpoints {
    pub api: String,
    #[serde(default)]
    pub telemetry: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CopilotUserResponse {
    #[serde(default)]
    endpoints: Option<CopilotEndpoints>,
}

#[derive(Debug, Clone)]
pub struct CopilotDeviceError {
    pub status: StatusCode,
    pub message: String,
}

impl CopilotDeviceError {
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
            message: message.into(),
        }
    }
}

impl fmt::Display for CopilotDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CopilotDeviceError {}

pub async fn start_device_flow(
    http: &reqwest::Client,
    github_domain: Option<&str>,
) -> Result<GitHubDeviceCodeResponse, CopilotDeviceError> {
    let domain = normalize_github_domain(github_domain.unwrap_or(DEFAULT_GITHUB_DOMAIN))?;
    let response = http
        .post(github_device_code_url(&domain))
        .header("Accept", "application/json")
        .header("User-Agent", COPILOT_USER_AGENT)
        .form(&[
            ("client_id", github_client_id(&domain)),
            ("scope", "read:user"),
        ])
        .send()
        .await
        .map_err(|error| {
            let hint = if error.is_connect() || error.is_timeout() {
                " (server cannot reach GitHub; configure upstream proxy in server settings)"
            } else {
                ""
            };
            CopilotDeviceError::bad_gateway(format!(
                "github copilot device code request failed: {error}{hint}"
            ))
        })?;
    let mut body: GitHubDeviceCodeResponse = handle_json_response(response).await?;
    body.github_domain = domain;
    Ok(body)
}

pub async fn poll_device_flow(
    http: &reqwest::Client,
    device_code: &str,
    github_domain: Option<&str>,
    now_ms: i64,
) -> Result<CopilotDevicePollResult, CopilotDeviceError> {
    let device_code = device_code.trim();
    if device_code.is_empty() {
        return Err(CopilotDeviceError::bad_request("deviceCode is required"));
    }
    let domain = normalize_github_domain(github_domain.unwrap_or(DEFAULT_GITHUB_DOMAIN))?;
    let response = http
        .post(github_oauth_token_url(&domain))
        .header("Accept", "application/json")
        .header("User-Agent", COPILOT_USER_AGENT)
        .form(&[
            ("client_id", github_client_id(&domain)),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("github device token poll failed: {error}"))
        })?;
    let oauth: GitHubOAuthResponse = handle_json_response(response).await?;
    if let Some(error) = oauth.error.as_deref() {
        return match error {
            "authorization_pending" => Ok(CopilotDevicePollResult {
                pending: true,
                message: "authorization pending".to_string(),
                retry_after_secs: Some(5),
                account_input: None,
            }),
            "slow_down" => Ok(CopilotDevicePollResult {
                pending: true,
                message: "authorization pending; slow down polling".to_string(),
                retry_after_secs: Some(10),
                account_input: None,
            }),
            "expired_token" => Err(CopilotDeviceError::unauthorized("device code expired")),
            "access_denied" => Err(CopilotDeviceError::unauthorized("access denied")),
            other => Err(CopilotDeviceError::bad_gateway(format!(
                "{}: {}",
                other,
                oauth.error_description.unwrap_or_default()
            ))),
        };
    }
    let github_token = oauth.access_token.clone().ok_or_else(|| {
        CopilotDeviceError::bad_gateway("github token response lacks access_token")
    })?;
    let github_user = fetch_github_user(http, &domain, &github_token).await?;
    let copilot_token = if is_ghes(&domain) {
        None
    } else {
        Some(fetch_copilot_token(http, &domain, &github_token).await?)
    };
    let copilot_usage = fetch_copilot_usage(http, &domain, &github_token).await.ok();
    let account_input = account_input_from_device_flow(
        &domain,
        oauth,
        github_token,
        github_user,
        copilot_token,
        copilot_usage,
        now_ms,
    )?;
    Ok(CopilotDevicePollResult {
        pending: false,
        message: "github copilot device authorization completed".to_string(),
        retry_after_secs: None,
        account_input: Some(account_input),
    })
}

fn account_input_from_device_flow(
    domain: &str,
    oauth: GitHubOAuthResponse,
    github_token: String,
    user: GitHubUser,
    copilot_token: Option<CopilotTokenResponse>,
    copilot_usage: Option<Value>,
    now_ms: i64,
) -> Result<UpsertAccountInput, CopilotDeviceError> {
    let account_id = composite_account_id(domain, user.id);
    let access_token = copilot_token
        .as_ref()
        .map(|token| token.token.clone())
        .unwrap_or_else(|| github_token.clone());
    let refresh_token = github_token.clone();
    let scope = oauth.scope.clone();
    let expires_at = copilot_token
        .as_ref()
        .map(|token| token.expires_at.saturating_mul(1000));
    let profile = json!({
        "id": user.id,
        "login": user.login.as_str(),
        "email": user.email.as_deref(),
        "name": user.name.as_deref(),
        "avatarUrl": user.avatar_url.as_deref(),
        "githubDomain": domain,
        "ghes": is_ghes(domain),
    });
    let raw = json!({
        "githubDomain": domain,
        "githubToken": github_token.as_str(),
        "githubTokenType": oauth.token_type.as_deref(),
        "githubScopes": scope.as_deref(),
        "copilotToken": copilot_token.as_ref(),
        "copilotUsage": copilot_usage.as_ref(),
        "copilotApiBase": copilot_api_base(domain),
        "importedBy": "github_copilot_device_flow",
        "importedAtMs": now_ms,
    });
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::GitHubCopilot,
        email: user
            .email
            .or_else(|| Some(format!("{}@{domain}", user.login))),
        access_token: Some(access_token),
        refresh_token: Some(refresh_token),
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key: None,
        scopes: oauth
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect(),
        profile: Some(profile),
        raw: Some(raw),
        subscription_level: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        last_refresh_error: None,
    })
}

async fn fetch_github_user(
    http: &reqwest::Client,
    domain: &str,
    github_token: &str,
) -> Result<GitHubUser, CopilotDeviceError> {
    let response = http
        .get(github_user_url(domain))
        .bearer_auth(github_token)
        .header("Accept", "application/json")
        .header("User-Agent", COPILOT_USER_AGENT)
        .send()
        .await
        .map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("github user request failed: {error}"))
        })?;
    handle_json_response(response).await
}

async fn fetch_copilot_token(
    http: &reqwest::Client,
    domain: &str,
    github_token: &str,
) -> Result<CopilotTokenResponse, CopilotDeviceError> {
    fetch_copilot_token_from_url(http, &copilot_token_url(domain), github_token).await
}

async fn fetch_copilot_token_from_url(
    http: &reqwest::Client,
    url: &str,
    github_token: &str,
) -> Result<CopilotTokenResponse, CopilotDeviceError> {
    let response = http
        .get(url)
        .header("Authorization", format!("token {github_token}"))
        .header("User-Agent", COPILOT_USER_AGENT)
        .header("editor-version", COPILOT_EDITOR_VERSION)
        .header("editor-plugin-version", COPILOT_PLUGIN_VERSION)
        .header("x-github-api-version", COPILOT_API_VERSION)
        .send()
        .await
        .map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("copilot token request failed: {error}"))
        })?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(CopilotDeviceError::unauthorized(
            "github account does not have an active Copilot subscription",
        ));
    }
    handle_json_response(response).await
}

pub async fn fetch_copilot_internal_token(
    http: &reqwest::Client,
    domain: &str,
    github_token: &str,
) -> Result<CopilotTokenResponse, CopilotDeviceError> {
    let domain = normalize_github_domain(domain)?;
    fetch_copilot_token(http, &domain, github_token).await
}

pub async fn fetch_copilot_api_endpoint(
    http: &reqwest::Client,
    domain: &str,
    github_token: &str,
) -> Result<String, CopilotDeviceError> {
    let domain = normalize_github_domain(domain)?;
    fetch_copilot_api_endpoint_from_url(http, &copilot_usage_url(&domain), &domain, github_token)
        .await
}

async fn fetch_copilot_api_endpoint_from_url(
    http: &reqwest::Client,
    url: &str,
    fallback_domain: &str,
    github_token: &str,
) -> Result<String, CopilotDeviceError> {
    let response = http
        .get(url)
        .header("Authorization", format!("token {github_token}"))
        .header("Content-Type", "application/json")
        .header("editor-version", COPILOT_EDITOR_VERSION)
        .header("editor-plugin-version", COPILOT_PLUGIN_VERSION)
        .header("user-agent", COPILOT_USER_AGENT)
        .header("x-github-api-version", COPILOT_API_VERSION)
        .send()
        .await
        .map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("copilot endpoint request failed: {error}"))
        })?;
    let user: CopilotUserResponse = handle_json_response(response).await?;
    Ok(user
        .endpoints
        .map(|endpoints| endpoints.api)
        .filter(|endpoint| !endpoint.trim().is_empty())
        .unwrap_or_else(|| copilot_api_base(fallback_domain)))
}

async fn fetch_copilot_usage(
    http: &reqwest::Client,
    domain: &str,
    github_token: &str,
) -> Result<Value, CopilotDeviceError> {
    let response = http
        .get(copilot_usage_url(domain))
        .header("Authorization", format!("token {github_token}"))
        .header("Content-Type", "application/json")
        .header("editor-version", COPILOT_EDITOR_VERSION)
        .header("editor-plugin-version", COPILOT_PLUGIN_VERSION)
        .header("user-agent", COPILOT_USER_AGENT)
        .header("x-github-api-version", COPILOT_API_VERSION)
        .send()
        .await
        .map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("copilot usage request failed: {error}"))
        })?;
    handle_json_response(response).await
}

async fn handle_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, CopilotDeviceError> {
    if response.status().is_success() {
        return response.json::<T>().await.map_err(|error| {
            CopilotDeviceError::bad_gateway(format!("parse copilot response failed: {error}"))
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
    Err(CopilotDeviceError::remote(status, message))
}

pub fn normalize_github_domain(raw: &str) -> Result<String, CopilotDeviceError> {
    let value = raw.trim();
    let value = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    let host = value.split(&['/', '?', '#'][..]).next().unwrap_or(value);
    if host.contains('@') {
        return Err(CopilotDeviceError::bad_request("invalid GitHub domain"));
    }
    let normalized = host.to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(CopilotDeviceError::bad_request("invalid GitHub domain"));
    }
    Ok(normalized)
}

fn github_client_id(domain: &str) -> &'static str {
    if domain == DEFAULT_GITHUB_DOMAIN {
        GITHUB_CLIENT_ID
    } else {
        GITHUB_CLIENT_ID_GHES
    }
}

fn github_device_code_url(domain: &str) -> String {
    format!("https://{domain}/login/device/code")
}

fn github_oauth_token_url(domain: &str) -> String {
    format!("https://{domain}/login/oauth/access_token")
}

pub fn github_api_base(domain: &str) -> String {
    if domain == DEFAULT_GITHUB_DOMAIN {
        "https://api.github.com".to_string()
    } else {
        format!("https://{domain}/api/v3")
    }
}

fn github_user_url(domain: &str) -> String {
    format!("{}/user", github_api_base(domain))
}

pub fn copilot_token_url(domain: &str) -> String {
    format!("{}/copilot_internal/v2/token", github_api_base(domain))
}

pub fn copilot_usage_url(domain: &str) -> String {
    format!("{}/copilot_internal/user", github_api_base(domain))
}

pub fn copilot_api_base(domain: &str) -> String {
    if domain == DEFAULT_GITHUB_DOMAIN {
        "https://api.githubcopilot.com".to_string()
    } else {
        format!("https://copilot-api.{domain}")
    }
}

pub fn is_ghes(domain: &str) -> bool {
    domain != DEFAULT_GITHUB_DOMAIN
}

fn composite_account_id(domain: &str, user_id: u64) -> String {
    if domain == DEFAULT_GITHUB_DOMAIN {
        user_id.to_string()
    } else {
        format!("{domain}:{user_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::get;
    use axum::Router;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn normalizes_github_domain() {
        assert_eq!(
            normalize_github_domain("https://GitHub.COM/login").unwrap(),
            "github.com"
        );
        assert!(normalize_github_domain("https://user@example.com").is_err());
    }

    #[test]
    fn copilot_urls_follow_public_and_ghes_rules() {
        assert_eq!(github_api_base("github.com"), "https://api.github.com");
        assert_eq!(
            github_api_base("ghe.example.com"),
            "https://ghe.example.com/api/v3"
        );
        assert_eq!(
            copilot_token_url("github.com"),
            "https://api.github.com/copilot_internal/v2/token"
        );
        assert_eq!(
            copilot_usage_url("ghe.example.com"),
            "https://ghe.example.com/api/v3/copilot_internal/user"
        );
        assert_eq!(
            copilot_api_base("ghe.example.com"),
            "https://copilot-api.ghe.example.com"
        );
    }

    #[tokio::test]
    async fn fetch_copilot_internal_token_sends_expected_headers() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/api/v3/copilot_internal/v2/token",
                get(
                    |State(seen): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                        assert_eq!(
                            headers.get("authorization").and_then(|v| v.to_str().ok()),
                            Some("token github-token")
                        );
                        assert_eq!(
                            headers.get("editor-version").and_then(|v| v.to_str().ok()),
                            Some(COPILOT_EDITOR_VERSION)
                        );
                        assert_eq!(
                            headers
                                .get("editor-plugin-version")
                                .and_then(|v| v.to_str().ok()),
                            Some(COPILOT_PLUGIN_VERSION)
                        );
                        assert_eq!(
                            headers
                                .get("x-github-api-version")
                                .and_then(|v| v.to_str().ok()),
                            Some(COPILOT_API_VERSION)
                        );
                        seen.fetch_add(1, Ordering::SeqCst);
                        axum::Json(json!({
                            "token": "copilot-internal-token",
                            "expires_at": 4_102_444_800_i64
                        }))
                    },
                ),
            )
            .with_state(seen.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let response = fetch_copilot_token_from_url(
            &reqwest::Client::new(),
            &format!("http://{addr}/api/v3/copilot_internal/v2/token"),
            "github-token",
        )
        .await
        .unwrap();

        assert_eq!(response.token, "copilot-internal-token");
        assert_eq!(response.expires_at, 4_102_444_800);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fetch_copilot_api_endpoint_uses_discovered_endpoint_or_fallback() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/v3/copilot_internal/user",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers.get("authorization").and_then(|v| v.to_str().ok()),
                    Some("token github-token")
                );
                axum::Json(json!({
                    "endpoints": {
                        "api": "https://copilot-api.enterprise.example.com"
                    }
                }))
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let endpoint = fetch_copilot_api_endpoint_from_url(
            &reqwest::Client::new(),
            &format!("http://{addr}/api/v3/copilot_internal/user"),
            &addr.to_string(),
            "github-token",
        )
        .await
        .unwrap();

        assert_eq!(endpoint, "https://copilot-api.enterprise.example.com");

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/v3/copilot_internal/user",
            get(|| async { axum::Json(json!({})) }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let endpoint = fetch_copilot_api_endpoint_from_url(
            &reqwest::Client::new(),
            &format!("http://{addr}/api/v3/copilot_internal/user"),
            &addr.to_string(),
            "github-token",
        )
        .await
        .unwrap();

        assert_eq!(endpoint, copilot_api_base(&addr.to_string()));
    }
}
