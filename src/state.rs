use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{sleep, Duration};

use crate::api::web::coverage::ProviderCoverage;
use crate::cli::Cli;
use crate::clients::oauth::codex_device::{CodexDeviceFlowStore, PendingCodexDeviceFlow};
use crate::clients::oauth::copilot_device;
use crate::clients::oauth::kiro_device::{
    KiroDeviceFlowStore, PendingKiroDeviceFlow, PendingKiroSocialDeviceFlow,
};
use crate::clients::oauth::quota::{refresh_account_quota, QuotaRefreshResult};
use crate::clients::oauth::refresh::{
    account_needs_native_refresh, execute_native_account_refresh,
};
use crate::clients::router::client::{
    self, IssueLeaseResponse, ShareEditAckPayload, ShareEditView,
};
use crate::clients::router::tunnel::{self, LeaseFn, RenewLeaseFn, TunnelSupervisor};
use crate::domain::accounts::login::OAuthLoginStore;
use crate::domain::accounts::managers::AccountRefreshLocks;
use crate::domain::accounts::oauth::oauth_quota_auth_provider_label;
use crate::domain::accounts::store::{Account, AccountRefreshUpdate, AccountStore};
use crate::domain::failover::FailoverStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::ProviderStore;
use crate::domain::settings::config::{
    mask_proxy_url, PayoutProfile, PayoutProfileState, ServerConfig,
};
use crate::domain::settings::ui_settings::{self, UiSettingsStore};
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareRequestLogEntry, ShareSyncOperation,
};
use crate::domain::sharing::shares::{
    Share, ShareInvocation, ShareInvocationRejection, ShareMarketGrantStatus, ShareStore,
};
use crate::domain::usage::pricing::ModelPricingStore;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::logging::{LogTailAccessError, LogTailResponse, SharedLogCapture};
use crate::proxy::cursor::session::CursorSessionManager;

#[derive(Debug)]
pub struct ServerStateInner {
    pub bind_addr: SocketAddr,
    pub config_dir: PathBuf,
    pub web_dist_dir: Option<PathBuf>,
    pub provider_coverage: ProviderCoverage,
    pub(crate) config: RwLock<ServerConfig>,
    pub(crate) providers: RwLock<ProviderStore>,
    pub(crate) accounts: RwLock<AccountStore>,
    pub(crate) failover: RwLock<FailoverStore>,
    pub(crate) pricing: RwLock<ModelPricingStore>,
    pub(crate) usage: RwLock<UsageStore>,
    pub(crate) shares: RwLock<ShareStore>,
    pub(crate) ui_settings: RwLock<UiSettingsStore>,
    pub(crate) sessions: RwLock<Vec<Session>>,
    pub(crate) oauth_logins: RwLock<OAuthLoginStore>,
    pub(crate) copilot_upstream_auth: RwLock<BTreeMap<String, CachedCopilotUpstreamAuth>>,
    grok_media_sessions: Mutex<BTreeMap<String, GrokMediaSessionBinding>>,
    kiro_device_flows: RwLock<KiroDeviceFlowStore>,
    codex_device_flows: RwLock<CodexDeviceFlowStore>,
    pub cursor_sessions: CursorSessionManager,
    pub account_refresh_locks: AccountRefreshLocks,
    pub account_in_flight: Arc<AccountInFlightTracker>,
    pub share_in_flight: Arc<ShareInFlightTracker>,
    pub control_nonces: Arc<ControlNonceCache>,
    pub http_client: RwLock<reqwest::Client>,
    pub events: broadcast::Sender<ServerEvent>,
    pub tunnels: Arc<TunnelSupervisor>,
    pub web_auth: crate::domain::web_auth::WebAuthStore,
    pub debounced_saves: Arc<DebouncedStoreSaves>,
    pub started_at: std::time::Instant,
    pub upgrade: crate::self_update::upgrade::SharedUpgradeRegistry,
    pub(crate) log_capture: SharedLogCapture,
}

pub type ServerState = Arc<ServerStateInner>;

#[derive(Debug)]
pub enum ManagedAccountRefreshError {
    Conflict { provider_type: ProviderType },
    NotFound,
    Refresh { status_code: u16, message: String },
}

#[derive(Debug)]
pub enum CopilotUpstreamAuthError {
    NotFound,
    MissingGitHubToken { account_id: String },
    TokenExchange { status_code: u16, message: String },
}

#[derive(Debug)]
pub enum DeepSeekUpstreamError {
    NotFound,
    MissingToken,
    Client(String),
}

#[derive(Debug, Clone)]
pub struct CopilotUpstreamAuth {
    pub account_id: String,
    pub token: String,
    pub api_endpoint: String,
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedCopilotUpstreamAuth {
    token: String,
    api_endpoint: String,
    expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct GrokMediaSessionBinding {
    pub provider_id: String,
    pub account_id: Option<String>,
    pub expires_at_ms: i64,
}

pub fn backup_targets(config_dir: &Path) -> Vec<PathBuf> {
    vec![
        crate::domain::settings::config::config_path(config_dir),
        crate::clients::router::email_auth::email_auth_path(config_dir),
        crate::domain::providers::store::providers_path(config_dir),
        crate::domain::accounts::store::accounts_path(config_dir),
        crate::domain::accounts::store::accounts_key_path(config_dir),
        crate::domain::failover::failover_path(config_dir),
        crate::domain::usage::pricing::model_pricing_path(config_dir),
        crate::domain::usage::store::usage_path(config_dir),
        crate::domain::sharing::shares::shares_path(config_dir),
        crate::clients::router::tunnel::tunnels_path(config_dir),
    ]
}

#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEvent {
    pub event_type: String,
    pub resource: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    pub created_at_ms: u128,
}

impl ServerEvent {
    pub fn new(event_type: impl Into<String>, resource: impl Into<String>) -> Self {
        Self {
            event_type: event_type.into(),
            resource: resource.into(),
            id: None,
            app: None,
            message: None,
            auth_provider: None,
            account_id: None,
            success: None,
            created_at_ms: crate::infra::time::now_ms(),
        }
    }

    pub fn auth_provider(mut self, auth_provider: impl Into<String>) -> Self {
        self.auth_provider = Some(auth_provider.into());
        self
    }

    pub fn account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    pub fn success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn app(mut self, app: AppKind) -> Self {
        self.app = Some(app);
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[derive(Debug, Default)]
pub struct ShareInFlightTracker {
    counts: Mutex<BTreeMap<String, u32>>,
}

#[derive(Debug)]
pub struct ShareInFlightGuard {
    tracker: Arc<ShareInFlightTracker>,
    share_id: String,
}

#[derive(Debug, Default)]
pub struct AccountInFlightTracker {
    counts: Mutex<BTreeMap<String, u32>>,
}

#[derive(Debug, Clone, Default)]
pub struct AccountInFlightSnapshot {
    counts: BTreeMap<String, u32>,
}

#[derive(Debug)]
pub struct AccountInFlightGuard {
    tracker: Arc<AccountInFlightTracker>,
    key: String,
    provider_type: String,
    account_id: String,
    max_concurrent: u32,
}

#[derive(Debug, Default)]
pub struct ControlNonceCache {
    seen: Mutex<BTreeMap<String, i64>>,
}

#[derive(Debug, Default)]
pub struct DebouncedStoreSaves {
    accounts: StoreSaveDebouncer,
    shares: StoreSaveDebouncer,
}

#[derive(Debug, Default)]
struct StoreSaveDebouncer {
    state: Mutex<StoreSaveDebounceState>,
}

#[derive(Debug, Default)]
struct StoreSaveDebounceState {
    scheduled: bool,
    dirty: bool,
}

impl ShareInFlightTracker {
    pub fn try_acquire(
        self: &Arc<Self>,
        share_id: &str,
        parallel_limit: Option<u32>,
    ) -> Option<ShareInFlightGuard> {
        let mut counts = self.counts.lock().ok()?;
        let current = *counts.get(share_id).unwrap_or(&0);
        if parallel_limit.is_some_and(|limit| current >= limit) {
            return None;
        }
        counts.insert(share_id.to_string(), current.saturating_add(1));
        Some(ShareInFlightGuard {
            tracker: self.clone(),
            share_id: share_id.to_string(),
        })
    }

    fn release(&self, share_id: &str) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        let Some(current) = counts.get_mut(share_id) else {
            return;
        };
        if *current <= 1 {
            counts.remove(share_id);
        } else {
            *current -= 1;
        }
    }
}

impl Drop for ShareInFlightGuard {
    fn drop(&mut self) {
        self.tracker.release(&self.share_id);
    }
}

impl AccountInFlightTracker {
    pub fn snapshot(&self) -> AccountInFlightSnapshot {
        let counts = self
            .counts
            .lock()
            .map(|counts| counts.clone())
            .unwrap_or_default();
        AccountInFlightSnapshot { counts }
    }

    pub fn try_acquire(
        self: &Arc<Self>,
        provider_type: ProviderType,
        account_id: &str,
        max_concurrent: u32,
    ) -> Option<AccountInFlightGuard> {
        let key = account_in_flight_key(provider_type, account_id);
        let mut counts = self.counts.lock().ok()?;
        let current = *counts.get(&key).unwrap_or(&0);
        if current >= max_concurrent {
            return None;
        }
        let next = current.saturating_add(1);
        counts.insert(key.clone(), next);
        let provider_type_label = provider_type.as_str().to_string();
        crate::metrics::record_account_inflight(
            &provider_type_label,
            account_id,
            next,
            max_concurrent,
        );
        Some(AccountInFlightGuard {
            tracker: self.clone(),
            key,
            provider_type: provider_type_label,
            account_id: account_id.to_string(),
            max_concurrent,
        })
    }

