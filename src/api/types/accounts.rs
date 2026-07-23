use crate::domain::accounts::login::{OAuthLoginCancellation, OAuthLoginFinish, OAuthLoginStart};
use crate::domain::accounts::oauth::{
    OAuthErrorKind, OAuthHttpRequest, OAuthQuotaStrategy, OAuthSupportStage,
};
use crate::domain::accounts::store::{Account, AccountQuota};
use crate::domain::accounts::subscription_expiry::SubscriptionExpiryRule;
use crate::domain::providers::model::ProviderType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListAccountsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) accounts: Vec<AccountPublicView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertAccountResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: AccountPublicView,
}

/// Control-plane representation of an account. Stored credentials and opaque
/// upstream payloads deliberately remain on the internal `Account` model.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountPublicView {
    pub(in crate::api) id: String,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) auth_identity_generation: u64,
    pub(in crate::api) token_refresh_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) token_type: Option<String>,
    pub(in crate::api) scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) entitlement_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) quota: Option<AccountQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) quota_refreshed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) quota_next_refresh_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) manual_subscription_expires_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) manual_subscription_expiry_updated_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) manual_subscription_expiry_rule: Option<SubscriptionExpiryRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) rate_limited_until: Option<i64>,
    pub(in crate::api) has_access_token: bool,
    pub(in crate::api) has_refresh_token: bool,
    pub(in crate::api) has_id_token: bool,
    pub(in crate::api) has_api_key: bool,
    pub(in crate::api) has_extra_headers: bool,
    pub(in crate::api) has_profile: bool,
    pub(in crate::api) has_raw: bool,
    pub(in crate::api) has_refresh_error: bool,
    pub(in crate::api) refresh_consecutive_failures: u32,
    pub(in crate::api) needs_relogin: bool,
}

impl From<&Account> for AccountPublicView {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            provider_type: account.provider_type,
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
            email: account.email.clone(),
            token_type: account.token_type.clone(),
            scopes: account.scopes.clone(),
            subscription_level: account.subscription_level.clone(),
            entitlement_status: account.entitlement_status.clone(),
            quota_percent: account.quota_percent,
            quota: account_quota_public_view(account, account.quota.as_ref()),
            quota_refreshed_at: account.quota_refreshed_at,
            quota_next_refresh_at: account.quota_next_refresh_at,
            expires_at: account.expires_at,
            manual_subscription_expires_at_ms: account.manual_subscription_expires_at_ms,
            manual_subscription_expiry_updated_at_ms: account
                .manual_subscription_expiry_updated_at_ms,
            manual_subscription_expiry_rule: account.manual_subscription_expiry_rule.clone(),
            rate_limited_until: account.rate_limited_until,
            has_access_token: has_non_empty_secret(account.access_token.as_deref()),
            has_refresh_token: has_non_empty_secret(account.refresh_token.as_deref()),
            has_id_token: has_non_empty_secret(account.id_token.as_deref()),
            has_api_key: has_non_empty_secret(account.api_key.as_deref()),
            has_extra_headers: !account.extra_headers.is_empty(),
            has_profile: account.profile.is_some(),
            has_raw: account.raw.is_some(),
            has_refresh_error: account
                .last_refresh_error
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            refresh_consecutive_failures: account.refresh_consecutive_failures,
            needs_relogin: account.needs_relogin,
        }
    }
}

impl From<Account> for AccountPublicView {
    fn from(account: Account) -> Self {
        Self::from(&account)
    }
}

