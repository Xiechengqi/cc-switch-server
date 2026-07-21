use crate::domain::accounts::login::{OAuthLoginCancellation, OAuthLoginFinish, OAuthLoginStart};
use crate::domain::accounts::oauth::{OAuthHttpRequest, OAuthQuotaStrategy, OAuthSupportStage};
use crate::domain::accounts::store::{Account, AccountQuota};
use crate::domain::providers::model::ProviderType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListAccountsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) accounts: Vec<Account>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertAccountResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: Account,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ExportClaudeCredentialsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) credentials: serde_json::Value,
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
    pub(in crate::api) session_id: Option<String>,
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
    pub(in crate::api) account: Option<Account>,
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
