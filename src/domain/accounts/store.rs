#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::domain::accounts::oauth::{oauth_provider_spec, OAuthErrorKind};
use crate::domain::accounts::subscription_expiry::{
    SubscriptionExpiryRule, SubscriptionExpiryRuleDraft,
};
use crate::domain::providers::model::ProviderType;

const ACCOUNTS_FILE_NAME: &str = "accounts.json";
const ENCRYPTED_PREFIX: &str = "ccenc:v1:";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStore {
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider_type: ProviderType,
    #[serde(default = "initial_auth_identity_generation")]
    pub auth_identity_generation: u64,
    #[serde(default)]
    pub token_refresh_generation: u64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub raw: Option<Value>,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub entitlement_status: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub quota: Option<AccountQuota>,
    #[serde(default)]
    pub quota_refreshed_at: Option<i64>,
    #[serde(default)]
    pub quota_next_refresh_at: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_subscription_expires_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_subscription_expiry_updated_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_subscription_expiry_rule: Option<SubscriptionExpiryRule>,
    #[serde(default)]
    pub rate_limited_until: Option<i64>,
    #[serde(default)]
    pub last_refresh_error: Option<String>,
    #[serde(default)]
    pub refresh_consecutive_failures: u32,
    #[serde(default)]
    pub needs_relogin: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuota {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub credential_message: Option<String>,
    #[serde(default)]
    pub tiers: Vec<AccountQuotaTier>,
    #[serde(default)]
    pub extra_usage: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaTier {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountUsageBlockKind {
    RateLimited,
    QuotaExhausted,
}

impl AccountUsageBlockKind {
    pub fn availability(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUsageBlock {
    pub kind: AccountUsageBlockKind,
    pub reason: &'static str,
    pub scope: &'static str,
    pub until_ms: i64,
}

const DEFAULT_QUOTA_EVIDENCE_TTL_MS: i64 = 60 * 60 * 1000;
const MIN_QUOTA_REFRESH_INTERVAL_MS: i64 = 5 * 60 * 1000;
const MAX_QUOTA_REFRESH_INTERVAL_MS: i64 = 6 * 60 * 60 * 1000;

pub fn active_account_usage_block(account: &Account, now_ms: i64) -> Option<AccountUsageBlock> {
    if let Some(until_ms) = account.rate_limited_until.filter(|until| *until > now_ms) {
        return Some(AccountUsageBlock {
            kind: AccountUsageBlockKind::RateLimited,
            reason: "upstream rate limit is active",
            scope: "account_rate_limit",
            until_ms,
        });
    }

    let quota = account.quota.as_ref()?;
    if !quota.success || account.last_refresh_error.is_some() {
        return None;
    }
    let evidence_deadline = quota_evidence_deadline(account, now_ms)?;
    let extra = quota.extra_usage.as_ref()?;
    let (reason, scope) = match account.provider_type {
        ProviderType::CodexOAuth
            if extra
                .pointer("/subscriptionEvidence/usageLimitReached")
                .and_then(Value::as_bool)
                == Some(true)
                || extra
                    .pointer("/subscriptionEvidence/usageAllowed")
                    .and_then(Value::as_bool)
                    == Some(false) =>
        {
            ("upstream reported the Codex usage limit", "codex_account")
        }
        ProviderType::GrokOAuth
            if extra
                .pointer("/spendingLimitReached")
                .and_then(Value::as_bool)
                == Some(true) =>
        {
            ("upstream reported the Grok spending limit", "grok_account")
        }
        ProviderType::KiroOAuth
            if extra.pointer("/overageEnabled").and_then(Value::as_bool) == Some(false)
                && quota.tiers.iter().any(quota_tier_is_exhausted) =>
        {
            ("upstream reported the Kiro usage limit", "kiro_account")
        }
        _ => return None,
    };
    let until_ms = quota
        .tiers
        .iter()
        .filter(|tier| quota_tier_is_exhausted(tier))
        .filter_map(|tier| tier.resets_at)
        .filter(|reset| *reset > now_ms)
        .min()
        .map(|reset| reset.min(evidence_deadline))
        .unwrap_or(evidence_deadline);
    Some(AccountUsageBlock {
        kind: AccountUsageBlockKind::QuotaExhausted,
        reason,
        scope,
        until_ms,
    })
}

fn quota_evidence_deadline(account: &Account, now_ms: i64) -> Option<i64> {
    let refreshed_at = account.quota_refreshed_at?;
    let deadline = account
        .quota_next_refresh_at
        .filter(|next| *next > refreshed_at)
        .map(|next| {
            let interval = next
                .saturating_sub(refreshed_at)
                .clamp(MIN_QUOTA_REFRESH_INTERVAL_MS, MAX_QUOTA_REFRESH_INTERVAL_MS);
            next.saturating_add(interval)
        })
        .unwrap_or_else(|| refreshed_at.saturating_add(DEFAULT_QUOTA_EVIDENCE_TTL_MS));
    (deadline > now_ms).then_some(deadline)
}

fn quota_tier_is_exhausted(tier: &AccountQuotaTier) -> bool {
    tier.utilization
        .filter(|value| value.is_finite())
        .is_some_and(|value| value >= 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertAccountInput {
    #[serde(default)]
    pub id: Option<String>,
    pub provider_type: ProviderType,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub extra_headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub raw: Option<Value>,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub entitlement_status: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub quota: Option<AccountQuota>,
    #[serde(default)]
    pub quota_refreshed_at: Option<i64>,
    #[serde(default)]
    pub quota_next_refresh_at: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub rate_limited_until: Option<i64>,
    #[serde(default)]
    pub last_refresh_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AccountRefreshUpdate {
    pub email: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub token_type: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub profile: Option<Value>,
    pub raw: Option<Value>,
    pub subscription_level: Option<String>,
    pub entitlement_status: Option<String>,
    pub quota_percent: Option<f64>,
    pub quota: Option<AccountQuota>,
    pub quota_refreshed_at: Option<i64>,
    pub quota_next_refresh_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub rate_limited_until: Option<i64>,
    pub last_refresh_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkspace {
    pub id: String,
    pub name: String,
}

const VERIFIED_OPENAI_CLAIMS_KEY: &str = "verifiedOpenAiClaims";
const CODEX_WORKSPACE_PROVENANCE_KEY: &str = "codexWorkspaceProvenance";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedCodexWorkspace {
    pub id: String,
    pub source: String,
}

pub fn set_verified_openai_claims(profile: &mut Option<Value>, claims: Option<Value>) {
    if !profile.as_ref().is_some_and(Value::is_object) {
        *profile = Some(Value::Object(Map::new()));
    }
    let object = profile
        .as_mut()
        .and_then(Value::as_object_mut)
        .expect("Codex profile was normalized to an object");
    object.remove(VERIFIED_OPENAI_CLAIMS_KEY);
    if let Some(claims) = claims {
        object.insert(VERIFIED_OPENAI_CLAIMS_KEY.to_string(), claims);
    }
}

pub fn clear_codex_workspace_provenance(profile: &mut Option<Value>) {
    if let Some(object) = profile.as_mut().and_then(Value::as_object_mut) {
        object.remove(CODEX_WORKSPACE_PROVENANCE_KEY);
    }
}

pub fn set_codex_workspace_provenance(
    profile: &mut Option<Value>,
    workspace_id: &str,
    source: &str,
    verified_at_ms: i64,
) {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return;
    }
    if !profile.as_ref().is_some_and(Value::is_object) {
        *profile = Some(Value::Object(Map::new()));
    }
    let object = profile
        .as_mut()
        .and_then(Value::as_object_mut)
        .expect("Codex profile was normalized to an object");
    object.insert(
        CODEX_WORKSPACE_PROVENANCE_KEY.to_string(),
        serde_json::json!({
            "workspaceId": workspace_id,
            "source": source,
            "verifiedAt": verified_at_ms,
        }),
    );
}

impl AccountStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = accounts_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read accounts {}", path.display()))?;
        let mut value: Value = serde_json::from_str(&content)
            .with_context(|| format!("parse accounts {}", path.display()))?;
        if account_store_has_encrypted_fields(&value) {
            let key = load_accounts_key(config_dir)?;
            decrypt_account_store_value(&mut value, &key)
                .with_context(|| format!("decrypt accounts {}", path.display()))?;
        }
        serde_json::from_value(value).with_context(|| format!("parse accounts {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = accounts_path(config_dir);
        let key = load_or_create_accounts_key(config_dir)?;
        let mut value = serde_json::to_value(self).context("serialize accounts")?;
        encrypt_account_store_value(&mut value, &key)
            .with_context(|| format!("encrypt accounts {}", path.display()))?;
        crate::infra::storage::write_json_pretty(&path, &value)
            .with_context(|| format!("write accounts {}", path.display()))
    }

    pub fn upsert(&mut self, input: UpsertAccountInput) -> Account {
        let mut account = Account {
            id: input.id.unwrap_or_else(generate_account_id),
            provider_type: input.provider_type,
            auth_identity_generation: initial_auth_identity_generation(),
            token_refresh_generation: 0,
            email: input.email,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            id_token: input.id_token,
            token_type: input.token_type,
            api_key: input.api_key,
            extra_headers: input.extra_headers.clone().unwrap_or_default(),
            scopes: input.scopes,
            profile: input.profile,
            raw: input.raw,
            subscription_level: input.subscription_level,
            entitlement_status: input.entitlement_status,
            quota_percent: input.quota_percent,
            quota: input.quota,
            quota_refreshed_at: input.quota_refreshed_at,
            quota_next_refresh_at: input.quota_next_refresh_at,
            expires_at: input.expires_at,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: input.rate_limited_until,
            last_refresh_error: input.last_refresh_error,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        };

        if let Some(existing) = self.accounts.iter_mut().find(|item| item.id == account.id) {
            let previous = existing.clone();
            use crate::domain::accounts::subscription_expiry::{
                subscription_expiry_capability, supports_manual_expiry,
            };
            if supports_manual_expiry(subscription_expiry_capability(existing.provider_type))
                && supports_manual_expiry(subscription_expiry_capability(account.provider_type))
            {
                account.manual_subscription_expires_at_ms =
                    existing.manual_subscription_expires_at_ms;
                account.manual_subscription_expiry_updated_at_ms =
                    existing.manual_subscription_expiry_updated_at_ms;
                account.manual_subscription_expiry_rule =
                    existing.manual_subscription_expiry_rule.clone();
            }
            if input.extra_headers.is_none() {
                account.extra_headers = existing.extra_headers.clone();
            }
            if account.provider_type == ProviderType::CodexOAuth {
                if let Some(profile) = account.profile.as_mut() {
                    preserve_codex_profile_selection(existing.profile.as_ref(), profile);
                }
            }
            advance_account_generations(&previous, &mut account);
            *existing = account.clone();
        } else {
            initialize_account_generations(&mut account);
            self.accounts.push(account.clone());
        }

        account
    }

    pub fn set_manual_subscription_expiry(
        &mut self,
        account_id: &str,
        expires_at_ms: Option<i64>,
        updated_at_ms: i64,
    ) -> Result<Account, ManualSubscriptionExpiryError> {
        use crate::domain::accounts::subscription_expiry::{
            subscription_expiry_capability, supports_manual_expiry,
        };

        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)
            .ok_or_else(|| ManualSubscriptionExpiryError::NotFound(account_id.to_string()))?;
        if !supports_manual_expiry(subscription_expiry_capability(account.provider_type)) {
            return Err(ManualSubscriptionExpiryError::Unsupported(
                account.provider_type,
            ));
        }
        if expires_at_ms.is_some_and(|value| value <= 0) || updated_at_ms < 0 {
            return Err(ManualSubscriptionExpiryError::InvalidTimestamp);
        }

        account.manual_subscription_expires_at_ms = expires_at_ms;
        account.manual_subscription_expiry_updated_at_ms = Some(updated_at_ms);
        account.manual_subscription_expiry_rule = None;
        Ok(account.clone())
    }

    pub fn set_subscription_expiry_rule(
        &mut self,
        account_id: &str,
        draft: Option<SubscriptionExpiryRuleDraft>,
        updated_at_ms: i64,
    ) -> Result<Account, ManualSubscriptionExpiryError> {
        use crate::domain::accounts::subscription_expiry::{
            subscription_expiry_capability, supports_manual_expiry,
        };

        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)
            .ok_or_else(|| ManualSubscriptionExpiryError::NotFound(account_id.to_string()))?;
        if !supports_manual_expiry(subscription_expiry_capability(account.provider_type)) {
            return Err(ManualSubscriptionExpiryError::Unsupported(
                account.provider_type,
            ));
        }
        let rule = draft
            .map(|draft| draft.into_rule(updated_at_ms))
            .transpose()
            .map_err(|error| ManualSubscriptionExpiryError::InvalidRule(error.to_string()))?;

        account.manual_subscription_expiry_rule = rule;
        account.manual_subscription_expires_at_ms = None;
        account.manual_subscription_expiry_updated_at_ms = None;
        Ok(account.clone())
    }

    pub fn find_for_provider(
        &self,
        provider_type: ProviderType,
        account_id: Option<&str>,
    ) -> Option<&Account> {
        if let Some(account_id) = account_id {
            return self
                .accounts
                .iter()
                .find(|item| item.id == account_id && item.provider_type == provider_type);
        }

        self.accounts
            .iter()
            .find(|item| item.provider_type == provider_type)
    }

    pub fn codex_account_id_for_verified_subject(&self, subject: &str) -> Option<&str> {
        let subject = subject.trim();
        if subject.is_empty() {
            return None;
        }
        self.accounts
            .iter()
            .find(|account| {
                account.provider_type == ProviderType::CodexOAuth
                    && account
                        .profile
                        .as_ref()
                        .and_then(|profile| profile.pointer("/verifiedOpenAiClaims/subject"))
                        .and_then(Value::as_str)
                        .is_some_and(|candidate| candidate.trim() == subject)
            })
            .map(|account| account.id.as_str())
    }

    pub fn refresh_token_owner(
        &self,
        provider_type: ProviderType,
        refresh_token: &str,
        except_account_id: Option<&str>,
    ) -> Option<&Account> {
        let fingerprint = refresh_token_fingerprint(refresh_token)?;
        self.accounts.iter().find(|account| {
            account.provider_type == provider_type
                && except_account_id != Some(account.id.as_str())
                && account
                    .refresh_token
                    .as_deref()
                    .and_then(refresh_token_fingerprint)
                    .is_some_and(|candidate| candidate == fingerprint)
        })
    }

    pub fn select_codex_workspace(
        &mut self,
        account_id: &str,
        workspace_id: &str,
    ) -> Result<Account, String> {
        let account = self
            .accounts
            .iter_mut()
            .find(|account| {
                account.id == account_id && account.provider_type == ProviderType::CodexOAuth
            })
            .ok_or_else(|| "codex account not found".to_string())?;
        let previous = account.clone();
        let workspace_id = workspace_id.trim();
        let options = codex_workspace_options(account);
        let selected = options
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or_else(|| {
                "workspace is not present in the verified OpenAI account claims".to_string()
            })?;
        let selection_changed =
            effective_codex_workspace_id(account).as_deref() != Some(selected.id.as_str());
        let mut profile = account
            .profile
            .take()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new()));
        if let Some(object) = profile.as_object_mut() {
            object.insert(
                "selectedChatgptAccountId".to_string(),
                Value::String(selected.id.clone()),
            );
            object.insert(
                "selectedWorkspace".to_string(),
                serde_json::to_value(selected).unwrap_or(Value::Null),
            );
        }
        account.profile = Some(profile);
        if selection_changed {
            // Codex quota and reset-credit snapshots are scoped by
            // ChatGPT-Account-Id. Never carry a workspace A snapshot into
            // workspace B while the fresh details request is unavailable.
            account.subscription_level = None;
            account.entitlement_status = None;
            account.quota_percent = None;
            account.quota = None;
            account.quota_refreshed_at = None;
            account.quota_next_refresh_at = None;
            account.rate_limited_until = None;
            account.last_refresh_error = None;
            if let Some(raw) = account.raw.as_mut().and_then(Value::as_object_mut) {
                for key in [
                    "bankedReset",
                    "banked_reset",
                    "codexBankedReset",
                    "codex_banked_reset",
                    "rateLimitResetCredits",
                    "rate_limit_reset_credits",
                ] {
                    raw.remove(key);
                }
            }
        }
        advance_account_generations(&previous, account);
        Ok(account.clone())
    }

    pub fn delete(&mut self, account_id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|item| item.id != account_id);
        self.accounts.len() != before
    }

    pub fn refresh_status(&mut self, account_id: &str, now_ms: i64) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        let previous = account.clone();
        if account
            .expires_at
            .is_some_and(|expires_at| expires_at <= now_ms)
        {
            account.access_token = None;
            account.last_refresh_error = Some(
                if oauth_provider_spec(account.provider_type)
                    .is_some_and(|spec| spec.server_native_refresh_enabled())
                {
                    "access token expired; refreshToken is required for server-native refresh"
                } else {
                    "access token expired; provider refresh flow is not enabled"
                }
                .to_string(),
            );
        }
        advance_account_generations(&previous, account);
        Some(account.clone())
    }

    pub fn mark_refresh_success(
        &mut self,
        account_id: &str,
        update: AccountRefreshUpdate,
    ) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        let previous = account.clone();
        if let Some(value) = update.email {
            account.email = Some(value);
        }
        if let Some(value) = update.access_token {
            account.access_token = Some(value);
        }
        if let Some(value) = update.refresh_token {
            account.refresh_token = Some(value);
        }
        if let Some(value) = update.id_token {
            account.id_token = Some(value);
        }
        if let Some(value) = update.token_type {
            account.token_type = Some(value);
        }
        if let Some(value) = update.scopes {
            account.scopes = value;
        }
        if let Some(mut value) = update.profile {
            if account.provider_type == ProviderType::CodexOAuth {
                preserve_codex_profile_state(account.profile.as_ref(), &mut value);
            }
            account.profile = Some(value);
        }
        if let Some(value) = update.raw {
            account.raw = Some(value);
        }
        if let Some(value) = update.subscription_level {
            account.subscription_level = Some(value);
        }
        if let Some(value) = update.entitlement_status {
            account.entitlement_status = Some(value);
        }
        if let Some(value) = update.quota_percent {
            account.quota_percent = Some(value);
        }
        if let Some(value) = update.quota {
            account.quota = Some(value);
        }
        if let Some(value) = update.quota_refreshed_at {
            account.quota_refreshed_at = Some(value);
        }
        if let Some(value) = update.quota_next_refresh_at {
            account.quota_next_refresh_at = Some(value);
        }
        if let Some(value) = update.expires_at {
            account.expires_at = Some(value);
        }
        if let Some(value) = update.rate_limited_until {
            account.rate_limited_until = Some(value);
        }
        account.last_refresh_error = update.last_refresh_error;
        advance_account_generations(&previous, account);
        Some(account.clone())
    }

    pub fn mark_rate_limited_until(
        &mut self,
        account_id: &str,
        rate_limited_until: i64,
    ) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        let extends_cooldown = account
            .rate_limited_until
            .is_none_or(|current| rate_limited_until >= current);
        if extends_cooldown {
            account.rate_limited_until = Some(rate_limited_until);
        }
        Some(account.clone())
    }

    pub fn update_entitlement_snapshot(
        &mut self,
        account_id: &str,
        subscription_level: Option<String>,
        entitlement_status: Option<String>,
        updated_at_ms: i64,
    ) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        if let Some(value) = subscription_level.as_ref() {
            account.subscription_level = Some(value.clone());
        }
        if let Some(value) = entitlement_status.as_ref() {
            account.entitlement_status = Some(value.clone());
        }
        if subscription_level.is_some() || entitlement_status.is_some() {
            let mut profile = account
                .profile
                .take()
                .filter(Value::is_object)
                .unwrap_or_else(|| Value::Object(Map::new()));
            if let Some(object) = profile.as_object_mut() {
                if let Some(value) = subscription_level {
                    object.insert("tier".to_string(), Value::String(value.clone()));
                    object.insert("subscriptionTier".to_string(), Value::String(value));
                }
                if let Some(value) = entitlement_status {
                    object.insert(
                        "entitlementStatus".to_string(),
                        Value::String(value.clone()),
                    );
                    object.insert("entitlement_status".to_string(), Value::String(value));
                }
                object.insert(
                    "entitlementUpdatedAtMs".to_string(),
                    Value::Number(updated_at_ms.into()),
                );
            }
            account.profile = Some(profile);
        }
        Some(account.clone())
    }

    pub fn mark_refresh_failure(&mut self, account_id: &str, error: String) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        account.last_refresh_error = Some(error);
        Some(account.clone())
    }

    pub fn mark_native_refresh_success(
        &mut self,
        account_id: &str,
        update: AccountRefreshUpdate,
    ) -> Option<Account> {
        self.mark_refresh_success(account_id, update)?;
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        account.refresh_consecutive_failures = 0;
        account.needs_relogin = false;
        Some(account.clone())
    }

    pub fn mark_native_refresh_failure(
        &mut self,
        account_id: &str,
        error: String,
        kind: OAuthErrorKind,
    ) -> Option<Account> {
        self.mark_native_refresh_failure_with_threshold(
            account_id,
            error,
            kind,
            native_refresh_failure_threshold(),
        )
    }

    fn mark_native_refresh_failure_with_threshold(
        &mut self,
        account_id: &str,
        error: String,
        kind: OAuthErrorKind,
        threshold: u32,
    ) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        account.last_refresh_error = Some(error);
        if kind == OAuthErrorKind::InvalidGrant {
            account.refresh_consecutive_failures =
                account.refresh_consecutive_failures.saturating_add(1);
            if invalid_grant_requires_immediate_relogin(
                account.last_refresh_error.as_deref().unwrap_or_default(),
            ) || account.refresh_consecutive_failures >= threshold.max(1)
            {
                account.needs_relogin = true;
            }
        }
        Some(account.clone())
    }
}