fn has_non_empty_secret(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

pub(in crate::api) fn account_quota_public_view(
    account: &Account,
    quota: Option<&AccountQuota>,
) -> Option<AccountQuota> {
    let mut quota = quota.cloned()?;
    let secrets = account_secret_values(account);
    if let Some(message) = quota.credential_message.as_mut() {
        *message = crate::logging::redact_sensitive_text(message);
        redact_account_secret_text_in_place(message, &secrets);
    }
    for tier in &mut quota.tiers {
        redact_account_secret_text_in_place(&mut tier.name, &secrets);
        if let Some(label) = tier.label.as_mut() {
            redact_account_secret_text_in_place(label, &secrets);
        }
        if let Some(unit) = tier.unit.as_mut() {
            redact_account_secret_text_in_place(unit, &secrets);
        }
    }
    if let Some(extra_usage) = quota.extra_usage.as_mut() {
        redact_account_public_value(extra_usage, &secrets);
    }
    Some(quota)
}

pub(in crate::api) fn redact_account_public_text(account: &Account, value: &str) -> String {
    let mut value = value.to_string();
    redact_account_secret_text_in_place(&mut value, &account_secret_values(account));
    value
}

pub(in crate::api) fn redact_account_public_diagnostic(account: &Account, value: &str) -> String {
    let value = crate::logging::redact_sensitive_text(value);
    redact_account_public_text(account, &value)
        .chars()
        .take(800)
        .collect()
}

pub(in crate::api) fn oauth_error_public_message(kind: OAuthErrorKind) -> &'static str {
    match kind {
        OAuthErrorKind::AuthorizationPending => "OAuth authorization is still pending",
        OAuthErrorKind::AccessDenied => "OAuth authorization was denied",
        OAuthErrorKind::InvalidGrant | OAuthErrorKind::ExpiredToken => {
            "OAuth credentials were rejected; sign in again"
        }
        OAuthErrorKind::MissingCredential => "OAuth credentials are incomplete",
        OAuthErrorKind::RateLimited => "OAuth provider rate limited the request; retry later",
        OAuthErrorKind::ProviderRejected => "OAuth provider rejected the request",
        OAuthErrorKind::Network => "OAuth provider request failed",
        OAuthErrorKind::Parse => "OAuth provider returned an invalid response",
        OAuthErrorKind::Unsupported => "OAuth operation is not supported for this account",
        OAuthErrorKind::Unknown => "OAuth request failed",
    }
}

fn account_secret_values(account: &Account) -> Vec<&str> {
    account
        .access_token
        .iter()
        .chain(account.refresh_token.iter())
        .chain(account.id_token.iter())
        .chain(account.api_key.iter())
        .map(String::as_str)
        .chain(account.extra_headers.values().map(String::as_str))
        .filter(|value| !value.is_empty())
        .collect()
}