    fn release(&self, key: &str, provider_type: &str, account_id: &str, max_concurrent: u32) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        let Some(current) = counts.get_mut(key) else {
            return;
        };
        let next = current.saturating_sub(1);
        if next == 0 {
            counts.remove(key);
        } else {
            *current = next;
        }
        crate::metrics::record_account_inflight(provider_type, account_id, next, max_concurrent);
    }
}

impl AccountInFlightSnapshot {
    pub fn current(&self, provider_type: ProviderType, account_id: &str) -> u32 {
        self.counts
            .get(&account_in_flight_key(provider_type, account_id))
            .copied()
            .unwrap_or_default()
    }
}

impl Drop for AccountInFlightGuard {
    fn drop(&mut self) {
        self.tracker.release(
            &self.key,
            &self.provider_type,
            &self.account_id,
            self.max_concurrent,
        );
    }
}

fn account_in_flight_key(provider_type: ProviderType, account_id: &str) -> String {
    format!("{}:{account_id}", provider_type.as_str())
}

impl CachedCopilotUpstreamAuth {
    fn is_valid(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .map(|expires_at| expires_at.saturating_sub(60_000) > now_ms)
            .unwrap_or(true)
            && !self.token.trim().is_empty()
            && !self.api_endpoint.trim().is_empty()
    }

    fn into_auth(self, account_id: String) -> CopilotUpstreamAuth {
        CopilotUpstreamAuth {
            account_id,
            token: self.token,
            api_endpoint: self.api_endpoint,
            expires_at_ms: self.expires_at_ms,
        }
    }
}

fn cached_copilot_auth_from_account(
    account: &Account,
    domain: &str,
    now_ms: i64,
) -> Option<CachedCopilotUpstreamAuth> {
    let token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())?;
    let cached = CachedCopilotUpstreamAuth {
        token: token.to_string(),
        api_endpoint: copilot_account_api_endpoint(account, domain),
        expires_at_ms: account.expires_at,
    };
    cached.is_valid(now_ms).then_some(cached)
}

fn copilot_account_domain(account: &Account) -> Result<String, CopilotUpstreamAuthError> {
    let domain = account
        .raw
        .as_ref()
        .and_then(|value| json_string(value, &["/githubDomain", "/github_domain"]))
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| json_string(value, &["/githubDomain", "/github_domain"]))
        })
        .unwrap_or_else(|| "github.com".to_string());
    copilot_device::normalize_github_domain(&domain).map_err(|error| {
        CopilotUpstreamAuthError::TokenExchange {
            status_code: error.status.as_u16(),
            message: error.message,
        }
    })
}

fn copilot_github_token(account: &Account) -> Option<String> {
    account
        .raw
        .as_ref()
        .and_then(|value| json_string(value, &["/githubToken", "/github_token"]))
        .or_else(|| account.refresh_token.clone())
        .or_else(|| account.api_key.clone())
        .or_else(|| {
            account.access_token.clone().filter(|_| {
                account
                    .profile
                    .as_ref()
                    .and_then(|value| json_bool(value, &["/ghes"]))
                    .unwrap_or(false)
            })
        })
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn copilot_account_api_endpoint(account: &Account, domain: &str) -> String {
    account
        .raw
        .as_ref()
        .and_then(|value| {
            json_string(
                value,
                &[
                    "/copilotUsage/endpoints/api",
                    "/copilot_usage/endpoints/api",
                    "/copilotApiBase",
                    "/copilot_api_base",
                ],
            )
        })
        .filter(|endpoint| !endpoint.trim().is_empty())
        .unwrap_or_else(|| copilot_device::copilot_api_base(domain))
}

fn json_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    })
}

fn json_bool(value: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_bool))
}

impl ControlNonceCache {
    pub fn register(&self, installation_id: &str, nonce: &str, now_ms: i64, ttl_ms: i64) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            return false;
        };
        seen.retain(|_, seen_at| now_ms.saturating_sub(*seen_at) <= ttl_ms);
        let key = format!("{installation_id}:{nonce}");
        if seen.contains_key(&key) {
            return false;
        }
        seen.insert(key, now_ms);
        true
    }
}

#[derive(Debug, Clone, Copy)]
enum DebouncedStoreKind {
    Accounts,
    Shares,
}

impl DebouncedStoreSaves {
    fn mark_dirty(&self, kind: DebouncedStoreKind) -> bool {
        let Ok(mut state) = self.debouncer(kind).state.lock() else {
            return true;
        };
        state.dirty = true;
        if state.scheduled {
            return false;
        }
        state.scheduled = true;
        true
    }

    fn begin_flush(&self, kind: DebouncedStoreKind) {
        if let Ok(mut state) = self.debouncer(kind).state.lock() {
            state.dirty = false;
        }
    }

    fn finish_flush(&self, kind: DebouncedStoreKind) -> bool {
        let Ok(mut state) = self.debouncer(kind).state.lock() else {
            return false;
        };
        if state.dirty {
            return true;
        }
        state.scheduled = false;
        false
    }

    fn debouncer(&self, kind: DebouncedStoreKind) -> &StoreSaveDebouncer {
        match kind {
            DebouncedStoreKind::Accounts => &self.accounts,
            DebouncedStoreKind::Shares => &self.shares,
        }
    }
}

fn save_accounts_debounced(state: &ServerState) {
    schedule_debounced_save(state.clone(), DebouncedStoreKind::Accounts);
}

fn save_shares_debounced(state: &ServerState) {
    schedule_debounced_save(state.clone(), DebouncedStoreKind::Shares);
}

fn schedule_debounced_save(state: ServerState, kind: DebouncedStoreKind) {
    if !state.debounced_saves.mark_dirty(kind) {
        return;
    }
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_millis(750)).await;
            state.debounced_saves.begin_flush(kind);
            let result = match kind {
                DebouncedStoreKind::Accounts => state.save_accounts().await,
                DebouncedStoreKind::Shares => state.save_shares().await,
            };
            if let Err(error) = result {
                tracing::warn!(error = %error, kind = ?kind, "debounced store save failed");
            }
            if !state.debounced_saves.finish_flush(kind) {
                break;
            }
        }
    });
}

impl ServerStateInner {
    pub fn load(cli: Cli, log_capture: SharedLogCapture) -> anyhow::Result<ServerState> {
        let config_dir = cli.resolved_config_dir()?;

        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;

        let provider_coverage = ProviderCoverage::load_embedded()?;
        let config = ServerConfig::load_or_default(&config_dir)?;
        crate::domain::providers::migrate::migrate_remove_universal_layer(&config_dir)?;
        let providers = ProviderStore::load_or_default(&config_dir)?;
        let accounts = AccountStore::load_or_default(&config_dir)?;
        let failover = FailoverStore::load_or_default(&config_dir)?;
        let pricing = ModelPricingStore::load_or_default(&config_dir)?;
        let usage = UsageStore::load_or_default(&config_dir)?;
        let shares = ShareStore::load_or_default(&config_dir)?;
        let ui_settings = UiSettingsStore::load_or_default(&config_dir)?;
        log_capture.apply_config(
            &ui_settings::parse_log_config(&ui_settings::log_config_for_frontend(&ui_settings)),
            &config_dir,
        );
        let bind_addr = SocketAddr::new(cli.host, cli.port);
        let http_client = build_http_client(&config, bind_addr)?;
        let (events, _) = broadcast::channel(256);

        let tunnels = TunnelSupervisor::load_or_default(&config_dir)?;
        let web_auth = crate::domain::web_auth::WebAuthStore::load(config_dir.clone());
        let upgrade = Arc::new(crate::self_update::upgrade::UpgradeRegistry::load(
            &config_dir,
        )?);

        Ok(Arc::new(Self {
            bind_addr,
            config_dir,
            web_dist_dir: cli.resolved_web_dist_dir(),
            provider_coverage,
            config: RwLock::new(config),
            providers: RwLock::new(providers),
            accounts: RwLock::new(accounts),
            failover: RwLock::new(failover),
            pricing: RwLock::new(pricing),
            usage: RwLock::new(usage),
            shares: RwLock::new(shares),
            ui_settings: RwLock::new(ui_settings),
            sessions: RwLock::new(Vec::new()),
            oauth_logins: RwLock::new(OAuthLoginStore::default()),
            copilot_upstream_auth: RwLock::new(BTreeMap::new()),
            grok_media_sessions: Mutex::new(BTreeMap::new()),
            kiro_device_flows: RwLock::new(KiroDeviceFlowStore::default()),
            codex_device_flows: RwLock::new(CodexDeviceFlowStore::default()),
            cursor_sessions: CursorSessionManager::default(),
            account_refresh_locks: AccountRefreshLocks::default(),
            account_in_flight: Arc::new(AccountInFlightTracker::default()),
            share_in_flight: Arc::new(ShareInFlightTracker::default()),
            control_nonces: Arc::new(ControlNonceCache::default()),
            http_client: RwLock::new(http_client),
            events,
            tunnels,
            web_auth,
            debounced_saves: Arc::new(DebouncedStoreSaves::default()),
            started_at: std::time::Instant::now(),
            upgrade,
            log_capture,
        }))
    }

    pub async fn sync_log_config_from_ui_settings(&self) {
        let store = self.ui_settings.read().await;
        let config = ui_settings::parse_log_config(&ui_settings::log_config_for_frontend(&store));
        drop(store);
        self.log_capture.apply_config(&config, &self.config_dir);
        let level = if config.enabled {
            config.level.as_str()
        } else {
            "off"
        };
        crate::logging::reload_log_level(level);
    }

    pub async fn read_admin_log_tail(
        &self,
        requested_lines: Option<usize>,
    ) -> Result<LogTailResponse, LogTailAccessError> {
        let store = self.ui_settings.read().await;
        let config = ui_settings::parse_log_config(&ui_settings::log_config_for_frontend(&store));
        drop(store);
        if !config.enabled || !config.api_enabled {
            return Err(LogTailAccessError::Disabled);
        }
        let lines = crate::logging::clamp_tail_lines(requested_lines, config.api_tail_lines);
        Ok(self.log_capture.read_tail(&config, &self.config_dir, lines))
    }

    pub async fn replace_config(&self, config: ServerConfig) -> anyhow::Result<()> {
        let http_client = build_http_client(&config, self.bind_addr)?;
        config.save(&self.config_dir)?;
        *self.http_client.write().await = http_client;
        *self.config.write().await = config;
        Ok(())
    }

    pub async fn config_snapshot(&self) -> ServerConfig {
        self.config.read().await.clone()
    }

    pub async fn update_owner_payout_profile(
        &self,
        profile: PayoutProfile,
    ) -> anyhow::Result<PayoutProfileState> {
        let mut config = self.config.write().await;
        let state = config.update_owner_payout_profile(
            profile,
            crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
        )?;
        config.save(&self.config_dir)?;
        Ok(state)
    }

