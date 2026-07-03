#![allow(dead_code)]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::core::accounts::{
    Account, AccountQuota, AccountQuotaTier, AccountRefreshUpdate, UpsertAccountInput,
};
use crate::core::provider::ProviderType;

const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthSupportStage {
    NativeRefreshProfile,
    FixtureReadyNativeDisabled,
    RequestShapeOnly,
    ManualImportOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthRequestBodyFormat {
    Form,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthAuthorizeFlow {
    AuthorizationCode,
    AuthorizationCodePkce,
    CursorDeepControl,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthProfileStrategy {
    JwtClaims,
    TokenResponseAccount,
    UserInfoEndpoint,
    ProviderSpecific,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthQuotaStrategy {
    ProviderSnapshot,
    UserInfoEndpoint,
    ProviderSpecific,
    NotAvailable,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthProviderSpec {
    pub provider_type: ProviderType,
    pub stage: OAuthSupportStage,
    pub authorize_url: Option<&'static str>,
    pub authorize_flow: OAuthAuthorizeFlow,
    pub authorize_scope: Option<&'static str>,
    pub token_urls: &'static [&'static str],
    pub token_body_format: OAuthRequestBodyFormat,
    pub client_id: Option<&'static str>,
    pub client_id_env: Option<&'static str>,
    pub client_secret: Option<&'static str>,
    pub client_secret_env: Option<&'static str>,
    pub refresh_scope: Option<&'static str>,
    pub user_agent: Option<&'static str>,
    pub profile_url: Option<&'static str>,
    pub profile_strategy: OAuthProfileStrategy,
    pub quota_strategy: OAuthQuotaStrategy,
}

impl OAuthProviderSpec {
    pub fn server_native_refresh_enabled(self) -> bool {
        matches!(self.stage, OAuthSupportStage::NativeRefreshProfile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthHttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    pub body_format: OAuthRequestBodyFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthErrorKind {
    AuthorizationPending,
    AccessDenied,
    InvalidGrant,
    ExpiredToken,
    MissingCredential,
    RateLimited,
    ProviderRejected,
    Network,
    Parse,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthErrorClassification {
    pub kind: OAuthErrorKind,
    pub retryable: bool,
    pub refresh_token_may_have_rotated: bool,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthTokenResponse {
    #[serde(alias = "api_key")]
    #[serde(alias = "apiKey")]
    #[serde(alias = "access_token")]
    pub access_token: String,
    #[serde(default)]
    #[serde(alias = "refresh_token")]
    pub refresh_token: Option<String>,
    #[serde(default)]
    #[serde(alias = "id_token")]
    pub id_token: Option<String>,
    #[serde(default)]
    #[serde(alias = "token_type")]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    #[serde(alias = "expires_in")]
    pub expires_in: Option<i64>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthIdentity {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

static CODEX_TOKEN_URLS: &[&str] = &["https://auth.openai.com/oauth/token"];
static CODEX_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
static CODEX_AUTHORIZE_SCOPE: &str = "openid profile email offline_access";
static CODEX_OAUTH_ORIGINATOR: &str = "codex_cli_rs";
static CLAUDE_TOKEN_URLS: &[&str] = &[
    "https://api.anthropic.com/v1/oauth/token",
    "https://platform.claude.com/v1/oauth/token",
];
static CLAUDE_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
static GEMINI_TOKEN_URLS: &[&str] = &["https://oauth2.googleapis.com/token"];
static GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
static CURSOR_TOKEN_URLS: &[&str] = &["https://api2.cursor.sh/oauth/token"];
static CURSOR_AUTHORIZE_URL: &str = "https://www.cursor.com/loginDeepControl";
static CURSOR_POLL_URL: &str = "https://api2.cursor.sh/auth/poll";
static CURSOR_USER_AGENT: &str = "Cursor/1.1.6 (cc-switch browser login)";
static ANTIGRAVITY_TOKEN_URLS: &[&str] = &["https://oauth2.googleapis.com/token"];
static ANTIGRAVITY_AUTHORIZE_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

pub fn oauth_provider_spec(provider_type: ProviderType) -> Option<OAuthProviderSpec> {
    match provider_type {
        ProviderType::CodexOAuth => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::NativeRefreshProfile,
            authorize_url: Some(CODEX_AUTHORIZE_URL),
            authorize_flow: OAuthAuthorizeFlow::AuthorizationCodePkce,
            authorize_scope: Some(CODEX_AUTHORIZE_SCOPE),
            token_urls: CODEX_TOKEN_URLS,
            token_body_format: OAuthRequestBodyFormat::Form,
            client_id: Some("app_EMoamEEZ73f0CkXaXp7hrann"),
            client_id_env: None,
            client_secret: None,
            client_secret_env: None,
            refresh_scope: Some("openid profile email"),
            user_agent: Some("cc-switch-server-codex-oauth"),
            profile_url: None,
            profile_strategy: OAuthProfileStrategy::JwtClaims,
            quota_strategy: OAuthQuotaStrategy::ProviderSnapshot,
        }),
        ProviderType::ClaudeOAuth => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::NativeRefreshProfile,
            authorize_url: Some(CLAUDE_AUTHORIZE_URL),
            authorize_flow: OAuthAuthorizeFlow::AuthorizationCodePkce,
            authorize_scope: Some(
                "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload",
            ),
            token_urls: CLAUDE_TOKEN_URLS,
            token_body_format: OAuthRequestBodyFormat::Json,
            client_id: Some("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
            client_id_env: None,
            client_secret: None,
            client_secret_env: None,
            refresh_scope: Some(
                "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload",
            ),
            user_agent: Some("cc-switch-server-claude-oauth"),
            profile_url: None,
            profile_strategy: OAuthProfileStrategy::TokenResponseAccount,
            quota_strategy: OAuthQuotaStrategy::ProviderSnapshot,
        }),
        ProviderType::GeminiCli => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::NativeRefreshProfile,
            authorize_url: Some(GOOGLE_AUTHORIZE_URL),
            authorize_flow: OAuthAuthorizeFlow::AuthorizationCode,
            authorize_scope: Some(
                "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile",
            ),
            token_urls: GEMINI_TOKEN_URLS,
            token_body_format: OAuthRequestBodyFormat::Form,
            client_id: None,
            client_id_env: Some("CC_SWITCH_SERVER_GEMINI_CLIENT_ID"),
            client_secret: None,
            client_secret_env: Some("CC_SWITCH_SERVER_GEMINI_CLIENT_SECRET"),
            refresh_scope: Some(
                "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile",
            ),
            user_agent: Some("cc-switch-server-gemini-oauth"),
            profile_url: Some("https://www.googleapis.com/oauth2/v2/userinfo"),
            profile_strategy: OAuthProfileStrategy::UserInfoEndpoint,
            quota_strategy: OAuthQuotaStrategy::ProviderSnapshot,
        }),
        ProviderType::CursorOAuth => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::NativeRefreshProfile,
            authorize_url: Some(CURSOR_AUTHORIZE_URL),
            authorize_flow: OAuthAuthorizeFlow::CursorDeepControl,
            authorize_scope: None,
            token_urls: CURSOR_TOKEN_URLS,
            token_body_format: OAuthRequestBodyFormat::Json,
            client_id: Some("KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB"),
            client_id_env: Some("CC_SWITCH_SERVER_CURSOR_CLIENT_ID"),
            client_secret: None,
            client_secret_env: None,
            refresh_scope: None,
            user_agent: Some("cc-switch-server-cursor-oauth"),
            profile_url: Some("https://cursor.com/api/auth/me"),
            profile_strategy: OAuthProfileStrategy::UserInfoEndpoint,
            quota_strategy: OAuthQuotaStrategy::ProviderSnapshot,
        }),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::NativeRefreshProfile,
            authorize_url: Some(GOOGLE_AUTHORIZE_URL),
            authorize_flow: OAuthAuthorizeFlow::AuthorizationCode,
            authorize_scope: Some(ANTIGRAVITY_AUTHORIZE_SCOPE),
            token_urls: ANTIGRAVITY_TOKEN_URLS,
            token_body_format: OAuthRequestBodyFormat::Form,
            client_id: None,
            client_id_env: Some("CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_ID"),
            client_secret: None,
            client_secret_env: Some("CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_SECRET"),
            refresh_scope: Some(
                "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile",
            ),
            user_agent: Some("cc-switch-server-antigravity-oauth"),
            profile_url: Some("https://www.googleapis.com/oauth2/v1/userinfo"),
            profile_strategy: OAuthProfileStrategy::UserInfoEndpoint,
            quota_strategy: OAuthQuotaStrategy::ProviderSpecific,
        }),
        ProviderType::GitHubCopilot
        | ProviderType::DeepSeekAccount
        | ProviderType::KiroOAuth
        | ProviderType::CursorApiKey
        | ProviderType::OllamaCloud
        | ProviderType::AwsBedrock
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => Some(OAuthProviderSpec {
            provider_type,
            stage: OAuthSupportStage::ManualImportOnly,
            authorize_url: None,
            authorize_flow: OAuthAuthorizeFlow::Unsupported,
            authorize_scope: None,
            token_urls: &[],
            token_body_format: OAuthRequestBodyFormat::Json,
            client_id: None,
            client_id_env: None,
            client_secret: None,
            client_secret_env: None,
            refresh_scope: None,
            user_agent: None,
            profile_url: None,
            profile_strategy: OAuthProfileStrategy::ProviderSpecific,
            quota_strategy: if provider_type == ProviderType::OllamaCloud {
                OAuthQuotaStrategy::ProviderSpecific
            } else {
                OAuthQuotaStrategy::ProviderSnapshot
            },
        }),
        _ => None,
    }
}

pub fn oauth_specs() -> Vec<OAuthProviderSpec> {
    [
        ProviderType::CodexOAuth,
        ProviderType::ClaudeOAuth,
        ProviderType::GeminiCli,
        ProviderType::CursorOAuth,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::CursorApiKey,
        ProviderType::AntigravityOAuth,
        ProviderType::AgyOAuth,
        ProviderType::OllamaCloud,
        ProviderType::AwsBedrock,
        ProviderType::Nvidia,
        ProviderType::DeepSeekApi,
    ]
    .into_iter()
    .filter_map(oauth_provider_spec)
    .collect()
}

pub fn provider_login_request_shape_available(provider_type: ProviderType) -> bool {
    oauth_provider_spec(provider_type).is_some_and(|spec| {
        spec.authorize_url.is_some()
            && !matches!(spec.authorize_flow, OAuthAuthorizeFlow::Unsupported)
    })
}

pub fn provider_token_exchange_available(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::CodexOAuth
            | ProviderType::ClaudeOAuth
            | ProviderType::GeminiCli
            | ProviderType::CursorOAuth
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
    )
}

pub fn build_authorize_url(
    provider_type: ProviderType,
    redirect_uri: Option<&str>,
    code_challenge: Option<&str>,
    state: &str,
) -> Result<String, OAuthErrorClassification> {
    let spec = oauth_provider_spec(provider_type).ok_or_else(|| unsupported(provider_type))?;
    let authorize_url = spec
        .authorize_url
        .ok_or_else(|| unsupported_login(provider_type))?;
    let client_id = resolve_spec_client_id(&spec)?;

    match spec.authorize_flow {
        OAuthAuthorizeFlow::AuthorizationCodePkce => {
            let redirect_uri = redirect_uri
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing_credential("redirect_uri is required"))?;
            let code_challenge = code_challenge
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing_credential("code_challenge is required"))?;
            let scope = spec.authorize_scope.unwrap_or_default();
            let mut params = vec![
                ("response_type", "code".to_string()),
                ("client_id", client_id),
                ("redirect_uri", redirect_uri.to_string()),
                ("scope", scope.to_string()),
                ("code_challenge", code_challenge.to_string()),
                ("code_challenge_method", "S256".to_string()),
                ("state", state.to_string()),
            ];
            match provider_type {
                ProviderType::CodexOAuth => {
                    params.push(("id_token_add_organizations", "true".to_string()));
                    params.push(("codex_cli_simplified_flow", "true".to_string()));
                    params.push(("prompt", "login".to_string()));
                    params.push(("originator", CODEX_OAUTH_ORIGINATOR.to_string()));
                }
                ProviderType::ClaudeOAuth => {
                    params.insert(0, ("code", "true".to_string()));
                }
                _ => {}
            }
            Ok(format!("{authorize_url}?{}", query_string(&params)))
        }
        OAuthAuthorizeFlow::AuthorizationCode => {
            let redirect_uri = redirect_uri
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing_credential("redirect_uri is required"))?;
            let scope = spec
                .authorize_scope
                .or(spec.refresh_scope)
                .unwrap_or_default();
            let params = vec![
                ("client_id", client_id),
                ("redirect_uri", redirect_uri.to_string()),
                ("response_type", "code".to_string()),
                ("scope", scope.to_string()),
                ("access_type", "offline".to_string()),
                ("prompt", "consent".to_string()),
                ("state", state.to_string()),
            ];
            Ok(format!("{authorize_url}?{}", query_string(&params)))
        }
        OAuthAuthorizeFlow::CursorDeepControl => {
            let code_challenge = code_challenge
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| missing_credential("code_challenge is required"))?;
            let params = vec![
                ("challenge", code_challenge.to_string()),
                ("uuid", state.to_string()),
                ("mode", "login".to_string()),
                ("redirectTarget", "cli".to_string()),
            ];
            Ok(format!("{authorize_url}?{}", query_string(&params)))
        }
        OAuthAuthorizeFlow::Unsupported => Err(unsupported_login(provider_type)),
    }
}