fn redact_account_public_value(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if account_public_key_is_sensitive(key) {
                    *item = Value::String("[REDACTED]".to_string());
                } else {
                    redact_account_public_value(item, secrets);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_account_public_value(item, secrets);
            }
        }
        Value::String(text) => {
            redact_account_secret_text_in_place(text, secrets);
            if let Ok(mut nested) = serde_json::from_str::<Value>(text) {
                if nested.is_object() || nested.is_array() {
                    redact_account_public_value(&mut nested, secrets);
                    if let Ok(redacted) = serde_json::to_string(&nested) {
                        *text = redacted;
                    }
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_account_secret_text_in_place(value: &mut String, secrets: &[&str]) {
    for secret in secrets {
        if value == secret {
            *value = "[REDACTED]".to_string();
            return;
        }
        if secret.len() >= 8 && value.contains(secret) {
            *value = value.replace(secret, "[REDACTED]");
        }
    }
}

fn account_public_key_is_sensitive(key: &str) -> bool {
    let compact = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    matches!(
        compact.as_str(),
        "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "bearertoken"
            | "sessiontoken"
            | "apikey"
            | "authorization"
            | "proxyauthorization"
            | "cookie"
            | "setcookie"
            | "clientsecret"
            | "clientassertion"
            | "password"
            | "secret"
            | "privatekey"
            | "signingkey"
            | "codeverifier"
            | "authorizationcode"
            | "credential"
            | "credentials"
    ) || [
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "apikey",
        "clientsecret",
        "privatekey",
        "signingkey",
        "sessioncookie",
    ]
    .iter()
    .any(|suffix| compact.ends_with(suffix))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountDeletePreview {
    pub(in crate::api) account_id: String,
    pub(in crate::api) provider_keys: Vec<crate::domain::providers::registry::ProviderKey>,
    pub(in crate::api) blocked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountDeletePreviewResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) preview: AccountDeletePreview,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountCapabilitiesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) capabilities:
        Vec<crate::domain::accounts::managers::AccountManagerCapability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountImportTemplatesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) templates: Vec<crate::domain::accounts::managers::AccountImportTemplate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportClaudeCredentialsRequest {
    pub(in crate::api) credentials: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportClaudeCredentialsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: AccountLoginAccountSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportGrokAuthJsonRequest {
    pub(in crate::api) auth_json: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportGrokAuthJsonResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: AccountLoginAccountSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportKiroCredentialsRequest {
    pub(in crate::api) credentials: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportKiroLocalCredentialsRequest {
    #[serde(default)]
    pub(in crate::api) path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportKiroApiKeyRequest {
    pub(in crate::api) api_key: String,
    #[serde(default)]
    pub(in crate::api) region: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportKiroCredentialsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: AccountLoginAccountSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) source: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportCursorLocalAuthResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: AccountLoginAccountSummary,
    pub(in crate::api) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) profile_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartAccountLoginRequest {
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    #[serde(default)]
    pub(in crate::api) redirect_uri: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartAccountLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) login: OAuthLoginStart,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCopilotDeviceLoginRequest {
    #[serde(default)]
    pub(in crate::api) github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCopilotDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::copilot_device::GitHubDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCopilotDeviceLoginRequest {
    pub(in crate::api) device_code: String,
    #[serde(default)]
    pub(in crate::api) github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCopilotDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartKiroDeviceLoginRequest {
    #[serde(default)]
    pub(in crate::api) region: Option<String>,
    #[serde(default)]
    pub(in crate::api) start_url: Option<String>,
    #[serde(default)]
    pub(in crate::api) issuer_url: Option<String>,
    #[serde(default)]
    pub(in crate::api) login_provider: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartKiroDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::kiro_device::KiroDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollKiroDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCodexDeviceLoginRequest {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCodexDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::codex_device::CodexDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCodexDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelCodexDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelCodexDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) cancelled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCodexDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartGrokDeviceLoginRequest {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartGrokDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::grok_device::GrokDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollGrokDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelGrokDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelGrokDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) cancelled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollGrokDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollKiroDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountLoginCallbackQuery {
    #[serde(default)]
    pub(in crate::api) state: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
    #[serde(default)]
    pub(in crate::api) error: Option<String>,
    #[serde(default, alias = "error_description")]
    pub(in crate::api) error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FinishAccountLoginRequest {
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) state: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
    #[serde(default)]
    pub(in crate::api) execute_token_exchange: Option<bool>,
    #[serde(skip)]
    pub(in crate::api) expected_provider_type: Option<ProviderType>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FinishAccountLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) login: OAuthLoginFinish,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelAccountLoginRequest {
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) state: Option<String>,
    #[serde(skip)]
    pub(in crate::api) expected_provider_type: Option<ProviderType>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CancelAccountLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) login: OAuthLoginCancellation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountLoginAccountSummary {
    pub(in crate::api) id: String,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) auth_identity_generation: u64,
    pub(in crate::api) token_refresh_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) entitlement_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) expires_at: Option<i64>,
    pub(in crate::api) has_access_token: bool,
    pub(in crate::api) has_refresh_token: bool,
    pub(in crate::api) scopes: Vec<String>,
}

impl AccountLoginAccountSummary {
    pub(in crate::api) fn from_account(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            provider_type: account.provider_type,
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
            email: account.email.clone(),
            subscription_level: account.subscription_level.clone(),
            entitlement_status: account.entitlement_status.clone(),
            expires_at: account.expires_at,
            has_access_token: account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            has_refresh_token: account
                .refresh_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            scopes: account.scopes.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountQuotaResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) quota: Option<AccountQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountPublicView>,
    pub(in crate::api) refreshed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) next_refresh_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountQuotaQuery {
    #[serde(default)]
    pub(in crate::api) refresh: Option<bool>,
    #[serde(default)]
    pub(in crate::api) force: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountRefreshPlanResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account_id: String,
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    pub(in crate::api) refresh_required: bool,
    pub(in crate::api) server_native_stage: Option<OAuthSupportStage>,
    pub(in crate::api) quota_strategy: Option<OAuthQuotaStrategy>,
    pub(in crate::api) refresh_request: Option<OAuthHttpRequest>,
    pub(in crate::api) profile_request: Option<OAuthHttpRequest>,
    pub(in crate::api) message: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn account_public_view_redacts_quota_credentials_and_reflected_secrets() {
        let account: Account = serde_json::from_value(json!({
            "id": "acct-public",
            "providerType": "codex_oauth",
            "accessToken": "access-secret-value",
            "refreshToken": "refresh-secret-value",
            "idToken": "id-secret-value",
            "apiKey": "api-secret-value",
            "extraHeaders": {"x-session": "header-secret-value"},
            "quota": {
                "success": true,
                "credentialMessage": "plan access-secret-value\napi_key=unknown-credential-secret",
                "tiers": [{"name": "seven_day", "label": "Plus", "unit": "tokens"}],
                "extraUsage": {
                    "raw": {
                        "access_token": "access-secret-value",
                        "nested": {
                            "Authorization": "Bearer access-secret-value",
                            "clientSecret": "unknown-client-secret",
                            "safe": "visible"
                        },
                        "encoded": "{\"refresh_token\":\"unknown-refresh-secret\",\"usage\":42}",
                        "reflected": "prefix header-secret-value suffix",
                        "tokenLimit": 8192
                    }
                }
            }
        }))
        .unwrap();

        let serialized = serde_json::to_string(&AccountPublicView::from(&account)).unwrap();

        for secret in [
            "access-secret-value",
            "refresh-secret-value",
            "id-secret-value",
            "api-secret-value",
            "header-secret-value",
            "unknown-client-secret",
            "unknown-refresh-secret",
            "unknown-credential-secret",
        ] {
            assert!(!serialized.contains(secret), "leaked secret: {secret}");
        }
        assert!(serialized.contains("visible"));
        assert!(serialized.contains("tokenLimit"));
        assert!(serialized.contains("[REDACTED]"));
    }

    #[test]
    fn account_public_text_redacts_exact_short_and_embedded_long_secrets() {
        let account: Account = serde_json::from_value(json!({
            "id": "acct-public-text",
            "providerType": "codex_oauth",
            "apiKey": "abc",
            "accessToken": "long-access-secret"
        }))
        .unwrap();

        assert_eq!(redact_account_public_text(&account, "abc"), "[REDACTED]");
        assert_eq!(
            redact_account_public_text(&account, "Bearer long-access-secret"),
            "Bearer [REDACTED]"
        );
        assert_eq!(redact_account_public_text(&account, "alphabet"), "alphabet");
    }

    #[test]
    fn account_public_diagnostic_redacts_unknown_fields_and_stored_secrets() {
        let account: Account = serde_json::from_value(json!({
            "id": "acct-public-diagnostic",
            "providerType": "codex_oauth",
            "accessToken": "stored-access-secret"
        }))
        .unwrap();

        let unknown = redact_account_public_diagnostic(&account, "api_key=unknown-secret");
        assert!(!unknown.contains("unknown-secret"));
        assert!(unknown.contains("[REDACTED]"));

        let reflected = redact_account_public_diagnostic(
            &account,
            "upstream rejected stored-access-secret during refresh",
        );
        assert!(!reflected.contains("stored-access-secret"));
        assert!(reflected.contains("[REDACTED]"));
    }
}
