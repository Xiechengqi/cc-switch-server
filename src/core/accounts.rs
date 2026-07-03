#![allow(dead_code)]

use std::fs;
use std::path::Path;

use anyhow::Context;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::oauth_clients::oauth_provider_spec;
use crate::core::provider::ProviderType;

const ACCOUNTS_FILE_NAME: &str = "accounts.json";

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
    pub scopes: Vec<String>,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub raw: Option<Value>,
    #[serde(default)]
    pub subscription_level: Option<String>,
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
    pub last_refresh_error: Option<String>,
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
    pub scopes: Vec<String>,
    #[serde(default)]
    pub profile: Option<Value>,
    #[serde(default)]
    pub raw: Option<Value>,
    #[serde(default)]
    pub subscription_level: Option<String>,
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
    pub quota_percent: Option<f64>,
    pub quota: Option<AccountQuota>,
    pub quota_refreshed_at: Option<i64>,
    pub quota_next_refresh_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub last_refresh_error: Option<String>,
}

impl AccountStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = accounts_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read accounts {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse accounts {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = accounts_path(config_dir);
        crate::core::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write accounts {}", path.display()))
    }

    pub fn upsert(&mut self, input: UpsertAccountInput) -> Account {
        let account = Account {
            id: input.id.unwrap_or_else(generate_account_id),
            provider_type: input.provider_type,
            email: input.email,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            id_token: input.id_token,
            token_type: input.token_type,
            api_key: input.api_key,
            scopes: input.scopes,
            profile: input.profile,
            raw: input.raw,
            subscription_level: input.subscription_level,
            quota_percent: input.quota_percent,
            quota: input.quota,
            quota_refreshed_at: input.quota_refreshed_at,
            quota_next_refresh_at: input.quota_next_refresh_at,
            expires_at: input.expires_at,
            last_refresh_error: input.last_refresh_error,
        };

        if let Some(existing) = self.accounts.iter_mut().find(|item| item.id == account.id) {
            *existing = account.clone();
        } else {
            self.accounts.push(account.clone());
        }

        account
    }

    pub fn find_for_provider(
        &self,
        provider_type: ProviderType,
        account_id: Option<&str>,
    ) -> Option<&Account> {
        if let Some(account_id) = account_id {
            return self.accounts.iter().find(|item| item.id == account_id);
        }

        self.accounts
            .iter()
            .find(|item| item.provider_type == provider_type)
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
        if let Some(value) = update.profile {
            account.profile = Some(value);
        }
        if let Some(value) = update.raw {
            account.raw = Some(value);
        }
        if let Some(value) = update.subscription_level {
            account.subscription_level = Some(value);
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
        account.last_refresh_error = update.last_refresh_error;
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
}

pub fn accounts_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(ACCOUNTS_FILE_NAME)
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
            scopes: vec!["openid".to_string()],
            profile: None,
            raw: None,
            subscription_level: Some("pro".to_string()),
            quota: None,
            quota_percent: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
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
                quota_percent: has_percent_quota.then_some(23.5),
                quota: has_percent_quota.then_some(AccountQuota {
                    success: true,
                    credential_message: Some("ok".to_string()),
                    tiers: vec![AccountQuotaTier {
                        name: provider_type.as_str().to_string(),
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
            scopes: vec!["openid".to_string()],
            profile: Some(json!({"plan": "plus"})),
            raw: Some(json!({"source": "fixture"})),
            subscription_level: Some("plus".to_string()),
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: Some(1000),
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
}