pub fn build_authorization_code_request(
    provider_type: ProviderType,
    code: &str,
    redirect_uri: &str,
    code_verifier: Option<&str>,
    state: &str,
) -> Result<OAuthHttpRequest, OAuthErrorClassification> {
    let spec = oauth_provider_spec(provider_type).ok_or_else(|| unsupported(provider_type))?;
    if !matches!(
        spec.authorize_flow,
        OAuthAuthorizeFlow::AuthorizationCode | OAuthAuthorizeFlow::AuthorizationCodePkce
    ) {
        return Err(unsupported_login(provider_type));
    }
    let token_url = spec
        .token_urls
        .first()
        .copied()
        .ok_or_else(|| unsupported(provider_type))?;
    let client_id = resolve_spec_client_id(&spec)?;
    let client_secret = resolve_spec_client_secret(&spec);

    let mut headers = vec![
        (
            "Content-Type".to_string(),
            match spec.token_body_format {
                OAuthRequestBodyFormat::Form => "application/x-www-form-urlencoded".to_string(),
                OAuthRequestBodyFormat::Json => "application/json".to_string(),
            },
        ),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
    ];
    if let Some(user_agent) = spec.user_agent {
        headers.push(("User-Agent".to_string(), user_agent.to_string()));
    }

    let mut body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": client_id,
    });
    if let Some(code_verifier) = code_verifier.filter(|value| !value.trim().is_empty()) {
        body["code_verifier"] = Value::String(code_verifier.to_string());
    }
    if let Some(client_secret) = client_secret {
        body["client_secret"] = Value::String(client_secret);
    }
    if provider_type == ProviderType::ClaudeOAuth {
        body["state"] = Value::String(state.to_string());
    }

    Ok(OAuthHttpRequest {
        method: "POST",
        url: token_url.to_string(),
        headers,
        body,
        body_format: spec.token_body_format,
    })
}

