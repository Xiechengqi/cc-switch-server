#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::domain::accounts::oauth::{
    oauth_provider_spec, provider_login_request_shape_available, OAuthErrorKind,
    OAuthProfileStrategy, OAuthQuotaCapability, OAuthQuotaStrategy, OAuthRefreshCapability,
    OAuthSupportStage,
};
use crate::domain::accounts::store::{Account, AccountQuota, AccountStore, UpsertAccountInput};
use crate::domain::accounts::subscription_expiry::{
    subscription_expiry_capability, SubscriptionExpiryCapability,
};
use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum AccountManagerSupport {
    #[serde(rename = "native_oauth")]
    NativeOAuth,
    ManualTokenStore,
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountLoginFlowKind {
    #[serde(rename = "browser_oauth")]
    BrowserOAuth,
    DeviceCode,
    CliManualCallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginFlowCapability {
    pub kind: AccountLoginFlowKind,
    pub supports_callback: bool,
    pub supports_poll: bool,
    pub supports_cancel: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountManagerCapability {
    pub provider_type: ProviderType,
    pub manager: &'static str,
    pub manager_kind: AccountManagerKind,
    pub support: AccountManagerSupport,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_reason: Option<&'static str>,
    pub login_flows: Vec<AccountLoginFlowCapability>,
    pub supports_start_login: bool,
    pub supports_callback: bool,
    pub supports_refresh: bool,
    pub supports_quota: bool,
    pub supports_refresh_plan: bool,
    pub supports_cached_quota: bool,
    pub supports_live_quota_refresh: bool,
    pub refresh_capability: OAuthRefreshCapability,
    pub quota_capability: OAuthQuotaCapability,
    pub inference_binding_supported: bool,
    pub credential_ownership: AccountCredentialOwnership,
    pub deprecated_for_inference: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration_target: Option<&'static str>,
    pub supports_import: bool,
    pub supports_delete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_native_stage: Option<OAuthSupportStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_strategy: Option<OAuthProfileStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_strategy: Option<OAuthQuotaStrategy>,
    pub subscription_expiry_capability: SubscriptionExpiryCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountManagerKind {
    #[serde(rename = "native_oauth")]
    NativeOAuth,
    ImportOnly,
    StaticCredential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCredentialOwnership {
    ManagedAccount,
    Provider,
    MetadataOnly,
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

#[derive(Debug, Clone, Copy)]
pub struct AccountProviderDriver {
    provider_type: ProviderType,
    kind: AccountManagerKind,
}

#[derive(Debug, Default)]
pub struct AccountRefreshLocks {
    active: Mutex<BTreeMap<String, Arc<AsyncMutex<AccountRefreshFlightState>>>>,
}

#[derive(Debug)]
pub struct AccountRefreshGuard {
    guard: Option<OwnedMutexGuard<AccountRefreshFlightState>>,
    waited: bool,
}

#[derive(Debug, Clone)]
pub struct AccountRefreshFlightFailure {
    pub stage: AccountRefreshFlightStage,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
    pub status_code: u16,
    pub upstream_status: Option<u16>,
    pub message: String,
    pub public_message: Option<String>,
    pub kind: OAuthErrorKind,
    pub retryable: bool,
    pub retry_after_ms: Option<i64>,
    retry_not_before_ms: Option<i64>,
    pub immediate_relogin: bool,
}

#[derive(Debug, Clone)]
pub struct AccountRefreshFlightFailureDetails {
    pub status_code: u16,
    pub upstream_status: Option<u16>,
    pub message: String,
    pub public_message: Option<String>,
    pub kind: OAuthErrorKind,
    pub retryable: bool,
    pub retry_after_ms: Option<i64>,
    pub immediate_relogin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountRefreshFlightStage {
    NativeRefresh,
    QuotaRefresh,
    GeminiProjectDiscovery,
}

impl AccountRefreshFlightFailure {
    pub fn for_account(
        account: &Account,
        stage: AccountRefreshFlightStage,
        details: AccountRefreshFlightFailureDetails,
    ) -> Self {
        let retry_not_before_ms = details.retry_after_ms.map(|retry_after_ms| {
            (crate::infra::time::now_ms().min(i64::MAX as u128) as i64)
                .saturating_add(retry_after_ms.max(0))
        });
        Self {
            stage,
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
            status_code: details.status_code,
            upstream_status: details.upstream_status,
            message: details.message,
            public_message: details.public_message,
            kind: details.kind,
            retryable: details.retryable,
            retry_after_ms: details.retry_after_ms,
            retry_not_before_ms,
            immediate_relogin: details.immediate_relogin,
        }
    }

    fn matches_account(&self, account: &Account) -> bool {
        self.auth_identity_generation == account.auth_identity_generation
            && self.token_refresh_generation == account.token_refresh_generation
    }

    fn cooldown_active(&self) -> bool {
        self.retry_not_before_ms.is_some_and(|retry_not_before_ms| {
            retry_not_before_ms > crate::infra::time::now_ms().min(i64::MAX as u128) as i64
        })
    }
}

#[derive(Debug, Default)]
struct AccountRefreshFlightState {
    failure: Option<AccountRefreshFlightFailure>,
}

impl AccountRefreshLocks {
    pub async fn lock(&self, provider_type: ProviderType, account_id: &str) -> AccountRefreshGuard {
        let lock = self.lock_for(provider_type, account_id);
        match Arc::clone(&lock).try_lock_owned() {
            Ok(mut guard) => {
                if !guard
                    .failure
                    .as_ref()
                    .is_some_and(AccountRefreshFlightFailure::cooldown_active)
                {
                    guard.failure = None;
                }
                AccountRefreshGuard {
                    guard: Some(guard),
                    waited: false,
                }
            }
            Err(_) => AccountRefreshGuard {
                guard: Some(lock.lock_owned().await),
                waited: true,
            },
        }
    }

    pub fn try_lock(
        &self,
        provider_type: ProviderType,
        account_id: &str,
    ) -> Option<AccountRefreshGuard> {
        let lock = self.lock_for(provider_type, account_id);
        lock.try_lock_owned().ok().map(|mut guard| {
            if !guard
                .failure
                .as_ref()
                .is_some_and(AccountRefreshFlightFailure::cooldown_active)
            {
                guard.failure = None;
            }
            AccountRefreshGuard {
                guard: Some(guard),
                waited: false,
            }
        })
    }

    pub fn is_locked(&self, provider_type: ProviderType, account_id: &str) -> bool {
        let lock = self.lock_for(provider_type, account_id);
        lock.try_lock_owned().is_err()
    }

    fn lock_for(
        &self,
        provider_type: ProviderType,
        account_id: &str,
    ) -> Arc<AsyncMutex<AccountRefreshFlightState>> {
        let key = refresh_lock_key(provider_type, account_id);
        let mut active = self.active.lock().expect("account refresh lock poisoned");
        active.retain(|_, lock| {
            if Arc::strong_count(lock) > 1 {
                return true;
            }
            match lock.try_lock() {
                Ok(state) => state
                    .failure
                    .as_ref()
                    .is_some_and(AccountRefreshFlightFailure::cooldown_active),
                Err(_) => true,
            }
        });
        if let Some(lock) = active.get(&key) {
            return Arc::clone(lock);
        }
        let lock = Arc::new(AsyncMutex::new(AccountRefreshFlightState::default()));
        active.insert(key, Arc::clone(&lock));
        lock
    }
}

impl AccountRefreshGuard {
    pub fn coalesced_native_failure_for(
        &self,
        account: &Account,
    ) -> Option<&AccountRefreshFlightFailure> {
        self.coalesced_failure_for(account)
            .filter(|failure| failure.stage == AccountRefreshFlightStage::NativeRefresh)
    }

    pub fn coalesced_quota_failure_for(
        &self,
        account: &Account,
    ) -> Option<&AccountRefreshFlightFailure> {
        self.coalesced_failure_for(account)
    }

    pub fn coalesced_gemini_project_failure_for(
        &self,
        account: &Account,
    ) -> Option<&AccountRefreshFlightFailure> {
        self.coalesced_failure_for(account).filter(|failure| {
            matches!(
                failure.stage,
                AccountRefreshFlightStage::NativeRefresh
                    | AccountRefreshFlightStage::GeminiProjectDiscovery
            )
        })
    }

    fn coalesced_failure_for(&self, account: &Account) -> Option<&AccountRefreshFlightFailure> {
        self.guard
            .as_ref()
            .and_then(|guard| guard.failure.as_ref())
            .filter(|failure| failure.matches_account(account))
            .filter(|failure| self.waited || failure.cooldown_active())
    }

    pub fn record_failure(&mut self, failure: AccountRefreshFlightFailure) {
        if let Some(guard) = self.guard.as_mut() {
            guard.failure = Some(failure);
        }
    }

    pub fn release(mut self) {
        self.guard.take();
    }
}

impl AccountProviderDriver {
    fn new(provider_type: ProviderType) -> Self {
        Self {
            provider_type,
            kind: account_manager_kind_for(provider_type),
        }
    }

    fn ensure_provider_type(self, provider_type: ProviderType) -> Result<(), AccountManagerError> {
        if provider_type == self.provider_type {
            return Ok(());
        }
        Err(AccountManagerError::CredentialUnavailable(format!(
            "account driver for {} cannot manage {}",
            self.provider_type.as_str(),
            provider_type.as_str()
        )))
    }
}

impl AccountManager for AccountProviderDriver {
    fn capability(&self, _provider_type: ProviderType) -> AccountManagerCapability {
        manual_capability(self.provider_type)
    }

    fn start_login(&self, provider_type: ProviderType) -> Result<LoginStart, AccountManagerError> {
        self.ensure_provider_type(provider_type)?;
        if provider_type == ProviderType::CursorOAuth {
            return Ok(LoginStart {
                provider_type,
                method: "cursor_deep_control",
                message: "start Cursor OAuth through the server account login endpoint".to_string(),
            });
        }
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
            ProviderType::CursorOAuth => unreachable!(),
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
        mut input: UpsertAccountInput,
    ) -> Result<Account, AccountManagerError> {
        self.ensure_provider_type(input.provider_type)?;
        validate_required_account_credential(&input)?;
        if let Some(account_id) = input.id.as_deref() {
            if let Some(existing) = store
                .accounts
                .iter()
                .find(|account| account.id == account_id)
            {
                if existing.provider_type != input.provider_type {
                    return Err(AccountManagerError::CredentialUnavailable(format!(
                        "account {account_id} is already bound to providerType {}; create a new account instead of changing its type to {}",
                        existing.provider_type.as_str(),
                        input.provider_type.as_str()
                    )));
                }
            }
        }
        if matches!(
            input.provider_type,
            ProviderType::CursorOAuth | ProviderType::CursorApiKey
        ) {
            if let Some(existing) = store.accounts.iter().find(|account| {
                account.provider_type == input.provider_type
                    && input.id.as_deref() != Some(account.id.as_str())
            }) {
                return Err(AccountManagerError::CredentialUnavailable(format!(
                    "{} supports one proxy credential; update or remove account {} before adding another",
                    input.provider_type.as_str(),
                    existing.id
                )));
            }
        }
        if input.provider_type == ProviderType::CodexOAuth {
            if let Some(refresh_token) = input
                .refresh_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if let Some(owner) = store.refresh_token_owner(
                    ProviderType::CodexOAuth,
                    refresh_token,
                    input.id.as_deref(),
                ) {
                    return Err(AccountManagerError::CredentialUnavailable(format!(
                        "codex oauth refresh token is already managed by account {}; use the existing account or start a new device login",
                        owner.id
                    )));
                }
            }
            let mut raw = input
                .raw
                .take()
                .filter(Value::is_object)
                .unwrap_or_else(|| Value::Object(Map::new()));
            if let Some(object) = raw.as_object_mut() {
                object.insert(
                    "tokenAuthority".to_string(),
                    Value::String("cc-switch-server".to_string()),
                );
                object.insert(
                    "refreshOwnership".to_string(),
                    Value::String("exclusive".to_string()),
                );
            }
            input.raw = Some(raw);
        }
        Ok(store.upsert(input))
    }

    fn get_valid_token(
        &self,
        store: &AccountStore,
        provider_type: ProviderType,
        account_id: Option<&str>,
        now_ms: i64,
    ) -> Result<AccountCredential, AccountManagerError> {
        self.ensure_provider_type(provider_type)?;
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
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| AccountManagerError::NotFound(account_id.to_string()))?;
        self.ensure_provider_type(account.provider_type)?;
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
        self.ensure_provider_type(account.provider_type)?;
        Ok(account.quota.clone())
    }

    fn revoke_or_delete(
        &self,
        store: &mut AccountStore,
        account_id: &str,
    ) -> Result<bool, AccountManagerError> {
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| AccountManagerError::NotFound(account_id.to_string()))?;
        self.ensure_provider_type(account.provider_type)?;
        Ok(store.delete(account_id))
    }
}

pub fn capability_for(provider_type: ProviderType) -> AccountManagerCapability {
    manager_for(provider_type).capability(provider_type)
}

pub fn manager_for(provider_type: ProviderType) -> AccountProviderDriver {
    AccountProviderDriver::new(provider_type)
}

pub fn manager_registration_for(provider_type: ProviderType) -> AccountManagerRegistration {
    let driver = manager_for(provider_type);
    AccountManagerRegistration {
        provider_type,
        kind: driver.kind,
        manager: match driver.kind {
            AccountManagerKind::NativeOAuth => "native_oauth_account",
            AccountManagerKind::ImportOnly => "import_only_account",
            AccountManagerKind::StaticCredential => "static_credential_metadata",
        },
    }
}

fn account_manager_kind_for(provider_type: ProviderType) -> AccountManagerKind {
    match provider_type {
        ProviderType::ClaudeOAuth
        | ProviderType::CodexOAuth
        | ProviderType::GrokOAuth
        | ProviderType::GeminiCli
        | ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::KimiCode
        | ProviderType::CursorOAuth
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth => AccountManagerKind::NativeOAuth,
        ProviderType::DeepSeekAccount => AccountManagerKind::ImportOnly,
        ProviderType::Claude
        | ProviderType::ClaudeAuth
        | ProviderType::Codex
        | ProviderType::Gemini
        | ProviderType::OpenRouter
        | ProviderType::CursorApiKey
        | ProviderType::OllamaCloud
        | ProviderType::AwsBedrock
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => AccountManagerKind::StaticCredential,
    }
}

fn validate_required_account_credential(
    input: &UpsertAccountInput,
) -> Result<(), AccountManagerError> {
    if input.provider_type == ProviderType::DeepSeekAccount
        && input
            .access_token
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AccountManagerError::CredentialUnavailable(
            "deepseek_account requires a non-empty accessToken".to_string(),
        ));
    }
    Ok(())
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

fn account_provider_types() -> [ProviderType; 16] {
    [
        ProviderType::ClaudeOAuth,
        ProviderType::CodexOAuth,
        ProviderType::GrokOAuth,
        ProviderType::GeminiCli,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::KimiCode,
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
    let login_flows = login_flows_for(provider_type);
    let supports_start_login = !login_flows.is_empty();
    let supports_callback = login_flows.iter().any(|flow| flow.supports_callback);
    let refresh_capability = oauth_spec
        .map(|spec| spec.refresh_capability)
        .unwrap_or(OAuthRefreshCapability::Unavailable);
    let quota_capability = oauth_spec
        .map(|spec| spec.quota_capability)
        .unwrap_or(OAuthQuotaCapability::Unavailable);
    let supports_native_refresh =
        !matches!(refresh_capability, OAuthRefreshCapability::Unavailable);
    let supports_refresh_plan = supports_native_refresh;
    let supports_cached_quota = !matches!(quota_capability, OAuthQuotaCapability::Unavailable);
    let supports_live_quota_refresh = matches!(quota_capability, OAuthQuotaCapability::LiveRefresh);
    let credential_ownership = account_credential_ownership(provider_type);
    let inference_binding_supported = matches!(
        credential_ownership,
        AccountCredentialOwnership::ManagedAccount
    );
    let manager_kind = account_manager_kind_for(provider_type);
    let deprecated_for_inference = matches!(
        credential_ownership,
        AccountCredentialOwnership::MetadataOnly
    );
    let native_oauth_planned = matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GeminiCli
            | ProviderType::GitHubCopilot
            | ProviderType::DeepSeekAccount
            | ProviderType::KiroOAuth
            | ProviderType::KimiCode
            | ProviderType::CursorOAuth
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
            | ProviderType::GrokOAuth
    );
    AccountManagerCapability {
        provider_type,
        manager: if supports_native_refresh {
            "manual_token_store_with_native_refresh"
        } else {
            "manual_token_store"
        },
        manager_kind,
        support: AccountManagerSupport::ManualTokenStore,
        status: if deprecated_for_inference {
            "metadata_only"
        } else if supports_start_login && supports_native_refresh {
            "native_login_refresh"
        } else if supports_start_login {
            "native_login"
        } else if supports_native_refresh {
            "manual_import_native_refresh"
        } else if native_oauth_planned {
            "manual_import_only"
        } else {
            "manual_api_key_available"
        },
        blocking_reason: if deprecated_for_inference {
            Some(
                "static credentials are Provider-owned; this Account record is metadata/quota compatibility only",
            )
        } else if supports_start_login {
            None
        } else if supports_native_refresh {
            Some("native browser login/callback is disabled; import refresh credentials first")
        } else {
            native_oauth_planned.then_some(
                "native oauth/login/refresh requires real credentials and has not been enabled",
            )
        },
        login_flows,
        supports_start_login,
        supports_callback,
        supports_refresh: supports_native_refresh,
        supports_quota: supports_cached_quota,
        supports_refresh_plan,
        supports_cached_quota,
        supports_live_quota_refresh,
        refresh_capability,
        quota_capability,
        inference_binding_supported,
        credential_ownership,
        deprecated_for_inference,
        migration_target: deprecated_for_inference.then_some("provider"),
        supports_import: true,
        supports_delete: true,
        server_native_stage: oauth_spec.map(|spec| spec.stage),
        profile_strategy: oauth_spec.map(|spec| spec.profile_strategy),
        quota_strategy: oauth_spec.map(|spec| spec.quota_strategy),
        subscription_expiry_capability: subscription_expiry_capability(provider_type),
    }
}

pub(crate) fn account_credential_ownership(
    provider_type: ProviderType,
) -> AccountCredentialOwnership {
    match provider_type {
        ProviderType::ClaudeOAuth
        | ProviderType::CodexOAuth
        | ProviderType::GrokOAuth
        | ProviderType::GeminiCli
        | ProviderType::GitHubCopilot
        | ProviderType::DeepSeekAccount
        | ProviderType::KiroOAuth
        | ProviderType::KimiCode
        | ProviderType::CursorOAuth
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth => AccountCredentialOwnership::ManagedAccount,
        ProviderType::CursorApiKey
        | ProviderType::OllamaCloud
        | ProviderType::AwsBedrock
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => AccountCredentialOwnership::MetadataOnly,
        ProviderType::Claude
        | ProviderType::ClaudeAuth
        | ProviderType::Codex
        | ProviderType::Gemini
        | ProviderType::OpenRouter => AccountCredentialOwnership::Provider,
    }
}

fn login_flows_for(provider_type: ProviderType) -> Vec<AccountLoginFlowCapability> {
    let mut flows = Vec::new();
    if provider_login_request_shape_available(provider_type) {
        flows.push(AccountLoginFlowCapability {
            kind: AccountLoginFlowKind::BrowserOAuth,
            supports_callback: true,
            supports_poll: true,
            supports_cancel: true,
        });
    }
    if matches!(
        provider_type,
        ProviderType::CodexOAuth
            | ProviderType::GitHubCopilot
            | ProviderType::KiroOAuth
            | ProviderType::KimiCode
            | ProviderType::GrokOAuth
    ) {
        flows.push(AccountLoginFlowCapability {
            kind: AccountLoginFlowKind::DeviceCode,
            supports_callback: false,
            supports_poll: true,
            supports_cancel: matches!(
                provider_type,
                ProviderType::CodexOAuth | ProviderType::GrokOAuth | ProviderType::KimiCode
            ),
        });
    }
    if matches!(
        provider_type,
        ProviderType::ClaudeOAuth | ProviderType::CodexOAuth
    ) {
        flows.push(AccountLoginFlowCapability {
            kind: AccountLoginFlowKind::CliManualCallback,
            supports_callback: true,
            supports_poll: true,
            supports_cancel: true,
        });
    }
    flows
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
            notes: "AWS Builder ID device flow import is available via /api/accounts/kiro/device/start|poll; Claude CodeWhisperer forwarding and server-native refresh are wired, with native capability still gated on real Kiro validation",
        },
        ProviderType::KimiCode => AccountImportTemplate {
            provider_type,
            credential_kind: "oauth_token",
            required_fields: vec!["providerType", "accessToken or refreshToken"],
            optional_fields: optional_oauth_fields.clone(),
            profile_hints: vec!["userId", "deviceId", "deviceName", "deviceModel", "osVersion"],
            raw_hints: vec!["Kimi OAuth token response", "deviceId", "loginMethod"],
            notes: "Kimi Code device login is available via /api/accounts/kimi/device/start|poll|cancel; each Provider must explicitly bind one account",
        },
        ProviderType::GrokOAuth => AccountImportTemplate {
            provider_type,
            credential_kind: "oauth_token",
            required_fields: vec!["providerType", "accessToken or refreshToken"],
            optional_fields: optional_oauth_fields.clone(),
            profile_hints: vec![
                "email",
                "preferred_username",
                "sub",
                "team_id",
                "tier",
                "principal_type",
            ],
            raw_hints: vec![
                "~/.grok/auth.json entry",
                "xAI OAuth token response",
                "xai-subscription-tier",
                "xai-entitlement-status",
            ],
            notes: "xAI/Grok OAuth token import; native refresh is enabled when refreshToken is present",
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

    fn codex_account_input(id: &str, refresh_token: &str) -> UpsertAccountInput {
        UpsertAccountInput {
            id: Some(id.to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: None,
            access_token: Some(format!("access-{id}")),
            refresh_token: Some(refresh_token.to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        }
    }

    #[test]
    fn account_capabilities_report_native_login_and_refresh_independently() {
        let capability = capability_for(ProviderType::CodexOAuth);
        assert_eq!(capability.support, AccountManagerSupport::ManualTokenStore);
        assert_eq!(capability.manager, "manual_token_store_with_native_refresh");
        assert_eq!(capability.manager_kind, AccountManagerKind::NativeOAuth);
        assert_eq!(capability.status, "native_login_refresh");
        assert!(capability.blocking_reason.is_none());
        assert!(capability.supports_start_login);
        assert!(capability.supports_callback);
        assert_eq!(capability.login_flows.len(), 3);
        assert!(capability.supports_refresh);
        assert_eq!(
            capability.refresh_capability,
            OAuthRefreshCapability::OAuthRequest
        );
        assert!(capability.supports_live_quota_refresh);
        assert!(capability.inference_binding_supported);
        assert_eq!(
            capability.credential_ownership,
            AccountCredentialOwnership::ManagedAccount
        );
        assert!(capability.supports_import);
        assert!(capability.supports_delete);
        assert_eq!(
            capability.server_native_stage,
            Some(OAuthSupportStage::NativeRefreshProfile)
        );
        assert_eq!(
            capability.subscription_expiry_capability,
            SubscriptionExpiryCapability::Automatic
        );
    }

    #[test]
    fn cursor_account_manager_rejects_a_second_proxy_credential() {
        for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
            let manager = manager_for(provider_type);
            let mut store = AccountStore::default();
            let mut first = codex_account_input("cursor-1", "refresh-1");
            first.provider_type = provider_type;
            first.api_key =
                (provider_type == ProviderType::CursorApiKey).then(|| "cursor-key-1".to_string());
            manager.finish_login(&mut store, first.clone()).unwrap();

            let mut second = first.clone();
            second.id = Some("cursor-2".to_string());
            let error = manager.finish_login(&mut store, second).unwrap_err();
            assert!(error.to_string().contains("supports one proxy credential"));

            first.email = Some("updated@example.com".to_string());
            let updated = manager.finish_login(&mut store, first).unwrap();
            assert_eq!(updated.email.as_deref(), Some("updated@example.com"));
        }
    }

    #[test]
    fn deepseek_account_manager_requires_access_token_before_create_or_update() {
        let manager = manager_for(ProviderType::DeepSeekAccount);

        for access_token in [None, Some(String::new()), Some("   ".to_string())] {
            let mut store = AccountStore::default();
            let mut input = codex_account_input("deepseek-new", "unused-refresh");
            input.provider_type = ProviderType::DeepSeekAccount;
            input.access_token = access_token;
            input.refresh_token = None;

            let error = manager.finish_login(&mut store, input).unwrap_err();

            assert!(matches!(
                error,
                AccountManagerError::CredentialUnavailable(ref message)
                    if message.contains("non-empty accessToken")
            ));
            assert!(store.accounts.is_empty());
        }

        let mut store = AccountStore::default();
        let mut initial = codex_account_input("deepseek-existing", "unused-refresh");
        initial.provider_type = ProviderType::DeepSeekAccount;
        initial.access_token = Some("deepseek-access-token".to_string());
        initial.refresh_token = None;
        initial.email = Some("original@example.com".to_string());
        manager.finish_login(&mut store, initial).unwrap();

        let mut invalid_update = codex_account_input("deepseek-existing", "unused-refresh");
        invalid_update.provider_type = ProviderType::DeepSeekAccount;
        invalid_update.access_token = None;
        invalid_update.refresh_token = None;
        invalid_update.email = Some("changed@example.com".to_string());

        let error = manager
            .finish_login(&mut store, invalid_update)
            .unwrap_err();

        assert!(error.to_string().contains("non-empty accessToken"));
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(
            store.accounts[0].access_token.as_deref(),
            Some("deepseek-access-token")
        );
        assert_eq!(
            store.accounts[0].email.as_deref(),
            Some("original@example.com")
        );
    }

    #[test]
    fn account_capabilities_expose_subscription_expiry_policy() {
        assert_eq!(
            capability_for(ProviderType::ClaudeOAuth).subscription_expiry_capability,
            SubscriptionExpiryCapability::ManualRequired
        );
        assert_eq!(
            capability_for(ProviderType::GrokOAuth).subscription_expiry_capability,
            SubscriptionExpiryCapability::AutomaticOrManual
        );
        assert_eq!(
            capability_for(ProviderType::OllamaCloud).subscription_expiry_capability,
            SubscriptionExpiryCapability::Automatic
        );
        assert_eq!(
            capability_for(ProviderType::CursorOAuth).subscription_expiry_capability,
            SubscriptionExpiryCapability::ResearchPending
        );
        assert_eq!(
            capability_for(ProviderType::DeepSeekApi).subscription_expiry_capability,
            SubscriptionExpiryCapability::NotApplicable
        );
    }

    #[test]
    fn long_tail_account_capabilities_keep_manual_storage_and_report_real_login_flows() {
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
            let expected_login = matches!(
                provider_type,
                ProviderType::CursorOAuth
                    | ProviderType::GitHubCopilot
                    | ProviderType::KiroOAuth
                    | ProviderType::AntigravityOAuth
                    | ProviderType::AgyOAuth
            );
            assert_eq!(capability.supports_start_login, expected_login);
            if matches!(
                provider_type,
                ProviderType::CursorOAuth
                    | ProviderType::KiroOAuth
                    | ProviderType::AntigravityOAuth
                    | ProviderType::AgyOAuth
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
        assert!(codex.supports_start_login);
        assert!(codex.supports_refresh);

        let ollama = capability_for(ProviderType::OllamaCloud);
        assert_eq!(
            ollama.quota_strategy,
            Some(OAuthQuotaStrategy::ProviderSpecific)
        );
        assert!(ollama.supports_quota);
        assert_eq!(ollama.status, "metadata_only");
        assert!(ollama.deprecated_for_inference);
        assert_eq!(ollama.migration_target, Some("provider"));
    }

    #[test]
    fn account_capability_execution_truth_table_is_complete() {
        let cases = [
            (
                ProviderType::ClaudeOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::CodexOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::GrokOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::GeminiCli,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::GitHubCopilot,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::ImportedSnapshot,
                true,
            ),
            (
                ProviderType::DeepSeekAccount,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::CachedOnly,
                true,
            ),
            (
                ProviderType::KiroOAuth,
                OAuthRefreshCapability::ProviderDynamic,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::KimiCode,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::Unavailable,
                true,
            ),
            (
                ProviderType::CursorOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::ImportedSnapshot,
                true,
            ),
            (
                ProviderType::CursorApiKey,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::ImportedSnapshot,
                false,
            ),
            (
                ProviderType::AntigravityOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::AgyOAuth,
                OAuthRefreshCapability::OAuthRequest,
                OAuthQuotaCapability::LiveRefresh,
                true,
            ),
            (
                ProviderType::OllamaCloud,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::LiveRefresh,
                false,
            ),
            (
                ProviderType::AwsBedrock,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::CachedOnly,
                false,
            ),
            (
                ProviderType::Nvidia,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::CachedOnly,
                false,
            ),
            (
                ProviderType::DeepSeekApi,
                OAuthRefreshCapability::Unavailable,
                OAuthQuotaCapability::CachedOnly,
                false,
            ),
        ];

        assert_eq!(cases.len(), account_provider_types().len());
        for (provider_type, refresh, quota, binding_supported) in cases {
            let capability = capability_for(provider_type);
            assert_eq!(capability.refresh_capability, refresh, "{provider_type:?}");
            assert_eq!(capability.quota_capability, quota, "{provider_type:?}");
            assert_eq!(
                capability.supports_refresh,
                refresh != OAuthRefreshCapability::Unavailable,
                "{provider_type:?}"
            );
            assert_eq!(
                capability.supports_refresh_plan,
                capability.supports_refresh
            );
            assert_eq!(
                capability.supports_live_quota_refresh,
                quota == OAuthQuotaCapability::LiveRefresh,
                "{provider_type:?}"
            );
            assert_eq!(
                capability.supports_cached_quota,
                quota != OAuthQuotaCapability::Unavailable,
                "{provider_type:?}"
            );
            assert_eq!(capability.supports_quota, capability.supports_cached_quota);
            assert_eq!(capability.inference_binding_supported, binding_supported);
            assert_eq!(
                capability.credential_ownership,
                if binding_supported {
                    AccountCredentialOwnership::ManagedAccount
                } else {
                    AccountCredentialOwnership::MetadataOnly
                }
            );
            assert_eq!(capability.deprecated_for_inference, !binding_supported);
            assert_eq!(
                capability.migration_target,
                (!binding_supported).then_some("provider")
            );
        }

        let kiro = serde_json::to_value(capability_for(ProviderType::KiroOAuth)).unwrap();
        assert_eq!(kiro["supportsRefresh"], true);
        assert_eq!(kiro["supportsRefreshPlan"], true);
        assert_eq!(kiro["supportsQuota"], true);
        assert_eq!(kiro["refreshCapability"], "provider_dynamic");
        assert_eq!(kiro["quotaCapability"], "live_refresh");
        assert_eq!(kiro["credentialOwnership"], "managed_account");
        assert_eq!(kiro["managerKind"], "native_oauth");
        assert_eq!(kiro["deprecatedForInference"], false);

        let codex = serde_json::to_value(capability_for(ProviderType::CodexOAuth)).unwrap();
        assert_eq!(codex["refreshCapability"], "oauth_request");
        assert_eq!(codex["managerKind"], "native_oauth");
    }

    #[test]
    fn account_manager_registry_reports_driver_kind() {
        let registrations = registered_account_managers();
        assert_eq!(registrations.len(), all_capabilities().len());
        let codex = manager_registration_for(ProviderType::CodexOAuth);

        assert_eq!(codex.provider_type, ProviderType::CodexOAuth);
        assert_eq!(codex.kind, AccountManagerKind::NativeOAuth);
        assert_eq!(codex.manager, "native_oauth_account");
        assert_eq!(
            registrations
                .iter()
                .filter(|item| item.kind == AccountManagerKind::NativeOAuth)
                .count(),
            10
        );
        assert_eq!(
            registrations
                .iter()
                .filter(|item| item.kind == AccountManagerKind::ImportOnly)
                .count(),
            1
        );
        assert_eq!(
            registrations
                .iter()
                .filter(|item| item.kind == AccountManagerKind::StaticCredential)
                .count(),
            5
        );
        assert_eq!(
            account_credential_ownership(ProviderType::Claude),
            AccountCredentialOwnership::Provider
        );
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
    fn inactive_refresh_locks_are_pruned() {
        let locks = AccountRefreshLocks::default();
        {
            let _guard = locks
                .try_lock(ProviderType::CodexOAuth, "retired-account")
                .expect("refresh lock");
            assert_eq!(locks.active.lock().unwrap().len(), 1);
        }

        let _guard = locks
            .try_lock(ProviderType::ClaudeOAuth, "active-account")
            .expect("replacement refresh lock");
        let active = locks.active.lock().unwrap();
        assert_eq!(active.len(), 1);
        assert!(active.contains_key(&refresh_lock_key(
            ProviderType::ClaudeOAuth,
            "active-account"
        )));
    }

    #[tokio::test]
    async fn gemini_project_waiters_and_cooldown_consume_native_refresh_failure() {
        let mut store = AccountStore::default();
        let mut input = codex_account_input("gemini-flight-account", "gemini-refresh");
        input.provider_type = ProviderType::GeminiCli;
        let account = store.upsert(input);
        let locks = AccountRefreshLocks::default();
        let mut leader = locks.lock(ProviderType::GeminiCli, &account.id).await;
        leader.record_failure(AccountRefreshFlightFailure::for_account(
            &account,
            AccountRefreshFlightStage::NativeRefresh,
            AccountRefreshFlightFailureDetails {
                status_code: 503,
                upstream_status: None,
                message: "credential commit failed".to_string(),
                public_message: Some("credential commit failed".to_string()),
                kind: OAuthErrorKind::Unknown,
                retryable: true,
                retry_after_ms: Some(60_000),
                immediate_relogin: false,
            },
        ));
        leader.release();

        let next = locks.lock(ProviderType::GeminiCli, &account.id).await;
        let failure = next
            .coalesced_gemini_project_failure_for(&account)
            .expect("active native refresh cooldown");
        assert_eq!(failure.stage, AccountRefreshFlightStage::NativeRefresh);
        assert_eq!(failure.status_code, 503);
    }

    #[test]
    fn codex_refresh_tokens_have_exclusive_server_authority() {
        let manager = manager_for(ProviderType::CodexOAuth);
        let mut store = AccountStore::default();
        let first = manager
            .finish_login(&mut store, codex_account_input("acct-1", "refresh-shared"))
            .unwrap();
        assert_eq!(
            first.raw.as_ref().unwrap()["tokenAuthority"],
            "cc-switch-server"
        );
        assert_eq!(first.raw.as_ref().unwrap()["refreshOwnership"], "exclusive");

        let duplicate = manager
            .finish_login(&mut store, codex_account_input("acct-2", "refresh-shared"))
            .unwrap_err();
        assert!(matches!(
            duplicate,
            AccountManagerError::CredentialUnavailable(_)
        ));

        let updated = manager
            .finish_login(&mut store, codex_account_input("acct-1", "refresh-shared"))
            .unwrap();
        assert_eq!(updated.id, "acct-1");
        assert_eq!(store.accounts.len(), 1);
    }

    #[test]
    fn account_provider_type_cannot_change_in_place() {
        let manager = manager_for(ProviderType::CodexOAuth);
        let mut store = AccountStore::default();
        manager
            .finish_login(&mut store, codex_account_input("acct-1", "refresh-1"))
            .unwrap();

        let mut replacement = codex_account_input("acct-1", "refresh-2");
        replacement.provider_type = ProviderType::ClaudeOAuth;
        let error = manager.finish_login(&mut store, replacement).unwrap_err();

        assert!(matches!(
            error,
            AccountManagerError::CredentialUnavailable(_)
        ));
        assert_eq!(store.accounts[0].provider_type, ProviderType::CodexOAuth);
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
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(2000),
                    rate_limited_until: None,
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
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1000),
                    rate_limited_until: None,
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
                    extra_headers: None,
                    scopes: Vec::new(),
                    profile: None,
                    raw: None,
                    subscription_level: None,
                    entitlement_status: None,
                    quota: None,
                    quota_percent: None,
                    quota_refreshed_at: None,
                    quota_next_refresh_at: None,
                    expires_at: Some(1000),
                    rate_limited_until: None,
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
}