    pub async fn clear_owner_payout_profile(&self) -> anyhow::Result<PayoutProfileState> {
        let mut config = self.config.write().await;
        let state = config
            .clear_owner_payout_profile(crate::infra::time::now_ms().min(i64::MAX as u128) as i64)?;
        config.save(&self.config_dir)?;
        Ok(state)
    }

    pub async fn mark_payout_profile_sync_success(&self, revision: i64) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        if config.owner.payout_profile.revision == revision {
            config.owner.payout_profile_sync.last_synced_revision = Some(revision);
            config.owner.payout_profile_sync.last_synced_at_ms =
                Some(crate::infra::time::now_ms().min(i64::MAX as u128) as i64);
            config.owner.payout_profile_sync.last_error = None;
            config.save(&self.config_dir)?;
        }
        Ok(())
    }

    pub async fn mark_payout_profile_sync_error(
        &self,
        revision: i64,
        error: String,
    ) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        if config.owner.payout_profile.revision == revision {
            config.owner.payout_profile_sync.last_error = Some(error);
            config.save(&self.config_dir)?;
        }
        Ok(())
    }

    pub async fn reload_persistent_stores(&self) -> anyhow::Result<()> {
        let config = ServerConfig::load_or_default(&self.config_dir)?;
        let http_client = build_http_client(&config, self.bind_addr)?;
        let providers = ProviderStore::load_or_default(&self.config_dir)?;
        let accounts = AccountStore::load_or_default(&self.config_dir)?;
        let failover = FailoverStore::load_or_default(&self.config_dir)?;
        let pricing = ModelPricingStore::load_or_default(&self.config_dir)?;
        let usage = UsageStore::load_or_default(&self.config_dir)?;
        let shares = ShareStore::load_or_default(&self.config_dir)?;
        let ui_settings = UiSettingsStore::load_or_default(&self.config_dir)?;

        *self.http_client.write().await = http_client;
        *self.config.write().await = config;
        *self.providers.write().await = providers;
        *self.accounts.write().await = accounts;
        *self.failover.write().await = failover;
        *self.pricing.write().await = pricing;
        *self.usage.write().await = usage;
        *self.shares.write().await = shares;
        *self.ui_settings.write().await = ui_settings;
        self.tunnels.reload_statuses().await?;
        Ok(())
    }

    pub async fn http_client(&self) -> reqwest::Client {
        self.http_client.read().await.clone()
    }

    pub fn emit_event(&self, event: ServerEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) async fn oauth_quota_refresh_interval_ms(&self) -> i64 {
        let store = self.ui_settings.read().await;
        ui_settings::oauth_quota_refresh_interval_ms(&store)
    }

    pub(crate) async fn oauth_quota_refresh_timeout_ms(&self) -> i64 {
        let store = self.ui_settings.read().await;
        ui_settings::oauth_quota_refresh_timeout_ms(&store)
    }

    pub(crate) fn emit_oauth_quota_updated_event(&self, account: &Account, success: bool) {
        self.emit_event(
            ServerEvent::new("oauth-quota-updated", "quota")
                .auth_provider(oauth_quota_auth_provider_label(account.provider_type))
                .account_id(account.id.clone())
                .success(success),
        );
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    pub async fn save_providers(&self) -> anyhow::Result<()> {
        self.providers.read().await.save(&self.config_dir)
    }

    pub async fn mutate_providers<R>(&self, mutate: impl FnOnce(&mut ProviderStore) -> R) -> R {
        let mut providers = self.providers.write().await;
        mutate(&mut providers)
    }

    pub async fn mutate_providers_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut ProviderStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_providers(mutate).await;
        self.save_providers().await?;
        Ok(result)
    }

    pub async fn mutate_providers_immediate_if_changed<R>(
        &self,
        mutate: impl FnOnce(&mut ProviderStore) -> (R, bool),
    ) -> anyhow::Result<R> {
        let (result, changed) = self.mutate_providers(mutate).await;
        if changed {
            self.save_providers().await?;
        }
        Ok(result)
    }

    pub async fn try_mutate_providers_immediate<R, E>(
        &self,
        mutate: impl FnOnce(&mut ProviderStore) -> Result<R, E>,
    ) -> anyhow::Result<Result<R, E>> {
        let result = self.mutate_providers(mutate).await;
        if result.is_ok() {
            self.save_providers().await?;
        }
        Ok(result)
    }

    pub async fn try_mutate_providers_immediate_if_changed<R, E>(
        &self,
        mutate: impl FnOnce(&mut ProviderStore) -> Result<(R, bool), E>,
    ) -> anyhow::Result<Result<R, E>> {
        let result = self.mutate_providers(mutate).await;
        if result.as_ref().is_ok_and(|(_, changed)| *changed) {
            self.save_providers().await?;
        }
        Ok(result.map(|(value, _)| value))
    }

    pub async fn save_accounts(&self) -> anyhow::Result<()> {
        self.accounts.read().await.save(&self.config_dir)
    }

    pub async fn accounts_snapshot(&self) -> AccountStore {
        self.accounts.read().await.clone()
    }

    pub async fn find_account_by_id(&self, account_id: &str) -> Option<Account> {
        self.accounts
            .read()
            .await
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
    }

    pub async fn find_account_for_provider(
        &self,
        provider_type: ProviderType,
        account_id: Option<&str>,
    ) -> Option<Account> {
        self.accounts
            .read()
            .await
            .find_for_provider(provider_type, account_id)
            .cloned()
    }

    pub async fn mutate_accounts<R>(&self, mutate: impl FnOnce(&mut AccountStore) -> R) -> R {
        let mut accounts = self.accounts.write().await;
        mutate(&mut accounts)
    }

    pub async fn mutate_accounts_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut AccountStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_accounts(mutate).await;
        self.save_accounts().await?;
        Ok(result)
    }

    pub async fn try_mutate_accounts_immediate<R, E>(
        &self,
        mutate: impl FnOnce(&mut AccountStore) -> Result<R, E>,
    ) -> anyhow::Result<Result<R, E>> {
        let result = self.mutate_accounts(mutate).await;
        if result.is_ok() {
            self.save_accounts().await?;
        }
        Ok(result)
    }

    pub async fn mutate_accounts_debounced<R>(
        self: &Arc<Self>,
        mutate: impl FnOnce(&mut AccountStore) -> R,
    ) -> R {
        let result = self.mutate_accounts(mutate).await;
        save_accounts_debounced(self);
        result
    }

    pub async fn save_failover(&self) -> anyhow::Result<()> {
        self.failover.read().await.save(&self.config_dir)
    }

    pub async fn mutate_failover<R>(&self, mutate: impl FnOnce(&mut FailoverStore) -> R) -> R {
        let mut failover = self.failover.write().await;
        mutate(&mut failover)
    }

    pub async fn mutate_failover_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut FailoverStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_failover(mutate).await;
        self.save_failover().await?;
        Ok(result)
    }

    pub async fn mutate_failover_best_effort_if_changed<R>(
        &self,
        persist_context: &'static str,
        mutate: impl FnOnce(&mut FailoverStore) -> (R, bool),
    ) -> R {
        let (result, changed) = self.mutate_failover(mutate).await;
        if changed {
            if let Err(error) = self.save_failover().await {
                tracing::warn!("failed to persist {persist_context}: {error}");
            }
        }
        result
    }

    pub async fn try_mutate_failover_best_effort_if_changed<R, E>(
        &self,
        persist_context: &'static str,
        mutate: impl FnOnce(&mut FailoverStore) -> Result<(R, bool), E>,
    ) -> Result<R, E> {
        let result = self.mutate_failover(mutate).await;
        if result.as_ref().is_ok_and(|(_, changed)| *changed) {
            if let Err(error) = self.save_failover().await {
                tracing::warn!("failed to persist {persist_context}: {error}");
            }
        }
        result.map(|(value, _)| value)
    }

    pub async fn save_pricing(&self) -> anyhow::Result<()> {
        self.pricing.read().await.save(&self.config_dir)
    }

    pub async fn mutate_pricing<R>(&self, mutate: impl FnOnce(&mut ModelPricingStore) -> R) -> R {
        let mut pricing = self.pricing.write().await;
        mutate(&mut pricing)
    }

    pub async fn mutate_pricing_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut ModelPricingStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_pricing(mutate).await;
        self.save_pricing().await?;
        Ok(result)
    }

    pub async fn try_mutate_pricing_immediate<R, E>(
        &self,
        mutate: impl FnOnce(&mut ModelPricingStore) -> Result<R, E>,
    ) -> anyhow::Result<Result<R, E>> {
        let result = self.mutate_pricing(mutate).await;
        if result.is_ok() {
            self.save_pricing().await?;
        }
        Ok(result)
    }

    pub async fn save_usage(&self) -> anyhow::Result<()> {
        self.usage.read().await.save(&self.config_dir)
    }

    pub async fn usage_snapshot(&self) -> UsageStore {
        self.usage.read().await.clone()
    }

    pub async fn push_usage_log(&self, log: UsageLog) -> anyhow::Result<()> {
        self.usage
            .write()
            .await
            .push_and_persist(&self.config_dir, log)
    }

    pub async fn update_usage_log(
        &self,
        request_id: &str,
        update: impl FnOnce(&mut UsageLog),
    ) -> anyhow::Result<Option<UsageLog>> {
        self.usage
            .write()
            .await
            .update_log_and_persist(&self.config_dir, request_id, update)
    }

    pub async fn backfill_usage_costs(
        &self,
        providers: &ProviderStore,
        pricing: &ModelPricingStore,
    ) -> anyhow::Result<usize> {
        let updated = { self.usage.write().await.backfill_costs(providers, pricing) };
        if updated > 0 {
            self.save_usage().await?;
        }
        Ok(updated)
    }

    pub async fn backfill_usage_costs_for_model(
        &self,
        providers: &ProviderStore,
        pricing: &ModelPricingStore,
        model_id: &str,
    ) -> anyhow::Result<usize> {
        let updated = {
            self.usage
                .write()
                .await
                .backfill_costs_for_model(providers, pricing, model_id)
        };
        if updated > 0 {
            self.save_usage().await?;
        }
        Ok(updated)
    }

    async fn save_shares(&self) -> anyhow::Result<()> {
        self.shares.read().await.save(&self.config_dir)
    }

    pub async fn refresh_managed_account_if_needed(
        self: &Arc<Self>,
        provider_type: ProviderType,
        account_id: Option<&str>,
    ) -> Result<(), ManagedAccountRefreshError> {
        let now = crate::infra::time::now_ms() as i64;
        let account = {
            let accounts = self.accounts.read().await;
            accounts
                .find_for_provider(provider_type, account_id)
                .cloned()
        };
        let Some(account) = account else {
            return Ok(());
        };
        if !account_needs_native_refresh(&account, now) {
            return Ok(());
        }

        let _refresh_guard = self
            .account_refresh_locks
            .lock(account.provider_type, &account.id)
            .await;

        let account = {
            let accounts = self.accounts.read().await;
            accounts
                .find_for_provider(provider_type, account_id)
                .cloned()
        }
        .ok_or(ManagedAccountRefreshError::NotFound)?;
        if !account_needs_native_refresh(&account, now) {
            return Ok(());
        }

        let http_client = self.http_client().await;
        let interval_ms = self.oauth_quota_refresh_interval_ms().await;
        let update =
            match execute_native_account_refresh(&http_client, &account, now, interval_ms).await {
                Ok(update) => update,
                Err(error) => {
                    {
                        let mut accounts = self.accounts.write().await;
                        accounts.mark_native_refresh_failure(
                            &account.id,
                            error.message.clone(),
                            error.kind,
                        );
                    }
                    save_accounts_debounced(self);
                    return Err(ManagedAccountRefreshError::Refresh {
                        status_code: error.status_code,
                        message: error.message,
                    });
                }
            };

        {
            let mut accounts = self.accounts.write().await;
            accounts
                .mark_native_refresh_success(&account.id, update)
                .ok_or(ManagedAccountRefreshError::NotFound)?;
        }
        save_accounts_debounced(self);
        Ok(())
    }

    pub async fn mark_account_rate_limited_until(
        self: &Arc<Self>,
        account_id: &str,
        rate_limited_until: i64,
        message: Option<String>,
    ) -> Option<Account> {
        let account = {
            let mut accounts = self.accounts.write().await;
            accounts.mark_rate_limited_until(account_id, rate_limited_until, message)
        };
        if account.is_some() {
            save_accounts_debounced(self);
        }
        account
    }

    pub async fn update_account_entitlement_snapshot(
        self: &Arc<Self>,
        account_id: &str,
        subscription_level: Option<String>,
        entitlement_status: Option<String>,
        updated_at_ms: i64,
    ) -> Option<Account> {
        if subscription_level.is_none() && entitlement_status.is_none() {
            return None;
        }
        let account = {
            let mut accounts = self.accounts.write().await;
            accounts.update_entitlement_snapshot(
                account_id,
                subscription_level,
                entitlement_status,
                updated_at_ms,
            )
        };
        if account.is_some() {
            save_accounts_debounced(self);
        }
        account
    }

    pub fn remember_grok_media_session(
        &self,
        session_key: String,
        provider_id: String,
        account_id: Option<String>,
        ttl_ms: i64,
    ) {
        let now = crate::infra::time::now_ms() as i64;
        let expires_at_ms = now.saturating_add(ttl_ms);
        let mut sessions = self
            .grok_media_sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.retain(|_, binding| binding.expires_at_ms > now);
        sessions.insert(
            session_key,
            GrokMediaSessionBinding {
                provider_id,
                account_id,
                expires_at_ms,
            },
        );
    }

    pub fn grok_media_session_binding(&self, session_key: &str) -> Option<GrokMediaSessionBinding> {
        let now = crate::infra::time::now_ms() as i64;
        let mut sessions = self
            .grok_media_sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.retain(|_, binding| binding.expires_at_ms > now);
        sessions.get(session_key).cloned()
    }

    pub async fn insert_kiro_device_flow(
        &self,
        device_code: String,
        flow: PendingKiroDeviceFlow,
        now_ms: i64,
    ) {
        self.kiro_device_flows
            .write()
            .await
            .insert(device_code, flow, now_ms);
    }

    pub async fn get_kiro_device_flow(
        &self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<PendingKiroDeviceFlow> {
        self.kiro_device_flows
            .write()
            .await
            .get(device_code, now_ms)
    }

    pub async fn remove_kiro_device_flow(&self, device_code: &str) {
        self.kiro_device_flows.write().await.remove(device_code);
    }

    pub async fn insert_kiro_social_device_flow(
        &self,
        device_code: String,
        flow: PendingKiroSocialDeviceFlow,
        now_ms: i64,
    ) {
        self.kiro_device_flows
            .write()
            .await
            .insert_social(device_code, flow, now_ms);
    }

    pub async fn get_kiro_social_device_flow(
        &self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<PendingKiroSocialDeviceFlow> {
        self.kiro_device_flows
            .write()
            .await
            .get_social(device_code, now_ms)
    }

    pub async fn remove_kiro_social_device_flow(&self, device_code: &str) {
        self.kiro_device_flows
            .write()
            .await
            .remove_social(device_code);
    }

    pub async fn insert_codex_device_flow(
        &self,
        device_code: String,
        flow: PendingCodexDeviceFlow,
        now_ms: i64,
    ) {
        self.codex_device_flows
            .write()
            .await
            .insert(device_code, flow, now_ms);
    }

    pub async fn get_codex_device_flow(
        &self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<PendingCodexDeviceFlow> {
        self.codex_device_flows
            .write()
            .await
            .get(device_code, now_ms)
    }

    pub async fn remove_codex_device_flow(&self, device_code: &str) {
        self.codex_device_flows.write().await.remove(device_code);
    }

    pub async fn prepare_copilot_upstream_auth(
        self: &Arc<Self>,
        account_id: Option<&str>,
    ) -> Result<CopilotUpstreamAuth, CopilotUpstreamAuthError> {
        let now_ms = crate::infra::time::now_ms() as i64;
        let account = self
            .find_account_for_provider(ProviderType::GitHubCopilot, account_id)
            .await
            .ok_or(CopilotUpstreamAuthError::NotFound)?;
        let account_id = account.id.clone();
        let domain = copilot_account_domain(&account)?;

        if let Some(cached) = self
            .copilot_upstream_auth
            .read()
            .await
            .get(&account_id)
            .filter(|cached| cached.is_valid(now_ms))
            .cloned()
        {
            return Ok(cached.into_auth(account_id));
        }

        if !copilot_device::is_ghes(&domain) {
            if let Some(cached) = cached_copilot_auth_from_account(&account, &domain, now_ms) {
                self.copilot_upstream_auth
                    .write()
                    .await
                    .insert(account_id.clone(), cached.clone());
                return Ok(cached.into_auth(account_id));
            }
        }

        let github_token = copilot_github_token(&account).ok_or_else(|| {
            CopilotUpstreamAuthError::MissingGitHubToken {
                account_id: account_id.clone(),
            }
        })?;
        let http_client = self.http_client().await;

        let (token, expires_at_ms) = if copilot_device::is_ghes(&domain) {
            (github_token.clone(), None)
        } else {
            let token =
                copilot_device::fetch_copilot_internal_token(&http_client, &domain, &github_token)
                    .await
                    .map_err(|error| CopilotUpstreamAuthError::TokenExchange {
                        status_code: error.status.as_u16(),
                        message: error.message,
                    })?;
            (token.token, Some(token.expires_at.saturating_mul(1000)))
        };

        let api_endpoint = match copilot_device::fetch_copilot_api_endpoint(
            &http_client,
            &domain,
            &github_token,
        )
        .await
        {
            Ok(endpoint) => endpoint,
            Err(error) => {
                tracing::debug!(
                    "failed to discover GitHub Copilot API endpoint for account {}: {}; falling back",
                    account_id,
                    error
                );
                copilot_device::copilot_api_base(&domain)
            }
        };

        let cached = CachedCopilotUpstreamAuth {
            token,
            api_endpoint,
            expires_at_ms,
        };
        self.copilot_upstream_auth
            .write()
            .await
            .insert(account_id.clone(), cached.clone());
        Ok(cached.into_auth(account_id))
    }

    pub async fn start_deepseek_chat_completion(
        self: &Arc<Self>,
        account_id: Option<&str>,
        model: &str,
        prompt: &str,
    ) -> Result<reqwest::Response, DeepSeekUpstreamError> {
        let account = self
            .find_account_for_provider(ProviderType::DeepSeekAccount, account_id)
            .await
            .ok_or(DeepSeekUpstreamError::NotFound)?;
        let token = account
            .access_token
            .filter(|value| !value.trim().is_empty())
            .ok_or(DeepSeekUpstreamError::MissingToken)?;
        let client = crate::clients::deepseek::DeepSeekWebClient::new();
        client
            .start_completion(&token, model, prompt)
            .await
            .map_err(|error| DeepSeekUpstreamError::Client(error.to_string()))
    }

    pub async fn mutate_shares<R>(&self, mutate: impl FnOnce(&mut ShareStore) -> R) -> R {
        let mut shares = self.shares.write().await;
        mutate(&mut shares)
    }

    pub async fn mutate_shares_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut ShareStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_shares(mutate).await;
        self.save_shares().await?;
        Ok(result)
    }

    pub async fn try_mutate_shares_immediate<R, E>(
        &self,
        mutate: impl FnOnce(&mut ShareStore) -> Result<R, E>,
    ) -> anyhow::Result<Result<R, E>> {
        let result = self.mutate_shares(mutate).await;
        if result.is_ok() {
            self.save_shares().await?;
        }
        Ok(result)
    }

    pub async fn mutate_shares_debounced<R>(
        self: &Arc<Self>,
        mutate: impl FnOnce(&mut ShareStore) -> R,
    ) -> R {
        let result = self.mutate_shares(mutate).await;
        save_shares_debounced(self);
        result
    }

    pub async fn validate_share_invocation(
        self: &Arc<Self>,
        share_id: &str,
        now_ms: i64,
    ) -> Result<ShareInvocation, ShareInvocationRejection> {
        let result = self
            .mutate_shares(|shares| shares.validate_for_invocation(share_id, now_ms))
            .await;
        if result
            .as_ref()
            .err()
            .is_some_and(|rejection| rejection.status_changed)
        {
            save_shares_debounced(self);
        }
        result
    }

    pub async fn mutate_share<R>(
        &self,
        share_id: &str,
        mutate: impl FnOnce(&mut Share) -> R,
    ) -> Option<R> {
        self.mutate_shares(|store| {
            store
                .shares
                .iter_mut()
                .find(|share| share.id == share_id)
                .map(mutate)
        })
        .await
    }

    pub async fn replace_shares(&self, shares: Vec<Share>) {
        self.mutate_shares(|store| {
            store.shares = shares;
        })
        .await;
    }

    pub async fn save_ui_settings(&self) -> anyhow::Result<()> {
        self.ui_settings.read().await.save(&self.config_dir)
    }

    pub async fn mutate_ui_settings<R>(&self, mutate: impl FnOnce(&mut UiSettingsStore) -> R) -> R {
        let mut ui_settings = self.ui_settings.write().await;
        mutate(&mut ui_settings)
    }

    pub async fn mutate_ui_settings_immediate<R>(
        &self,
        mutate: impl FnOnce(&mut UiSettingsStore) -> R,
    ) -> anyhow::Result<R> {
        let result = self.mutate_ui_settings(mutate).await;
        self.save_ui_settings().await?;
        Ok(result)
    }

    pub async fn apply_ui_settings_patch_immediate(&self, patch: Value) -> anyhow::Result<()> {
        self.mutate_ui_settings_immediate(|ui_settings| {
            ui_settings.apply_patch(patch);
        })
        .await
    }

    pub async fn clear_sessions(&self) {
        self.sessions.write().await.clear();
    }

    pub async fn push_session(&self, session: Session) {
        self.sessions.write().await.push(session);
    }

    pub async fn mutate_oauth_logins<R>(
        &self,
        mutate: impl FnOnce(&mut OAuthLoginStore) -> R,
    ) -> R {
        let mut oauth_logins = self.oauth_logins.write().await;
        mutate(&mut oauth_logins)
    }
}

pub fn build_provider_http_client(
    proxy_url: &str,
    bind_addr: SocketAddr,
) -> anyhow::Result<reqwest::Client> {
    build_http_client_from_proxy(Some(proxy_url), false, bind_addr)
}

fn build_http_client(
    config: &ServerConfig,
    bind_addr: SocketAddr,
) -> anyhow::Result<reqwest::Client> {
    build_http_client_from_proxy(
        config.upstream_proxy.url.as_deref(),
        config.upstream_proxy.follow_system_proxy,
        bind_addr,
    )
}

fn build_http_client_from_proxy(
    proxy_url: Option<&str>,
    follow_system_proxy: bool,
    bind_addr: SocketAddr,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        .no_gzip();

    if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        crate::domain::settings::config::validate_proxy_url(proxy_url)?;
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("configure upstream proxy {}", mask_proxy_url(proxy_url)))?;
        builder = builder.proxy(proxy);
    } else if !follow_system_proxy || system_proxy_points_to_self(bind_addr) {
        builder = builder.no_proxy();
    }

    builder.build().context("build http client")
}

