use std::fs;
use std::path::Path;

use crate::domain::sharing::share_router_domain::{
    normalize_share_router_domain, router_domain_from_url, share_router_region_for_domain,
};
use anyhow::{bail, Context};
use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

const CONFIG_FILE_NAME: &str = "server.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub auth: AuthConfig,
    pub owner: OwnerConfig,
    pub router: RouterConfig,
    pub client: ClientConfig,
    #[serde(default)]
    pub upstream_proxy: UpstreamProxyConfig,
    #[serde(default)]
    pub upgrade_policy: UpgradePolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradePolicyConfig {
    #[serde(default = "default_true")]
    pub delegate_upgrade_to_router_owner: bool,
    #[serde(default)]
    pub auto_upgrade_enabled: bool,
    #[serde(default = "default_auto_upgrade_check_interval_minutes")]
    pub auto_upgrade_check_interval_minutes: u64,
}

impl Default for UpgradePolicyConfig {
    fn default() -> Self {
        Self {
            delegate_upgrade_to_router_owner: true,
            auto_upgrade_enabled: false,
            auto_upgrade_check_interval_minutes: default_auto_upgrade_check_interval_minutes(),
        }
    }
}

fn default_auto_upgrade_check_interval_minutes() -> u64 {
    60
}

impl UpgradePolicyConfig {
    pub fn normalize(mut self) -> Self {
        self.auto_upgrade_check_interval_minutes =
            self.auto_upgrade_check_interval_minutes.clamp(5, 24 * 60);
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    #[serde(default)]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub api_token_hash: Option<String>,
    #[serde(default)]
    pub debug_token_hash: Option<String>,
    #[serde(default)]
    pub debug_token_expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerConfig {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub payout_profile: PayoutProfileState,
    #[serde(default)]
    pub payout_profile_sync: PayoutProfileSyncStatus,
}

pub const PAYOUT_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutAddressType {
    #[serde(rename = "evm")]
    Evm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutToken {
    USDC,
    USDT,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PayoutNetwork {
    #[serde(rename = "eip155:56")]
    Bsc,
    #[serde(rename = "eip155:8453")]
    Base,
    #[serde(rename = "eip155:42161")]
    ArbitrumOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutVerificationStatus {
    #[serde(rename = "self_declared")]
    SelfDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayoutProfile {
    pub address_type: PayoutAddressType,
    pub address: String,
    pub token: PayoutToken,
    pub networks: Vec<PayoutNetwork>,
    pub verification_status: PayoutVerificationStatus,
}

impl PayoutProfile {
    pub fn validate_and_normalize(mut self) -> anyhow::Result<Self> {
        self.address = normalize_evm_address(&self.address)?;
        self.networks.sort_unstable();
        self.networks.dedup();
        if self.networks.is_empty() {
            bail!("at least one payout network is required");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoutProfileState {
    #[serde(default = "default_payout_profile_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub revision: i64,
    #[serde(default)]
    pub profile: Option<PayoutProfile>,
    #[serde(default)]
    pub updated_at_ms: i64,
}

impl Default for PayoutProfileState {
    fn default() -> Self {
        Self {
            schema_version: PAYOUT_PROFILE_SCHEMA_VERSION,
            revision: 0,
            profile: None,
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoutProfileSyncStatus {
    #[serde(default)]
    pub last_synced_revision: Option<i64>,
    #[serde(default)]
    pub last_synced_at_ms: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_payout_profile_schema_version() -> u32 {
    PAYOUT_PROFILE_SCHEMA_VERSION
}

pub fn normalize_evm_address(value: &str) -> anyhow::Result<String> {
    if value.trim() != value || !value.starts_with("0x") || value.len() != 42 {
        bail!("EVM address must be 0x followed by 40 hexadecimal characters");
    }
    let body = &value[2..];
    if !body.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("EVM address must contain only hexadecimal characters");
    }
    let normalized = checksum_evm_address(&body.to_ascii_lowercase());
    let has_lower = body.bytes().any(|byte| byte.is_ascii_lowercase());
    let has_upper = body.bytes().any(|byte| byte.is_ascii_uppercase());
    if has_lower && has_upper && normalized != value {
        bail!("mixed-case EVM address must use a valid EIP-55 checksum");
    }
    Ok(normalized)
}

fn checksum_evm_address(lowercase_body: &str) -> String {
    let digest = Keccak256::digest(lowercase_body.as_bytes());
    let mut output = String::with_capacity(42);
    output.push_str("0x");
    for (index, byte) in lowercase_body.bytes().enumerate() {
        if byte.is_ascii_alphabetic() {
            let hash_nibble = if index % 2 == 0 {
                digest[index / 2] >> 4
            } else {
                digest[index / 2] & 0x0f
            };
            if hash_nibble >= 8 {
                output.push((byte as char).to_ascii_uppercase());
                continue;
            }
        }
        output.push(byte as char);
    }
    output
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default)]
    pub custom: bool,
    #[serde(default)]
    pub identity: Option<RouterIdentity>,
    #[serde(default)]
    pub last_register_error: Option<String>,
    #[serde(default)]
    pub last_registered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterIdentity {
    pub installation_id: String,
    pub public_key: String,
    pub private_key: String,
    #[serde(default)]
    pub control_secret: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub tunnel_status: Option<String>,
    #[serde(default)]
    pub last_heartbeat_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamProxyConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_follow_system_proxy")]
    pub follow_system_proxy: bool,
}

impl Default for UpstreamProxyConfig {
    fn default() -> Self {
        Self {
            url: None,
            follow_system_proxy: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupOptions {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_true")]
    pub allow_offline: bool,
    #[serde(default)]
    pub issue_session_token: bool,
    #[serde(default)]
    pub issue_api_token: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            allow_offline: true,
            issue_session_token: false,
            issue_api_token: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupInput {
    pub password: String,
    pub owner_email: String,
    pub router_url: String,
    #[serde(default)]
    pub client_tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub options: Option<SetupOptions>,
}

impl ServerConfig {
    pub fn empty() -> Self {
        Self {
            auth: AuthConfig::default(),
            owner: OwnerConfig::default(),
            router: RouterConfig::default(),
            client: ClientConfig::default(),
            upstream_proxy: UpstreamProxyConfig::default(),
            upgrade_policy: UpgradePolicyConfig::default(),
        }
    }

    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let config_path = config_path(config_dir);
        if !config_path.exists() {
            return Ok(Self::empty());
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("read config {}", config_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parse config {}", config_path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;

        let config_path = config_path(config_dir);
        crate::infra::storage::write_json_pretty(&config_path, self)
            .with_context(|| format!("write config {}", config_path.display()))
    }

    pub fn is_setup_complete(&self) -> bool {
        self.auth.password_hash.is_some()
            && self
                .owner
                .email
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self
                .router
                .url
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self
                .client
                .tunnel_subdomain
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }

    pub fn verify_password(&self, password: &str) -> bool {
        let password = password.trim();
        if password.is_empty() {
            return false;
        }
        let Some(hash) = self.auth.password_hash.as_deref() else {
            return false;
        };

        verify_secret(hash, password)
    }

    pub fn preview_client_subdomain(subdomain: &str) -> anyhow::Result<String> {
        normalize_subdomain(subdomain)
    }

    pub fn preview_router_url(router_url: &str) -> anyhow::Result<String> {
        normalize_router_url(router_url)
    }

    pub fn verify_api_token(&self, api_token: &str) -> bool {
        let Some(hash) = self.auth.api_token_hash.as_deref() else {
            return false;
        };

        verify_secret(hash, api_token)
    }

    pub fn verify_debug_token(&self, token: &str, now_ms: i64) -> bool {
        let Some(hash) = self.auth.debug_token_hash.as_deref() else {
            return false;
        };
        let Some(expires_at) = self.auth.debug_token_expires_at_ms else {
            return false;
        };
        if expires_at <= now_ms {
            return false;
        }
        let expected = hash.strip_prefix("keccak256:").unwrap_or_default();
        let actual = hex::encode(Keccak256::digest(token.as_bytes()));
        constant_time_eq(expected.as_bytes(), actual.as_bytes())
    }

    pub fn set_debug_token(&mut self, token: &str, expires_at_ms: i64) -> anyhow::Result<()> {
        self.auth.debug_token_hash = Some(format!(
            "keccak256:{}",
            hex::encode(Keccak256::digest(token.as_bytes()))
        ));
        self.auth.debug_token_expires_at_ms = Some(expires_at_ms);
        Ok(())
    }

    pub fn revoke_debug_token(&mut self) {
        self.auth.debug_token_hash = None;
        self.auth.debug_token_expires_at_ms = None;
    }

    pub fn set_api_token(&mut self, api_token: &str) -> anyhow::Result<()> {
        self.auth.api_token_hash = Some(hash_secret(api_token, 16)?);
        Ok(())
    }

    pub fn set_password(&mut self, new_password: &str) -> anyhow::Result<()> {
        self.auth.password_hash = Some(hash_secret(new_password.trim(), 8)?);
        Ok(())
    }

    pub fn change_password(
        &mut self,
        current_password: &str,
        new_password: &str,
    ) -> anyhow::Result<()> {
        if !self.verify_password(current_password) {
            bail!("invalid current password");
        }
        self.set_password(new_password)
    }

    pub fn from_setup(input: SetupInput) -> anyhow::Result<Self> {
        let owner_email = normalize_email(&input.owner_email)?;
        let router = router_config_from_setup_url(&input.router_url)?;
        let tunnel_subdomain = match input.client_tunnel_subdomain {
            Some(value) if !value.trim().is_empty() => normalize_subdomain(&value)?,
            _ => crate::domain::subdomain_suggest::generate_memorable_subdomain(
                &mut rand::thread_rng(),
            ),
        };

        Ok(Self {
            auth: AuthConfig {
                password_hash: Some(hash_secret(&input.password, 8)?),
                api_token_hash: None,
                debug_token_hash: None,
                debug_token_expires_at_ms: None,
            },
            owner: OwnerConfig {
                email: Some(owner_email),
                ..OwnerConfig::default()
            },
            router,
            client: ClientConfig {
                tunnel_subdomain: Some(tunnel_subdomain),
                tunnel_status: Some("claimed".to_string()),
                last_heartbeat_ms: None,
            },
            upstream_proxy: UpstreamProxyConfig::default(),
            upgrade_policy: UpgradePolicyConfig::default(),
        })
    }

    pub fn update_router(&mut self, input: UpdateRouterConfigInput) -> anyhow::Result<()> {
        if let Some(url) = input.url {
            self.router.url = Some(normalize_router_url(&url)?);
        }
        if let Some(api_base) = input.api_base {
            self.router.api_base = Some(normalize_router_url(&api_base)?);
        }
        if let Some(domain) = input.domain {
            self.router.domain = optional_trimmed(domain);
        }
        if let Some(region) = input.region {
            self.router.region = optional_trimmed(region);
        }
        if let Some(ssh_host) = input.ssh_host {
            self.router.ssh_host = optional_trimmed(ssh_host);
        }
        if let Some(ssh_user) = input.ssh_user {
            self.router.ssh_user = optional_trimmed(ssh_user);
        }
        if let Some(custom) = input.custom {
            self.router.custom = custom;
        }
        Ok(())
    }

    pub fn router_api_base(&self) -> Option<&str> {
        self.router
            .api_base
            .as_deref()
            .or(self.router.url.as_deref())
            .filter(|value| !value.trim().is_empty())
    }

    pub fn update_client_tunnel(&mut self, input: UpdateClientTunnelInput) -> anyhow::Result<()> {
        if let Some(subdomain) = input.tunnel_subdomain {
            self.client.tunnel_subdomain = Some(normalize_subdomain(&subdomain)?);
        }
        if let Some(status) = input.tunnel_status {
            self.client.tunnel_status = optional_trimmed(status);
        }
        Ok(())
    }

    pub fn update_upstream_proxy(&mut self, input: UpdateUpstreamProxyInput) -> anyhow::Result<()> {
        if input.clear.unwrap_or(false) {
            self.upstream_proxy.url = None;
        }
        if let Some(url) = input.url {
            self.upstream_proxy.url = optional_proxy_url(url)?;
        }
        if let Some(follow_system_proxy) = input.follow_system_proxy {
            self.upstream_proxy.follow_system_proxy = follow_system_proxy;
        }
        Ok(())
    }

    pub fn update_owner_payout_profile(
        &mut self,
        profile: PayoutProfile,
        updated_at_ms: i64,
    ) -> anyhow::Result<PayoutProfileState> {
        if updated_at_ms <= 0 {
            bail!("payout profile update timestamp must be positive");
        }
        let profile = profile.validate_and_normalize()?;
        self.owner.payout_profile = PayoutProfileState {
            schema_version: PAYOUT_PROFILE_SCHEMA_VERSION,
            revision: self.owner.payout_profile.revision.saturating_add(1).max(1),
            profile: Some(profile),
            updated_at_ms,
        };
        self.owner.payout_profile_sync.last_error = None;
        Ok(self.owner.payout_profile.clone())
    }

    pub fn clear_owner_payout_profile(
        &mut self,
        updated_at_ms: i64,
    ) -> anyhow::Result<PayoutProfileState> {
        if updated_at_ms <= 0 {
            bail!("payout profile update timestamp must be positive");
        }
        self.owner.payout_profile = PayoutProfileState {
            schema_version: PAYOUT_PROFILE_SCHEMA_VERSION,
            revision: self.owner.payout_profile.revision.saturating_add(1).max(1),
            profile: None,
            updated_at_ms,
        };
        self.owner.payout_profile_sync.last_error = None;
        Ok(self.owner.payout_profile.clone())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRouterConfigInput {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub ssh_user: Option<String>,
    #[serde(default)]
    pub custom: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateClientTunnelInput {
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub tunnel_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUpstreamProxyInput {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
    #[serde(default)]
    pub follow_system_proxy: Option<bool>,
}

pub fn config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(CONFIG_FILE_NAME)
}

pub fn normalize_email(email: &str) -> anyhow::Result<String> {
    let value = email.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.contains(char::is_whitespace)
        || value.matches('@').count() != 1
        || value.starts_with('@')
        || value.ends_with('@')
        || !value.rsplit_once('@').is_some_and(|(_, domain)| {
            domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        })
    {
        bail!("owner email format is invalid");
    }
    Ok(value)
}

fn normalize_router_url(router_url: &str) -> anyhow::Result<String> {
    let value = router_url.trim().trim_end_matches('/').to_string();
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        bail!("router url must start with http:// or https://");
    }
    Ok(value)
}

fn router_config_from_setup_url(router_url: &str) -> anyhow::Result<RouterConfig> {
    let url = normalize_router_url(router_url)?;
    let domain = router_domain_from_url(Some(&url))
        .map(|value| normalize_share_router_domain(&value).unwrap_or(value));
    let region = domain
        .as_deref()
        .and_then(share_router_region_for_domain)
        .map(str::to_string);
    Ok(RouterConfig {
        url: Some(url),
        api_base: None,
        domain,
        region,
        ssh_host: None,
        ssh_user: None,
        custom: false,
        identity: None,
        last_register_error: None,
        last_registered_at_ms: None,
    })
}

fn optional_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn optional_proxy_url(value: String) -> anyhow::Result<Option<String>> {
    let Some(value) = optional_trimmed(value) else {
        return Ok(None);
    };
    validate_proxy_url(&value)?;
    Ok(Some(value))
}

pub fn validate_proxy_url(value: &str) -> anyhow::Result<()> {
    let scheme = value
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .ok_or_else(|| anyhow::anyhow!("proxy url must include a scheme"))?;
    if !matches!(scheme.as_str(), "http" | "https" | "socks5" | "socks5h") {
        bail!("proxy url scheme must be one of http, https, socks5, socks5h");
    }
    reqwest::Proxy::all(value)
        .with_context(|| format!("invalid proxy url {}", mask_proxy_url(value)))?;
    Ok(())
}

pub fn mask_proxy_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    match rest.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}"),
        None => value.to_string(),
    }
}

fn default_follow_system_proxy() -> bool {
    true
}

fn normalize_subdomain(subdomain: &str) -> anyhow::Result<String> {
    let value = subdomain.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 63
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        bail!("client tunnel subdomain format is invalid");
    }
    Ok(value)
}

fn verify_secret(hash: &str, secret: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(hash) else {
        return false;
    };

    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed_hash)
        .is_ok()
}

fn hash_secret(secret: &str, min_len: usize) -> anyhow::Result<String> {
    if secret.len() < min_len {
        bail!("secret must be at least {min_len} characters");
    }

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow::anyhow!("hash secret: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_generates_memorable_subdomain_when_blank() {
        let config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "Alice.Example@Example.COM".to_string(),
            router_url: "https://router.example.com/".to_string(),
            client_tunnel_subdomain: None,
            options: None,
        })
        .unwrap();

        assert_eq!(
            config.owner.email.as_deref(),
            Some("alice.example@example.com")
        );
        assert_eq!(
            config.router.url.as_deref(),
            Some("https://router.example.com")
        );
        assert_eq!(config.router.domain.as_deref(), Some("router.example.com"));
        let subdomain = config.client.tunnel_subdomain.as_deref().unwrap();
        assert!(subdomain.len() >= 6);
        assert!(subdomain.chars().all(|ch| ch.is_ascii_lowercase()));
        assert!(config.verify_password("password123"));
        assert!(!config.verify_password("wrong-password"));
    }

    #[test]
    fn setup_accepts_custom_subdomain() {
        let config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "http://router.local".to_string(),
            client_tunnel_subdomain: Some("route-abc12".to_string()),
            options: None,
        })
        .unwrap();

        assert_eq!(
            config.client.tunnel_subdomain.as_deref(),
            Some("route-abc12")
        );
    }

    #[test]
    fn setup_resolves_known_share_router_domain_and_region() {
        let config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://sgptokenswitch.cc/".to_string(),
            client_tunnel_subdomain: Some("us01".to_string()),
            options: None,
        })
        .unwrap();

        assert_eq!(config.router.domain.as_deref(), Some("sgptokenswitch.cc"));
        assert_eq!(config.router.region.as_deref(), Some("singapore"));
    }

    #[test]
    fn setup_rejects_invalid_email() {
        let result = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "not-an-email".to_string(),
            router_url: "https://router.example.com".to_string(),
            client_tunnel_subdomain: None,
            options: None,
        });

        assert!(result.is_err());
    }

    #[test]
    fn change_password_updates_hash_and_rejects_invalid_current() {
        let mut config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://router.example.com".to_string(),
            client_tunnel_subdomain: Some("owner".to_string()),
            options: None,
        })
        .unwrap();

        assert!(config
            .change_password("password123", "newpassword1")
            .is_ok());
        assert!(config.verify_password("newpassword1"));
        assert!(!config.verify_password("password123"));
        assert!(config
            .change_password("wrong-password", "anotherpass1")
            .is_err());
    }

    #[test]
    fn upstream_proxy_update_validates_and_can_clear_url() {
        let mut config = ServerConfig::empty();

        config
            .update_upstream_proxy(UpdateUpstreamProxyInput {
                url: Some("socks5h://user:pass@127.0.0.1:1080".to_string()),
                clear: None,
                follow_system_proxy: Some(false),
            })
            .unwrap();
        assert_eq!(
            config.upstream_proxy.url.as_deref(),
            Some("socks5h://user:pass@127.0.0.1:1080")
        );
        assert!(!config.upstream_proxy.follow_system_proxy);

        config
            .update_upstream_proxy(UpdateUpstreamProxyInput {
                url: None,
                clear: Some(true),
                follow_system_proxy: None,
            })
            .unwrap();
        assert!(config.upstream_proxy.url.is_none());

        assert!(config
            .update_upstream_proxy(UpdateUpstreamProxyInput {
                url: Some("ftp://127.0.0.1:21".to_string()),
                clear: None,
                follow_system_proxy: None,
            })
            .is_err());
    }

    #[test]
    fn payout_profile_normalizes_address_networks_and_increments_revision() {
        let mut config = ServerConfig::empty();
        let first = config
            .update_owner_payout_profile(
                PayoutProfile {
                    address_type: PayoutAddressType::Evm,
                    address: "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
                    token: PayoutToken::USDC,
                    networks: vec![
                        PayoutNetwork::ArbitrumOne,
                        PayoutNetwork::Bsc,
                        PayoutNetwork::Bsc,
                    ],
                    verification_status: PayoutVerificationStatus::SelfDeclared,
                },
                100,
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(
            first.profile.as_ref().unwrap().address,
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );
        assert_eq!(
            first.profile.as_ref().unwrap().networks,
            vec![PayoutNetwork::Bsc, PayoutNetwork::ArbitrumOne]
        );

        let cleared = config.clear_owner_payout_profile(200).unwrap();
        assert_eq!(cleared.revision, 2);
        assert!(cleared.profile.is_none());
        assert_eq!(cleared.updated_at_ms, 200);
    }

    #[test]
    fn payout_profile_rejects_invalid_mixed_case_checksum_and_empty_networks() {
        let invalid_checksum = PayoutProfile {
            address_type: PayoutAddressType::Evm,
            address: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAee".into(),
            token: PayoutToken::USDT,
            networks: vec![PayoutNetwork::Base],
            verification_status: PayoutVerificationStatus::SelfDeclared,
        };
        assert!(invalid_checksum.validate_and_normalize().is_err());

        let no_networks = PayoutProfile {
            address_type: PayoutAddressType::Evm,
            address: "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
            token: PayoutToken::USDT,
            networks: vec![],
            verification_status: PayoutVerificationStatus::SelfDeclared,
        };
        assert!(no_networks.validate_and_normalize().is_err());
    }

    #[test]
    fn payout_state_canonical_json_matches_router_signing_contract() {
        let state = PayoutProfileState {
            schema_version: 1,
            revision: 3,
            profile: Some(PayoutProfile {
                address_type: PayoutAddressType::Evm,
                address: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".into(),
                token: PayoutToken::USDT,
                networks: vec![PayoutNetwork::Bsc, PayoutNetwork::Base],
                verification_status: PayoutVerificationStatus::SelfDeclared,
            }),
            updated_at_ms: 1_753_000_000_000,
        };
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"schemaVersion":1,"revision":3,"profile":{"addressType":"evm","address":"0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed","token":"USDT","networks":["eip155:56","eip155:8453"],"verificationStatus":"self_declared"},"updatedAtMs":1753000000000}"#
        );
    }

    #[test]
    fn proxy_url_mask_hides_credentials() {
        assert_eq!(
            mask_proxy_url("http://user:pass@proxy.example.com:8080"),
            "http://proxy.example.com:8080"
        );
        assert_eq!(
            mask_proxy_url("socks5h://127.0.0.1:1080"),
            "socks5h://127.0.0.1:1080"
        );
    }

    #[test]
    fn debug_token_expires_and_can_be_revoked() {
        let mut config = ServerConfig::empty();
        config.set_debug_token("temporary-secret", 2_000).unwrap();
        assert!(config.verify_debug_token("temporary-secret", 1_999));
        assert!(!config.verify_debug_token("wrong-secret", 1_999));
        assert!(!config.verify_debug_token("temporary-secret", 2_000));
        config.revoke_debug_token();
        assert!(!config.verify_debug_token("temporary-secret", 1_000));
    }
}