pub fn build_cursor_poll_request(
    state: &str,
    verifier: &str,
) -> Result<OAuthHttpRequest, OAuthErrorClassification> {
    let params = vec![
        ("uuid", state.to_string()),
        ("verifier", verifier.to_string()),
    ];
    Ok(OAuthHttpRequest {
        method: "GET",
        url: format!("{CURSOR_POLL_URL}?{}", query_string(&params)),
        headers: vec![
            (
                "Accept".to_string(),
                "application/json, text/plain, */*".to_string(),
            ),
            ("User-Agent".to_string(), CURSOR_USER_AGENT.to_string()),
        ],
        body: Value::Null,
        body_format: OAuthRequestBodyFormat::Json,
    })
}

pub fn token_expires_soon(account: &Account, now_ms: i64) -> bool {
    account
        .expires_at
        .is_some_and(|expires_at| expires_at.saturating_sub(now_ms) <= TOKEN_REFRESH_BUFFER_MS)
}

pub fn build_refresh_request(
    provider_type: ProviderType,
    account: &Account,
) -> Result<OAuthHttpRequest, OAuthErrorClassification> {
    let spec = oauth_provider_spec(provider_type).ok_or_else(|| unsupported(provider_type))?;
    let token_url = spec
        .token_urls
        .first()
        .copied()
        .ok_or_else(|| unsupported(provider_type))?;
    build_refresh_request_for_token_url(provider_type, account, token_url)
}

pub fn build_refresh_request_for_token_url(
    provider_type: ProviderType,
    account: &Account,
    token_url: &str,
) -> Result<OAuthHttpRequest, OAuthErrorClassification> {
    let spec = oauth_provider_spec(provider_type).ok_or_else(|| unsupported(provider_type))?;
    let refresh_token = account
        .refresh_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| missing_credential("refresh token is required"))?;
    let client_id = resolve_client_id(&spec, account)?;
    let client_secret = resolve_client_secret(&spec, account)?;

    let mut headers = vec![(
        "Content-Type".to_string(),
        match spec.token_body_format {
            OAuthRequestBodyFormat::Form => "application/x-www-form-urlencoded".to_string(),
            OAuthRequestBodyFormat::Json => "application/json".to_string(),
        },
    )];
    if let Some(user_agent) = spec.user_agent {
        headers.push(("User-Agent".to_string(), user_agent.to_string()));
    }

    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    if let Some(client_id) = client_id {
        body["client_id"] = Value::String(client_id);
    }
    if let Some(client_secret) = client_secret {
        body["client_secret"] = Value::String(client_secret);
    }
    if let Some(scope) = spec.refresh_scope {
        body["scope"] = Value::String(scope.to_string());
    }

    Ok(OAuthHttpRequest {
        method: "POST",
        url: token_url.to_string(),
        headers,
        body,
        body_format: spec.token_body_format,
    })
}

pub fn build_profile_request(
    provider_type: ProviderType,
    access_token: &str,
) -> Option<OAuthHttpRequest> {
    let spec = oauth_provider_spec(provider_type)?;
    let profile_url = spec.profile_url?;
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {}", access_token.trim()),
        ),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
    ];
    if let Some(user_agent) = spec.user_agent {
        headers.push(("User-Agent".to_string(), user_agent.to_string()));
    }
    Some(OAuthHttpRequest {
        method: "GET",
        url: profile_url.to_string(),
        headers,
        body: Value::Null,
        body_format: OAuthRequestBodyFormat::Json,
    })
}

pub fn refresh_update_from_token_response(
    provider_type: ProviderType,
    response: &OAuthTokenResponse,
    raw: Value,
    now_ms: i64,
) -> AccountRefreshUpdate {
    let mut identity = identity_from_token_response(response);
    if identity == OAuthIdentity::default() {
        identity = identity_from_provider_value(&raw).unwrap_or_default();
    }
    let quota = quota_from_provider_snapshot(provider_type, &raw);
    let quota_percent = quota
        .as_ref()
        .and_then(|quota| quota.tiers.first())
        .and_then(|tier| tier.utilization)
        .map(|utilization| utilization * 100.0);
    AccountRefreshUpdate {
        email: identity.email.clone(),
        access_token: Some(response.access_token.clone()),
        refresh_token: response.refresh_token.clone(),
        id_token: response.id_token.clone(),
        token_type: response.token_type.clone(),
        scopes: response.scope.as_deref().map(split_scopes),
        profile: profile_value(provider_type, &identity, &raw),
        raw: Some(raw),
        subscription_level: identity.plan_type.clone(),
        quota_percent,
        quota,
        quota_refreshed_at: quota_percent.map(|_| now_ms),
        quota_next_refresh_at: quota_percent.map(|_| now_ms.saturating_add(5 * 60 * 1000)),
        expires_at: response
            .expires_in
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000))),
        ..Default::default()
    }
}