fn system_proxy_points_to_self(bind_addr: SocketAddr) -> bool {
    const KEYS: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    KEYS.iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .any(|value| proxy_points_to_addr(&value, bind_addr))
}

fn proxy_points_to_addr(value: &str, bind_addr: SocketAddr) -> bool {
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let Ok(parsed) = reqwest::Url::parse(&candidate) else {
        return false;
    };
    let Some(port) = parsed.port_or_known_default() else {
        return false;
    };
    if port != bind_addr.port() {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub async fn restore_tunnels(state: ServerState) {
    if state.config.read().await.is_setup_complete()
        && should_restore_client_tunnel(
            state
                .tunnels
                .status(&tunnel::client_tunnel_key())
                .await
                .as_ref(),
        )
    {
        start_client_tunnel(state.clone()).await;
    }

    let auto_start_share_ids = share_tunnel_restore_ids(&state.shares.read().await.shares);
    for share_id in auto_start_share_ids {
        start_share_tunnel(state.clone(), share_id).await;
    }

    if let Err(error) = reconcile_all_shares_to_router(state.clone()).await {
        tracing::warn!(error = %error, "automatic router share reconcile failed");
    }
    if let Err(error) = reconcile_payout_profile_to_router(state.clone()).await {
        tracing::warn!(error = %error, "automatic router payout profile reconcile failed");
    }
}

pub fn spawn_periodic_backups(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(6 * 60 * 60)).await;
            match crate::infra::backup::create_backup(
                &state.config_dir,
                &backup_targets(&state.config_dir),
                Some("periodic".to_string()),
            ) {
                Ok(manifest) => {
                    state.emit_event(
                        ServerEvent::new("backup.created", "backup")
                            .id(manifest.id)
                            .message("periodic"),
                    );
                }
                Err(error) => {
                    tracing::warn!(error = %error, "periodic backup failed");
                }
            }
        }
    });
}

