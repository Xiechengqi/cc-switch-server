use std::fs;
use std::path::Path;

use crate::domain::router::ClientSubdomain;
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

use crate::domain::providers::runtime::ProviderRuntimeDefaults;

const CONFIG_FILE_NAME: &str = "server.json";
pub const ROUTER_CONTROL_DB_FILE_NAME: &str = "router-control.sqlite";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub auth: AuthConfig,
    pub owner: OwnerConfig,
    pub router: RouterConfig,
    pub client: ClientConfig,
    #[serde(default)]
    pub setup_completion_notification: Option<SetupCompletionNotificationState>,
    #[serde(default)]
    pub upgrade_policy: UpgradePolicyConfig,
    #[serde(default)]
    pub provider_runtime_defaults: ProviderRuntimeDefaults,
    /// Web ops terminal entry. Default on; PTY still lazy-created on first attach.
    /// Disable via `enableWebTerminal: false` or `CC_SWITCH_ENABLE_WEB_TERMINAL=0|false|off`.
    #[serde(default = "default_true")]
    pub enable_web_terminal: bool,
    /// 本地请求体上限（MB）。生效值是 `min(本地上限, Router 声明上限)`；
    /// 默认取 Router 允许的最大档位，因此默认由 Router settings 决定实际天花板。
    /// 改动需要重启进程（路由层的 `DefaultBodyLimit` 是静态 layer）。
    #[serde(default)]
    pub request_body_limits: RequestBodyLimitsConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupCompletionNotificationStatus {
    WaitingForClaim,
    Pending,
    Acknowledged,
    TerminalFailed,
}

