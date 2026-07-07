#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::Mutex;

use serde::Serialize;

use crate::domain::accounts::oauth::{
    build_refresh_request, oauth_provider_spec, token_expires_soon, OAuthHttpRequest,
    OAuthProfileStrategy, OAuthQuotaStrategy, OAuthSupportStage,
};
use crate::domain::accounts::store::{Account, AccountQuota, AccountStore, UpsertAccountInput};
use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum AccountManagerSupport {
    NativeOAuth,
    ManualTokenStore,
    Planned,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountManagerCapability {
    pub provider_type: ProviderType,
    pub manager: &'static str,
    pub support: AccountManagerSupport,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_reason: Option<&'static str>,
    pub supports_start_login: bool,
    pub supports_callback: bool,
    pub supports_refresh: bool,
    pub supports_quota: bool,
    pub supports_refresh_plan: bool,
    pub supports_import: bool,
    pub supports_delete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_native_stage: Option<OAuthSupportStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_strategy: Option<OAuthProfileStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_strategy: Option<OAuthQuotaStrategy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountManagerKind {
    ManualTokenStore,
    NativeOAuth,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountManagerRegistration {
    pub provider_type: ProviderType,
    pub kind: AccountManagerKind,
    pub manager: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportTemplate {
    pub provider_type: ProviderType,
    pub credential_kind: &'static str,
    pub required_fields: Vec<&'static str>,
    pub optional_fields: Vec<&'static str>,
    pub profile_hints: Vec<&'static str>,
    pub raw_hints: Vec<&'static str>,
    pub notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub provider_type: ProviderType,
    pub method: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredential {
    pub account_id: String,
    pub provider_type: ProviderType,
    pub credential_kind: CredentialKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    AccessToken,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountManagerError {
    Unsupported(&'static str),
    NotFound(String),
    CredentialUnavailable(String),
}

impl std::fmt::Display for AccountManagerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message) => formatter.write_str(message),
            Self::NotFound(account_id) => write!(formatter, "account not found: {account_id}"),
            Self::CredentialUnavailable(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for AccountManagerError {}

pub trait AccountManager {
    fn capability(&self, provider_type: ProviderType) -> AccountManagerCapability;
    fn start_login(&self, provider_type: ProviderType) -> Result<LoginStart, AccountManagerError>;
    fn finish_login(
        &self,
        store: &mut AccountStore,
        input: UpsertAccountInput,
    ) -> Result<Account, AccountManagerError>;
    fn get_valid_token(
        &self,
        store: &AccountStore,
        provider_type: ProviderType,
        account_id: Option<&str>,
        now_ms: i64,
    ) -> Result<AccountCredential, AccountManagerError>;
    fn refresh_token(
        &self,
        store: &mut AccountStore,
        account_id: &str,
        now_ms: i64,
    ) -> Result<Account, AccountManagerError>;
    fn query_quota(
        &self,
        store: &AccountStore,
        account_id: &str,
    ) -> Result<Option<AccountQuota>, AccountManagerError>;
    fn revoke_or_delete(
        &self,
        store: &mut AccountStore,
        account_id: &str,
    ) -> Result<bool, AccountManagerError>;
}

pub struct ManualTokenAccountManager;

#[derive(Debug, Default)]
pub struct CodexOAuthAccountManager {
    refresh_locks: AccountRefreshLocks,
}

#[derive(Debug, Default)]
pub struct AccountRefreshLocks {
    active: Mutex<HashSet<String>>,
}

#[derive(Debug)]
pub struct AccountRefreshGuard<'a> {
    locks: &'a AccountRefreshLocks,
    key: String,
    active: bool,
}

impl AccountRefreshLocks {
    pub fn try_lock(
        &self,
        provider_type: ProviderType,
        account_id: &str,
    ) -> Option<AccountRefreshGuard<'_>> {
        let key = refresh_lock_key(provider_type, account_id);
        let mut active = self.active.lock().ok()?;
        if !active.insert(key.clone()) {
            return None;
        }
        Some(AccountRefreshGuard {
            locks: self,
            key,
            active: true,
        })
    }

    pub fn is_locked(&self, provider_type: ProviderType, account_id: &str) -> bool {
        self.active
            .lock()
            .map(|active| active.contains(&refresh_lock_key(provider_type, account_id)))
            .unwrap_or(false)
    }

    fn release(&self, key: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(key);
        }
    }
}

impl Drop for AccountRefreshGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.locks.release(&self.key);
            self.active = false;
        }
    }
}

impl AccountRefreshGuard<'_> {
    pub fn release(mut self) {
        if self.active {
            self.locks.release(&self.key);
            self.active = false;
        }
    }
}