pub fn spawn_share_edit_event_listener(state: ServerState) {
    tokio::spawn(async move {
        share_edit_event_loop(state).await;
    });
}

pub fn spawn_account_quota_refresh(state: ServerState) {
    tokio::spawn(async move {
        sleep(Duration::from_secs(60)).await;
        loop {
            refresh_due_native_account_tokens(&state).await;
            refresh_due_account_quotas(&state).await;
            let delay = next_account_quota_refresh_delay(&state).await;
            sleep(delay).await;
        }
    });
}

async fn refresh_due_native_account_tokens(state: &ServerState) {
    let now = crate::infra::time::now_ms() as i64;
    let accounts = state.accounts.read().await.accounts.clone();
    for account in accounts
        .into_iter()
        .filter(|account| account_needs_native_refresh(account, now))
    {
        refresh_one_native_account_token(state, account, now).await;
    }
}

async fn refresh_one_native_account_token(state: &ServerState, account: Account, now: i64) {
    let Some(_guard) = state
        .account_refresh_locks
        .try_lock(account.provider_type, &account.id)
    else {
        return;
    };
    let Some(account) = ({
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(account.provider_type, Some(&account.id))
            .cloned()
    }) else {
        return;
    };
    if !account_needs_native_refresh(&account, now) {
        return;
    }

    let http_client = state.http_client().await;
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;
    match execute_native_account_refresh(&http_client, &account, now, interval_ms).await {
        Ok(update) => {
            crate::metrics::record_warm_refresh(account.provider_type.as_str(), "success");
            {
                let mut store = state.accounts.write().await;
                store.mark_native_refresh_success(&account.id, update);
            }
            save_accounts_debounced(state);
        }
        Err(error) => {
            let metric_result =
                if error.kind == crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant {
                    "invalid_grant"
                } else {
                    "failure"
                };
            crate::metrics::record_warm_refresh(account.provider_type.as_str(), metric_result);
            tracing::warn!(
                account_id = %account.id,
                provider_type = %account.provider_type.as_str(),
                error = %error.message,
                "background OAuth token warm-refresh failed"
            );
            let updated = {
                let mut store = state.accounts.write().await;
                store.mark_native_refresh_failure(&account.id, error.message, error.kind)
            };
            if updated.is_some_and(|account| account.needs_relogin) {
                tracing::error!(
                    account_id = %account.id,
                    provider_type = %account.provider_type.as_str(),
                    "managed OAuth account isolated after repeated invalid_grant refresh failures"
                );
            }
            save_accounts_debounced(state);
        }
    }
}

async fn next_account_quota_refresh_delay(state: &ServerState) -> Duration {
    let now = crate::infra::time::now_ms() as i64;
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;
    let accounts = state.accounts.read().await.accounts.clone();
    let next_due = accounts
        .iter()
        .filter(|account| account_quota_refresh_candidate(account))
        .filter_map(|account| account.quota_next_refresh_at)
        .min();
    let delay_ms = next_due
        .map(|due| due.saturating_sub(now).max(0) as u64)
        .unwrap_or(interval_ms as u64);
    Duration::from_millis(delay_ms.clamp(1_000, interval_ms as u64))
}

fn account_quota_refresh_candidate(account: &Account) -> bool {
    match account.provider_type {
        ProviderType::CodexOAuth
        | ProviderType::ClaudeOAuth
        | ProviderType::GeminiCli
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth
        | ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth
        | ProviderType::CursorApiKey
        | ProviderType::OllamaCloud => {
            account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || account
                    .refresh_token
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                || account
                    .api_key
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        }
        _ => false,
    }
}

fn emit_oauth_quota_updated(state: &ServerState, account: &Account, success: bool) {
    state.emit_oauth_quota_updated_event(account, success);
}

async fn refresh_due_account_quotas(state: &ServerState) {
    let now = crate::infra::time::now_ms() as i64;
    let accounts = state.accounts.read().await.accounts.clone();
    for account in accounts
        .into_iter()
        .filter(|account| account_quota_refresh_due(account, now))
    {
        refresh_one_account_quota(state, account, now).await;
    }
}

async fn refresh_one_account_quota(state: &ServerState, account: Account, now: i64) {
    let Some(_guard) = state
        .account_refresh_locks
        .try_lock(account.provider_type, &account.id)
    else {
        return;
    };
    let success_cooldown_ms = state.oauth_quota_refresh_interval_ms().await;
    let mut active_account = account;
    let mut account_mutated = false;
    if account_needs_native_refresh(&active_account, now) {
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(
            &http_client,
            &active_account,
            now,
            success_cooldown_ms,
        )
        .await
        {
            Ok(update) => update,
            Err(error) => {
                mark_account_quota_refresh_error(state, &active_account.id, error.message, now)
                    .await;
                return;
            }
        };
        active_account = {
            let mut store = state.accounts.write().await;
            match store.mark_native_refresh_success(&active_account.id, update) {
                Some(account) => account,
                None => return,
            }
        };
        account_mutated = true;
    }

    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    match refresh_account_quota(
        &http_client,
        &active_account,
        now,
        false,
        success_cooldown_ms,
        timeout_ms,
    )
    .await
    {
        Ok(QuotaRefreshResult::Updated { update, .. }) => {
            let account = {
                let mut store = state.accounts.write().await;
                store
                    .mark_refresh_success(&active_account.id, update)
                    .unwrap_or(active_account)
            };
            emit_oauth_quota_updated(state, &account, true);
            save_accounts_debounced(state);
        }
        Ok(QuotaRefreshResult::SkippedCooldown { .. }) => {
            if account_mutated {
                save_accounts_debounced(state);
            }
        }
        Err(error) => {
            let mut update = AccountRefreshUpdate {
                quota_next_refresh_at: error.next_refresh_at,
                last_refresh_error: Some(error.message),
                ..Default::default()
            };
            if update.quota_next_refresh_at.is_none() {
                update.quota_next_refresh_at = Some(
                    now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS),
                );
            }
            {
                let mut store = state.accounts.write().await;
                store.mark_refresh_success(&active_account.id, update);
            }
            save_accounts_debounced(state);
        }
    }
}