impl SetupCompletionNotificationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForClaim => "waiting_for_claim",
            Self::Pending => "pending",
            Self::Acknowledged => "acknowledged",
            Self::TerminalFailed => "terminal_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupCompletionNotificationState {
    pub setup_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hint: Option<String>,
    pub status: SetupCompletionNotificationStatus,
    #[serde(default)]
    pub attempt_count: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_ack_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl SetupCompletionNotificationState {
    pub fn new(setup_id: String, password_hint: String, now_ms: i64) -> Self {
        Self {
            setup_id,
            password_hint: Some(password_hint),
            status: SetupCompletionNotificationStatus::WaitingForClaim,
            attempt_count: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_attempt_at_ms: None,
            next_attempt_at_ms: None,
            acknowledged_at_ms: None,
            router_ack_status: None,
            last_error: None,
        }
    }
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterIdentity {
    pub installation_id: String,
    pub public_key: String,
    pub private_key: String,
    #[serde(default)]
    pub control_secret: Option<String>,
}

impl RouterIdentity {
    pub fn is_registered(&self) -> bool {
        !self.installation_id.trim().is_empty()
            && !self.public_key.trim().is_empty()
            && !self.private_key.trim().is_empty()
    }

    pub fn has_keypair(&self) -> bool {
        !self.public_key.trim().is_empty() && !self.private_key.trim().is_empty()
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_pending: Option<ClientTunnelClaimIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain_adoption: Option<ClientSubdomainAdoption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSubdomainAdoptionStatus {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientSubdomainAdoption {
    pub takeover_id: String,
    pub from_subdomain: String,
    pub to_subdomain: String,
    pub status: ClientSubdomainAdoptionStatus,
    pub activate_at_ms: i64,
    pub prepared_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientTunnelClaimIntent {
    pub installation_id: String,
    pub router_api_base: String,
    pub owner_email: String,
    pub subdomain: String,
}

impl ClientTunnelClaimIntent {
    pub fn from_config(config: &ServerConfig) -> anyhow::Result<Self> {
        let installation_id = config
            .registered_router_identity()
            .map(|identity| identity.installation_id.trim())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
        let router_api_base = config
            .router_api_base()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
            .trim_end_matches('/');
        let owner_email = config
            .owner
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("owner email is not configured"))?;
        let subdomain = config
            .client
            .tunnel_subdomain
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))?;
        Ok(Self {
            installation_id: installation_id.to_string(),
            router_api_base: router_api_base.to_string(),
            owner_email: owner_email.to_ascii_lowercase(),
            subdomain: subdomain.to_string(),
        })
    }

    pub fn matches_config(&self, config: &ServerConfig) -> bool {
        Self::from_config(config).is_ok_and(|current| current == *self)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

// ── 请求体上限（本地主权上限） ───────────────────────────────────────────────
//
// Router 会在每个 ingress 请求上声明它自己对该请求应用的上限
// (`x-cc-switch-ingress-body-limit`)。Client 的生效值是
// `min(本地上限, Router 声明上限)`：Router 只能在本地上限之内放宽，
// 卖家永远保留一个自己说了算的天花板。
//
// 默认值取 Router 允许配置的最大档位，因此**开箱即用时 Router settings 直接生效**，
// 不会出现第二道隐藏的 413。只有明确希望限制内存占用的卖家才需要调小它。
// 注意：请求体整体驻留内存，峰值 ≈ 上限 × 并发请求数（重试路径还会额外持有一份原始 body）。

/// 单档上限的最小值（MB）。
pub const MIN_REQUEST_BODY_LIMIT_MB: u64 = 1;
/// 普通 API（`/v1/responses`、`/v1/messages` 等）的上限区间上界（MB）。
pub const MAX_REQUEST_BODY_LIMIT_MB: u64 = 64;
/// 视频/图片档的上限区间上界（MB）。
pub const MAX_MEDIA_REQUEST_BODY_LIMIT_MB: u64 = 256;

const DEFAULT_REQUEST_BODY_LIMIT_MB: u64 = MAX_REQUEST_BODY_LIMIT_MB;
const DEFAULT_MEDIA_REQUEST_BODY_LIMIT_MB: u64 = MAX_MEDIA_REQUEST_BODY_LIMIT_MB;
const DEFAULT_IMAGE_REQUEST_BODY_LIMIT_MB: u64 = MAX_MEDIA_REQUEST_BODY_LIMIT_MB;

const REQUEST_BODY_LIMIT_ENV: &str = "CC_SWITCH_REQUEST_BODY_LIMIT_MB";
const MEDIA_REQUEST_BODY_LIMIT_ENV: &str = "CC_SWITCH_MEDIA_REQUEST_BODY_LIMIT_MB";
const IMAGE_REQUEST_BODY_LIMIT_ENV: &str = "CC_SWITCH_IMAGE_REQUEST_BODY_LIMIT_MB";

fn default_request_body_limit_mb() -> u64 {
    DEFAULT_REQUEST_BODY_LIMIT_MB
}

fn default_media_request_body_limit_mb() -> u64 {
    DEFAULT_MEDIA_REQUEST_BODY_LIMIT_MB
}

fn default_image_request_body_limit_mb() -> u64 {
    DEFAULT_IMAGE_REQUEST_BODY_LIMIT_MB
}

/// `server.json` 中 `requestBodyLimits` 的持久化形态，单位 MB。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBodyLimitsConfig {
    /// 普通 API 档。
    #[serde(default = "default_request_body_limit_mb")]
    pub default_mb: u64,
    /// `/v1/videos/generations` 档。
    #[serde(default = "default_media_request_body_limit_mb")]
    pub media_mb: u64,
    /// `/v1/images/{generations,edits}` 档。
    #[serde(default = "default_image_request_body_limit_mb")]
    pub image_mb: u64,
}

impl Default for RequestBodyLimitsConfig {
    fn default() -> Self {
        Self {
            default_mb: DEFAULT_REQUEST_BODY_LIMIT_MB,
            media_mb: DEFAULT_MEDIA_REQUEST_BODY_LIMIT_MB,
            image_mb: DEFAULT_IMAGE_REQUEST_BODY_LIMIT_MB,
        }
    }
}

/// 已解析为字节的本地上限快照。启动时定格一次：路由层的
/// `DefaultBodyLimit` 是静态 layer，改这些值需要重启进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestBodyLimits {
    pub default_bytes: usize,
    pub media_bytes: usize,
    pub image_bytes: usize,
}

impl RequestBodyLimits {
    /// 按请求路径挑选所属档位。路径匹配规则与 Router 的
    /// `proxy_request_body_limit()` 保持一致。
    pub fn for_path(&self, path: &str) -> usize {
        match path.split_once('?').map_or(path, |(path, _)| path) {
            "/v1/images/generations"
            | "/images/generations"
            | "/v1/images/edits"
            | "/images/edits" => self.image_bytes,
            "/v1/videos/generations" | "/videos/generations" => self.media_bytes,
            _ => self.default_bytes,
        }
    }
}

impl Default for RequestBodyLimits {
    fn default() -> Self {
        RequestBodyLimitsConfig::default().resolve()
    }
}

/// 读取一个 MB 环境变量覆盖；非法值忽略并告警，避免打错一个字符就把上限压到 1 MB。
fn env_limit_mb_override(key: &str, min: u64, max: u64) -> Option<u64> {
    let raw = std::env::var(key).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(value) if (min..=max).contains(&value) => Some(value),
        _ => {
            tracing::warn!(
                env = key,
                value = trimmed,
                min,
                max,
                "ignoring out-of-range request body limit override"
            );
            None
        }
    }
}

fn mb_to_bytes(megabytes: u64) -> usize {
    usize::try_from(megabytes.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX)
}

impl RequestBodyLimitsConfig {
    /// 解析为字节，并应用 env 覆盖与区间钳制。
    ///
    /// 越界的 `server.json` 值被钳制而不是拒绝启动：这是一个内存旋钮，
    /// 不值得让一台已经在跑的卖家机器因为手改配置而起不来。
    pub fn resolve(&self) -> RequestBodyLimits {
        let default_mb = env_limit_mb_override(
            REQUEST_BODY_LIMIT_ENV,
            MIN_REQUEST_BODY_LIMIT_MB,
            MAX_REQUEST_BODY_LIMIT_MB,
        )
        .unwrap_or(self.default_mb)
        .clamp(MIN_REQUEST_BODY_LIMIT_MB, MAX_REQUEST_BODY_LIMIT_MB);
        let media_mb = env_limit_mb_override(
            MEDIA_REQUEST_BODY_LIMIT_ENV,
            MIN_REQUEST_BODY_LIMIT_MB,
            MAX_MEDIA_REQUEST_BODY_LIMIT_MB,
        )
        .unwrap_or(self.media_mb)
        .clamp(MIN_REQUEST_BODY_LIMIT_MB, MAX_MEDIA_REQUEST_BODY_LIMIT_MB);
        let image_mb = env_limit_mb_override(
            IMAGE_REQUEST_BODY_LIMIT_ENV,
            MIN_REQUEST_BODY_LIMIT_MB,
            MAX_MEDIA_REQUEST_BODY_LIMIT_MB,
        )
        .unwrap_or(self.image_mb)
        .clamp(MIN_REQUEST_BODY_LIMIT_MB, MAX_MEDIA_REQUEST_BODY_LIMIT_MB);
        // 媒体档不得低于普通档：否则一个图片请求会比同样大小的文本请求先被拒。
        RequestBodyLimits {
            default_bytes: mb_to_bytes(default_mb),
            media_bytes: mb_to_bytes(media_mb.max(default_mb)),
            image_bytes: mb_to_bytes(image_mb.max(default_mb)),
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
            setup_completion_notification: None,
            upgrade_policy: UpgradePolicyConfig::default(),
            provider_runtime_defaults: ProviderRuntimeDefaults::default(),
            enable_web_terminal: true,
            request_body_limits: RequestBodyLimitsConfig::default(),
        }
    }

    /// Effective web terminal switch: env overrides `server.json` when set.
    pub fn is_web_terminal_enabled(&self) -> bool {
        if let Ok(value) = std::env::var("CC_SWITCH_ENABLE_WEB_TERMINAL") {
            let normalized = value.trim().to_ascii_lowercase();
            if matches!(normalized.as_str(), "0" | "false" | "no" | "off") {
                return false;
            }
            if matches!(normalized.as_str(), "1" | "true" | "yes" | "on") {
                return true;
            }
        }
        self.enable_web_terminal
    }

    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let config_path = config_path(config_dir);
        if !config_path.exists() {
            return Ok(Self::empty());
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("read config {}", config_path.display()))?;
        let config: Self = serde_json::from_str(&content)
            .with_context(|| format!("parse config {}", config_path.display()))?;
        config
            .provider_runtime_defaults
            .validate()
            .with_context(|| format!("validate config {}", config_path.display()))?;
        Ok(config)
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        self.provider_runtime_defaults
            .validate()
            .context("validate Provider runtime defaults")?;
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;

        let config_path = config_path(config_dir);
        crate::infra::storage::write_json_pretty(&config_path, self)
            .with_context(|| format!("write config {}", config_path.display()))
    }

    pub fn is_local_setup_complete(&self) -> bool {
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

    pub fn is_router_client_ready(&self) -> bool {
        if self
            .router
            .url
            .as_deref()
            .is_none_or(|value| value.is_empty())
        {
            return true;
        }
        if matches!(
            self.client.tunnel_status.as_deref(),
            Some("claimed_remote")
                | Some("connected")
                | Some("active")
                | Some("running")
                | Some("claim_skipped")
        ) {
            return true;
        }
        if !self.has_registered_router_identity() {
            return false;
        }
        false
    }

    pub fn is_setup_complete(&self) -> bool {
        self.is_local_setup_complete() && self.is_router_client_ready()
    }

    pub fn has_registered_router_identity(&self) -> bool {
        self.router
            .identity
            .as_ref()
            .is_some_and(RouterIdentity::is_registered)
    }

    pub fn registered_router_identity(&self) -> Option<&RouterIdentity> {
        self.router
            .identity
            .as_ref()
            .filter(|identity| identity.is_registered())
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
            _ => {
                crate::domain::subdomain_suggest::generate_client_subdomain(&mut rand::thread_rng())
            }
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
            },
            router,
            client: ClientConfig {
                tunnel_subdomain: Some(tunnel_subdomain),
                tunnel_status: Some("claimed".to_string()),
                last_heartbeat_ms: None,
                claim_pending: None,
                subdomain_adoption: None,
            },
            setup_completion_notification: None,
            upgrade_policy: UpgradePolicyConfig::default(),
            provider_runtime_defaults: ProviderRuntimeDefaults::default(),
            enable_web_terminal: true,
            request_body_limits: RequestBodyLimitsConfig::default(),
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
            let subdomain = normalize_subdomain(&subdomain)?;
            if self.client.tunnel_subdomain.as_deref() != Some(subdomain.as_str()) {
                bail!(
                    "client_subdomain_immutable: client subdomain can only be chosen during setup"
                );
            }
        }
        if let Some(status) = input.tunnel_status {
            self.client.tunnel_status = optional_trimmed(status);
        }
        Ok(())
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

pub fn config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(CONFIG_FILE_NAME)
}

