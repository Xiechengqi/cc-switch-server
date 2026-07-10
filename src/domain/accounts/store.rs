#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::domain::accounts::oauth::{oauth_provider_spec, OAuthErrorKind};
use crate::domain::providers::model::ProviderType;

const ACCOUNTS_FILE_NAME: &str = "accounts.json";
const ACCOUNTS_KEY_FILE_NAME: &str = "accounts.key";
const ENCRYPTED_PREFIX: &str = "ccenc:v1:";
const ACCOUNTS_KEY_ENV: &str = "CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY";
const SECRET_FIELDS: &[&str] = &[
    "accessToken",
    "refreshToken",
    "idToken",
    "apiKey",
    "clientSecret",
    "kiroApiKey",
];

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
            entitlement_status: input.entitlement_status,
            quota_percent: input.quota_percent,
            quota: input.quota,
            quota_refreshed_at: input.quota_refreshed_at,
            quota_next_refresh_at: input.quota_next_refresh_at,
            expires_at: input.expires_at,
            rate_limited_until: input.rate_limited_until,
            last_refresh_error: input.last_refresh_error,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
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
        Some(account.clone())
    }

    pub fn mark_rate_limited_until(
        &mut self,
        account_id: &str,
        rate_limited_until: i64,
        message: Option<String>,
    ) -> Option<Account> {
        let account = self
            .accounts
            .iter_mut()
            .find(|item| item.id == account_id)?;
        account.rate_limited_until = Some(rate_limited_until);
        if let Some(message) = message {
            account.last_refresh_error = Some(message);
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
            if account.refresh_consecutive_failures >= threshold.max(1) {
                account.needs_relogin = true;
            }
        }
        Some(account.clone())
    }
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
    config_dir.join(ACCOUNTS_KEY_FILE_NAME)
}

fn load_or_create_accounts_key(config_dir: &Path) -> anyhow::Result<[u8; 32]> {
    if let Some(key) = load_accounts_key_if_present(config_dir)? {
        return Ok(key);
    }
    let path = accounts_key_path(config_dir);
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config dir {}", config_dir.display()))?;
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let encoded = URL_SAFE_NO_PAD.encode(key);
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(encoded.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    Ok(key)
}

fn load_accounts_key(config_dir: &Path) -> anyhow::Result<[u8; 32]> {
    load_accounts_key_if_present(config_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "accounts encryption key is required to read encrypted accounts: {}",
            accounts_key_path(config_dir).display()
        )
    })
}

fn load_accounts_key_if_present(config_dir: &Path) -> anyhow::Result<Option<[u8; 32]>> {
    if let Ok(value) = std::env::var(ACCOUNTS_KEY_ENV) {
        return decode_accounts_key(value.trim())
            .context("decode accounts encryption env key")
            .map(Some);
    }
    let path = accounts_key_path(config_dir);
    if path.exists() {
        let content =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        return decode_accounts_key(content.trim())
            .with_context(|| format!("decode {}", path.display()))
            .map(Some);
    }
    Ok(None)
}

fn decode_accounts_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
        .context("base64 decode key")?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("accounts encryption key must be 32 bytes"))?;
    Ok(key)
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
            (SECRET_FIELDS.contains(&field.as_str())
                && value
                    .as_str()
                    .is_some_and(|value| value.starts_with(ENCRYPTED_PREFIX)))
                || value_has_encrypted_secret(value)
        }),
        Value::Array(values) => values.iter().any(value_has_encrypted_secret),
        _ => false,
    }
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
                if SECRET_FIELDS.contains(&field.as_str()) {
                    if let Value::String(secret) = value {
                        if !secret.trim().is_empty() {
                            *secret = transform(secret)?;
                        }
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
                entitlement_status: None,
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
            scopes: Vec::new(),
            profile: None,
            raw: Some(serde_json::json!({
                "clientSecret": "kiro-client-secret",
                "tokenResponse": {"refreshToken": "nested-refresh-secret"}
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