pub fn refresh_update_from_profile_response(
    provider_type: ProviderType,
    raw: Value,
    now_ms: i64,
) -> AccountRefreshUpdate {
    let identity = identity_from_provider_value(&raw).unwrap_or_default();
    let quota = quota_from_provider_snapshot(provider_type, &raw);
    let quota_percent = quota
        .as_ref()
        .and_then(|quota| quota.tiers.first())
        .and_then(|tier| tier.utilization)
        .map(|utilization| utilization * 100.0)
        .or_else(|| quota_percent_from_value(&raw));
    AccountRefreshUpdate {
        email: identity.email.clone(),
        profile: Some(json!({
            "providerType": provider_type.as_str(),
            "source": "profile_response",
            "accountId": identity.account_id,
            "email": identity.email,
            "planType": identity.plan_type,
            "raw": raw
        })),
        subscription_level: identity.plan_type,
        quota_percent,
        quota,
        quota_refreshed_at: quota_percent.map(|_| now_ms),
        quota_next_refresh_at: quota_percent.map(|_| now_ms.saturating_add(5 * 60 * 1000)),
        ..Default::default()
    }
}

pub fn upsert_input_from_token_response(
    provider_type: ProviderType,
    response: &OAuthTokenResponse,
    raw: Value,
    now_ms: i64,
) -> Result<UpsertAccountInput, OAuthErrorClassification> {
    upsert_input_from_login_response(provider_type, response, raw, None, now_ms)
}

pub fn upsert_input_from_login_response(
    provider_type: ProviderType,
    response: &OAuthTokenResponse,
    token_raw: Value,
    profile_raw: Option<Value>,
    now_ms: i64,
) -> Result<UpsertAccountInput, OAuthErrorClassification> {
    let identity = login_identity(provider_type, response, &token_raw, profile_raw.as_ref());
    let account_id = identity.account_id.clone().ok_or_else(|| {
        missing_credential(format!(
            "{} token response does not contain an account id",
            provider_type.as_str()
        ))
    })?;
    if provider_token_exchange_available(provider_type)
        && response
            .refresh_token
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(missing_credential(format!(
            "{} oauth token response is missing refresh_token",
            provider_type.as_str()
        )));
    }

    let mut update =
        refresh_update_from_token_response(provider_type, response, token_raw.clone(), now_ms);
    if let Some(profile_raw) = profile_raw.clone() {
        update = merge_refresh_updates(
            update,
            refresh_update_from_profile_response(provider_type, profile_raw, now_ms),
        );
    }
    if update.email.is_none() {
        update.email = identity.email.clone();
    }
    if update.subscription_level.is_none() {
        update.subscription_level = identity.plan_type.clone();
    }
    update.profile =
        login_profile_value(provider_type, &identity, &token_raw, profile_raw.as_ref())
            .or(update.profile);
    update.raw = Some(login_raw_value(token_raw, profile_raw));

    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type,
        email: update.email,
        access_token: update.access_token,
        refresh_token: update.refresh_token,
        id_token: update.id_token,
        token_type: update.token_type,
        api_key: None,
        scopes: update.scopes.unwrap_or_default(),
        profile: update.profile,
        raw: update.raw,
        subscription_level: update.subscription_level,
        quota_percent: update.quota_percent,
        quota: update.quota,
        quota_refreshed_at: update.quota_refreshed_at,
        quota_next_refresh_at: update.quota_next_refresh_at,
        expires_at: update.expires_at,
        last_refresh_error: None,
    })
}

pub fn merge_refresh_updates(
    mut base: AccountRefreshUpdate,
    overlay: AccountRefreshUpdate,
) -> AccountRefreshUpdate {
    if overlay.email.is_some() {
        base.email = overlay.email;
    }
    if overlay.access_token.is_some() {
        base.access_token = overlay.access_token;
    }
    if overlay.refresh_token.is_some() {
        base.refresh_token = overlay.refresh_token;
    }
    if overlay.id_token.is_some() {
        base.id_token = overlay.id_token;
    }
    if overlay.token_type.is_some() {
        base.token_type = overlay.token_type;
    }
    if overlay.scopes.is_some() {
        base.scopes = overlay.scopes;
    }
    if overlay.profile.is_some() {
        base.profile = overlay.profile;
    }
    if overlay.raw.is_some() {
        base.raw = overlay.raw;
    }
    if overlay.subscription_level.is_some() {
        base.subscription_level = overlay.subscription_level;
    }
    if overlay.quota_percent.is_some() {
        base.quota_percent = overlay.quota_percent;
    }
    if overlay.quota.is_some() {
        base.quota = overlay.quota;
    }
    if overlay.quota_refreshed_at.is_some() {
        base.quota_refreshed_at = overlay.quota_refreshed_at;
    }
    if overlay.quota_next_refresh_at.is_some() {
        base.quota_next_refresh_at = overlay.quota_next_refresh_at;
    }
    if overlay.expires_at.is_some() {
        base.expires_at = overlay.expires_at;
    }
    if overlay.last_refresh_error.is_some() {
        base.last_refresh_error = overlay.last_refresh_error;
    }
    base
}

pub fn identity_from_token_response(response: &OAuthTokenResponse) -> OAuthIdentity {
    response
        .id_token
        .as_deref()
        .and_then(identity_from_jwt)
        .or_else(|| identity_from_jwt(&response.access_token))
        .unwrap_or_default()
}

pub fn identity_from_jwt(token: &str) -> Option<OAuthIdentity> {
    let claims = decode_jwt_claims(token)?;
    Some(OAuthIdentity {
        account_id: string_at(&claims, &["/chatgpt_account_id"])
            .or_else(|| string_at(&claims, &["/openai_auth/chatgpt_account_id"]))
            .or_else(|| string_at(&claims, &["/organizations/0/id"]))
            .or_else(|| string_at(&claims, &["/sub"])),
        email: string_at(&claims, &["/email"]),
        plan_type: string_at(
            &claims,
            &[
                "/openai_auth/chatgpt_plan_type",
                "/plan_type",
                "/plan",
                "/tier",
            ],
        ),
    })
}