impl CodexOAuthAccountManager {
    pub fn capability(&self) -> AccountManagerCapability {
        manual_capability(ProviderType::CodexOAuth)
    }

    pub fn plan_refresh_request(
        &self,
        store: &AccountStore,
        account_id: &str,
        now_ms: i64,
    ) -> Result<(AccountRefreshGuard<'_>, OAuthHttpRequest), AccountManagerError> {
        let account = store
            .accounts
            .iter()
            .find(|item| item.id == account_id && item.provider_type == ProviderType::CodexOAuth)
            .ok_or_else(|| AccountManagerError::NotFound(account_id.to_string()))?;

        if !token_expires_soon(account, now_ms) {
            return Err(AccountManagerError::CredentialUnavailable(
                "codex oauth access token is still valid; refresh not required".to_string(),
            ));
        }

        let guard = self
            .refresh_locks
            .try_lock(ProviderType::CodexOAuth, account_id)
            .ok_or_else(|| {
                AccountManagerError::CredentialUnavailable(
                    "codex oauth refresh is already in progress".to_string(),
                )
            })?;
        let request = build_refresh_request(ProviderType::CodexOAuth, account)
            .map_err(|error| AccountManagerError::CredentialUnavailable(error.message))?;
        Ok((guard, request))
    }
}

impl AccountManager for ManualTokenAccountManager {
    fn capability(&self, provider_type: ProviderType) -> AccountManagerCapability {
        manual_capability(provider_type)
    }

    fn start_login(&self, provider_type: ProviderType) -> Result<LoginStart, AccountManagerError> {
        Err(AccountManagerError::Unsupported(match provider_type {
            ProviderType::ClaudeOAuth => {
                "claude oauth browser login is disabled until real account validation; use login exchange/import preview"
            }
            ProviderType::CodexOAuth => {
                "codex oauth browser login is disabled until real account validation; use login exchange/import preview"
            }
            ProviderType::GeminiCli => {
                "gemini oauth browser login is disabled until real account validation; use login exchange/import preview"
            }
            ProviderType::GitHubCopilot => {
                "github copilot device import is available via /api/accounts/copilot/device/start|poll; native login remains disabled until real proxy validation"
            }
            ProviderType::DeepSeekAccount => {
                "deepseek account password login is disabled; import an access token/session snapshot"
            }
            ProviderType::KiroOAuth => {
                "kiro device import is available via /api/accounts/kiro/device/start|poll; native login remains disabled until real proxy validation"
            }
            ProviderType::CursorOAuth => {
                "cursor oauth browser login is disabled until real Cursor AgentService validation; use login exchange/import preview"
            }
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
                "antigravity oauth browser login is disabled until real account validation; use login exchange/import preview"
            }
            ProviderType::CursorApiKey | ProviderType::OllamaCloud => {
                "api key providers use direct account upsert"
            }
            _ => "account login flow is not implemented for this provider type",
        }))
    }

    fn finish_login(
        &self,
        store: &mut AccountStore,
        input: UpsertAccountInput,
    ) -> Result<Account, AccountManagerError> {
        Ok(store.upsert(input))
    }

    fn get_valid_token(
        &self,
        store: &AccountStore,
        provider_type: ProviderType,
        account_id: Option<&str>,
        now_ms: i64,
    ) -> Result<AccountCredential, AccountManagerError> {
        let account = store
            .find_for_provider(provider_type, account_id)
            .ok_or_else(|| {
                AccountManagerError::NotFound(account_id.unwrap_or("<default>").into())
            })?;

        if let Some(api_key) = account.api_key.as_ref().filter(|value| !value.is_empty()) {
            return Ok(AccountCredential {
                account_id: account.id.clone(),
                provider_type: account.provider_type,
                credential_kind: CredentialKind::ApiKey,
                value: api_key.clone(),
            });
        }

        if account
            .expires_at
            .is_some_and(|expires_at| expires_at <= now_ms)
        {
            let message = if oauth_provider_spec(provider_type)
                .is_some_and(|spec| spec.server_native_refresh_enabled())
            {
                "access token expired; refreshToken is required for server-native refresh"
            } else {
                "access token expired; provider refresh flow is not enabled"
            };
            return Err(AccountManagerError::CredentialUnavailable(
                message.to_string(),
            ));
        }

        if let Some(token) = account
            .access_token
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            return Ok(AccountCredential {
                account_id: account.id.clone(),
                provider_type: account.provider_type,
                credential_kind: CredentialKind::AccessToken,
                value: token.clone(),
            });
        }

        Err(AccountManagerError::CredentialUnavailable(
            "account has no access token or api key".to_string(),
        ))
    }

    fn refresh_token(
        &self,
        store: &mut AccountStore,
        account_id: &str,
        now_ms: i64,
    ) -> Result<Account, AccountManagerError> {
        store
            .refresh_status(account_id, now_ms)
            .ok_or_else(|| AccountManagerError::NotFound(account_id.to_string()))
    }

    fn query_quota(
        &self,
        store: &AccountStore,
        account_id: &str,
    ) -> Result<Option<AccountQuota>, AccountManagerError> {
        let account = store
            .accounts
            .iter()
            .find(|item| item.id == account_id)
            .ok_or_else(|| AccountManagerError::NotFound(account_id.to_string()))?;
        Ok(account.quota.clone())
    }

    fn revoke_or_delete(
        &self,
        store: &mut AccountStore,
        account_id: &str,
    ) -> Result<bool, AccountManagerError> {
        Ok(store.delete(account_id))
    }
}