async fn mark_account_quota_refresh_error(
    state: &ServerState,
    account_id: &str,
    message: String,
    now: i64,
) {
    {
        let mut store = state.accounts.write().await;
        store.mark_refresh_success(
            account_id,
            AccountRefreshUpdate {
                quota_next_refresh_at: Some(
                    now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS),
                ),
                last_refresh_error: Some(message),
                ..Default::default()
            },
        );
    }
    save_accounts_debounced(state);
}

fn account_quota_refresh_due(account: &Account, now: i64) -> bool {
    if account
        .quota_next_refresh_at
        .is_some_and(|next_refresh_at| next_refresh_at > now)
    {
        return false;
    }
    account_quota_refresh_candidate(account)
}

fn should_restore_client_tunnel(
    status: Option<&crate::clients::router::tunnel::TunnelRuntimeStatus>,
) -> bool {
    status.is_none_or(|status| status.status != "stopped")
}

pub(crate) fn should_restore_share_tunnel(share: &crate::domain::sharing::shares::Share) -> bool {
    share.enabled && share.status == "active"
}

fn share_tunnel_restore_ids(shares: &[crate::domain::sharing::shares::Share]) -> Vec<String> {
    shares
        .iter()
        .filter(|share| should_restore_share_tunnel(share))
        .map(|share| share.id.clone())
        .collect()
}

pub async fn ensure_share_tunnel_running(state: ServerState, share_id: &str) {
    let share = {
        let shares = state.shares.read().await;
        shares.get(share_id).cloned()
    };
    let Some(share) = share else {
        return;
    };
    if !should_restore_share_tunnel(&share) {
        return;
    }
    let share = match state
        .mutate_shares_immediate(|store| store.set_share_tunnel_status(share_id, "active", None))
        .await
    {
        Ok(Some(share)) => share,
        _ => return,
    };
    start_share_tunnel(state, share.id).await;
}

pub async fn start_client_tunnel(state: ServerState) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let lease_state = state.clone();
    let lease_fn: LeaseFn = Arc::new(move || {
        let lease_state = lease_state.clone();
        Box::pin(async move { issue_client_tunnel_lease(lease_state).await })
    });
    let renew_state = state.clone();
    let renew_lease_fn: RenewLeaseFn = Arc::new(move |lease_id, connection_id| {
        let renew_state = renew_state.clone();
        Box::pin(
            async move { renew_router_tunnel_lease(renew_state, lease_id, connection_id).await },
        )
    });
    state
        .tunnels
        .start(
            tunnel::client_tunnel_key(),
            "client-web",
            local_addr,
            lease_fn,
            renew_lease_fn,
        )
        .await;
}

pub async fn stop_client_tunnel(state: &ServerState) {
    state
        .tunnels
        .stop(&tunnel::client_tunnel_key(), "stopped")
        .await;
}

pub async fn start_share_tunnel(state: ServerState, share_id: String) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let key = tunnel::share_tunnel_key(&share_id);
    let lease_state = state.clone();
    let lease_share_id = share_id.clone();
    let lease_fn: LeaseFn = Arc::new(move || {
        let lease_state = lease_state.clone();
        let lease_share_id = lease_share_id.clone();
        Box::pin(async move { issue_share_tunnel_lease(lease_state, lease_share_id).await })
    });
    let renew_state = state.clone();
    let renew_lease_fn: RenewLeaseFn = Arc::new(move |lease_id, connection_id| {
        let renew_state = renew_state.clone();
        Box::pin(
            async move { renew_router_tunnel_lease(renew_state, lease_id, connection_id).await },
        )
    });
    state
        .tunnels
        .start(key, "share-http", local_addr, lease_fn, renew_lease_fn)
        .await;
}

async fn renew_router_tunnel_lease(
    state: ServerState,
    lease_id: String,
    connection_id: String,
) -> Result<String, crate::clients::router::client::RenewLeaseError> {
    let config = state.config.read().await.clone();
    let http_client = state.http_client().await;
    client::renew_tunnel_lease(&http_client, &config, lease_id, connection_id).await
}

pub async fn stop_share_tunnel(state: &ServerState, share_id: &str) {
    state
        .tunnels
        .stop(&tunnel::share_tunnel_key(share_id), "stopped")
        .await;
}

pub async fn sync_latest_direct_share_log(state: ServerState) {
    let _ = sync_pending_direct_share_logs(state, 1, false).await;
}

#[derive(Debug, Clone, Default)]
pub struct RouterLogSyncSummary {
    pub attempted: usize,
    pub synced: usize,
    pub failed: usize,
}

pub async fn sync_pending_direct_share_logs(
    state: ServerState,
    limit: usize,
    retry_failed: bool,
) -> RouterLogSyncSummary {
    let logs = {
        let usage = state.usage.read().await;
        usage
            .logs
            .iter()
            .filter(|log| {
                log.share_id.is_some()
                    && log.data_source.as_deref() == Some("direct")
                    && log.router_last_synced_at_ms.is_none()
                    && (retry_failed || log.router_sync_attempt_count < 3)
                    && is_router_request_id(&log.request_id)
            })
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut summary = RouterLogSyncSummary::default();
    if logs.is_empty() {
        return summary;
    }
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return summary;
    }
    for log in logs {
        summary.attempted += 1;
        let Some(entry) = share_request_log_entry(&state, &log).await else {
            mark_usage_router_sync(
                &state,
                &log.request_id,
                Err("share not found for router request log sync".to_string()),
            )
            .await;
            summary.failed += 1;
            continue;
        };
        let http_client = state.http_client().await;
        let result = client::batch_sync_share_request_logs(&http_client, &config, vec![entry])
            .await
            .map_err(|error| error.to_string());
        if result.is_ok() {
            summary.synced += 1;
        } else {
            summary.failed += 1;
        }
        mark_usage_router_sync(&state, &log.request_id, result).await;
    }
    summary
}

pub async fn pending_router_log_count(state: &ServerState) -> usize {
    let usage = state.usage.read().await;
    usage
        .logs
        .iter()
        .filter(|log| {
            log.share_id.is_some()
                && log.data_source.as_deref() == Some("direct")
                && log.router_last_synced_at_ms.is_none()
                && is_router_request_id(&log.request_id)
        })
        .count()
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditSyncSummary {
    pub pulled: usize,
    pub applied: usize,
    pub rejected: usize,
    pub acked: usize,
    pub ack_failed: usize,
    pub remote_synced: usize,
    pub remote_sync_failed: usize,
    pub error: Option<String>,
}

pub async fn pull_and_apply_pending_share_edits(state: ServerState) -> ShareEditSyncSummary {
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return ShareEditSyncSummary {
            error: Some("router installation is not registered".to_string()),
            ..ShareEditSyncSummary::default()
        };
    }
    let share_ids = {
        let shares = state.shares.read().await;
        shares
            .shares
            .iter()
            .map(|share| share.id.clone())
            .collect::<Vec<_>>()
    };
    if share_ids.is_empty() {
        return ShareEditSyncSummary::default();
    }
    let http_client = state.http_client().await;
    let edits = match client::pending_share_edits(&http_client, &config, share_ids).await {
        Ok(edits) => edits,
        Err(error) => {
            record_share_edit_sync_error(&state, error.to_string()).await;
            return ShareEditSyncSummary {
                error: Some(error.to_string()),
                ..ShareEditSyncSummary::default()
            };
        }
    };
    let mut summary = ShareEditSyncSummary {
        pulled: edits.len(),
        ..ShareEditSyncSummary::default()
    };
    for edit in edits {
        apply_and_ack_share_edit(&state, &config, edit, &mut summary).await;
    }
    summary
}

async fn apply_and_ack_share_edit(
    state: &ServerState,
    config: &ServerConfig,
    edit: ShareEditView,
    summary: &mut ShareEditSyncSummary,
) {
    let apply_result = apply_share_edit_locally(state, &edit).await;
    let (ack_status, ack_error) = match apply_result {
        Ok(()) => {
            summary.applied += 1;
            let sync_result = sync_one_share_to_router(state, config, &edit.share_id).await;
            match sync_result {
                Ok(()) => summary.remote_synced += 1,
                Err(error) => {
                    summary.remote_sync_failed += 1;
                    tracing::warn!(
                        share_id = %edit.share_id,
                        edit_id = %edit.id,
                        error = %error,
                        "router share sync after edit failed"
                    );
                }
            }
            ("applied".to_string(), None)
        }
        Err(error) => {
            summary.rejected += 1;
            let message = error.to_string();
            mark_share_edit_market_grant(
                state,
                &edit.share_id,
                ShareMarketGrantStatus {
                    status: "error".to_string(),
                    grant_id: Some(edit.id.clone()),
                    last_error: Some(message.clone()),
                    updated_at_ms: Some(crate::infra::time::now_ms()),
                },
            )
            .await;
            ("rejected".to_string(), Some(message))
        }
    };
    let ack = ShareEditAckPayload {
        edit_id: edit.id.clone(),
        revision: edit.revision,
        status: ack_status,
        error_message: ack_error,
    };
    let http_client = state.http_client().await;
    match client::ack_share_edit(&http_client, config, ack).await {
        Ok(()) => summary.acked += 1,
        Err(error) => {
            summary.ack_failed += 1;
            tracing::warn!(
                share_id = %edit.share_id,
                edit_id = %edit.id,
                error = %error,
                "router share edit ack failed"
            );
        }
    }
}