pub fn router_control_db_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(ROUTER_CONTROL_DB_FILE_NAME)
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

fn normalize_subdomain(subdomain: &str) -> anyhow::Result<String> {
    let value = subdomain.trim().to_ascii_lowercase();
    ClientSubdomain::parse(&value)
        .map_err(|_| anyhow::anyhow!("client tunnel subdomain format is invalid"))?;
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
    fn request_body_limits_default_to_the_router_ceiling() {
        // 默认不引入第二道隐藏闸门：本地上限取 Router 允许配置的最大档位，
        // 因此 `min(本地, Router 声明)` 恒等于 Router 声明。
        let limits = RequestBodyLimitsConfig::default().resolve();
        assert_eq!(limits.default_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.media_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.image_bytes, 256 * 1024 * 1024);
    }

    #[test]
    fn request_body_limits_clamp_out_of_range_values_instead_of_failing() {
        let limits = RequestBodyLimitsConfig {
            default_mb: 0,
            media_mb: 100_000,
            image_mb: 100_000,
        }
        .resolve();
        assert_eq!(limits.default_bytes, 1024 * 1024);
        assert_eq!(limits.media_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.image_bytes, 256 * 1024 * 1024);
    }

    #[test]
    fn media_lanes_never_fall_below_the_default_lane() {
        let limits = RequestBodyLimitsConfig {
            default_mb: 64,
            media_mb: 8,
            image_mb: 8,
        }
        .resolve();
        assert_eq!(limits.default_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.media_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.image_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn request_body_limit_lane_selection_matches_the_router_path_table() {
        let limits = RequestBodyLimits {
            default_bytes: 1,
            media_bytes: 2,
            image_bytes: 3,
        };
        for path in [
            "/v1/images/generations",
            "/images/generations",
            "/v1/images/edits",
            "/images/edits",
            "/v1/images/edits?mask=true",
        ] {
            assert_eq!(limits.for_path(path), 3, "{path}");
        }
        for path in ["/v1/videos/generations", "/videos/generations"] {
            assert_eq!(limits.for_path(path), 2, "{path}");
        }
        for path in ["/v1/responses", "/v1/messages?beta=true", "/v1beta/models"] {
            assert_eq!(limits.for_path(path), 1, "{path}");
        }
    }

    #[test]
    fn request_body_limits_deserialize_field_by_field() {
        // 老的 server.json 没有这一段：整段缺失走 `#[serde(default)]`，
        // 单个字段缺失走各自的 `default_*` 函数。
        let full: RequestBodyLimitsConfig = serde_json::from_str("{}").expect("parse empty object");
        assert_eq!(full, RequestBodyLimitsConfig::default());

        let partial: RequestBodyLimitsConfig =
            serde_json::from_str(r#"{"defaultMb":8}"#).expect("parse partial object");
        assert_eq!(partial.default_mb, 8);
        assert_eq!(
            partial.media_mb,
            RequestBodyLimitsConfig::default().media_mb
        );
        assert_eq!(
            partial.image_mb,
            RequestBodyLimitsConfig::default().image_mb
        );
    }

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
    fn provider_runtime_defaults_round_trip_in_server_config() {
        let mut config = ServerConfig::empty();
        config.provider_runtime_defaults.transport.timeout_ms = 75_000;
        config.provider_runtime_defaults.test_models.codex = "test-codex".to_string();

        let encoded = serde_json::to_value(&config).unwrap();
        let decoded: ServerConfig = serde_json::from_value(encoded).unwrap();

        assert_eq!(
            decoded.provider_runtime_defaults,
            config.provider_runtime_defaults
        );
    }

    #[test]
    fn client_subdomain_is_immutable_after_setup() {
        let mut config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://router.example.com".to_string(),
            client_tunnel_subdomain: Some("client-alpha".to_string()),
            options: None,
        })
        .unwrap();

        config
            .update_client_tunnel(UpdateClientTunnelInput {
                tunnel_subdomain: Some("client-alpha".to_string()),
                tunnel_status: Some("connected".to_string()),
            })
            .expect("same subdomain and status update must remain valid");
        assert_eq!(config.client.tunnel_status.as_deref(), Some("connected"));

        let error = config
            .update_client_tunnel(UpdateClientTunnelInput {
                tunnel_subdomain: Some("client-beta".to_string()),
                tunnel_status: None,
            })
            .expect_err("setup subdomain must not be replaceable");
        assert!(error.to_string().contains("client_subdomain_immutable"));
        assert_eq!(
            config.client.tunnel_subdomain.as_deref(),
            Some("client-alpha")
        );
    }

    #[test]
    fn setup_resolves_known_share_router_domain_and_region() {
        let config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://sgptokenswitch.cc/".to_string(),
            client_tunnel_subdomain: Some("us-east".to_string()),
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

    fn sample_local_complete_config() -> ServerConfig {
        let mut config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://router.example.com".to_string(),
            client_tunnel_subdomain: Some("client-alpha".to_string()),
            options: None,
        })
        .unwrap();
        config.router.identity = Some(RouterIdentity {
            installation_id: "inst-test".to_string(),
            public_key: "public-key".to_string(),
            private_key: "private-key".to_string(),
            control_secret: None,
        });
        config.client.tunnel_status = None;
        config
    }

    #[test]
    fn is_setup_complete_requires_router_claim_when_router_configured() {
        let mut config = sample_local_complete_config();
        assert!(!config.is_setup_complete());
        assert!(config.is_local_setup_complete());
        assert!(!config.is_router_client_ready());

        config.client.tunnel_status = Some("claimed_remote".to_string());
        assert!(config.is_setup_complete());

        config.client.tunnel_status = Some("claim_skipped".to_string());
        assert!(config.is_setup_complete());

        config.client.tunnel_status = Some("pending".to_string());
        assert!(!config.is_setup_complete());
    }

    #[test]
    fn is_router_client_ready_skips_claim_when_router_url_missing() {
        let mut config = sample_local_complete_config();
        config.router.url = None;
        config.router.domain = None;
        assert!(config.is_router_client_ready());
        assert!(!config.is_local_setup_complete());
        assert!(!config.is_setup_complete());
    }

    #[test]
    fn change_password_updates_hash_and_rejects_invalid_current() {
        let mut config = ServerConfig::from_setup(SetupInput {
            password: "password123".to_string(),
            owner_email: "owner@example.com".to_string(),
            router_url: "https://router.example.com".to_string(),
            client_tunnel_subdomain: Some("owner1".to_string()),
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
    fn owner_config_ignores_legacy_payout_fields_on_load() {
        let json = r#"{
            "auth": {},
            "owner": {
                "email": "owner@example.com",
                "payoutProfile": {
                    "schemaVersion": 1,
                    "revision": 2,
                    "profile": {
                        "addressType": "evm",
                        "address": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
                        "token": "USDC",
                        "networks": ["eip155:56"],
                        "verificationStatus": "self_declared"
                    },
                    "updatedAtMs": 100
                },
                "payoutProfileSync": {
                    "lastSyncedRevision": 2,
                    "lastSyncedAtMs": 100,
                    "lastError": null
                }
            },
            "router": {},
            "client": {}
        }"#;
        let config: ServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.owner.email.as_deref(), Some("owner@example.com"));
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

    #[test]
    fn pending_router_identity_is_not_treated_as_registered() {
        let mut config = ServerConfig::empty();
        config.router.identity = Some(RouterIdentity {
            installation_id: String::new(),
            public_key: "public-key".to_string(),
            private_key: "private-key".to_string(),
            control_secret: None,
        });

        assert!(!config.has_registered_router_identity());
        config.router.identity.as_mut().unwrap().installation_id = "inst-1".to_string();
        assert!(config.has_registered_router_identity());
    }

    #[test]
    fn web_terminal_defaults_on_and_can_be_disabled() {
        let mut config = ServerConfig::empty();
        assert!(config.is_web_terminal_enabled());
        config.enable_web_terminal = false;
        assert!(!config.is_web_terminal_enabled());
    }
}