pub fn capability_for(provider_type: ProviderType) -> AccountManagerCapability {
    ManualTokenAccountManager.capability(provider_type)
}

pub fn manager_for(_provider_type: ProviderType) -> ManualTokenAccountManager {
    ManualTokenAccountManager
}

pub fn manager_registration_for(provider_type: ProviderType) -> AccountManagerRegistration {
    AccountManagerRegistration {
        provider_type,
        kind: AccountManagerKind::ManualTokenStore,
        manager: "manual_token_store",
    }
}

pub fn registered_account_managers() -> Vec<AccountManagerRegistration> {
    account_provider_types()
        .into_iter()
        .map(manager_registration_for)
        .collect()
}

pub fn all_capabilities() -> Vec<AccountManagerCapability> {
    account_provider_types()
        .into_iter()
        .map(capability_for)
        .collect()
}

pub fn account_import_templates() -> Vec<AccountImportTemplate> {
    account_provider_types()
        .into_iter()
        .map(account_import_template_for)
        .collect()
}

fn account_provider_types() -> [ProviderType; 14] {
    [
        ProviderType::ClaudeOAuth,
        ProviderType::CodexOAuth,
        ProviderType::GeminiCli,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::CursorOAuth,
        ProviderType::CursorApiKey,
        ProviderType::AntigravityOAuth,
        ProviderType::AgyOAuth,
        ProviderType::OllamaCloud,
        ProviderType::AwsBedrock,
        ProviderType::Nvidia,
        ProviderType::DeepSeekApi,
    ]
}