const fn initial_auth_identity_generation() -> u64 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountAuthIdentitySnapshot {
    provider_type: ProviderType,
    principal: Option<AccountPrincipal>,
    fallback_email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountPrincipal {
    kind: &'static str,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountTokenSnapshot {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    api_key: Option<String>,
    extra_headers: BTreeMap<String, String>,
    expires_at: Option<i64>,
    nested_secret_fingerprint: Option<[u8; 32]>,
}

impl AccountTokenSnapshot {
    fn has_material(&self) -> bool {
        self.access_token.is_some()
            || self.refresh_token.is_some()
            || self.id_token.is_some()
            || self.api_key.is_some()
            || !self.extra_headers.is_empty()
            || self.nested_secret_fingerprint.is_some()
    }
}

fn initialize_account_generations(account: &mut Account) {
    account.auth_identity_generation = initial_auth_identity_generation();
    account.token_refresh_generation = u64::from(account_token_snapshot(account).has_material());
}

fn advance_account_generations(previous: &Account, current: &mut Account) {
    let previous_identity = account_auth_identity_snapshot(previous);
    let current_identity = account_auth_identity_snapshot(current);
    let previous_token = account_token_snapshot(previous);
    let current_token = account_token_snapshot(current);

    current.auth_identity_generation = previous.auth_identity_generation.max(1);
    current.token_refresh_generation = previous.token_refresh_generation;
    if account_auth_identity_changed(&previous_identity, &current_identity) {
        current.auth_identity_generation = current.auth_identity_generation.saturating_add(1);
    }
    if previous_token != current_token {
        current.token_refresh_generation = current.token_refresh_generation.saturating_add(1);
    }
}

fn account_auth_identity_snapshot(account: &Account) -> AccountAuthIdentitySnapshot {
    AccountAuthIdentitySnapshot {
        provider_type: account.provider_type,
        principal: strongest_account_principal(account),
        fallback_email: normalized_identity_email(account.email.as_deref()),
    }
}

fn account_auth_identity_changed(
    previous: &AccountAuthIdentitySnapshot,
    current: &AccountAuthIdentitySnapshot,
) -> bool {
    if previous.provider_type != current.provider_type {
        return true;
    }
    match (&previous.principal, &current.principal) {
        (Some(previous), Some(current)) => previous != current,
        (None, None) => previous.fallback_email != current.fallback_email,
        (None, Some(_)) => matches!(
            (&previous.fallback_email, &current.fallback_email),
            (Some(previous), Some(current)) if previous != current
        ),
        (Some(_), None) => true,
    }
}

fn strongest_account_principal(account: &Account) -> Option<AccountPrincipal> {
    if account.provider_type == ProviderType::CodexOAuth {
        if let Some(value) = account_principal_value(account, &["/verifiedOpenAiClaims/subject"]) {
            return Some(AccountPrincipal {
                kind: "openai_subject",
                value,
            });
        }
    }
    if account.provider_type == ProviderType::ClaudeOAuth {
        const CLAUDE_ACCOUNT_UUID_POINTERS: &[&str] = &[
            "/accountUUID",
            "/account_uuid",
            "/account/uuid",
            "/oauth_account/account_uuid",
            "/token/account/uuid",
            "/token/accountUUID",
            "/profile/accountUUID",
            "/profile/account_uuid",
            "/profileRaw/accountUUID",
            "/profileRaw/account_uuid",
            "/profileRaw/account/uuid",
            "/raw/account/uuid",
        ];
        if let Some(value) = account_principal_value(account, CLAUDE_ACCOUNT_UUID_POINTERS) {
            return Some(AccountPrincipal {
                kind: "claude_account_uuid",
                value,
            });
        }
    }

    const PRINCIPAL_FIELDS: &[(&str, &[&str])] = &[
        ("subject", &["/sub"]),
        ("user_id", &["/userId", "/user_id"]),
        ("account_id", &["/accountId", "/account_id"]),
        ("profile_arn", &["/profileArn"]),
        ("login", &["/login"]),
        ("username", &["/username"]),
        (
            "organization_id",
            &[
                "/organizationId",
                "/organization_id",
                "/organizationUUID",
                "/organization_uuid",
                "/organization/uuid",
            ],
        ),
    ];
    for (kind, pointers) in PRINCIPAL_FIELDS {
        if let Some(value) = account_principal_value(account, pointers) {
            if account.provider_type == ProviderType::ClaudeOAuth
                && *kind == "account_id"
                && normalized_identity_email(account.email.as_deref())
                    .is_some_and(|email| email.eq_ignore_ascii_case(&value))
            {
                continue;
            }
            return Some(AccountPrincipal { kind, value });
        }
    }
    if account.provider_type == ProviderType::CodexOAuth {
        if let Some(value) = effective_codex_workspace_id(account) {
            return Some(AccountPrincipal {
                kind: "codex_workspace_id",
                value,
            });
        }
    }
    None
}

fn account_principal_value(account: &Account, pointers: &[&str]) -> Option<String> {
    let sources = [account.profile.as_ref(), account.raw.as_ref()];
    pointers.iter().find_map(|pointer| {
        sources
            .iter()
            .filter_map(|source| *source)
            .find_map(|source| source.pointer(pointer).and_then(identity_marker_value))
    })
}

fn normalized_identity_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn identity_marker_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()).filter(|value| !value.is_empty()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn account_token_snapshot(account: &Account) -> AccountTokenSnapshot {
    AccountTokenSnapshot {
        access_token: normalized_secret(account.access_token.as_deref()),
        refresh_token: normalized_secret(account.refresh_token.as_deref()),
        id_token: normalized_secret(account.id_token.as_deref()),
        token_type: account
            .token_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
        api_key: normalized_secret(account.api_key.as_deref()),
        extra_headers: account
            .extra_headers
            .iter()
            .filter_map(|(name, value)| {
                let value = value.trim();
                (!value.is_empty()).then(|| (name.trim().to_ascii_lowercase(), value.to_string()))
            })
            .collect(),
        expires_at: account.expires_at,
        nested_secret_fingerprint: account.raw.as_ref().and_then(nested_secret_fingerprint),
    }
}

fn normalized_secret(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn nested_secret_fingerprint(value: &Value) -> Option<[u8; 32]> {
    let mut secrets = Vec::new();
    collect_nested_secrets(value, "", &mut secrets);
    if secrets.is_empty() {
        return None;
    }
    secrets.sort();
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-account-nested-secret-v1\0");
    for (path, secret) in secrets {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(secret.as_bytes());
        digest.update([0]);
    }
    Some(digest.finalize().into())
}

fn collect_nested_secrets(value: &Value, path: &str, secrets: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            let mut fields = object.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.cmp(right.0));
            for (field, value) in fields {
                let next_path = format!("{path}/{field}");
                if account_extra_headers_field(field) {
                    if let Some(headers) = value.as_object() {
                        for (name, value) in headers {
                            if let Some(secret) = value.as_str().and_then(|value| {
                                let value = value.trim();
                                (!value.is_empty()).then_some(value)
                            }) {
                                secrets.push((format!("{next_path}/{name}"), secret.to_string()));
                            }
                        }
                    }
                } else if account_secret_field(field) {
                    if let Some(secret) = value.as_str().and_then(|value| {
                        let value = value.trim();
                        (!value.is_empty()).then_some(value)
                    }) {
                        secrets.push((next_path, secret.to_string()));
                    } else {
                        collect_nested_secrets(value, &next_path, secrets);
                    }
                } else {
                    collect_nested_secrets(value, &next_path, secrets);
                }
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_nested_secrets(value, &format!("{path}/{index}"), secrets);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualSubscriptionExpiryError {
    NotFound(String),
    Unsupported(ProviderType),
    InvalidTimestamp,
    InvalidRule(String),
}

impl std::fmt::Display for ManualSubscriptionExpiryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(account_id) => write!(formatter, "account not found: {account_id}"),
            Self::Unsupported(provider_type) => write!(
                formatter,
                "manual subscription expiry is not supported for {} accounts",
                provider_type.as_str()
            ),
            Self::InvalidTimestamp => {
                formatter.write_str("subscription expiry timestamp must be after the Unix epoch")
            }
            Self::InvalidRule(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ManualSubscriptionExpiryError {}

pub fn selected_codex_workspace_id(account: &Account) -> Option<String> {
    let selected = account
        .profile
        .as_ref()
        .and_then(|value| value.get("selectedChatgptAccountId"))
        .or_else(|| {
            account
                .raw
                .as_ref()
                .and_then(|value| value.get("selectedChatgptAccountId"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)?;
    codex_workspace_options(account)
        .iter()
        .any(|workspace| workspace.id == selected)
        .then_some(selected)
}

pub fn effective_codex_workspace_id(account: &Account) -> Option<String> {
    trusted_codex_workspace(account).map(|workspace| workspace.id)
}

pub fn trusted_codex_workspace(account: &Account) -> Option<TrustedCodexWorkspace> {
    selected_codex_workspace_id(account)
        .map(|id| TrustedCodexWorkspace {
            id,
            source: "user_selected".to_string(),
        })
        .or_else(|| {
            let default_id = account
                .profile
                .as_ref()
                .and_then(|profile| profile.get(VERIFIED_OPENAI_CLAIMS_KEY))
                .and_then(codex_account_id_from_value)?;
            codex_workspace_options(account)
                .iter()
                .any(|workspace| workspace.id == default_id)
                .then_some(TrustedCodexWorkspace {
                    id: default_id,
                    source: "verified_openai_token".to_string(),
                })
        })
        .or_else(|| {
            let provenance = account
                .profile
                .as_ref()?
                .get(CODEX_WORKSPACE_PROVENANCE_KEY)?;
            let id = provenance
                .get("workspaceId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let source = provenance
                .get("source")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("authenticated_discovery")
                .to_string();
            Some(TrustedCodexWorkspace { id, source })
        })
}

pub fn codex_workspace_options(account: &Account) -> Vec<CodexWorkspace> {
    let mut workspaces = std::collections::BTreeMap::<String, String>::new();
    if let Some(provenance) = account
        .profile
        .as_ref()
        .and_then(|profile| profile.get(CODEX_WORKSPACE_PROVENANCE_KEY))
    {
        if let Some(id) = provenance
            .get("workspaceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            workspaces.insert(id.to_string(), id.to_string());
        }
    }
    if let Some(value) = account
        .profile
        .as_ref()
        .and_then(|profile| profile.get(VERIFIED_OPENAI_CLAIMS_KEY))
    {
        if let Some(id) = codex_account_id_from_value(value) {
            workspaces.entry(id.clone()).or_insert(id);
        }
        for organizations in [
            value.get("organizations"),
            value
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("organizations")),
            value
                .get("openai_auth")
                .and_then(|auth| auth.get("organizations")),
            value
                .get("openaiAuth")
                .and_then(|auth| auth.get("organizations")),
        ] {
            if let Some(items) = organizations.and_then(Value::as_array) {
                for item in items {
                    let Some(id) = [
                        "id",
                        "account_id",
                        "accountId",
                        "organization_id",
                        "organizationId",
                    ]
                    .into_iter()
                    .find_map(|key| item.get(key).and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|id| !id.is_empty()) else {
                        continue;
                    };
                    let name = ["name", "title", "display_name", "displayName"]
                        .into_iter()
                        .find_map(|key| item.get(key).and_then(Value::as_str))
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(id);
                    workspaces
                        .entry(id.to_string())
                        .or_insert_with(|| name.to_string());
                }
            }
        }
    }
    workspaces
        .into_iter()
        .map(|(id, name)| CodexWorkspace { id, name })
        .collect()
}

fn preserve_codex_profile_state(existing: Option<&Value>, incoming: &mut Value) {
    let (Some(existing), Some(incoming)) = (
        existing.and_then(Value::as_object),
        incoming.as_object_mut(),
    ) else {
        return;
    };
    for key in [
        VERIFIED_OPENAI_CLAIMS_KEY,
        CODEX_WORKSPACE_PROVENANCE_KEY,
        "selectedChatgptAccountId",
        "selectedWorkspace",
    ] {
        if !incoming.contains_key(key) {
            if let Some(value) = existing.get(key) {
                incoming.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn preserve_codex_profile_selection(existing: Option<&Value>, incoming: &mut Value) {
    let (Some(existing), Some(incoming)) = (
        existing.and_then(Value::as_object),
        incoming.as_object_mut(),
    ) else {
        return;
    };
    for key in ["selectedChatgptAccountId", "selectedWorkspace"] {
        if !incoming.contains_key(key) {
            if let Some(value) = existing.get(key) {
                incoming.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn codex_account_id_from_value(value: &Value) -> Option<String> {
    value
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .or_else(|| value.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| value.get("chatgptAccountId").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("openai_auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("openaiAuth")
                .and_then(|auth| auth.get("chatgptAccountId"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn refresh_token_fingerprint(refresh_token: &str) -> Option<[u8; 32]> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return None;
    }
    Some(Sha256::digest(refresh_token.as_bytes()).into())
}

fn invalid_grant_requires_immediate_relogin(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "refresh_token_reused",
        "refresh token reused",
        "refresh token already used",
        "refresh token has already been used",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

fn native_refresh_failure_threshold() -> u32 {
    std::env::var("CC_SWITCH_REFRESH_FAILURES_BEFORE_RELOGIN")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(20)
}

pub fn accounts_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(ACCOUNTS_FILE_NAME)
}

pub fn accounts_key_path(config_dir: &Path) -> std::path::PathBuf {
    crate::infra::credentials::root_key_path(config_dir)
}

fn load_or_create_accounts_key(config_dir: &Path) -> anyhow::Result<[u8; 32]> {
    Ok(crate::infra::credentials::load_or_create_root_key(config_dir)?.key)
}

fn load_accounts_key(config_dir: &Path) -> anyhow::Result<[u8; 32]> {
    crate::infra::credentials::load_root_key(config_dir)
        .map(|resolved| resolved.key)
        .with_context(|| {
            format!(
                "accounts encryption key is required to read encrypted accounts: {}",
                accounts_key_path(config_dir).display()
            )
        })
}

fn account_store_has_encrypted_fields(value: &Value) -> bool {
    value
        .get("accounts")
        .and_then(Value::as_array)
        .is_some_and(|accounts| accounts.iter().any(value_has_encrypted_secret))
}

fn value_has_encrypted_secret(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(field, value)| {
            (account_secret_field(field)
                && value
                    .as_str()
                    .is_some_and(|value| value.starts_with(ENCRYPTED_PREFIX)))
                || (account_extra_headers_field(field)
                    && extra_headers_have_encrypted_secret(value))
                || value_has_encrypted_secret(value)
        }),
        Value::Array(values) => values.iter().any(value_has_encrypted_secret),
        _ => false,
    }
}

fn extra_headers_have_encrypted_secret(value: &Value) -> bool {
    value.as_object().is_some_and(|headers| {
        headers.values().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.starts_with(ENCRYPTED_PREFIX))
        })
    })
}

fn encrypt_account_store_value(value: &mut Value, key: &[u8; 32]) -> anyhow::Result<()> {
    transform_account_secret_fields(value, |plain| encrypt_secret(plain, key))
}

fn decrypt_account_store_value(value: &mut Value, key: &[u8; 32]) -> anyhow::Result<()> {
    transform_account_secret_fields(value, |cipher| decrypt_secret(cipher, key))
}

fn transform_account_secret_fields(
    value: &mut Value,
    transform: impl Fn(&str) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let Some(accounts) = value.get_mut("accounts").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for account in accounts {
        transform_secret_fields(account, &transform)?;
    }
    Ok(())
}

fn transform_secret_fields(
    value: &mut Value,
    transform: &impl Fn(&str) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    match value {
        Value::Object(object) => {
            for (field, value) in object {
                if account_extra_headers_field(field) {
                    transform_extra_header_values(value, transform)?;
                } else if account_secret_field(field) {
                    if let Value::String(secret) = value {
                        if !secret.trim().is_empty() {
                            *secret = transform(secret)?;
                        }
                    } else {
                        transform_secret_fields(value, transform)?;
                    }
                } else {
                    transform_secret_fields(value, transform)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                transform_secret_fields(value, transform)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn account_extra_headers_field(field: &str) -> bool {
    compact_account_field(field) == "extraheaders"
}

fn account_secret_field(field: &str) -> bool {
    let compact = compact_account_field(field);
    matches!(
        compact.as_str(),
        "token"
            | "key"
            | "secret"
            | "authorization"
            | "proxyauthorization"
            | "cookie"
            | "password"
            | "sessiontoken"
            | "githubtoken"
            | "copilottoken"
            | "devicecode"
            | "usercode"
            | "codeverifier"
            | "authorizationcode"
            | "clientassertion"
    ) || [
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "apikey",
        "clientsecret",
        "kiroapikey",
        "secretaccesskey",
        "privatekey",
        "signingkey",
    ]
    .iter()
    .any(|suffix| compact.ends_with(suffix))
}

fn compact_account_field(field: &str) -> String {
    field
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn transform_extra_header_values(
    value: &mut Value,
    transform: &impl Fn(&str) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let Value::Object(headers) = value else {
        return Ok(());
    };
    for value in headers.values_mut() {
        if let Value::String(secret) = value {
            if !secret.trim().is_empty() {
                *secret = transform(secret)?;
            }
        } else {
            transform_secret_fields(value, transform)?;
        }
    }
    Ok(())
}

fn encrypt_secret(plain: &str, key: &[u8; 32]) -> anyhow::Result<String> {
    if plain.starts_with(ENCRYPTED_PREFIX) {
        return Ok(plain.to_string());
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plain.as_bytes())
        .map_err(|_| anyhow::anyhow!("encrypt account secret"))?;
    Ok(format!(
        "{ENCRYPTED_PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(ciphertext)
    ))
}

fn decrypt_secret(ciphertext: &str, key: &[u8; 32]) -> anyhow::Result<String> {
    let Some(encoded) = ciphertext.strip_prefix(ENCRYPTED_PREFIX) else {
        return Ok(ciphertext.to_string());
    };
    let (nonce, body) = encoded
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("invalid encrypted account secret"))?;
    let nonce = URL_SAFE_NO_PAD.decode(nonce).context("decode nonce")?;
    let body = URL_SAFE_NO_PAD.decode(body).context("decode ciphertext")?;
    let nonce: [u8; 24] = nonce
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid encrypted account secret nonce"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let plain = cipher
        .decrypt(XNonce::from_slice(&nonce), body.as_ref())
        .map_err(|_| anyhow::anyhow!("decrypt account secret"))?;
    String::from_utf8(plain).context("account secret is not utf-8")
}

fn generate_account_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("acct-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_input(provider_type: ProviderType) -> UpsertAccountInput {
        UpsertAccountInput {
            id: None,
            provider_type,
            email: None,
            access_token: None,
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
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        }
    }

    fn quota_account(
        provider_type: ProviderType,
        now_ms: i64,
        extra_usage: Value,
        tiers: Vec<AccountQuotaTier>,
    ) -> Account {
        let mut input = fixture_input(provider_type);
        input.id = Some("quota-account".to_string());
        input.quota = Some(AccountQuota {
            success: true,
            credential_message: None,
            tiers,
            extra_usage: Some(extra_usage),
        });
        input.quota_percent = Some(100.0);
        input.quota_refreshed_at = Some(now_ms - 5 * 60 * 1000);
        input.quota_next_refresh_at = Some(now_ms + 25 * 60 * 1000);
        AccountStore::default().upsert(input)
    }

    #[test]
    fn aggregate_quota_percent_does_not_create_a_usage_block() {
        let now_ms = 1_000_000_000;
        let account = quota_account(
            ProviderType::ClaudeOAuth,
            now_ms,
            json!({"raw": {}}),
            vec![AccountQuotaTier {
                name: "five_hour".to_string(),
                utilization: Some(1.0),
                resets_at: Some(now_ms + 60 * 60 * 1000),
                ..Default::default()
            }],
        );

        assert!(active_account_usage_block(&account, now_ms).is_none());
    }

    #[test]
    fn fresh_explicit_codex_limit_creates_a_bounded_usage_block() {
        let now_ms = 1_000_000_000;
        let account = quota_account(
            ProviderType::CodexOAuth,
            now_ms,
            json!({
                "subscriptionEvidence": {
                    "usageAllowed": false,
                    "usageLimitReached": true
                }
            }),
            vec![AccountQuotaTier {
                name: "seven_day".to_string(),
                utilization: Some(1.0),
                resets_at: Some(now_ms + 7 * 24 * 60 * 60 * 1000),
                ..Default::default()
            }],
        );

        let block = active_account_usage_block(&account, now_ms).expect("active block");
        assert_eq!(block.kind, AccountUsageBlockKind::QuotaExhausted);
        assert_eq!(block.scope, "codex_account");
        assert_eq!(block.until_ms, now_ms + 55 * 60 * 1000);
    }

    #[test]
    fn stale_or_failed_quota_evidence_does_not_block() {
        let now_ms = 1_000_000_000;
        let mut account = quota_account(
            ProviderType::GrokOAuth,
            now_ms,
            json!({"spendingLimitReached": true}),
            Vec::new(),
        );
        let stale_at = now_ms + 56 * 60 * 1000;
        assert!(active_account_usage_block(&account, stale_at).is_none());

        account.last_refresh_error = Some("quota refresh failed".to_string());
        assert!(active_account_usage_block(&account, now_ms).is_none());
    }

    #[test]
    fn active_rate_limit_uses_its_authoritative_expiry() {
        let now_ms = 1_000_000_000;
        let mut input = fixture_input(ProviderType::CursorApiKey);
        input.rate_limited_until = Some(now_ms + 90_000);
        let account = AccountStore::default().upsert(input);

        let block = active_account_usage_block(&account, now_ms).expect("active block");
        assert_eq!(block.kind, AccountUsageBlockKind::RateLimited);
        assert_eq!(block.until_ms, now_ms + 90_000);
        assert!(active_account_usage_block(&account, now_ms + 90_000).is_none());
    }

    #[test]
    fn repeated_rate_limit_markers_never_shorten_an_active_cooldown() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::KiroOAuth);
        input.id = Some("rate-limited-account".to_string());
        store.upsert(input);

        store.mark_rate_limited_until("rate-limited-account", 1_000_000);
        let account = store
            .mark_rate_limited_until("rate-limited-account", 100_000)
            .expect("account");

        assert_eq!(account.rate_limited_until, Some(1_000_000));
        assert!(account.last_refresh_error.is_none());
    }

    #[test]
    fn expired_rate_limit_does_not_hide_fresh_explicit_quota_exhaustion() {
        let now_ms = 1_000_000_000;
        let account = quota_account(
            ProviderType::CodexOAuth,
            now_ms,
            json!({
                "subscriptionEvidence": {
                    "usageAllowed": false,
                    "usageLimitReached": true
                }
            }),
            vec![AccountQuotaTier {
                name: "seven_day".to_string(),
                utilization: Some(1.0),
                resets_at: Some(now_ms + 7 * 24 * 60 * 60 * 1000),
                ..Default::default()
            }],
        );
        let mut store = AccountStore {
            accounts: vec![account],
        };
        store.mark_rate_limited_until("quota-account", now_ms + 60_000);

        let account = store
            .accounts
            .iter()
            .find(|account| account.id == "quota-account")
            .expect("account");
        let block = active_account_usage_block(account, now_ms + 90_000).expect("quota block");
        assert_eq!(block.kind, AccountUsageBlockKind::QuotaExhausted);
        assert!(account.last_refresh_error.is_none());
    }

    #[test]
    fn kiro_exhaustion_only_blocks_when_overage_is_disabled() {
        let now_ms = 1_000_000_000;
        let tier = AccountQuotaTier {
            name: "kiro_agentic_requests".to_string(),
            utilization: Some(1.0),
            resets_at: Some(now_ms + 20 * 60 * 1000),
            ..Default::default()
        };
        let overage = quota_account(
            ProviderType::KiroOAuth,
            now_ms,
            json!({"overageEnabled": true}),
            vec![tier.clone()],
        );
        let capped = quota_account(
            ProviderType::KiroOAuth,
            now_ms,
            json!({"overageEnabled": false}),
            vec![tier],
        );

        assert!(active_account_usage_block(&overage, now_ms).is_none());
        assert!(active_account_usage_block(&capped, now_ms).is_some());
    }

    #[test]
    fn upserts_and_finds_account_by_provider_type() {
        let mut store = AccountStore::default();
        let account = store.upsert(UpsertAccountInput {
            id: Some("a1".to_string()),
            provider_type: ProviderType::ClaudeOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("token".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: vec!["openid".to_string()],
            profile: None,
            raw: None,
            subscription_level: Some("pro".to_string()),
            entitlement_status: None,
            quota: None,
            quota_percent: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        });

        assert_eq!(account.id, "a1");
        assert_eq!(
            store
                .find_for_provider(ProviderType::ClaudeOAuth, None)
                .unwrap()
                .access_token
                .as_deref(),
            Some("token")
        );
        assert_eq!(account.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(account.scopes, vec!["openid"]);
        assert!(store
            .find_for_provider(ProviderType::CodexOAuth, Some("a1"))
            .is_none());
    }

    #[test]
    fn upsert_preserves_manual_subscription_expiry_without_exposing_import_fields() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("claude-account".to_string());
        store.upsert(input);
        store
            .set_manual_subscription_expiry(
                "claude-account",
                Some(1_786_924_800_000),
                1_784_000_000_000,
            )
            .unwrap();
        let refreshed = store
            .mark_refresh_success(
                "claude-account",
                AccountRefreshUpdate {
                    access_token: Some("refreshed".to_string()),
                    quota: Some(AccountQuota {
                        success: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            refreshed.manual_subscription_expires_at_ms,
            Some(1_786_924_800_000)
        );

        let imported: UpsertAccountInput = serde_json::from_value(json!({
            "id": "claude-account",
            "providerType": "claude_oauth",
            "accessToken": "replacement",
            "manualSubscriptionExpiresAtMs": 1,
            "manualSubscriptionExpiryUpdatedAtMs": 2
        }))
        .unwrap();
        let updated = store.upsert(imported);

        assert_eq!(updated.access_token.as_deref(), Some("replacement"));
        assert_eq!(
            updated.manual_subscription_expires_at_ms,
            Some(1_786_924_800_000)
        );
        assert_eq!(
            updated.manual_subscription_expiry_updated_at_ms,
            Some(1_784_000_000_000)
        );

        let persisted = serde_json::to_value(&store).unwrap();
        assert_eq!(
            persisted.pointer("/accounts/0/manualSubscriptionExpiresAtMs"),
            Some(&json!(1_786_924_800_000_i64))
        );
        let reloaded: AccountStore = serde_json::from_value(persisted).unwrap();
        assert_eq!(
            reloaded.accounts[0].manual_subscription_expires_at_ms,
            Some(1_786_924_800_000)
        );
        assert_eq!(
            reloaded.accounts[0].manual_subscription_expiry_updated_at_ms,
            Some(1_784_000_000_000)
        );

        let mut changed_type = fixture_input(ProviderType::CodexOAuth);
        changed_type.id = Some("claude-account".to_string());
        let changed_type = store.upsert(changed_type);
        assert_eq!(changed_type.manual_subscription_expires_at_ms, None);
        assert_eq!(changed_type.manual_subscription_expiry_updated_at_ms, None);
    }

    #[test]
    fn old_account_json_defaults_manual_subscription_expiry_fields() {
        let store: AccountStore = serde_json::from_value(json!({
            "accounts": [{
                "id": "legacy",
                "providerType": "claude_oauth"
            }]
        }))
        .unwrap();

        assert_eq!(store.accounts[0].manual_subscription_expires_at_ms, None);
        assert_eq!(
            store.accounts[0].manual_subscription_expiry_updated_at_ms,
            None
        );
        assert_eq!(store.accounts[0].manual_subscription_expiry_rule, None);
        assert_eq!(store.accounts[0].auth_identity_generation, 1);
        assert_eq!(store.accounts[0].token_refresh_generation, 0);
    }

    #[test]
    fn account_generations_separate_identity_from_token_and_observation_updates() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::CodexOAuth);
        input.id = Some("generation-account".to_string());
        input.email = Some("owner@example.com".to_string());
        input.access_token = Some("access-1".to_string());
        input.refresh_token = Some("refresh-1".to_string());
        input.scopes = vec!["openid".to_string(), "profile".to_string()];
        input.profile = Some(json!({"sub": "principal-1", "plan": "free"}));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);
        assert_eq!(created.token_refresh_generation, 1);

        let refreshed = store
            .mark_refresh_success(
                "generation-account",
                AccountRefreshUpdate {
                    access_token: Some("access-2".to_string()),
                    refresh_token: Some("refresh-2".to_string()),
                    expires_at: Some(2_000),
                    quota_percent: Some(12.5),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(refreshed.auth_identity_generation, 1);
        assert_eq!(refreshed.token_refresh_generation, 2);

        let observed = store
            .update_entitlement_snapshot(
                "generation-account",
                Some("plus".to_string()),
                Some("active".to_string()),
                3_000,
            )
            .unwrap();
        assert_eq!(observed.auth_identity_generation, 1);
        assert_eq!(observed.token_refresh_generation, 2);

        let enriched = store
            .mark_refresh_success(
                "generation-account",
                AccountRefreshUpdate {
                    scopes: Some(vec!["openid".to_string(), "email".to_string()]),
                    email: Some("OWNER@EXAMPLE.COM".to_string()),
                    profile: Some(json!({
                        "sub": "principal-1",
                        "plan": "pro",
                        "displayName": "Owner"
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(enriched.auth_identity_generation, 1);
        assert_eq!(enriched.token_refresh_generation, 2);

        let changed_principal = store
            .mark_refresh_success(
                "generation-account",
                AccountRefreshUpdate {
                    profile: Some(json!({"sub": "principal-2", "plan": "pro"})),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(changed_principal.auth_identity_generation, 2);
        assert_eq!(changed_principal.token_refresh_generation, 2);

        let persisted = serde_json::to_value(&store).unwrap();
        let reloaded: AccountStore = serde_json::from_value(persisted).unwrap();
        assert_eq!(reloaded.accounts[0].auth_identity_generation, 2);
        assert_eq!(reloaded.accounts[0].token_refresh_generation, 2);
    }

    #[test]
    fn codex_identity_generation_prefers_verified_subject_over_workspace() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::CodexOAuth);
        input.id = Some("codex-subject-account".to_string());
        input.profile = Some(json!({
            "accountId": "workspace-a",
            "verifiedOpenAiClaims": {
                "subject": "user-a",
                "chatgpt_account_id": "workspace-a"
            }
        }));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);
        assert_eq!(
            store.codex_account_id_for_verified_subject(" user-a "),
            Some("codex-subject-account")
        );
        assert_eq!(store.codex_account_id_for_verified_subject("user-b"), None);

        let moved_workspace = store
            .mark_refresh_success(
                "codex-subject-account",
                AccountRefreshUpdate {
                    profile: Some(json!({
                        "accountId": "workspace-b",
                        "verifiedOpenAiClaims": {
                            "subject": "user-a",
                            "chatgpt_account_id": "workspace-b"
                        }
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(moved_workspace.auth_identity_generation, 1);

        let replaced_subject = store
            .mark_refresh_success(
                "codex-subject-account",
                AccountRefreshUpdate {
                    profile: Some(json!({
                        "accountId": "workspace-b",
                        "verifiedOpenAiClaims": {
                            "subject": "user-b",
                            "chatgpt_account_id": "workspace-b"
                        }
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(replaced_subject.auth_identity_generation, 2);
    }

    #[test]
    fn account_identity_uses_email_only_without_a_stable_principal() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("email-fallback-account".to_string());
        input.email = Some("Owner@Example.COM".to_string());
        input.profile = Some(json!({"plan": "pro"}));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);

        let same_email = store
            .mark_refresh_success(
                "email-fallback-account",
                AccountRefreshUpdate {
                    email: Some(" owner@example.com ".to_string()),
                    profile: Some(json!({"plan": "max", "displayName": "Owner"})),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(same_email.auth_identity_generation, 1);

        let different_email = store
            .mark_refresh_success(
                "email-fallback-account",
                AccountRefreshUpdate {
                    email: Some("other@example.com".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(different_email.auth_identity_generation, 2);
    }

    #[test]
    fn account_identity_is_stable_when_principal_moves_between_snapshots() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("principal-source-account".to_string());
        input.profile = Some(json!({"userId": "user-123"}));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);

        let moved = store
            .mark_refresh_success(
                "principal-source-account",
                AccountRefreshUpdate {
                    profile: Some(json!({"plan": "pro"})),
                    raw: Some(json!({"user_id": "user-123"})),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(moved.auth_identity_generation, 1);
    }

    #[test]
    fn claude_identity_prefers_account_uuid_and_detects_replacement() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("claude-principal-account".to_string());
        input.email = Some("owner@example.com".to_string());
        input.profile = Some(json!({
            "accountId": "owner@example.com",
            "accountUUID": "claude-account-1"
        }));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);

        let enriched = store
            .mark_refresh_success(
                "claude-principal-account",
                AccountRefreshUpdate {
                    email: Some("OWNER@EXAMPLE.COM".to_string()),
                    profile: Some(json!({
                        "accountId": "owner@example.com",
                        "accountUUID": "claude-account-1",
                        "plan": "max"
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(enriched.auth_identity_generation, 1);

        let replaced = store
            .mark_refresh_success(
                "claude-principal-account",
                AccountRefreshUpdate {
                    profile: Some(json!({
                        "accountId": "owner@example.com",
                        "accountUUID": "claude-account-2"
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(replaced.auth_identity_generation, 2);
    }

    #[test]
    fn claude_stable_principal_enrichment_does_not_stale_email_binding() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("claude-bootstrap-account".to_string());
        input.email = Some("owner@example.com".to_string());
        input.profile = Some(json!({"plan": "pro"}));
        let created = store.upsert(input);
        assert_eq!(created.auth_identity_generation, 1);

        let bootstrapped = store
            .mark_refresh_success(
                "claude-bootstrap-account",
                AccountRefreshUpdate {
                    email: Some("owner@example.com".to_string()),
                    profile: Some(json!({
                        "accountUUID": "claude-account-1",
                        "plan": "pro"
                    })),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(bootstrapped.auth_identity_generation, 1);

        let replaced = store
            .mark_refresh_success(
                "claude-bootstrap-account",
                AccountRefreshUpdate {
                    raw: Some(json!({
                        "account": {"uuid": "claude-account-2"}
                    })),
                    profile: Some(json!({"plan": "pro"})),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(replaced.auth_identity_generation, 2);
    }

    #[test]
    fn recurring_subscription_rule_replaces_legacy_value_and_survives_refresh() {
        use crate::domain::accounts::subscription_expiry::SubscriptionExpiryCadence;

        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::ClaudeOAuth);
        input.id = Some("claude-recurring".to_string());
        store.upsert(input);
        store
            .set_manual_subscription_expiry(
                "claude-recurring",
                Some(1_786_924_800_000),
                1_784_000_000_000,
            )
            .unwrap();

        let account = store
            .set_subscription_expiry_rule(
                "claude-recurring",
                Some(SubscriptionExpiryRuleDraft {
                    cadence: SubscriptionExpiryCadence::Monthly,
                    month: None,
                    day: 10,
                    time_zone: "Asia/Shanghai".to_string(),
                }),
                1_785_000_000_000,
            )
            .unwrap();
        assert_eq!(account.manual_subscription_expires_at_ms, None);
        assert_eq!(account.manual_subscription_expiry_updated_at_ms, None);
        assert_eq!(
            account
                .manual_subscription_expiry_rule
                .as_ref()
                .map(|rule| (rule.cadence, rule.day, rule.time_zone.as_str())),
            Some((SubscriptionExpiryCadence::Monthly, 10, "Asia/Shanghai"))
        );

        let mut refreshed = fixture_input(ProviderType::ClaudeOAuth);
        refreshed.id = Some("claude-recurring".to_string());
        refreshed.access_token = Some("refreshed-token".to_string());
        let account = store.upsert(refreshed);
        assert!(account.manual_subscription_expiry_rule.is_some());

        let persisted = serde_json::to_value(&store).unwrap();
        assert_eq!(
            persisted.pointer("/accounts/0/manualSubscriptionExpiryRule/cadence"),
            Some(&json!("monthly"))
        );
        let reloaded: AccountStore = serde_json::from_value(persisted).unwrap();
        assert_eq!(
            reloaded.accounts[0].manual_subscription_expiry_rule,
            account.manual_subscription_expiry_rule
        );

        let cleared = store
            .set_subscription_expiry_rule("claude-recurring", None, 1_786_000_000_000)
            .unwrap();
        assert_eq!(cleared.manual_subscription_expiry_rule, None);
        assert_eq!(cleared.manual_subscription_expires_at_ms, None);
    }

    #[test]
    fn recurring_subscription_rule_rejects_unsupported_accounts_and_invalid_shapes() {
        use crate::domain::accounts::subscription_expiry::SubscriptionExpiryCadence;

        let mut store = AccountStore::default();
        for provider_type in [ProviderType::ClaudeOAuth, ProviderType::CodexOAuth] {
            let mut input = fixture_input(provider_type);
            input.id = Some(provider_type.as_str().to_string());
            store.upsert(input);
        }
        let monthly_with_month = SubscriptionExpiryRuleDraft {
            cadence: SubscriptionExpiryCadence::Monthly,
            month: Some(7),
            day: 10,
            time_zone: "UTC".to_string(),
        };

        assert!(matches!(
            store.set_subscription_expiry_rule(
                ProviderType::ClaudeOAuth.as_str(),
                Some(monthly_with_month.clone()),
                1_000,
            ),
            Err(ManualSubscriptionExpiryError::InvalidRule(_))
        ));
        assert!(matches!(
            store.set_subscription_expiry_rule(
                ProviderType::CodexOAuth.as_str(),
                Some(monthly_with_month),
                1_000,
            ),
            Err(ManualSubscriptionExpiryError::Unsupported(
                ProviderType::CodexOAuth
            ))
        ));
    }

    #[test]
    fn grok_upsert_preserves_manual_subscription_expiry_fallback() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::GrokOAuth);
        input.id = Some("grok-account".to_string());
        store.upsert(input);
        store
            .set_manual_subscription_expiry(
                "grok-account",
                Some(1_787_097_600_000),
                1_784_000_000_000,
            )
            .unwrap();

        let mut refreshed = fixture_input(ProviderType::GrokOAuth);
        refreshed.id = Some("grok-account".to_string());
        refreshed.access_token = Some("refreshed-token".to_string());
        let account = store.upsert(refreshed);

        assert_eq!(account.access_token.as_deref(), Some("refreshed-token"));
        assert_eq!(
            account.manual_subscription_expires_at_ms,
            Some(1_787_097_600_000)
        );
        assert_eq!(
            account.manual_subscription_expiry_updated_at_ms,
            Some(1_784_000_000_000)
        );
    }

    #[test]
    fn manual_subscription_expiry_is_restricted_to_manual_capable_accounts() {
        let mut store = AccountStore::default();
        for provider_type in [
            ProviderType::ClaudeOAuth,
            ProviderType::GrokOAuth,
            ProviderType::CodexOAuth,
        ] {
            let mut input = fixture_input(provider_type);
            input.id = Some(provider_type.as_str().to_string());
            store.upsert(input);
        }

        let claude = store
            .set_manual_subscription_expiry(
                ProviderType::ClaudeOAuth.as_str(),
                Some(1_786_924_800_000),
                1_784_000_000_000,
            )
            .unwrap();
        assert_eq!(
            claude.manual_subscription_expires_at_ms,
            Some(1_786_924_800_000)
        );

        let grok = store
            .set_manual_subscription_expiry(
                ProviderType::GrokOAuth.as_str(),
                Some(1_787_097_600_000),
                1_784_000_000_000,
            )
            .unwrap();
        assert_eq!(
            grok.manual_subscription_expires_at_ms,
            Some(1_787_097_600_000)
        );

        assert!(matches!(
            store.set_manual_subscription_expiry(
                ProviderType::CodexOAuth.as_str(),
                Some(1_786_924_800_000),
                1_784_000_000_000,
            ),
            Err(ManualSubscriptionExpiryError::Unsupported(
                ProviderType::CodexOAuth
            ))
        ));
        assert!(matches!(
            store.set_manual_subscription_expiry("missing", None, 1_784_000_000_000),
            Err(ManualSubscriptionExpiryError::NotFound(account_id)) if account_id == "missing"
        ));
        assert!(matches!(
            store.set_manual_subscription_expiry(
                ProviderType::ClaudeOAuth.as_str(),
                Some(0),
                1_784_000_000_000,
            ),
            Err(ManualSubscriptionExpiryError::InvalidTimestamp)
        ));

        let cleared = store
            .set_manual_subscription_expiry(
                ProviderType::ClaudeOAuth.as_str(),
                None,
                1_785_000_000_000,
            )
            .unwrap();
        assert_eq!(cleared.manual_subscription_expires_at_ms, None);
        assert_eq!(
            cleared.manual_subscription_expiry_updated_at_ms,
            Some(1_785_000_000_000)
        );
    }

    #[test]
    fn long_tail_account_fixtures_preserve_profile_raw_subscription_and_quota_shape() {
        let mut store = AccountStore::default();
        let cases = [
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

        for provider_type in cases {
            let has_percent_quota = provider_type != ProviderType::OllamaCloud;
            let account = store.upsert(UpsertAccountInput {
                id: Some(format!("acct-{}", provider_type.as_str())),
                provider_type,
                email: Some("owner@example.com".to_string()),
                access_token: Some(format!("access-{}", provider_type.as_str())),
                refresh_token: Some(format!("refresh-{}", provider_type.as_str())),
                id_token: Some(format!("id-{}", provider_type.as_str())),
                token_type: Some("Bearer".to_string()),
                api_key: if matches!(
                    provider_type,
                    ProviderType::CursorApiKey
                        | ProviderType::OllamaCloud
                        | ProviderType::Nvidia
                        | ProviderType::DeepSeekApi
                        | ProviderType::AwsBedrock
                ) {
                    Some(format!("key-{}", provider_type.as_str()))
                } else {
                    None
                },
                extra_headers: None,
                scopes: vec!["profile".to_string(), provider_type.as_str().to_string()],
                profile: Some(json!({
                    "providerType": provider_type.as_str(),
                    "displayName": "Owner"
                })),
                raw: Some(json!({
                    "source": "fixture",
                    "nested": {"providerType": provider_type.as_str()}
                })),
                subscription_level: Some("pro".to_string()),
                entitlement_status: None,
                quota_percent: has_percent_quota.then_some(23.5),
                quota: has_percent_quota.then_some(AccountQuota {
                    success: true,
                    credential_message: Some("ok".to_string()),
                    tiers: vec![AccountQuotaTier {
                        name: provider_type.as_str().to_string(),
                        label: None,
                        utilization: Some(0.235),
                        used: Some(235.0),
                        limit: Some(1000.0),
                        unit: Some("requests".to_string()),
                        resets_at: Some(123456),
                    }],
                    extra_usage: Some(json!({"raw": true})),
                }),
                quota_refreshed_at: has_percent_quota.then_some(1000),
                quota_next_refresh_at: has_percent_quota.then_some(2000),
                expires_at: Some(3000),
                rate_limited_until: None,
                last_refresh_error: None,
            });

            assert_eq!(account.provider_type, provider_type);
            assert_eq!(
                account
                    .profile
                    .as_ref()
                    .and_then(|value| value.get("providerType")),
                Some(&json!(provider_type.as_str()))
            );
            assert_eq!(
                account
                    .raw
                    .as_ref()
                    .and_then(|value| value.pointer("/nested/providerType")),
                Some(&json!(provider_type.as_str()))
            );
            assert_eq!(account.subscription_level.as_deref(), Some("pro"));
            if provider_type == ProviderType::OllamaCloud {
                assert_eq!(account.quota_percent, None);
                assert!(account.quota.is_none());
                assert_ne!(account.quota_percent, Some(0.0));
            } else {
                assert_eq!(account.quota_percent, Some(23.5));
                assert_eq!(
                    account
                        .quota
                        .as_ref()
                        .and_then(|quota| quota.tiers.first())
                        .map(|tier| tier.name.as_str()),
                    Some(provider_type.as_str())
                );
            }
        }
    }

    #[test]
    fn records_refresh_success_and_failure_without_losing_profile_context() {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("old-token".to_string()),
            refresh_token: Some("old-refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: vec!["openid".to_string()],
            profile: Some(json!({"plan": "plus"})),
            raw: Some(json!({"source": "fixture"})),
            subscription_level: Some("plus".to_string()),
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: Some(1000),
            rate_limited_until: None,
            last_refresh_error: Some("previous".to_string()),
        });

        let refreshed = store
            .mark_refresh_success(
                "acct-1",
                AccountRefreshUpdate {
                    access_token: Some("new-token".to_string()),
                    expires_at: Some(2000),
                    quota_percent: Some(12.0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(refreshed.access_token.as_deref(), Some("new-token"));
        assert_eq!(refreshed.refresh_token.as_deref(), Some("old-refresh"));
        assert_eq!(
            refreshed
                .profile
                .as_ref()
                .and_then(|value| value.get("plan")),
            Some(&json!("plus"))
        );
        assert_eq!(refreshed.quota_percent, Some(12.0));
        assert!(refreshed.last_refresh_error.is_none());

        let failed = store
            .mark_refresh_failure("acct-1", "quota endpoint failed".to_string())
            .unwrap();
        assert_eq!(
            failed.last_refresh_error.as_deref(),
            Some("quota endpoint failed")
        );
        assert_eq!(failed.access_token.as_deref(), Some("new-token"));
    }

    #[test]
    fn codex_workspace_selection_only_accepts_verified_claim_options() {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: Some(json!({
                "chatgpt_account_id": "account-default",
                "verifiedOpenAiClaims": {
                    "chatgpt_account_id": "account-default",
                    "organizations": [
                        {"id": "account-team", "name": "Team"},
                        {"id": "account-enterprise", "name": "Enterprise"}
                    ]
                },
                "organizations": [
                    {"id": "account-team", "name": "Team"},
                    {"id": "account-enterprise", "name": "Enterprise"}
                ]
            })),
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
        });

        let account = store
            .select_codex_workspace("acct-1", "account-team")
            .unwrap();
        assert_eq!(
            selected_codex_workspace_id(&account).as_deref(),
            Some("account-team")
        );
        assert_eq!(codex_workspace_options(&account).len(), 3);
        assert!(store
            .select_codex_workspace("acct-1", "attacker-account")
            .is_err());
    }

    #[test]
    fn codex_effective_workspace_prefers_verified_default_over_sorted_options() {
        let mut store = AccountStore::default();
        let mut profile = Some(json!({}));
        set_verified_openai_claims(
            &mut profile,
            Some(json!({
                "chatgpt_account_id": "workspace-z-default",
                "organizations": [
                    {"id": "workspace-a-team", "name": "A Team"}
                ]
            })),
        );
        let account = store.upsert(UpsertAccountInput {
            id: Some("acct-effective-workspace".to_string()),
            provider_type: ProviderType::CodexOAuth,
            profile,
            ..fixture_input(ProviderType::CodexOAuth)
        });

        assert_eq!(
            codex_workspace_options(&account)
                .first()
                .map(|workspace| workspace.id.as_str()),
            Some("workspace-a-team")
        );
        assert_eq!(
            effective_codex_workspace_id(&account).as_deref(),
            Some("workspace-z-default")
        );
    }

    #[test]
    fn codex_authenticated_provenance_is_a_trusted_workspace_option() {
        let mut store = AccountStore::default();
        let mut profile = Some(json!({"chatgpt_account_id": "workspace-1"}));
        set_codex_workspace_provenance(&mut profile, "workspace-1", "authenticated_discovery", 123);
        let account = store.upsert(UpsertAccountInput {
            id: Some("acct-provenance".to_string()),
            provider_type: ProviderType::CodexOAuth,
            profile,
            ..fixture_input(ProviderType::CodexOAuth)
        });

        assert_eq!(codex_workspace_options(&account).len(), 1);
        assert_eq!(
            trusted_codex_workspace(&account),
            Some(TrustedCodexWorkspace {
                id: "workspace-1".to_string(),
                source: "authenticated_discovery".to_string(),
            })
        );
        assert_eq!(
            account
                .profile
                .as_ref()
                .and_then(|profile| profile.pointer("/codexWorkspaceProvenance/verifiedAt"))
                .and_then(Value::as_i64),
            Some(123)
        );
    }

    #[test]
    fn codex_verified_claims_take_precedence_and_provenance_can_be_scrubbed() {
        let mut profile = Some(json!({}));
        set_codex_workspace_provenance(
            &mut profile,
            "workspace-migrated",
            "authenticated_discovery",
            123,
        );
        set_verified_openai_claims(
            &mut profile,
            Some(json!({"chatgpt_account_id": "workspace-signed"})),
        );
        let mut account = AccountStore::default().upsert(UpsertAccountInput {
            id: Some("acct-provenance-priority".to_string()),
            provider_type: ProviderType::CodexOAuth,
            profile,
            ..fixture_input(ProviderType::CodexOAuth)
        });

        assert_eq!(
            trusted_codex_workspace(&account),
            Some(TrustedCodexWorkspace {
                id: "workspace-signed".to_string(),
                source: "verified_openai_token".to_string(),
            })
        );
        clear_codex_workspace_provenance(&mut account.profile);
        assert!(account
            .profile
            .as_ref()
            .and_then(|profile| profile.get("codexWorkspaceProvenance"))
            .is_none());
        assert_eq!(
            effective_codex_workspace_id(&account).as_deref(),
            Some("workspace-signed")
        );
    }

    #[test]
    fn codex_workspace_change_invalidates_workspace_scoped_quota_cache() {
        let mut store = AccountStore::default();
        let mut input = fixture_input(ProviderType::CodexOAuth);
        input.id = Some("acct-workspace-cache".to_string());
        input.profile = Some(json!({
            "verifiedOpenAiClaims": {
                "chatgpt_account_id": "account-default",
                "organizations": [{"id": "account-team", "name": "Team"}]
            },
            "selectedChatgptAccountId": "account-default"
        }));
        input.raw = Some(json!({
            "bankedReset": {"availableCount": 2},
            "rate_limit_reset_credits": {"available_count": 2},
            "unrelated": "preserved"
        }));
        input.subscription_level = Some("ChatGPT Plus".to_string());
        input.entitlement_status = Some("active".to_string());
        input.quota_percent = Some(50.0);
        input.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "bankedReset": {
                    "workspaceId": "account-default",
                    "availableCount": 2
                }
            })),
            ..Default::default()
        });
        input.quota_refreshed_at = Some(1_000);
        input.quota_next_refresh_at = Some(2_000);
        input.rate_limited_until = Some(3_000);
        input.last_refresh_error = Some("old workspace error".to_string());
        store.upsert(input);

        let account = store
            .select_codex_workspace("acct-workspace-cache", "account-team")
            .unwrap();

        assert_eq!(
            selected_codex_workspace_id(&account).as_deref(),
            Some("account-team")
        );
        assert!(account.subscription_level.is_none());
        assert!(account.entitlement_status.is_none());
        assert!(account.quota_percent.is_none());
        assert!(account.quota.is_none());
        assert!(account.quota_refreshed_at.is_none());
        assert!(account.quota_next_refresh_at.is_none());
        assert!(account.rate_limited_until.is_none());
        assert!(account.last_refresh_error.is_none());
        let raw = account.raw.as_ref().unwrap();
        assert!(raw.get("bankedReset").is_none());
        assert!(raw.get("rate_limit_reset_credits").is_none());
        assert_eq!(raw["unrelated"], "preserved");
    }

    #[test]
    fn codex_workspace_selection_ignores_unverified_profile_fields() {
        let mut store = AccountStore::default();
        let account = store.upsert(UpsertAccountInput {
            id: Some("acct-unverified".to_string()),
            provider_type: ProviderType::CodexOAuth,
            profile: Some(json!({
                "chatgpt_account_id": "attacker-account",
                "organizations": [{"id": "attacker-team"}]
            })),
            ..fixture_input(ProviderType::CodexOAuth)
        });
        assert!(codex_workspace_options(&account).is_empty());
        assert!(store
            .select_codex_workspace("acct-unverified", "attacker-team")
            .is_err());
    }

    #[test]
    fn codex_refresh_preserves_verified_workspace_state_without_new_id_token() {
        let mut store = AccountStore::default();
        let mut profile = Some(json!({"chatgpt_account_id": "account-default"}));
        set_verified_openai_claims(
            &mut profile,
            Some(json!({
                "chatgpt_account_id": "account-default",
                "organizations": [{"id": "account-team"}]
            })),
        );
        store.upsert(UpsertAccountInput {
            id: Some("acct-refresh".to_string()),
            provider_type: ProviderType::CodexOAuth,
            profile,
            ..fixture_input(ProviderType::CodexOAuth)
        });
        store
            .select_codex_workspace("acct-refresh", "account-team")
            .unwrap();

        let refreshed = store
            .mark_native_refresh_success(
                "acct-refresh",
                AccountRefreshUpdate {
                    access_token: Some("new-access".to_string()),
                    profile: Some(json!({"chatgpt_account_id": "account-default"})),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            selected_codex_workspace_id(&refreshed).as_deref(),
            Some("account-team")
        );
        assert_eq!(codex_workspace_options(&refreshed).len(), 2);
    }

    #[test]
    fn native_refresh_invalid_grants_require_relogin_after_threshold() {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type: ProviderType::ClaudeOAuth,
            email: None,
            access_token: None,
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
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        });

        let network_failure = store
            .mark_native_refresh_failure(
                "acct-1",
                "network unavailable".to_string(),
                OAuthErrorKind::Network,
            )
            .unwrap();
        assert_eq!(network_failure.refresh_consecutive_failures, 0);
        assert!(!network_failure.needs_relogin);

        let first = store
            .mark_native_refresh_failure_with_threshold(
                "acct-1",
                "invalid grant".to_string(),
                OAuthErrorKind::InvalidGrant,
                2,
            )
            .unwrap();
        assert_eq!(first.refresh_consecutive_failures, 1);
        assert!(!first.needs_relogin);

        let second = store
            .mark_native_refresh_failure_with_threshold(
                "acct-1",
                "invalid grant".to_string(),
                OAuthErrorKind::InvalidGrant,
                2,
            )
            .unwrap();
        assert_eq!(second.refresh_consecutive_failures, 2);
        assert!(second.needs_relogin);

        let replay_rejected = store
            .mark_native_refresh_failure_with_threshold(
                "acct-1",
                "invalid_grant: refresh_token_reused".to_string(),
                OAuthErrorKind::InvalidGrant,
                20,
            )
            .unwrap();
        assert!(replay_rejected.needs_relogin);

        let recovered = store
            .mark_native_refresh_success(
                "acct-1",
                AccountRefreshUpdate {
                    access_token: Some("new-token".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(recovered.refresh_consecutive_failures, 0);
        assert!(!recovered.needs_relogin);
        assert!(recovered.last_refresh_error.is_none());
    }

    #[test]
    fn update_entitlement_snapshot_preserves_tier_and_entitlement_status() {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type: ProviderType::GrokOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("token".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: Some(json!({"source": "fixture"})),
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
        });

        let account = store
            .update_entitlement_snapshot(
                "acct-1",
                Some("SuperGrok".to_string()),
                Some("denied".to_string()),
                1_234,
            )
            .unwrap();

        assert_eq!(account.subscription_level.as_deref(), Some("SuperGrok"));
        assert_eq!(account.entitlement_status.as_deref(), Some("denied"));
        let profile = account.profile.as_ref().unwrap();
        assert_eq!(profile["subscriptionTier"], json!("SuperGrok"));
        assert_eq!(profile["entitlementStatus"], json!("denied"));
        assert_eq!(profile["entitlementUpdatedAtMs"], json!(1_234));
    }

    #[test]
    fn save_encrypts_account_secret_fields_and_load_decrypts_them() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-account-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&config_dir);
        fs::create_dir_all(&config_dir).expect("tempdir");

        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some("acct-1".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: Some("owner@example.com".to_string()),
            access_token: Some("access-secret".to_string()),
            refresh_token: Some("refresh-secret".to_string()),
            id_token: Some("id-secret".to_string()),
            token_type: Some("Bearer".to_string()),
            api_key: Some("api-secret".to_string()),
            extra_headers: None,
            scopes: Vec::new(),
            profile: None,
            raw: Some(serde_json::json!({
                "clientSecret": "kiro-client-secret",
                "tokenResponse": {
                    "access_token": "nested-snake-access-secret",
                    "refreshToken": "nested-refresh-secret",
                    "id_token": "nested-snake-id-secret"
                },
                "githubToken": "github-token-secret",
                "copilotToken": {"token": "copilot-token-secret"},
                "entry": {"key": "grok-key-secret"},
                "extra_headers": {"x-private": "extra-header-secret"}
            })),
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        });

        store.save(&config_dir).expect("save");
        let content = fs::read_to_string(accounts_path(&config_dir)).expect("read accounts");
        assert!(!content.contains("access-secret"));
        assert!(!content.contains("refresh-secret"));
        assert!(!content.contains("kiro-client-secret"));
        assert!(!content.contains("nested-refresh-secret"));
        assert!(!content.contains("nested-snake-access-secret"));
        assert!(!content.contains("nested-snake-id-secret"));
        assert!(!content.contains("github-token-secret"));
        assert!(!content.contains("copilot-token-secret"));
        assert!(!content.contains("grok-key-secret"));
        assert!(!content.contains("extra-header-secret"));
        assert!(content.contains(ENCRYPTED_PREFIX));
        assert!(accounts_key_path(&config_dir).exists());

        let loaded = AccountStore::load_or_default(&config_dir).expect("load");
        let account = loaded
            .find_for_provider(ProviderType::CodexOAuth, Some("acct-1"))
            .expect("account");
        assert_eq!(account.access_token.as_deref(), Some("access-secret"));
        assert_eq!(account.refresh_token.as_deref(), Some("refresh-secret"));
        assert_eq!(account.id_token.as_deref(), Some("id-secret"));
        assert_eq!(account.api_key.as_deref(), Some("api-secret"));
        assert_eq!(
            account
                .raw
                .as_ref()
                .and_then(|value| value.pointer("/clientSecret"))
                .and_then(Value::as_str),
            Some("kiro-client-secret")
        );
        assert_eq!(
            account
                .raw
                .as_ref()
                .and_then(|value| value.pointer("/tokenResponse/refreshToken"))
                .and_then(Value::as_str),
            Some("nested-refresh-secret")
        );
        assert_eq!(
            account
                .raw
                .as_ref()
                .and_then(|value| value.pointer("/tokenResponse/access_token"))
                .and_then(Value::as_str),
            Some("nested-snake-access-secret")
        );
        assert_eq!(
            account
                .raw
                .as_ref()
                .and_then(|value| value.pointer("/copilotToken/token"))
                .and_then(Value::as_str),
            Some("copilot-token-secret")
        );
        assert_eq!(
            account
                .raw
                .as_ref()
                .and_then(|value| value.pointer("/extra_headers/x-private"))
                .and_then(Value::as_str),
            Some("extra-header-secret")
        );

        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn loading_encrypted_accounts_requires_existing_key_without_creating_one() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-account-store-missing-key-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&config_dir);
        fs::create_dir_all(&config_dir).expect("tempdir");
        fs::write(
            accounts_path(&config_dir),
            r#"{"accounts":[{"id":"acct-1","providerType":"codex_oauth","accessToken":"ccenc:v1:nonce.body"}]}"#,
        )
        .expect("write accounts");

        let error = AccountStore::load_or_default(&config_dir).expect_err("missing key");
        assert!(error.to_string().contains("accounts encryption key"));
        assert!(!accounts_key_path(&config_dir).exists());

        let _ = fs::remove_dir_all(&config_dir);
    }
}