async fn apply_share_edit_locally(state: &ServerState, edit: &ShareEditView) -> anyhow::Result<()> {
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    {
        let mut shares = state.shares.write().await;
        shares.apply_settings_patch(&edit.share_id, edit.patch.clone())?;
        shares.update_market_grant(
            &edit.share_id,
            Some(ShareMarketGrantStatus {
                status: "applied".to_string(),
                grant_id: Some(edit.id.clone()),
                last_error: None,
                updated_at_ms: Some(crate::infra::time::now_ms()),
            }),
        );
        shares.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
        shares.router_registered = true;
        shares.last_router_error = None;
    }
    state.save_shares().await?;
    Ok(())
}

async fn mark_share_edit_market_grant(
    state: &ServerState,
    share_id: &str,
    market_grant: ShareMarketGrantStatus,
) {
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    {
        let mut shares = state.shares.write().await;
        shares.update_market_grant(share_id, Some(market_grant));
        shares.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
    }
    save_shares_debounced(state);
}

async fn record_share_edit_sync_error(state: &ServerState, message: String) {
    {
        let mut shares = state.shares.write().await;
        shares.last_router_error = Some(message);
    }
    save_shares_debounced(state);
}

async fn sync_one_share_to_router(
    state: &ServerState,
    config: &ServerConfig,
    share_id: &str,
) -> anyhow::Result<()> {
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|share| share.id == share_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("share not found"))?;
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let op = ShareSyncOperation {
        kind: "upsert".to_string(),
        share_id: None,
        share: Some(descriptor),
    };
    let http_client = state.http_client().await;
    let result = client::push_share_ops(&http_client, config, vec![op]).await;
    let router_base = config.router_api_base().map(str::to_string);
    {
        let mut store = state.shares.write().await;
        match &result {
            Ok(()) => {
                store.router_registered = true;
                store.last_router_error = None;
                store.mark_router_sync(share_id, router_base, Ok(crate::infra::time::now_ms()));
            }
            Err(error) => {
                let message = error.to_string();
                store.last_router_error = Some(message.clone());
                store.mark_router_sync(share_id, router_base, Err(message));
            }
        }
    }
    save_shares_debounced(state);
    result
}

/// Reconcile the router's installation-scoped share set with the current local
/// store. This is an internal recovery path used after startup/registration;
/// it is intentionally not exposed as a manual Web API action.
pub async fn reconcile_all_shares_to_router(state: ServerState) -> anyhow::Result<usize> {
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return Ok(0);
    }

    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares = state.shares.read().await.shares.clone();
    let mut ops = Vec::with_capacity(shares.len() + 1);
    ops.push(ShareSyncOperation {
        kind: "delete_all".to_string(),
        share_id: None,
        share: None,
    });
    ops.extend(shares.iter().map(|share| ShareSyncOperation {
        kind: "upsert".to_string(),
        share_id: None,
        share: Some(descriptor_for_share_with_accounts_and_usage(
            share,
            &providers,
            Some(&accounts),
            Some(&usage),
        )),
    }));

    let http_client = state.http_client().await;
    let result = client::push_share_ops(&http_client, &config, ops).await;
    let router_base = config.router_api_base().map(str::to_string);
    let now = crate::infra::time::now_ms();
    state
        .mutate_shares_debounced(|store| match &result {
            Ok(()) => {
                store.router_registered = true;
                store.last_router_error = None;
                for share in &shares {
                    store.mark_router_sync(&share.id, router_base.clone(), Ok(now));
                }
            }
            Err(error) => {
                let message = error.to_string();
                store.last_router_error = Some(message.clone());
                for share in &shares {
                    store.mark_router_sync(&share.id, router_base.clone(), Err(message.clone()));
                }
            }
        })
        .await;
    result.map(|()| shares.len())
}

pub async fn reconcile_payout_profile_to_router(state: ServerState) -> anyhow::Result<()> {
    let config = state.config_snapshot().await;
    let payout_state = config.owner.payout_profile.clone();
    if payout_state.revision <= 0 {
        return Ok(());
    }
    let revision = payout_state.revision;
    let result = async {
        let http_client = state.http_client().await;
        client::push_payout_profile(&http_client, &config, payout_state).await?;
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            state.mark_payout_profile_sync_success(revision).await?;
            Ok(())
        }
        Err(error) => {
            let message = error.to_string();
            state
                .mark_payout_profile_sync_error(revision, message.clone())
                .await?;
            Err(anyhow::anyhow!(message))
        }
    }
}

async fn share_edit_event_loop(state: ServerState) {
    loop {
        if let Err(error) = listen_for_share_edit_events_once(state.clone()).await {
            tracing::debug!(error = %error, "share edit event listener cycle ended");
        }
        sleep(Duration::from_secs(30)).await;
    }
}

async fn listen_for_share_edit_events_once(state: ServerState) -> anyhow::Result<()> {
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() || state.shares.read().await.shares.is_empty() {
        return Ok(());
    }
    let url = client::share_edit_events_url(&config)?;
    let http_client = state.http_client().await;
    let response = http_client
        .get(url)
        .send()
        .await
        .context("connect router share edit event stream")?;
    if !response.status().is_success() {
        anyhow::bail!(
            "router share edit event stream failed: {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read router share edit event chunk")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..index].trim().to_string();
            buffer = buffer[index + 1..].to_string();
            if line.starts_with("event: share_edit_available") || line.starts_with("event: resync")
            {
                let summary = pull_and_apply_pending_share_edits(state.clone()).await;
                tracing::info!(
                    pulled = summary.pulled,
                    applied = summary.applied,
                    rejected = summary.rejected,
                    acked = summary.acked,
                    "processed router share edit event"
                );
            }
        }
    }
    Ok(())
}

pub(crate) async fn share_request_log_entry(
    state: &ServerState,
    log: &UsageLog,
) -> Option<ShareRequestLogEntry> {
    let share_id = log.share_id.clone()?;
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|share| share.id == share_id)
        .cloned()?;
    let share_name = log
        .share_name
        .clone()
        .or_else(|| share.display_name.clone())
        .unwrap_or_else(|| share.id.clone());
    let model = log
        .model
        .clone()
        .or_else(|| log.requested_model.clone())
        .or_else(|| log.actual_model.clone())
        .unwrap_or_default();
    Some(ShareRequestLogEntry {
        request_id: log.request_id.clone(),
        share_id,
        share_name,
        provider_id: log.provider_id.clone(),
        provider_name: log.provider_name.clone(),
        app_type: app_key(log.app).to_string(),
        model: model.clone(),
        request_model: log.requested_model.clone().unwrap_or_else(|| model.clone()),
        request_agent: log.request_agent.clone().unwrap_or_default(),
        requested_model: log.requested_model.clone().unwrap_or_else(|| model.clone()),
        actual_model: log.actual_model.clone().unwrap_or_else(|| model.clone()),
        actual_model_source: log
            .actual_model_source
            .clone()
            .unwrap_or_else(|| "server".to_string()),
        status_code: log.status_code,
        latency_ms: clamp_u128_to_u64(log.duration_ms),
        first_token_ms: log.first_token_ms.map(clamp_u128_to_u64),
        input_tokens: clamp_u64_to_u32(log.input_tokens.unwrap_or(0)),
        output_tokens: clamp_u64_to_u32(log.output_tokens.unwrap_or(0)),
        cache_read_tokens: clamp_u64_to_u32(log.cache_read_tokens.unwrap_or(0)),
        cache_creation_tokens: clamp_u64_to_u32(log.cache_creation_tokens.unwrap_or(0)),
        is_streaming: log.is_streaming,
        session_id: log.session_id.clone(),
        user_country: log.user_country.clone(),
        user_country_iso3: log.user_country_iso3.clone(),
        user_email: log.user_email.clone(),
        created_at: (log.created_at_ms / 1000) as i64,
        is_health_check: log.is_health_check,
    })
}

async fn mark_usage_router_sync(state: &ServerState, request_id: &str, result: Result<(), String>) {
    let persisted =
        state
            .usage
            .write()
            .await
            .update_log_and_persist(&state.config_dir, request_id, |log| {
                log.router_sync_attempt_count = log.router_sync_attempt_count.saturating_add(1);
                match &result {
                    Ok(()) => {
                        log.router_last_synced_at_ms = Some(crate::infra::time::now_ms());
                        log.router_last_sync_error = None;
                    }
                    Err(error) => {
                        log.router_last_sync_error = Some(error.clone());
                    }
                }
            });
    if let Err(error) = persisted {
        tracing::warn!(error = %error, "persist usage after router request log sync failed");
    }
}

fn is_router_request_id(value: &str) -> bool {
    (8..=80).contains(&value.len())
        && value.starts_with("req_")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn app_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "claude",
        AppKind::Codex => "codex",
        AppKind::Gemini => "gemini",
    }
}

fn clamp_u64_to_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn clamp_u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

async fn issue_client_tunnel_lease(state: ServerState) -> anyhow::Result<IssueLeaseResponse> {
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        anyhow::bail!("setup is incomplete");
    }
    if config.router.identity.is_none() {
        let http_client = state.http_client().await;
        if let Err(error) = client::register_installation(&http_client, &mut config).await {
            record_router_error(&state, &config, error.to_string()).await;
            return Err(error);
        }
        state.replace_config(config.clone()).await?;
        {
            let mut shares = state.shares.write().await;
            shares.router_registered = true;
            shares.last_router_error = None;
        }
        state.save_shares().await?;
        reconcile_all_shares_to_router(state.clone()).await?;
        if let Err(error) = reconcile_payout_profile_to_router(state.clone()).await {
            tracing::warn!(error = %error, "router payout profile reconcile after implicit registration failed");
        }
    }

    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| anyhow::anyhow!("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))?;
    let http_client = state.http_client().await;
    if let Err(error) = client::claim_client_tunnel(
        &http_client,
        &config,
        client::ClientTunnelConfig {
            owner_email,
            subdomain: subdomain.clone(),
            enabled: true,
        },
    )
    .await
    {
        record_router_error(&state, &config, error.to_string()).await;
        return Err(error);
    }
    let lease = match client::issue_client_web_lease(&http_client, &config, subdomain).await {
        Ok(lease) => lease,
        Err(error) => {
            record_router_error(&state, &config, error.to_string()).await;
            return Err(error);
        }
    };

    let mut next = config;
    next.client.tunnel_status = Some("connected".to_string());
    next.router.ssh_host = Some(lease.ssh_addr.clone());
    next.router.last_register_error = None;
    state.replace_config(next).await?;
    Ok(lease)
}