pub fn classify_oauth_error(status_code: Option<u16>, body: &str) -> OAuthErrorClassification {
    let body_lower = body.to_ascii_lowercase();
    let json_message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            string_at(
                &value,
                &[
                    "/error",
                    "/error_description",
                    "/message",
                    "/detail",
                    "/error/message",
                ],
            )
        })
        .unwrap_or_else(|| body.trim().chars().take(300).collect::<String>());
    let message_lower = json_message.to_ascii_lowercase();
    let haystack = format!("{body_lower}\n{message_lower}");

    let kind = if haystack.contains("authorization_pending") {
        OAuthErrorKind::AuthorizationPending
    } else if haystack.contains("access_denied") {
        OAuthErrorKind::AccessDenied
    } else if haystack.contains("invalid_grant") || haystack.contains("refresh token") {
        OAuthErrorKind::InvalidGrant
    } else if haystack.contains("expired_token") {
        OAuthErrorKind::ExpiredToken
    } else if status_code == Some(429) || haystack.contains("rate limit") {
        OAuthErrorKind::RateLimited
    } else if matches!(status_code, Some(500..=599)) {
        OAuthErrorKind::ProviderRejected
    } else if matches!(status_code, Some(401 | 403)) {
        OAuthErrorKind::InvalidGrant
    } else {
        OAuthErrorKind::Unknown
    };

    OAuthErrorClassification {
        kind,
        retryable: matches!(
            kind,
            OAuthErrorKind::AuthorizationPending
                | OAuthErrorKind::RateLimited
                | OAuthErrorKind::ProviderRejected
                | OAuthErrorKind::Network
        ),
        refresh_token_may_have_rotated: matches!(kind, OAuthErrorKind::InvalidGrant),
        message: if json_message.is_empty() {
            "oauth request failed".to_string()
        } else {
            json_message
        },
    }
}

pub fn is_refresh_race_recoverable(error: &OAuthErrorClassification) -> bool {
    error.refresh_token_may_have_rotated
        || error.message.to_ascii_lowercase().contains("refresh token")
}

pub fn quota_from_provider_snapshot(
    provider_type: ProviderType,
    value: &Value,
) -> Option<AccountQuota> {
    if provider_type == ProviderType::OllamaCloud {
        return None;
    }
    let percent = quota_percent_from_value(value)?;
    Some(AccountQuota {
        success: true,
        credential_message: Some("quota parsed from provider snapshot".to_string()),
        tiers: vec![AccountQuotaTier {
            name: provider_type.as_str().to_string(),
            utilization: Some((percent / 100.0).clamp(0.0, 1.0)),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: integer_at(value, &["/resetsAt", "/reset_at", "/quota/resetAt"]),
        }],
        extra_usage: Some(value.clone()),
    })
}

fn resolve_client_id(
    spec: &OAuthProviderSpec,
    account: &Account,
) -> Result<Option<String>, OAuthErrorClassification> {
    if let Some(value) = string_at(account.raw.as_ref().unwrap_or(&Value::Null), &["/clientId"]) {
        return Ok(Some(value));
    }
    if let Some(env_name) = spec.client_id_env {
        if let Some(value) = std::env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(Some(value));
        }
    }
    if let Some(value) = spec.client_id {
        return Ok(Some(value.to_string()));
    }
    if let Some(env_name) = spec.client_id_env {
        return Err(missing_credential(format!("{env_name} is required")));
    }
    Ok(None)
}

fn resolve_client_secret(
    spec: &OAuthProviderSpec,
    account: &Account,
) -> Result<Option<String>, OAuthErrorClassification> {
    if let Some(value) = string_at(
        account.raw.as_ref().unwrap_or(&Value::Null),
        &["/clientSecret"],
    ) {
        return Ok(Some(value));
    }
    if let Some(env_name) = spec.client_secret_env {
        if let Some(value) = std::env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(Some(value));
        }
    }
    if let Some(value) = spec.client_secret {
        return Ok(Some(value.to_string()));
    }
    if let Some(env_name) = spec.client_secret_env {
        return Err(missing_credential(format!("{env_name} is required")));
    }
    Ok(None)
}

fn decode_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn profile_value(
    provider_type: ProviderType,
    identity: &OAuthIdentity,
    raw: &Value,
) -> Option<Value> {
    if identity == &OAuthIdentity::default() {
        return None;
    }
    Some(json!({
        "providerType": provider_type.as_str(),
        "accountId": identity.account_id,
        "email": identity.email,
        "planType": identity.plan_type,
        "source": "token_response",
        "rawKeys": raw.as_object().map(|object| object.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
    }))
}

fn login_identity(
    provider_type: ProviderType,
    response: &OAuthTokenResponse,
    token_raw: &Value,
    profile_raw: Option<&Value>,
) -> OAuthIdentity {
    let mut identity = identity_from_token_response(response);
    if identity == OAuthIdentity::default() {
        identity = identity_from_provider_value(token_raw).unwrap_or_default();
    }
    if identity == OAuthIdentity::default() {
        if let Some(profile_raw) = profile_raw {
            identity = identity_from_provider_value(profile_raw).unwrap_or_default();
        }
    }
    if provider_type == ProviderType::CursorOAuth {
        if let Some(refresh_token) = response
            .refresh_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            identity.account_id = Some(cursor_account_id_from_refresh_token(refresh_token));
        }
    }
    if identity.email.is_none() {
        identity.email = profile_raw.and_then(|value| {
            string_at(
                value,
                &[
                    "/email",
                    "/email_address",
                    "/user/email",
                    "/profile/email",
                    "/account/email",
                    "/account/email_address",
                ],
            )
        });
    }
    if matches!(
        provider_type,
        ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) && identity.account_id.is_none()
    {
        identity.account_id = identity.email.clone();
    }
    identity
}

fn login_profile_value(
    provider_type: ProviderType,
    identity: &OAuthIdentity,
    token_raw: &Value,
    profile_raw: Option<&Value>,
) -> Option<Value> {
    if identity == &OAuthIdentity::default() && profile_raw.is_none() {
        return None;
    }
    let mut value = json!({
        "providerType": provider_type.as_str(),
        "source": "login_exchange",
        "accountId": identity.account_id,
        "email": identity.email,
        "planType": identity.plan_type,
        "tokenRawKeys": token_raw.as_object().map(|object| object.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
    });
    if let Some(profile_raw) = profile_raw {
        value["profileRaw"] = profile_raw.clone();
    }
    if matches!(
        provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        value["postExchangeEnrichment"] =
            Value::String("project_and_tier_deferred_to_quota_refresh".to_string());
    }
    Some(value)
}

fn login_raw_value(token_raw: Value, profile_raw: Option<Value>) -> Value {
    match profile_raw {
        Some(profile_raw) => json!({
            "token": token_raw,
            "profile": profile_raw,
        }),
        None => token_raw,
    }
}

fn cursor_account_id_from_refresh_token(refresh_token: &str) -> String {
    let digest = Sha256::digest(refresh_token.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("cursor_{}", &hex[..24])
}

fn identity_from_provider_value(value: &Value) -> Option<OAuthIdentity> {
    let identity = OAuthIdentity {
        account_id: string_at(
            value,
            &[
                "/account/email_address",
                "/account/email",
                "/account/uuid",
                "/user/email",
                "/profile/email",
                "/email",
                "/email_address",
                "/id",
                "/sub",
            ],
        ),
        email: string_at(
            value,
            &[
                "/account/email_address",
                "/account/email",
                "/user/email",
                "/profile/email",
                "/email",
                "/email_address",
            ],
        ),
        plan_type: string_at(
            value,
            &[
                "/plan",
                "/Plan",
                "/planType",
                "/plan_type",
                "/subscriptionLevel",
                "/subscription_level",
                "/account/plan",
                "/account/plan_type",
                "/user/plan",
                "/profile/plan",
                "/tier",
            ],
        ),
    };
    (identity != OAuthIdentity::default()).then_some(identity)
}

fn split_scopes(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
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

fn integer_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_i64))
}