fn manual_capability(provider_type: ProviderType) -> AccountManagerCapability {
    let oauth_spec = crate::domain::accounts::oauth::oauth_provider_spec(provider_type);
    let supports_refresh_plan = oauth_spec.is_some_and(|spec| !spec.token_urls.is_empty());
    let supports_native_refresh = oauth_spec
        .is_some_and(|spec| !spec.token_urls.is_empty() && spec.server_native_refresh_enabled());
    let supports_quota = oauth_spec
        .is_some_and(|spec| !matches!(spec.quota_strategy, OAuthQuotaStrategy::NotAvailable));
    let native_oauth_planned = matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GeminiCli
            | ProviderType::GitHubCopilot
            | ProviderType::DeepSeekAccount
            | ProviderType::KiroOAuth
            | ProviderType::CursorOAuth
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
    );
    AccountManagerCapability {
        provider_type,
        manager: if supports_native_refresh {
            "manual_token_store_with_native_refresh"
        } else {
            "manual_token_store"
        },
        support: AccountManagerSupport::ManualTokenStore,
        status: if supports_native_refresh {
            "manual_import_native_refresh"
        } else if native_oauth_planned {
            "manual_import_only"
        } else {
            "manual_api_key_available"
        },
        blocking_reason: if supports_native_refresh {
            Some("native browser login/callback is disabled; import refresh credentials first")
        } else {
            native_oauth_planned.then_some(
                "native oauth/login/refresh requires real credentials and has not been enabled",
            )
        },
        supports_start_login: false,
        supports_callback: false,
        supports_refresh: supports_native_refresh,
        supports_quota,
        supports_refresh_plan,
        supports_import: true,
        supports_delete: true,
        server_native_stage: oauth_spec.map(|spec| spec.stage),
        profile_strategy: oauth_spec.map(|spec| spec.profile_strategy),
        quota_strategy: oauth_spec.map(|spec| spec.quota_strategy),
    }
}

fn account_import_template_for(provider_type: ProviderType) -> AccountImportTemplate {
    let optional_oauth_fields = vec![
        "id",
        "email",
        "refreshToken",
        "idToken",
        "tokenType",
        "scopes",
        "profile",
        "raw",
        "subscriptionLevel",
        "quotaPercent",
        "quota",
        "expiresAt",
    ];
    let optional_api_key_fields = vec![
        "id",
        "email",
        "profile",
        "raw",
        "subscriptionLevel",
        "quotaPercent",
        "quota",
    ];

    match provider_type {
        ProviderType::CursorApiKey
        | ProviderType::OllamaCloud
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => {
            AccountImportTemplate {
                provider_type,
                credential_kind: "api_key",
                required_fields: vec!["providerType", "apiKey"],
                optional_fields: optional_api_key_fields,
                profile_hints: vec!["email", "name", "plan"],
                raw_hints: vec!["provider account response", "billing or quota snapshot"],
                notes: "manual API key import; native refresh/login is disabled",
            }
        }
        ProviderType::AwsBedrock => AccountImportTemplate {
            provider_type,
            credential_kind: "aws_credentials",
            required_fields: vec![
                "providerType",
                "raw.awsAccessKeyId",
                "raw.awsSecretAccessKey",
                "raw.awsRegion",
            ],
            optional_fields: vec!["id", "email", "raw.awsSessionToken", "profile", "quota"],
            profile_hints: vec!["aws account alias", "iam user or role"],
            raw_hints: vec!["awsAccessKeyId", "awsSecretAccessKey", "awsRegion", "awsSessionToken"],
            notes: "Bedrock signing is planned; provider env credentials are still the active configuration path",
        },
        ProviderType::DeepSeekAccount => AccountImportTemplate {
            provider_type,
            credential_kind: "access_token",
            required_fields: vec!["providerType", "accessToken"],
            optional_fields: optional_oauth_fields.clone(),
            profile_hints: vec!["email", "name", "plan", "subscription"],
            raw_hints: vec![
                "DeepSeek account token/session export",
                "provider profile response",
                "billing or quota snapshot",
            ],
            notes: "import-only; cc-switch-server does not store DeepSeek account passwords",
        },
        ProviderType::GitHubCopilot => AccountImportTemplate {
            provider_type,
            credential_kind: "access_token",
            required_fields: vec!["providerType", "accessToken"],
            optional_fields: optional_oauth_fields.clone(),
            profile_hints: vec!["login", "email", "githubDomain", "ghes"],
            raw_hints: vec![
                "githubToken",
                "copilotToken",
                "copilotUsage",
                "copilotApiBase",
            ],
            notes: "device flow import is available via /api/accounts/copilot/device/start|poll; native forwarding remains disabled until real Copilot proxy validation",
        },
        ProviderType::KiroOAuth => AccountImportTemplate {
            provider_type,
            credential_kind: "access_token",
            required_fields: vec!["providerType", "accessToken"],
            optional_fields: optional_oauth_fields.clone(),
            profile_hints: vec!["email", "profileArn", "authRegion", "machineId"],
            raw_hints: vec![
                "clientId",
                "clientSecret",
                "profileArn",
                "kiroUsageLimits",
            ],
            notes: "AWS Builder ID device flow import is available via /api/accounts/kiro/device/start|poll; native forwarding remains disabled until real Kiro proxy validation",
        },
        _ => {
            let notes = if oauth_provider_spec(provider_type)
                .is_some_and(|spec| spec.server_native_refresh_enabled())
            {
                "manual token import; native refresh/profile is available when refreshToken is present"
            } else {
                "manual token import; native OAuth login/refresh remains disabled"
            };
            AccountImportTemplate {
                provider_type,
                credential_kind: "access_token",
                required_fields: vec!["providerType", "accessToken"],
                optional_fields: optional_oauth_fields,
                profile_hints: vec!["email", "name", "plan", "subscription"],
                raw_hints: vec!["provider token response", "provider profile response", "clientId"],
                notes,
            }
        }
    }
}