async fn record_router_error(state: &ServerState, config: &ServerConfig, message: String) {
    let mut next = config.clone();
    next.router.last_register_error = Some(message.clone());
    if let Err(error) = state.replace_config(next).await {
        tracing::warn!(error = %error, "save router error to config failed");
    }
    {
        let mut shares = state.shares.write().await;
        shares.router_registered = false;
        shares.last_router_error = Some(message);
    }
    save_shares_debounced(state);
}

async fn issue_share_tunnel_lease(
    state: ServerState,
    share_id: String,
) -> anyhow::Result<IssueLeaseResponse> {
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        anyhow::bail!("router installation is not registered");
    }
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|share| share.id == share_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("share not found"))?;
    if !share.enabled || share.status != "active" {
        anyhow::bail!("share is not active");
    }
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let requested_subdomain = descriptor.subdomain.clone();
    let http_client = state.http_client().await;
    client::claim_share_subdomain(&http_client, &config, descriptor.clone()).await?;
    client::issue_share_lease(&http_client, &config, requested_subdomain, descriptor).await
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::Cli;
    use crate::clients::router::tunnel::TunnelRuntimeStatus;
    use crate::domain::accounts::store::Account;
    use crate::domain::providers::model::{AppKind, ProviderType};
    use crate::domain::sharing::shares::{Share, ShareAcl, UpsertShareInput};
    use crate::domain::usage::store::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};
    use serde_json::json;

    use super::*;

    fn copilot_account_fixture(expires_at: Option<i64>) -> Account {
        Account {
            id: "acct-copilot".to_string(),
            provider_type: ProviderType::GitHubCopilot,
            email: Some("octo@example.com".to_string()),
            access_token: Some("cached-copilot-token".to_string()),
            refresh_token: Some("github-refresh-token".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: Vec::new(),
            profile: Some(json!({
                "githubDomain": "GitHub.COM",
                "ghes": false
            })),
            raw: Some(json!({
                "githubDomain": "GitHub.COM",
                "githubToken": "github-raw-token",
                "copilotUsage": {
                    "endpoints": {
                        "api": "https://copilot-api.enterprise.example.com"
                    }
                }
            })),
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        }
    }

    #[test]
    fn copilot_account_seed_cache_uses_imported_token_until_refresh_buffer() {
        let now_ms = 10_000;
        let account = copilot_account_fixture(Some(now_ms + 120_000));
        let domain = copilot_account_domain(&account).unwrap();
        let cached = cached_copilot_auth_from_account(&account, &domain, now_ms).unwrap();

        assert_eq!(domain, "github.com");
        assert_eq!(cached.token, "cached-copilot-token");
        assert_eq!(
            cached.api_endpoint,
            "https://copilot-api.enterprise.example.com"
        );
        assert_eq!(cached.expires_at_ms, Some(now_ms + 120_000));

        let expiring = copilot_account_fixture(Some(now_ms + 59_000));
        assert!(cached_copilot_auth_from_account(&expiring, &domain, now_ms).is_none());
    }

    #[test]
    fn copilot_github_token_prefers_raw_token_over_refresh_token() {
        let account = copilot_account_fixture(Some(120_000));
        assert_eq!(
            copilot_github_token(&account).as_deref(),
            Some("github-raw-token")
        );

        let mut fallback = account.clone();
        fallback.raw = None;
        assert_eq!(
            copilot_github_token(&fallback).as_deref(),
            Some("github-refresh-token")
        );
    }

    #[tokio::test]
    async fn share_request_log_entry_preserves_router_sync_fields() {
        let state = test_state();
        state
            .shares
            .write()
            .await
            .upsert(UpsertShareInput {
                id: Some("share-1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: Some("Codex Share".to_string()),
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: Some("codexshare".to_string()),
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                sale_market_kind: None,
                access_by_app: std::collections::BTreeMap::new(),
                app_settings: std::collections::BTreeMap::new(),
                for_sale_official_price_percent_by_app: std::collections::BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
            })
            .unwrap();

        let mut log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "Provider 1".to_string(),
            ProviderType::Codex,
            200,
            250,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: Some("glm-5.2".to_string()),
                actual_model_source: Some("model_mapping".to_string()),
                pricing_model: Some("glm-5.2".to_string()),
            },
            TokenUsage {
                raw_input_tokens: Some(100),
                billed_input_tokens: Some(70),
                input_tokens: Some(100),
                output_tokens: Some(10),
                cache_read_tokens: Some(30),
                cache_creation_tokens: Some(5),
                total_tokens: Some(110),
            },
        );
        log.first_token_ms = Some(42);
        log.apply_context(UsageLogContext {
            request_id: Some("req_router_1".to_string()),
            share_id: Some("share-1".to_string()),
            share_name: Some("Codex Share".to_string()),
            user_email: Some("user@example.com".to_string()),
            data_source: Some("direct".to_string()),
            user_country: Some("Japan".to_string()),
            user_country_iso3: Some("JPN".to_string()),
            is_health_check: true,
            is_streaming: true,
            stream_status: Some("completed".to_string()),
            ..UsageLogContext::default()
        });

        let entry = share_request_log_entry(&state, &log).await.unwrap();

        assert_eq!(entry.share_id, "share-1");
        assert_eq!(entry.share_name, "Codex Share");
        assert_eq!(entry.first_token_ms, Some(42));
        assert_eq!(entry.actual_model, "glm-5.2");
        assert_eq!(entry.actual_model_source, "model_mapping");
        assert_eq!(entry.user_country.as_deref(), Some("Japan"));
        assert_eq!(entry.user_country_iso3.as_deref(), Some("JPN"));
        assert!(entry.is_health_check);
        assert_eq!(entry.cache_read_tokens, 30);
        assert_eq!(entry.cache_creation_tokens, 5);
    }

    fn test_state() -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir = std::env::temp_dir().join(format!("cc-switch-server-state-test-{nanos}"));
        let log_capture = Arc::new(crate::logging::LogCapture::new(
            crate::logging::RING_BUFFER_CAPACITY,
        ));
        ServerStateInner::load(
            Cli {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(config_dir),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            log_capture,
        )
        .unwrap()
    }

    #[test]
    fn restore_tunnel_logic_skips_manually_stopped_client() {
        assert!(should_restore_client_tunnel(None));
        assert!(should_restore_client_tunnel(Some(&TunnelRuntimeStatus {
            status: "connected".to_string(),
            ..TunnelRuntimeStatus::default()
        })));
        assert!(!should_restore_client_tunnel(Some(&TunnelRuntimeStatus {
            status: "stopped".to_string(),
            ..TunnelRuntimeStatus::default()
        })));
    }

    #[test]
    fn share_in_flight_tracker_enforces_limit_until_guard_drops() {
        let tracker = Arc::new(ShareInFlightTracker::default());
        let first = tracker.try_acquire("share-1", Some(1));

        assert!(first.is_some());
        assert!(tracker.try_acquire("share-1", Some(1)).is_none());
        assert!(tracker.try_acquire("share-2", Some(1)).is_some());

        drop(first);
        assert!(tracker.try_acquire("share-1", Some(1)).is_some());
    }

    #[test]
    fn account_in_flight_tracker_enforces_limit_and_snapshots_load() {
        let tracker = Arc::new(AccountInFlightTracker::default());
        let guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .expect("first request should acquire account capacity");

        assert_eq!(
            tracker
                .snapshot()
                .current(ProviderType::ClaudeOAuth, "acct-1"),
            1
        );
        assert!(tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .is_none());

        drop(guard);
        assert_eq!(
            tracker
                .snapshot()
                .current(ProviderType::ClaudeOAuth, "acct-1"),
            0
        );
        assert!(tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .is_some());
    }

    #[test]
    fn control_nonce_cache_rejects_replay_within_window() {
        let cache = ControlNonceCache::default();

        assert!(cache.register("inst-1", "nonce-1", 10_000, 300_000));
        assert!(!cache.register("inst-1", "nonce-1", 11_000, 300_000));
        assert!(cache.register("inst-2", "nonce-1", 11_000, 300_000));
        assert!(cache.register("inst-1", "nonce-1", 400_001, 300_000));
    }

    #[test]
    fn restore_tunnel_logic_selects_active_enabled_shares() {
        let shares = vec![
            share("s1", true, true, "active"),
            share("s2", true, false, "active"),
            share("s3", true, true, "paused"),
            share("s4", false, true, "active"),
        ];

        assert_eq!(
            share_tunnel_restore_ids(&shares),
            vec!["s1".to_string(), "s4".to_string()]
        );
    }

    #[test]
    fn auto_start_share_ids_still_require_auto_start_flag() {
        fn auto_start_share_ids(shares: &[Share]) -> Vec<String> {
            shares
                .iter()
                .filter(|share| share.auto_start && share.enabled && share.status == "active")
                .map(|share| share.id.clone())
                .collect()
        }

        let shares = vec![
            share("s1", true, true, "active"),
            share("s2", true, false, "active"),
            share("s3", true, true, "paused"),
            share("s4", false, true, "active"),
        ];

        assert_eq!(auto_start_share_ids(&shares), vec!["s1".to_string()]);
    }

    fn share(id: &str, auto_start: bool, enabled: bool, status: &str) -> Share {
        Share {
            id: id.to_string(),
            owner_email: None,
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_type: ProviderType::Codex,
            display_name: None,
            enabled,
            status: status.to_string(),
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
            acl: ShareAcl::default(),
            token_limit: None,
            parallel_limit: None,
            tokens_used: 0,
            requests_count: 0,
            expires_at: None,
            created_at_ms: 0,
            for_sale: false,
            sale_market_kind: "token".to_string(),
            access_by_app: std::collections::BTreeMap::new(),
            app_settings: std::collections::BTreeMap::new(),
            for_sale_official_price_percent_by_app: std::collections::BTreeMap::new(),
            official_price_percent: None,
            auto_start,
            description: None,
            bindings: Vec::new(),
            binding_history: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            last_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
        }
    }
}