fn quota_percent_from_value(value: &Value) -> Option<f64> {
    [
        "/quotaPercent",
        "/quota_percent",
        "/usage/percent",
        "/usage/quotaPercent",
        "/limits/percent",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_f64))
    .filter(|percent| percent.is_finite())
}

fn resolve_spec_client_id(spec: &OAuthProviderSpec) -> Result<String, OAuthErrorClassification> {
    if let Some(env_name) = spec.client_id_env {
        if let Some(value) = std::env::var(env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(value);
        }
    }
    spec.client_id
        .map(str::to_string)
        .ok_or_else(|| missing_credential("client_id is required"))
}

fn resolve_spec_client_secret(spec: &OAuthProviderSpec) -> Option<String> {
    spec.client_secret_env
        .and_then(|env_name| std::env::var(env_name).ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| spec.client_secret.map(str::to_string))
}

fn query_string(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn unsupported(provider_type: ProviderType) -> OAuthErrorClassification {
    OAuthErrorClassification {
        kind: OAuthErrorKind::Unsupported,
        retryable: false,
        refresh_token_may_have_rotated: false,
        message: format!(
            "{} server-native oauth flow is not enabled",
            provider_type.as_str()
        ),
    }
}

fn unsupported_login(provider_type: ProviderType) -> OAuthErrorClassification {
    OAuthErrorClassification {
        kind: OAuthErrorKind::Unsupported,
        retryable: false,
        refresh_token_may_have_rotated: false,
        message: format!(
            "{} browser login request shape is not available",
            provider_type.as_str()
        ),
    }
}

fn missing_credential(message: impl Into<String>) -> OAuthErrorClassification {
    OAuthErrorClassification {
        kind: OAuthErrorKind::MissingCredential,
        retryable: false,
        refresh_token_may_have_rotated: false,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::accounts::AccountStore;
    use crate::core::accounts::UpsertAccountInput;

    fn account(provider_type: ProviderType) -> Account {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type,
            email: Some("owner@example.com".to_string()),
            access_token: Some("old-access".to_string()),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            quota: None,
            quota_percent: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: Some(1_100_000),
            last_refresh_error: None,
        })
    }

    fn account_with_raw(provider_type: ProviderType, raw: Value) -> Account {
        let mut account = account(provider_type);
        account.raw = Some(raw);
        account
    }

    fn jwt(payload: &str) -> String {
        format!("header.{}.sig", URL_SAFE_NO_PAD.encode(payload.as_bytes()))
    }

    #[test]
    fn codex_refresh_request_is_tauri_free_form_request() {
        let request =
            build_refresh_request(ProviderType::CodexOAuth, &account(ProviderType::CodexOAuth))
                .expect("codex request");

        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://auth.openai.com/oauth/token");
        assert_eq!(request.body_format, OAuthRequestBodyFormat::Form);
        assert_eq!(request.body["grant_type"], "refresh_token");
        assert_eq!(request.body["refresh_token"], "refresh-token");
        assert_eq!(request.body["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(request.body["scope"], "openid profile email");
    }

    #[test]
    fn claude_refresh_request_keeps_api_and_platform_fallback_urls() {
        let spec = oauth_provider_spec(ProviderType::ClaudeOAuth).unwrap();
        assert_eq!(spec.token_urls.len(), 2);
        assert_eq!(spec.token_body_format, OAuthRequestBodyFormat::Json);
        assert_eq!(
            spec.profile_strategy,
            OAuthProfileStrategy::TokenResponseAccount
        );
        assert_eq!(spec.quota_strategy, OAuthQuotaStrategy::ProviderSnapshot);

        let request = build_refresh_request(
            ProviderType::ClaudeOAuth,
            &account(ProviderType::ClaudeOAuth),
        )
        .expect("claude request");
        assert_eq!(request.url, "https://api.anthropic.com/v1/oauth/token");
        assert_eq!(request.body_format, OAuthRequestBodyFormat::Json);
    }

    #[test]
    fn google_style_provider_specs_require_external_client_credentials() {
        let gemini = oauth_provider_spec(ProviderType::GeminiCli).unwrap();
        assert_eq!(gemini.client_id, None);
        assert_eq!(
            gemini.client_id_env,
            Some("CC_SWITCH_SERVER_GEMINI_CLIENT_ID")
        );
        assert_eq!(gemini.client_secret, None);
        assert_eq!(
            gemini.client_secret_env,
            Some("CC_SWITCH_SERVER_GEMINI_CLIENT_SECRET")
        );

        let antigravity = oauth_provider_spec(ProviderType::AntigravityOAuth).unwrap();
        assert_eq!(antigravity.client_id, None);
        assert_eq!(
            antigravity.client_id_env,
            Some("CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_ID")
        );
        assert_eq!(antigravity.client_secret, None);
        assert_eq!(
            antigravity.client_secret_env,
            Some("CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_SECRET")
        );
    }

    #[test]
    fn detects_expiring_tokens_with_refresh_buffer() {
        let account = account(ProviderType::CodexOAuth);
        assert!(token_expires_soon(&account, 1_050_001));
        assert!(!token_expires_soon(&account, 1_000));
    }

    #[test]
    fn parses_codex_jwt_identity_and_refresh_update() {
        let id_token = jwt(
            r#"{"chatgpt_account_id":"acc-123","email":"owner@example.com","openai_auth":{"chatgpt_plan_type":"plus"}}"#,
        );
        let raw = json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "id_token": id_token,
            "token_type": "Bearer",
            "scope": "openid profile email",
            "expires_in": 3600
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let identity = identity_from_token_response(&response);
        assert_eq!(identity.account_id.as_deref(), Some("acc-123"));
        assert_eq!(identity.email.as_deref(), Some("owner@example.com"));
        assert_eq!(identity.plan_type.as_deref(), Some("plus"));

        let update =
            refresh_update_from_token_response(ProviderType::CodexOAuth, &response, raw, 1_000);
        assert_eq!(update.access_token.as_deref(), Some("access-new"));
        assert_eq!(update.refresh_token.as_deref(), Some("refresh-new"));
        assert_eq!(update.scopes.unwrap(), vec!["openid", "profile", "email"]);
        assert_eq!(update.subscription_level.as_deref(), Some("plus"));
        assert_eq!(update.expires_at, Some(3_601_000));
    }

    #[test]
    fn codex_token_response_builds_account_import_input() {
        let id_token = jwt(
            r#"{"chatgpt_account_id":"acc-123","email":"owner@example.com","openai_auth":{"chatgpt_plan_type":"pro"}}"#,
        );
        let raw = json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "id_token": id_token,
            "token_type": "Bearer",
            "scope": "openid profile email",
            "expires_in": 3600
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let input =
            upsert_input_from_token_response(ProviderType::CodexOAuth, &response, raw, 1_000)
                .expect("account input");

        assert_eq!(input.id.as_deref(), Some("acc-123"));
        assert_eq!(input.provider_type, ProviderType::CodexOAuth);
        assert_eq!(input.email.as_deref(), Some("owner@example.com"));
        assert_eq!(input.refresh_token.as_deref(), Some("refresh-new"));
        assert_eq!(input.scopes, vec!["openid", "profile", "email"]);
        assert_eq!(input.subscription_level.as_deref(), Some("pro"));
        assert_eq!(input.expires_at, Some(3_601_000));
    }

    #[test]
    fn codex_account_import_rejects_missing_refresh_token_or_account_id() {
        let id_token = jwt(r#"{"chatgpt_account_id":"acc-123","email":"owner@example.com"}"#);
        let raw = json!({
            "access_token": "access-new",
            "id_token": id_token,
            "expires_in": 3600
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let error =
            upsert_input_from_token_response(ProviderType::CodexOAuth, &response, raw, 1_000)
                .expect_err("missing refresh token");
        assert!(error.message.contains("refresh_token"));

        let raw = json!({
            "access_token": "plain-access-token",
            "refresh_token": "refresh-new"
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let error =
            upsert_input_from_token_response(ProviderType::CodexOAuth, &response, raw, 1_000)
                .expect_err("missing account id");
        assert!(error.message.contains("account id"));
    }

    #[test]
    fn claude_token_response_import_uses_email_then_account_uuid() {
        let raw = json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "expires_in": 3600,
            "account": {"uuid": "claude-account-uuid", "email_address": "owner@example.com"},
            "organization": {"uuid": "org-uuid"}
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let input = upsert_input_from_login_response(
            ProviderType::ClaudeOAuth,
            &response,
            raw,
            None,
            1_000,
        )
        .expect("account input");

        assert_eq!(input.id.as_deref(), Some("owner@example.com"));
        assert_eq!(input.email.as_deref(), Some("owner@example.com"));
        assert_eq!(input.refresh_token.as_deref(), Some("refresh-new"));
        assert_eq!(input.expires_at, Some(3_601_000));
        assert_eq!(
            input
                .profile
                .as_ref()
                .and_then(|value| value["source"].as_str()),
            Some("login_exchange")
        );
    }

    #[test]
    fn gemini_login_import_uses_userinfo_email() {
        let raw = json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "expires_in": 3600
        });
        let profile = json!({
            "email": "gemini@example.com",
            "name": "Gemini Owner",
            "picture": "https://example.com/avatar.png"
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let input = upsert_input_from_login_response(
            ProviderType::GeminiCli,
            &response,
            raw,
            Some(profile.clone()),
            1_000,
        )
        .expect("account input");

        assert_eq!(input.id.as_deref(), Some("gemini@example.com"));
        assert_eq!(input.email.as_deref(), Some("gemini@example.com"));
        assert_eq!(
            input
                .profile
                .as_ref()
                .and_then(|value| value["profileRaw"]["name"].as_str()),
            Some("Gemini Owner")
        );
        assert_eq!(
            input
                .raw
                .as_ref()
                .and_then(|value| value["profile"]["email"].as_str()),
            Some("gemini@example.com")
        );
    }

    #[test]
    fn antigravity_login_import_marks_project_enrichment_deferred() {
        let raw = json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "expires_in": 3600
        });
        let profile = json!({"email": "agy@example.com", "name": "Agy Owner"});
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let input = upsert_input_from_login_response(
            ProviderType::AntigravityOAuth,
            &response,
            raw,
            Some(profile),
            1_000,
        )
        .expect("account input");

        assert_eq!(input.id.as_deref(), Some("agy@example.com"));
        assert_eq!(
            input
                .profile
                .as_ref()
                .and_then(|value| value["postExchangeEnrichment"].as_str()),
            Some("project_and_tier_deferred_to_quota_refresh")
        );
    }

    #[test]
    fn cursor_poll_response_import_uses_refresh_token_hash_id() {
        let id_token = jwt(r#"{"email":"cursor@example.com"}"#);
        let raw = json!({
            "accessToken": "access-new",
            "refreshToken": "refresh-new",
            "idToken": id_token,
            "email": "cursor@example.com"
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let input = upsert_input_from_login_response(
            ProviderType::CursorOAuth,
            &response,
            raw,
            None,
            1_000,
        )
        .expect("account input");

        assert_eq!(
            input.id.as_deref(),
            Some(cursor_account_id_from_refresh_token("refresh-new").as_str())
        );
        assert_eq!(input.email.as_deref(), Some("cursor@example.com"));
        assert_eq!(input.refresh_token.as_deref(), Some("refresh-new"));
        assert!(input
            .id_token
            .as_deref()
            .is_some_and(|value| { value.contains("eyJlbWFpbCI6ImN1cnNvckBleGFtcGxlLmNvbSJ9") }));
    }

    #[test]
    fn parses_camel_case_token_response_and_access_token_identity() {
        let access_token =
            jwt(r#"{"sub":"subject-123","email":"owner@example.com","plan":"team"}"#);
        let raw = json!({
            "accessToken": access_token,
            "refreshToken": "refresh-new",
            "tokenType": "Bearer",
            "expiresIn": 120
        });
        let response: OAuthTokenResponse = serde_json::from_value(raw.clone()).unwrap();
        let identity = identity_from_token_response(&response);

        assert_eq!(identity.account_id.as_deref(), Some("subject-123"));
        assert_eq!(identity.email.as_deref(), Some("owner@example.com"));
        assert_eq!(identity.plan_type.as_deref(), Some("team"));

        let update =
            refresh_update_from_token_response(ProviderType::CodexOAuth, &response, raw, 10);
        assert_eq!(update.token_type.as_deref(), Some("Bearer"));
        assert_eq!(update.expires_at, Some(120_010));
        assert_eq!(update.subscription_level.as_deref(), Some("team"));
    }

    #[test]
    fn classifies_refresh_errors_and_race_recovery() {
        let error = classify_oauth_error(
            Some(400),
            r#"{"error":"invalid_grant","error_description":"refresh token already used"}"#,
        );
        assert_eq!(error.kind, OAuthErrorKind::InvalidGrant);
        assert!(error.refresh_token_may_have_rotated);
        assert!(is_refresh_race_recoverable(&error));

        let pending = classify_oauth_error(Some(400), r#"{"error":"authorization_pending"}"#);
        assert_eq!(pending.kind, OAuthErrorKind::AuthorizationPending);
        assert!(pending.retryable);
    }

    #[test]
    fn classifies_provider_oauth_error_matrix() {
        let denied = classify_oauth_error(Some(403), r#"{"error":"access_denied"}"#);
        assert_eq!(denied.kind, OAuthErrorKind::AccessDenied);
        assert!(!denied.retryable);

        let expired = classify_oauth_error(Some(400), r#"{"error":"expired_token"}"#);
        assert_eq!(expired.kind, OAuthErrorKind::ExpiredToken);
        assert!(!expired.retryable);

        let limited = classify_oauth_error(Some(429), r#"{"message":"rate limit exceeded"}"#);
        assert_eq!(limited.kind, OAuthErrorKind::RateLimited);
        assert!(limited.retryable);

        let upstream = classify_oauth_error(Some(502), "bad gateway");
        assert_eq!(upstream.kind, OAuthErrorKind::ProviderRejected);
        assert!(upstream.retryable);

        let unauthorized = classify_oauth_error(Some(401), r#"{"message":"unauthorized"}"#);
        assert_eq!(unauthorized.kind, OAuthErrorKind::InvalidGrant);
        assert!(unauthorized.refresh_token_may_have_rotated);
    }

    #[test]
    fn profile_request_exists_only_for_endpoint_based_providers() {
        assert!(build_profile_request(ProviderType::CodexOAuth, "token").is_none());
        let request = build_profile_request(ProviderType::CursorOAuth, "token").unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://cursor.com/api/auth/me");
        assert_eq!(
            request.headers[0],
            ("Authorization".to_string(), "Bearer token".to_string())
        );
    }

    #[test]
    fn ac6_ac7_refresh_requests_are_server_native_refresh_ready() {
        let gemini = build_refresh_request(
            ProviderType::GeminiCli,
            &account_with_raw(
                ProviderType::GeminiCli,
                json!({"clientId":"gemini-client-fixture","clientSecret":"gemini-secret-fixture"}),
            ),
        )
        .expect("gemini request");
        assert_eq!(gemini.url, "https://oauth2.googleapis.com/token");
        assert_eq!(gemini.body_format, OAuthRequestBodyFormat::Form);
        assert_eq!(gemini.body["client_id"], "gemini-client-fixture");
        assert_eq!(gemini.body["client_secret"], "gemini-secret-fixture");

        let cursor = build_refresh_request(
            ProviderType::CursorOAuth,
            &account_with_raw(
                ProviderType::CursorOAuth,
                json!({"clientId":"cursor-client-fixture"}),
            ),
        )
        .expect("cursor request");
        assert_eq!(cursor.url, "https://api2.cursor.sh/oauth/token");
        assert_eq!(cursor.body_format, OAuthRequestBodyFormat::Json);
        assert_eq!(cursor.body["client_id"], "cursor-client-fixture");
        assert!(cursor.body.get("client_secret").is_none());

        let antigravity = build_refresh_request(
            ProviderType::AntigravityOAuth,
            &account_with_raw(
                ProviderType::AntigravityOAuth,
                json!({"clientId":"antigravity-client-fixture","clientSecret":"antigravity-secret-fixture"}),
            ),
        )
        .expect("antigravity request");
        assert_eq!(antigravity.url, "https://oauth2.googleapis.com/token");
        assert_eq!(antigravity.body_format, OAuthRequestBodyFormat::Form);
        assert_eq!(
            antigravity.body["client_secret"],
            "antigravity-secret-fixture"
        );
    }

    #[test]
    fn oauth_specs_enable_refresh_without_claiming_browser_login() {
        let codex = oauth_provider_spec(ProviderType::CodexOAuth).unwrap();
        assert_eq!(codex.stage, OAuthSupportStage::NativeRefreshProfile);

        for provider_type in [
            ProviderType::ClaudeOAuth,
            ProviderType::GeminiCli,
            ProviderType::CursorOAuth,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
        ] {
            let spec = oauth_provider_spec(provider_type).unwrap();
            assert_eq!(spec.stage, OAuthSupportStage::NativeRefreshProfile);
            assert!(!spec.token_urls.is_empty());
        }

        for provider_type in [
            ProviderType::GitHubCopilot,
            ProviderType::DeepSeekAccount,
            ProviderType::KiroOAuth,
            ProviderType::CursorApiKey,
            ProviderType::OllamaCloud,
            ProviderType::AwsBedrock,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
        ] {
            let spec = oauth_provider_spec(provider_type).unwrap();
            assert_eq!(spec.stage, OAuthSupportStage::ManualImportOnly);
            assert!(spec.token_urls.is_empty());
        }
    }

    #[test]
    fn ollama_cloud_quota_does_not_emit_fake_zero_percent() {
        assert!(quota_from_provider_snapshot(ProviderType::OllamaCloud, &json!({})).is_none());
        assert!(quota_from_provider_snapshot(
            ProviderType::OllamaCloud,
            &json!({"quotaPercent": 0})
        )
        .is_none());

        let quota =
            quota_from_provider_snapshot(ProviderType::CursorOAuth, &json!({"quotaPercent": 42.0}))
                .expect("cursor quota");
        assert_eq!(quota.tiers[0].utilization, Some(0.42));
    }

    #[test]
    fn provider_quota_snapshot_parses_nested_percent_and_reset() {
        let quota = quota_from_provider_snapshot(
            ProviderType::CodexOAuth,
            &json!({"usage":{"quotaPercent": 125.0},"quota":{"resetAt": 1_234_567}}),
        )
        .expect("quota");

        assert_eq!(quota.tiers[0].utilization, Some(1.0));
        assert_eq!(quota.tiers[0].resets_at, Some(1_234_567));
        assert!(quota_from_provider_snapshot(ProviderType::CodexOAuth, &json!({})).is_none());
    }
}