fn refresh_lock_key(provider_type: ProviderType, account_id: &str) -> String {
    format!("{}:{account_id}", provider_type.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_capabilities_claim_refresh_without_browser_login() {
        let capability = capability_for(ProviderType::CodexOAuth);
        assert_eq!(capability.support, AccountManagerSupport::ManualTokenStore);
        assert_eq!(capability.status, "manual_import_native_refresh");
        assert!(capability.blocking_reason.is_some());
        assert!(!capability.supports_start_login);
        assert!(capability.supports_refresh);
        assert!(capability.supports_import);
        assert!(capability.supports_delete);
        assert_eq!(
            capability.server_native_stage,
            Some(OAuthSupportStage::NativeRefreshProfile)
        );
    }

    #[test]
    fn long_tail_account_capabilities_remain_explicit_manual_token_store() {
        let provider_types = [
            ProviderType::CursorApiKey,
            ProviderType::CursorOAuth,
            ProviderType::GitHubCopilot,
            ProviderType::KiroOAuth,
            ProviderType::DeepSeekAccount,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
            ProviderType::OllamaCloud,
            ProviderType::AwsBedrock,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
        ];

        for provider_type in provider_types {
            let capability = capability_for(provider_type);
            assert_eq!(capability.provider_type, provider_type);
            assert_eq!(capability.support, AccountManagerSupport::ManualTokenStore);
            assert!(!capability.status.is_empty());
            assert!(!capability.supports_start_login);
            assert!(!capability.supports_callback);
            if matches!(
                provider_type,
                ProviderType::CursorOAuth | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
            ) {
                assert!(capability.supports_refresh);
            } else {
                assert!(!capability.supports_refresh);
            }
            assert!(capability.supports_import);
            assert!(capability.supports_delete);
            assert!(capability.server_native_stage.is_some());
        }
    }

    #[test]
    fn account_capability_exposes_profile_and_quota_strategy_without_enabling_oauth() {
        let codex = capability_for(ProviderType::CodexOAuth);
        assert_eq!(
            codex.profile_strategy,
            Some(OAuthProfileStrategy::JwtClaims)
        );
        assert_eq!(
            codex.quota_strategy,
            Some(OAuthQuotaStrategy::ProviderSnapshot)
        );
        assert!(!codex.supports_start_login);
        assert!(codex.supports_refresh);

        let ollama = capability_for(ProviderType::OllamaCloud);
        assert_eq!(
            ollama.quota_strategy,
            Some(OAuthQuotaStrategy::ProviderSpecific)
        );
        assert!(ollama.supports_quota);
        assert_eq!(ollama.status, "manual_api_key_available");
    }

    #[test]
    fn refresh_plan_is_exposed_only_for_request_shape_ready_oauth_specs() {
        for provider_type in [
            ProviderType::CodexOAuth,
            ProviderType::ClaudeOAuth,
            ProviderType::GeminiCli,
            ProviderType::CursorOAuth,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
        ] {
            let capability = capability_for(provider_type);
            assert!(capability.supports_refresh_plan);
            assert!(capability.supports_refresh);
            assert_eq!(capability.support, AccountManagerSupport::ManualTokenStore);
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
            let capability = capability_for(provider_type);
            assert!(!capability.supports_refresh_plan);
            assert!(!capability.supports_refresh);
        }
    }

    #[test]
    fn account_manager_registry_is_explicit_and_conservative() {
        let registrations = registered_account_managers();
        assert_eq!(registrations.len(), all_capabilities().len());
        let codex = manager_registration_for(ProviderType::CodexOAuth);

        assert_eq!(codex.provider_type, ProviderType::CodexOAuth);
        assert_eq!(codex.kind, AccountManagerKind::ManualTokenStore);
        assert_eq!(codex.manager, "manual_token_store");
        assert!(!registrations
            .iter()
            .any(|item| item.kind == AccountManagerKind::NativeOAuth));
    }

    #[test]
    fn account_import_templates_cover_all_manual_account_types() {
        let templates = account_import_templates();
        assert_eq!(templates.len(), all_capabilities().len());

        let codex = templates
            .iter()
            .find(|item| item.provider_type == ProviderType::CodexOAuth)
            .unwrap();
        assert_eq!(codex.credential_kind, "access_token");
        assert!(codex.required_fields.contains(&"accessToken"));
        assert!(codex.optional_fields.contains(&"refreshToken"));
        assert!(codex.notes.contains("native refresh/profile"));

        let ollama = templates
            .iter()
            .find(|item| item.provider_type == ProviderType::OllamaCloud)
            .unwrap();
        assert_eq!(ollama.credential_kind, "api_key");
        assert!(ollama.required_fields.contains(&"apiKey"));

        let deepseek = templates
            .iter()
            .find(|item| item.provider_type == ProviderType::DeepSeekAccount)
            .unwrap();
        assert_eq!(deepseek.credential_kind, "access_token");
        assert_eq!(
            deepseek.required_fields.as_slice(),
            &["providerType", "accessToken"]
        );
        assert!(deepseek.notes.contains("import-only"));
        assert!(deepseek
            .notes
            .contains("does not store DeepSeek account passwords"));

        let bedrock = templates
            .iter()
            .find(|item| item.provider_type == ProviderType::AwsBedrock)
            .unwrap();
        assert_eq!(bedrock.credential_kind, "aws_credentials");
        assert!(bedrock.raw_hints.contains(&"awsSecretAccessKey"));
    }

    #[test]
    fn refresh_locks_are_scoped_by_provider_type_and_account_id() {
        let locks = AccountRefreshLocks::default();
        let codex = locks
            .try_lock(ProviderType::CodexOAuth, "acct-1")
            .expect("first codex lock");
        assert!(locks.is_locked(ProviderType::CodexOAuth, "acct-1"));
        assert!(locks.try_lock(ProviderType::CodexOAuth, "acct-1").is_none());
        assert!(locks
            .try_lock(ProviderType::ClaudeOAuth, "acct-1")
            .is_some());
        assert!(locks.try_lock(ProviderType::CodexOAuth, "acct-2").is_some());

        codex.release();
        assert!(!locks.is_locked(ProviderType::CodexOAuth, "acct-1"));
        assert!(locks.try_lock(ProviderType::CodexOAuth, "acct-1").is_some());
    }

    #[test]
    fn manual_manager_returns_valid_access_token() {
        let manager = manager_for(ProviderType::CodexOAuth);
        let mut store = AccountStore::default();
        let account = manager
            .finish_login(
                &mut store,
                UpsertAccountInput {
                    id: Some("acct-1".to_string()),
                    provider_type: ProviderType::CodexOAuth,
                    email: None,
                    access_token: Some("token".to_string()),
                    refresh_token: None,
                    id_token: None,
                    token_type: None,
                    api_key: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(2000),
                    last_refresh_error: None,
                },
            )
            .unwrap();

        let credential = manager
            .get_valid_token(&store, ProviderType::CodexOAuth, Some(&account.id), 1000)
            .unwrap();

        assert_eq!(credential.value, "token");
        assert_eq!(credential.credential_kind, CredentialKind::AccessToken);
    }

    #[test]
    fn manual_manager_rejects_expired_token_without_claiming_refresh() {
        let manager = manager_for(ProviderType::ClaudeOAuth);
        let mut store = AccountStore::default();
        manager
            .finish_login(
                &mut store,
                UpsertAccountInput {
                    id: Some("acct-1".to_string()),
                    provider_type: ProviderType::ClaudeOAuth,
                    email: None,
                    access_token: Some("token".to_string()),
                    refresh_token: Some("refresh".to_string()),
                    id_token: None,
                    token_type: None,
                    api_key: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1000),
                    last_refresh_error: None,
                },
            )
            .unwrap();

        let error = manager
            .get_valid_token(&store, ProviderType::ClaudeOAuth, Some("acct-1"), 2000)
            .unwrap_err();

        assert!(matches!(
            error,
            AccountManagerError::CredentialUnavailable(_)
        ));
    }

    #[test]
    fn manual_manager_does_not_expire_api_keys() {
        let manager = manager_for(ProviderType::OllamaCloud);
        let mut store = AccountStore::default();
        manager
            .finish_login(
                &mut store,
                UpsertAccountInput {
                    id: Some("acct-1".to_string()),
                    provider_type: ProviderType::OllamaCloud,
                    email: None,
                    access_token: None,
                    refresh_token: None,
                    id_token: None,
                    token_type: None,
                    api_key: Some("api-key".to_string()),
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1000),
                    last_refresh_error: None,
                },
            )
            .unwrap();

        let credential = manager
            .get_valid_token(&store, ProviderType::OllamaCloud, Some("acct-1"), 2000)
            .unwrap();

        assert_eq!(credential.value, "api-key");
        assert_eq!(credential.credential_kind, CredentialKind::ApiKey);
    }

    #[test]
    fn codex_oauth_manager_plans_refresh_request_and_exposes_refresh_capability() {
        let manager = CodexOAuthAccountManager::default();
        let mut store = AccountStore::default();
        manager_for(ProviderType::CodexOAuth)
            .finish_login(
                &mut store,
                UpsertAccountInput {
                    id: Some("acct-1".to_string()),
                    provider_type: ProviderType::CodexOAuth,
                    email: None,
                    access_token: Some("old".to_string()),
                    refresh_token: Some("refresh".to_string()),
                    id_token: None,
                    token_type: None,
                    api_key: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1_000),
                    last_refresh_error: None,
                },
            )
            .unwrap();

        let capability = manager.capability();
        assert_eq!(capability.support, AccountManagerSupport::ManualTokenStore);
        assert!(capability.supports_refresh);

        let (guard, request) = manager
            .plan_refresh_request(&store, "acct-1", 2_000)
            .expect("refresh request");
        assert_eq!(request.url, "https://auth.openai.com/oauth/token");
        assert_eq!(request.body["refresh_token"], "refresh");
        assert!(manager
            .plan_refresh_request(&store, "acct-1", 2_000)
            .is_err());
        guard.release();
        assert!(manager
            .plan_refresh_request(&store, "acct-1", 2_000)
            .is_ok());
    }

    #[test]
    fn codex_refresh_planning_respects_valid_and_deleted_accounts() {
        let manager = CodexOAuthAccountManager::default();
        let mut store = AccountStore::default();
        manager_for(ProviderType::CodexOAuth)
            .finish_login(
                &mut store,
                UpsertAccountInput {
                    id: Some("acct-valid".to_string()),
                    provider_type: ProviderType::CodexOAuth,
                    email: None,
                    access_token: Some("still-valid".to_string()),
                    refresh_token: Some("refresh".to_string()),
                    id_token: None,
                    token_type: None,
                    api_key: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(120_000),
                    last_refresh_error: None,
                },
            )
            .unwrap();

        let not_required = manager
            .plan_refresh_request(&store, "acct-valid", 1_000)
            .unwrap_err();
        assert!(matches!(
            not_required,
            AccountManagerError::CredentialUnavailable(_)
        ));

        assert!(store.delete("acct-valid"));
        let deleted = manager
            .plan_refresh_request(&store, "acct-valid", 20_000)
            .unwrap_err();
        assert!(matches!(deleted, AccountManagerError::NotFound(_)));
    }
}
