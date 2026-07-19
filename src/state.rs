use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use futures_util::StreamExt;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, watch, Mutex as AsyncMutex, RwLock};
use tokio::time::{sleep, timeout_at, Duration, Instant};

use crate::api::web::coverage::ProviderCoverage;
use crate::cli::Cli;
use crate::clients::oauth::codex_device::{
    CodexDeviceFlowStore, CodexDevicePollLease, CodexDevicePollResult, PendingCodexDeviceFlow,
};
use crate::clients::oauth::copilot_device;
use crate::clients::oauth::grok_device::{
    GrokDeviceFlowStore, GrokDevicePollLease, GrokDevicePollResult, PendingGrokDeviceFlow,
};
use crate::clients::oauth::kiro_device::{
    KiroDeviceFlowStore, PendingKiroDeviceFlow, PendingKiroSocialDeviceFlow,
};
use crate::clients::oauth::quota::{refresh_account_quota, QuotaRefreshResult};
use crate::clients::oauth::refresh::{
    account_needs_native_refresh, execute_native_account_refresh,
};
use crate::clients::router::client::{
    self, ActivateTunnelPayload, NamespaceLeasePayload, NamespaceLeaseResponse,
    NamespaceRenewLeasePayload, ShareEditAckPayload, ShareEditView, TunnelStatePayload,
};
use crate::clients::router::tunnel::{
    self, ActivateTunnelFn, LeaseFn, RenewLeaseFn, TunnelLeaseRequest, TunnelRenewalError,
    TunnelStateFn, TunnelSupervisor,
};
use crate::domain::accounts::login::OAuthLoginStore;
use crate::domain::accounts::managers::AccountRefreshLocks;
use crate::domain::accounts::oauth::oauth_quota_auth_provider_label;
use crate::domain::accounts::store::{
    Account, AccountRefreshUpdate, AccountStore, ManualSubscriptionExpiryError,
};
use crate::domain::failover::FailoverStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::ProviderStore;
use crate::domain::router::{ClientSubdomain, ShareSlug, PROTOCOL_EPOCH};
use crate::domain::settings::config::{
    mask_proxy_url, PayoutProfile, PayoutProfileState, RouterIdentity, ServerConfig,
    SetupCompletionNotificationStatus,
};
use crate::domain::settings::ui_settings::{self, UiSettingsStore};
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareRequestLogEntry, ShareSyncOperation,
};
use crate::domain::sharing::shares::{
    Share, ShareDeleteTombstone, ShareInvocation, ShareInvocationRejection, ShareMarketGrantStatus,
    ShareStore,
};
use crate::domain::usage::pricing::ModelPricingStore;
use crate::domain::usage::store::{UsageLog, UsageStore};
use crate::logging::{LogTailAccessError, LogTailResponse, SharedLogCapture};
use crate::proxy::cursor::session::CursorSessionManager;

const ROUTER_INSTALLATION_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);
const ROUTER_SHARE_SYNC_BATCH_SIZE: usize = 100;
const SETUP_COMPLETION_RETRY_BASE_MS: i64 = 30_000;
const SETUP_COMPLETION_RETRY_MAX_MS: i64 = 30 * 60_000;

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
    grok_device_flows: RwLock<GrokDeviceFlowStore>,
    kiro_device_flows: RwLock<KiroDeviceFlowStore>,
    codex_device_flows: RwLock<CodexDeviceFlowStore>,
    pub cursor_sessions: CursorSessionManager,
    pub account_refresh_locks: AccountRefreshLocks,
    pub account_in_flight: Arc<AccountInFlightTracker>,
    pub share_in_flight: Arc<ShareInFlightTracker>,
    pub control_nonces: Arc<ControlNonceCache>,
    router_registration_flight: AsyncMutex<Option<Arc<RouterRegistrationFlight>>>,
    setup_flight: AsyncMutex<()>,
    router_share_sync: AsyncMutex<()>,
    setup_completion_notification_flight: AsyncMutex<()>,
    router_share_prune_retry_pending: std::sync::atomic::AtomicBool,
    pub http_client: RwLock<reqwest::Client>,
    pub events: broadcast::Sender<ServerEvent>,
    pub tunnels: Arc<TunnelSupervisor>,
    pub web_auth: crate::domain::web_auth::WebAuthStore,
    pub debounced_saves: Arc<DebouncedStoreSaves>,
    pub started_at: std::time::Instant,
    pub process_instance_id: String,
    pub upgrade: crate::self_update::upgrade::SharedUpgradeRegistry,
    pub(crate) log_capture: SharedLogCapture,
}

#[derive(Debug)]
struct RouterRegistrationFlight {
    result: watch::Sender<Option<Result<client::RouterRegisterResult, RouterRegistrationFailure>>>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub(crate) struct RouterRegistrationFailure {
    message: String,
    unreachable: bool,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("router installation registration timed out after {timeout_seconds}s")]
pub(crate) struct RouterRegistrationTimeout {
    timeout_seconds: f64,
}

impl RouterRegistrationTimeout {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout_seconds: timeout.as_secs_f64(),
        }
    }
}

impl RouterRegistrationFailure {
    fn from_error(error: &anyhow::Error) -> Self {
        let unreachable = error.chain().any(|cause| {
            cause
                .downcast_ref::<client::RegisterInstallationAttemptError>()
                .is_some_and(client::RegisterInstallationAttemptError::is_transient)
                || cause.downcast_ref::<RouterRegistrationTimeout>().is_some()
        });
        Self {
            message: format!("{error:#}"),
            unreachable,
        }
    }

    pub(crate) fn is_unreachable(&self) -> bool {
        self.unreachable
    }
}

impl RouterRegistrationFlight {
    fn new() -> Self {
        Self {
            result: watch::channel(None).0,
        }
    }

    async fn wait(&self) -> Result<client::RouterRegisterResult, RouterRegistrationFailure> {
        let mut receiver = self.result.subscribe();
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| RouterRegistrationFailure {
                    message: "router installation registration flight stopped".to_string(),
                    unreachable: false,
                })?;
        }
    }
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
    user_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareInFlightAcquireError {
    ShareLimit,
    UserLimit,
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
        self.try_acquire_for_user(share_id, parallel_limit, None, None)
            .ok()
    }

    pub fn try_acquire_for_user(
        self: &Arc<Self>,
        share_id: &str,
        parallel_limit: Option<u32>,
        user_email: Option<&str>,
        user_parallel_limit: Option<u32>,
    ) -> Result<ShareInFlightGuard, ShareInFlightAcquireError> {
        let mut counts = self
            .counts
            .lock()
            .map_err(|_| ShareInFlightAcquireError::ShareLimit)?;
        let current = *counts.get(share_id).unwrap_or(&0);
        if parallel_limit.is_some_and(|limit| current >= limit) {
            return Err(ShareInFlightAcquireError::ShareLimit);
        }
        let user_key =
            user_email.map(|email| format!("{share_id}\u{1f}{}", email.to_ascii_lowercase()));
        if let Some(user_key) = user_key.as_deref() {
            let user_current = *counts.get(user_key).unwrap_or(&0);
            if user_parallel_limit.is_some_and(|limit| user_current >= limit) {
                return Err(ShareInFlightAcquireError::UserLimit);
            }
        }
        counts.insert(share_id.to_string(), current.saturating_add(1));
        if let Some(user_key) = user_key.as_deref() {
            let current = *counts.get(user_key).unwrap_or(&0);
            counts.insert(user_key.to_string(), current.saturating_add(1));
        }
        Ok(ShareInFlightGuard {
            tracker: self.clone(),
            share_id: share_id.to_string(),
            user_key,
        })
    }

    fn release(&self, share_id: &str, user_key: Option<&str>) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        for key in std::iter::once(share_id).chain(user_key) {
            let Some(current) = counts.get_mut(key) else {
                continue;
            };
            if *current <= 1 {
                counts.remove(key);
            } else {
                *current -= 1;
            }
        }
    }
}

impl Drop for ShareInFlightGuard {
    fn drop(&mut self) {
        self.tracker
            .release(&self.share_id, self.user_key.as_deref());
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
        let mut shares = ShareStore::load_or_default(&config_dir)?;
        let mut shares_changed = false;
        if let Some(owner_email) = config.owner.email.as_deref() {
            let migrated = shares
                .bind_all_to_client_owner(owner_email)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if !migrated.is_empty() {
                shares_changed = true;
                tracing::info!(
                    migrated_shares = migrated.len(),
                    "bound historical share owners to the client owner"
                );
            }
        }
        let migrated_grants = shares.migrate_user_grants_from_acl();
        if !migrated_grants.is_empty() {
            shares_changed = true;
            tracing::info!(
                migrated_shares = migrated_grants.len(),
                "initialized user policies for historical shares"
            );
        }
        if shares_changed {
            shares.save(&config_dir)?;
        }
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
            grok_device_flows: RwLock::new(GrokDeviceFlowStore::default()),
            kiro_device_flows: RwLock::new(KiroDeviceFlowStore::default()),
            codex_device_flows: RwLock::new(CodexDeviceFlowStore::default()),
            cursor_sessions: CursorSessionManager::default(),
            account_refresh_locks: AccountRefreshLocks::default(),
            account_in_flight: Arc::new(AccountInFlightTracker::default()),
            share_in_flight: Arc::new(ShareInFlightTracker::default()),
            control_nonces: Arc::new(ControlNonceCache::default()),
            router_registration_flight: AsyncMutex::new(None),
            setup_flight: AsyncMutex::new(()),
            router_share_sync: AsyncMutex::new(()),
            setup_completion_notification_flight: AsyncMutex::new(()),
            router_share_prune_retry_pending: std::sync::atomic::AtomicBool::new(false),
            http_client: RwLock::new(http_client),
            events,
            tunnels,
            web_auth,
            debounced_saves: Arc::new(DebouncedStoreSaves::default()),
            started_at: std::time::Instant::now(),
            process_instance_id: new_process_instance_id(),
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
        let api_config = ui_settings::parse_api_management_config(
            &ui_settings::api_management_config_for_frontend(&store),
        );
        drop(store);
        if !config.enabled || !api_config.log_enabled {
            return Err(LogTailAccessError::Disabled);
        }
        let lines = crate::logging::clamp_tail_lines(requested_lines, api_config.log_tail_lines);
        Ok(self.log_capture.read_tail(&config, &self.config_dir, lines))
    }

    pub async fn replace_config(&self, mut config: ServerConfig) -> anyhow::Result<()> {
        let mut current = self.config.write().await;
        preserve_router_identity_from_stale_snapshot(&current, &mut config);
        preserve_setup_completion_from_stale_snapshot(&current, &mut config);
        let http_client = build_http_client(&config, self.bind_addr)?;
        config.save(&self.config_dir)?;
        *self.http_client.write().await = http_client;
        *current = config;
        Ok(())
    }

    pub async fn set_upgrade_policy(
        &self,
        policy: crate::domain::settings::config::UpgradePolicyConfig,
    ) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        config.upgrade_policy = policy;
        config.save(&self.config_dir)?;
        Ok(())
    }

    pub async fn config_snapshot(&self) -> ServerConfig {
        self.config.read().await.clone()
    }

    pub(crate) async fn lock_setup(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.setup_flight.lock().await
    }

    pub(crate) async fn deliver_setup_completion_after_claim(&self) {
        if let Err(error) = self.deliver_setup_completion_notification(true).await {
            tracing::warn!(
                error = %error,
                "persist setup-completed notification state after client claim failed"
            );
        }
    }

    pub(crate) async fn retry_pending_setup_completion_notification(&self) {
        let authoritative_claim = {
            let config = self.config.read().await;
            config.has_registered_router_identity()
                && matches!(
                    config.client.tunnel_status.as_deref(),
                    Some("claimed_remote" | "connected" | "active" | "running")
                )
        };
        if let Err(error) = self
            .deliver_setup_completion_notification(authoritative_claim)
            .await
        {
            tracing::warn!(
                error = %error,
                "retry setup-completed notification state update failed"
            );
        }
    }

    async fn deliver_setup_completion_notification(
        &self,
        authoritative_claim: bool,
    ) -> anyhow::Result<()> {
        let _flight = self.setup_completion_notification_flight.lock().await;
        let now_ms = now_ms_i64();
        let (config, setup) = {
            let mut config = self.config.write().await;
            let Some(notification) = config.setup_completion_notification.as_mut() else {
                return Ok(());
            };
            match notification.status {
                SetupCompletionNotificationStatus::Acknowledged
                | SetupCompletionNotificationStatus::TerminalFailed => return Ok(()),
                SetupCompletionNotificationStatus::WaitingForClaim if !authoritative_claim => {
                    return Ok(())
                }
                SetupCompletionNotificationStatus::Pending
                    if notification
                        .next_attempt_at_ms
                        .is_some_and(|next_attempt_at_ms| next_attempt_at_ms > now_ms) =>
                {
                    return Ok(())
                }
                SetupCompletionNotificationStatus::WaitingForClaim
                | SetupCompletionNotificationStatus::Pending => {}
            }

            let Some(password_hint) = notification.password_hint.clone() else {
                notification.status = SetupCompletionNotificationStatus::TerminalFailed;
                notification.password_hint = None;
                notification.updated_at_ms = now_ms;
                notification.next_attempt_at_ms = None;
                notification.last_error =
                    Some("setup-completed notification password hint is missing".to_string());
                config.save(&self.config_dir)?;
                return Ok(());
            };
            let setup = match client::InstallationSetupCompletedPayload::new(
                notification.setup_id.clone(),
                password_hint,
            ) {
                Ok(setup) => setup,
                Err(error) => {
                    notification.status = SetupCompletionNotificationStatus::TerminalFailed;
                    notification.password_hint = None;
                    notification.updated_at_ms = now_ms;
                    notification.next_attempt_at_ms = None;
                    notification.last_error = Some(error.to_string());
                    config.save(&self.config_dir)?;
                    return Ok(());
                }
            };
            notification.status = SetupCompletionNotificationStatus::Pending;
            notification.attempt_count = notification.attempt_count.saturating_add(1);
            notification.updated_at_ms = now_ms;
            notification.last_attempt_at_ms = Some(now_ms);
            notification.next_attempt_at_ms = Some(now_ms.saturating_add(
                setup_completion_retry_delay_ms(&notification.setup_id, notification.attempt_count),
            ));
            notification.router_ack_status = None;
            notification.last_error = None;
            config.save(&self.config_dir)?;
            (config.clone(), setup)
        };

        let setup_id = setup.setup_id.clone();
        let http_client = self.http_client().await;
        let result = client::send_installation_setup_completed(&http_client, &config, setup).await;
        let completed_at_ms = now_ms_i64();
        let mut config = self.config.write().await;
        let Some(notification) = config.setup_completion_notification.as_mut() else {
            return Ok(());
        };
        if notification.setup_id != setup_id {
            return Ok(());
        }
        match result {
            Ok(ack) => {
                notification.status = SetupCompletionNotificationStatus::Acknowledged;
                notification.password_hint = None;
                notification.updated_at_ms = completed_at_ms;
                notification.acknowledged_at_ms = Some(completed_at_ms);
                notification.next_attempt_at_ms = None;
                notification.router_ack_status = Some(ack.as_str().to_string());
                notification.last_error = None;
                tracing::info!(
                    setup_id = %setup_id,
                    router_status = ack.as_str(),
                    "router durably acknowledged installation setup completion"
                );
            }
            Err(error) if error.is_terminal() => {
                notification.status = SetupCompletionNotificationStatus::TerminalFailed;
                notification.password_hint = None;
                notification.updated_at_ms = completed_at_ms;
                notification.next_attempt_at_ms = None;
                notification.router_ack_status = None;
                notification.last_error = Some(error.to_string());
                tracing::warn!(
                    setup_id = %setup_id,
                    error = %error,
                    "router permanently rejected installation setup completion"
                );
            }
            Err(error) => {
                notification.status = SetupCompletionNotificationStatus::Pending;
                notification.updated_at_ms = completed_at_ms;
                if let Some(retry_after_ms) = error.retry_after_ms() {
                    let router_retry_at_ms = completed_at_ms.saturating_add(retry_after_ms);
                    notification.next_attempt_at_ms = Some(
                        notification
                            .next_attempt_at_ms
                            .unwrap_or_default()
                            .max(router_retry_at_ms),
                    );
                }
                notification.last_error = Some(error.to_string());
                tracing::warn!(
                    setup_id = %setup_id,
                    error = %error,
                    next_attempt_at_ms = ?notification.next_attempt_at_ms,
                    "router setup-completed notification remains pending"
                );
            }
        }
        config.save(&self.config_dir)?;
        Ok(())
    }

    pub(crate) async fn lock_router_share_sync(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.router_share_sync.lock().await
    }

    fn request_router_share_prune_retry(&self) {
        self.router_share_prune_retry_pending
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn clear_router_share_prune_retry(&self) {
        self.router_share_prune_retry_pending
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn router_share_prune_retry_requested(&self) -> bool {
        self.router_share_prune_retry_pending
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub async fn register_router_installation(
        self: &Arc<Self>,
    ) -> anyhow::Result<client::RouterRegisterResult> {
        let deadline = Instant::now() + ROUTER_INSTALLATION_REGISTRATION_TIMEOUT;
        let (flight, should_start) = timeout_at(deadline, async {
            let mut current = self.router_registration_flight.lock().await;
            if let Some(flight) = current.as_ref() {
                (flight.clone(), false)
            } else {
                let flight = Arc::new(RouterRegistrationFlight::new());
                *current = Some(flight.clone());
                (flight, true)
            }
        })
        .await
        .map_err(|_| {
            anyhow::Error::new(RouterRegistrationTimeout::new(
                ROUTER_INSTALLATION_REGISTRATION_TIMEOUT,
            ))
        })?;

        if should_start {
            let state = self.clone();
            let running_flight = flight.clone();
            tokio::spawn(async move {
                let result = state
                    .register_router_installation_locked()
                    .await
                    .map_err(|error| RouterRegistrationFailure::from_error(&error));
                if let Err(error) = &result {
                    if let Err(save_error) = state
                        .record_router_registration_error(error.to_string())
                        .await
                    {
                        tracing::warn!(error = %save_error, "persist router registration error failed");
                    }
                }
                running_flight.result.send_replace(Some(result.clone()));
                let mut current = state.router_registration_flight.lock().await;
                if current
                    .as_ref()
                    .is_some_and(|flight| Arc::ptr_eq(flight, &running_flight))
                {
                    *current = None;
                }
            });
        }

        timeout_at(deadline, flight.wait())
            .await
            .map_err(|_| {
                anyhow::Error::new(RouterRegistrationTimeout::new(
                    ROUTER_INSTALLATION_REGISTRATION_TIMEOUT,
                ))
            })?
            .map_err(anyhow::Error::new)
    }

    pub async fn complete_router_registration_control_plane(
        self: &Arc<Self>,
        source: &'static str,
    ) -> anyhow::Result<()> {
        self.mutate_shares_immediate(|shares| {
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await?;
        if let Err(error) = reconcile_all_shares_to_router(self.clone()).await {
            tracing::warn!(%error, source, "router share reconcile after registration failed");
        }
        retry_pending_router_share_deletes(self.clone()).await;
        if let Err(error) = reconcile_payout_profile_to_router(self.clone()).await {
            tracing::warn!(%error, source, "router payout profile reconcile after registration failed");
        }
        Ok(())
    }

    async fn register_router_installation_locked(
        &self,
    ) -> anyhow::Result<client::RouterRegisterResult> {
        self.register_router_installation_locked_with_timeout(
            ROUTER_INSTALLATION_REGISTRATION_TIMEOUT,
        )
        .await
    }

    async fn register_router_installation_locked_with_timeout(
        &self,
        timeout: Duration,
    ) -> anyhow::Result<client::RouterRegisterResult> {
        tokio::time::timeout(timeout, self.run_router_registration_stages())
            .await
            .map_err(|_| anyhow::Error::new(RouterRegistrationTimeout::new(timeout)))?
    }

    async fn run_router_registration_stages(&self) -> anyhow::Result<client::RouterRegisterResult> {
        let mut identity = self.ensure_pending_router_identity().await?;
        let config = self.config_snapshot().await;
        let api_base = config
            .router_api_base()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
            .trim_end_matches('/')
            .to_string();
        let http = self.http_client().await;
        let mut response = match client::register_installation_v2(&http, &api_base, &identity).await
        {
            Ok(response) if response.control_secret.is_some() => response,
            Ok(response) => {
                identity = identity_for_registration_recovery(&identity, &response)?;
                client::recover_legacy_installation(&http, &api_base, &identity).await?
            }
            Err(error) if error.allows_legacy_fallback() => {
                let discovered =
                    client::discover_legacy_installation(&http, &api_base, &identity).await?;
                identity = identity_for_registration_recovery(&identity, &discovered)?;
                client::recover_legacy_installation(&http, &api_base, &identity).await?
            }
            Err(error) => return Err(error.into()),
        };
        if response.control_secret.is_none()
            && response.installation_id.trim() == identity.installation_id.trim()
        {
            response.control_secret = identity.control_secret.clone();
        }
        ensure_complete_router_registration_response(&identity, &response)?;
        let registered_at_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
        let identity = self
            .merge_router_registration_response(
                &api_base,
                &identity.public_key,
                response,
                Some(registered_at_ms),
            )
            .await?;
        Ok(client::RouterRegisterResult {
            installation_id: identity.installation_id,
            public_key: identity.public_key,
            control_secret_present: identity.control_secret.is_some(),
            registered_at_ms,
        })
    }

    async fn ensure_pending_router_identity(&self) -> anyhow::Result<RouterIdentity> {
        let mut config = self.config.write().await;
        if let Some(identity) = config.router.identity.as_ref() {
            if !identity.has_keypair() {
                anyhow::bail!("router installation identity keypair is incomplete");
            }
            return Ok(identity.clone());
        }

        let identity = client::generate_identity_without_installation();
        let mut next = config.clone();
        next.router.identity = Some(identity.clone());
        next.save(&self.config_dir)?;
        *config = next;
        Ok(identity)
    }

    async fn merge_router_registration_response(
        &self,
        expected_api_base: &str,
        expected_public_key: &str,
        response: client::RegisterInstallationResponse,
        registered_at_ms: Option<i64>,
    ) -> anyhow::Result<RouterIdentity> {
        let installation_id = response.installation_id.trim();
        if installation_id.is_empty() {
            anyhow::bail!("router installation register returned an empty installation id");
        }

        let mut config = self.config.write().await;
        let current_api_base = config
            .router_api_base()
            .map(str::trim)
            .unwrap_or_default()
            .trim_end_matches('/');
        if current_api_base != expected_api_base {
            anyhow::bail!("router configuration changed while registration was in progress");
        }
        let current = config
            .router
            .identity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pending router identity disappeared"))?;
        if current.public_key.trim() != expected_public_key.trim() {
            anyhow::bail!("pending router identity changed while registration was in progress");
        }

        let mut next = config.clone();
        let identity = {
            let identity = next
                .router
                .identity
                .as_mut()
                .expect("router identity checked above");
            identity.installation_id = installation_id.to_string();
            if response.control_secret.is_some() {
                identity.control_secret = response.control_secret;
            }
            identity.clone()
        };
        if let Some(registered_at_ms) = registered_at_ms {
            next.router.last_register_error = None;
            next.router.last_registered_at_ms = Some(registered_at_ms);
        }
        next.save(&self.config_dir)?;
        *config = next;
        Ok(identity)
    }

    async fn record_router_registration_error(&self, message: String) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        let mut next = config.clone();
        next.router.last_register_error = Some(message);
        next.save(&self.config_dir)?;
        *config = next;
        Ok(())
    }

    pub async fn set_debug_token(&self, token: &str, expires_at_ms: i64) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        config.set_debug_token(token, expires_at_ms)?;
        config.save(&self.config_dir)
    }

    pub async fn revoke_debug_token(&self) -> anyhow::Result<()> {
        let mut config = self.config.write().await;
        config.revoke_debug_token();
        config.save(&self.config_dir)
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

    pub async fn set_manual_subscription_expiry_and_sync(
        self: &Arc<Self>,
        provider_type: ProviderType,
        account_id: &str,
        expires_at_ms: Option<i64>,
    ) -> anyhow::Result<Result<Account, ManualSubscriptionExpiryError>> {
        let updated_at_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
        let result = self
            .try_mutate_accounts_immediate(|store| {
                let previous_effective_expiry = store
                    .accounts
                    .iter()
                    .find(|account| {
                        account.id == account_id && account.provider_type == provider_type
                    })
                    .ok_or_else(|| ManualSubscriptionExpiryError::NotFound(account_id.to_string()))
                    .map(|account| {
                        crate::domain::accounts::subscription_expiry::resolved_subscription_expiry(
                            account,
                        )
                        .expires_at_ms
                    })?;
                let account = store.set_manual_subscription_expiry(
                    account_id,
                    expires_at_ms,
                    updated_at_ms,
                )?;
                Ok((account, previous_effective_expiry))
            })
            .await?;
        let (account, previous_effective_expiry) = match result {
            Ok(result) => result,
            Err(error) => return Ok(Err(error)),
        };
        let effective_expiry =
            crate::domain::accounts::subscription_expiry::resolved_subscription_expiry(&account)
                .expires_at_ms;
        if previous_effective_expiry == effective_expiry {
            return Ok(Ok(account));
        }

        self.emit_oauth_quota_updated_event(&account, true);
        self.refresh_account_subscription_metadata(provider_type, Some(account_id))
            .await?;
        Ok(Ok(account))
    }

    pub async fn refresh_account_subscription_metadata(
        self: &Arc<Self>,
        provider_type: ProviderType,
        changed_account_id: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let providers = self.providers.read().await.clone();
        let accounts = self.accounts.read().await.clone();
        let default_account_id = accounts
            .accounts
            .iter()
            .find(|account| account.provider_type == provider_type)
            .map(|account| account.id.as_str());
        let include_unbound = changed_account_id.is_none()
            || changed_account_id.is_some_and(|account_id| default_account_id == Some(account_id));
        let provider_keys = providers
            .providers
            .iter()
            .filter(|provider| provider.provider_type == provider_type)
            .filter(|provider| {
                let bound_account_id = provider
                    .provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.auth_binding.as_ref())
                    .and_then(|binding| binding.account_id.as_deref());
                match (changed_account_id, bound_account_id) {
                    (Some(changed), Some(bound)) => changed == bound,
                    (Some(_), None) => include_unbound,
                    (None, None) => true,
                    (None, Some(_)) => false,
                }
            })
            .map(|provider| (provider.app, provider.provider.id.clone()))
            .collect::<BTreeSet<_>>();
        self.refresh_share_subscription_metadata_for_provider_keys(provider_keys)
            .await
    }

    pub async fn refresh_account_subscription_metadata_after_removal(
        self: &Arc<Self>,
        provider_type: ProviderType,
        account_id: &str,
        was_default: bool,
    ) -> anyhow::Result<Vec<String>> {
        let providers = self.providers.read().await.clone();
        let provider_keys = providers
            .providers
            .iter()
            .filter(|provider| provider.provider_type == provider_type)
            .filter(|provider| {
                let bound_account_id = provider
                    .provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.auth_binding.as_ref())
                    .and_then(|binding| binding.account_id.as_deref());
                bound_account_id == Some(account_id) || (was_default && bound_account_id.is_none())
            })
            .map(|provider| (provider.app, provider.provider.id.clone()))
            .collect::<BTreeSet<_>>();
        self.refresh_share_subscription_metadata_for_provider_keys(provider_keys)
            .await
    }

    async fn refresh_share_subscription_metadata_for_provider_keys(
        self: &Arc<Self>,
        provider_keys: BTreeSet<(AppKind, String)>,
    ) -> anyhow::Result<Vec<String>> {
        if provider_keys.is_empty() {
            return Ok(Vec::new());
        }

        let providers = self.providers.read().await.clone();
        let accounts = self.accounts.read().await.clone();
        let usage = self.usage.read().await.clone();
        let share_ids = self
            .mutate_shares_immediate(|shares| {
                shares.refresh_runtime_snapshots_for_providers(
                    &provider_keys,
                    &providers,
                    Some(&accounts),
                    &usage,
                )
            })
            .await?;
        if share_ids.is_empty() {
            return Ok(share_ids);
        }

        self.emit_event(
            ServerEvent::new("share.changed", "share")
                .message("account_subscription_expiry_updated"),
        );
        if self
            .config_snapshot()
            .await
            .has_registered_router_identity()
        {
            let state = self.clone();
            let pending_ids = share_ids.clone();
            tokio::spawn(async move {
                if let Err(error) = sync_shares_to_router(&state, &pending_ids).await {
                    tracing::warn!(
                        share_count = pending_ids.len(),
                        %error,
                        "router share subscription metadata sync remains pending"
                    );
                }
            });
        }
        Ok(share_ids)
    }

    pub async fn refresh_automatic_subscription_metadata_if_changed(
        self: &Arc<Self>,
        before: &Account,
        after: &Account,
    ) -> anyhow::Result<bool> {
        use crate::domain::accounts::subscription_expiry::{
            resolved_subscription_expiry, subscription_expiry_capability,
            SubscriptionExpiryCapability,
        };

        if before.id != after.id
            || before.provider_type != after.provider_type
            || subscription_expiry_capability(after.provider_type)
                != SubscriptionExpiryCapability::Automatic
            || resolved_subscription_expiry(before).expires_at_ms
                == resolved_subscription_expiry(after).expires_at_ms
        {
            return Ok(false);
        }

        self.save_accounts().await?;
        self.refresh_account_subscription_metadata(after.provider_type, Some(&after.id))
            .await?;
        Ok(true)
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

    pub(crate) async fn push_health_usage_log_if_due(
        &self,
        log: UsageLog,
        min_interval_ms: u128,
    ) -> anyhow::Result<UsageLog> {
        anyhow::ensure!(log.is_health_check, "usage log is not a health check");
        let mut usage = self.usage.write().await;
        let latest = usage
            .logs
            .iter()
            .filter(|existing| {
                existing.is_health_check
                    && existing.share_id == log.share_id
                    && existing.app == log.app
                    && existing.provider_id == log.provider_id
                    && existing.data_source == log.data_source
                    && existing.requested_model == log.requested_model
            })
            .max_by_key(|existing| existing.created_at_ms)
            .cloned();
        if let Some(existing) = latest {
            if log.created_at_ms.saturating_sub(existing.created_at_ms) < min_interval_ms {
                return Ok(existing);
            }
        }
        usage.push_and_persist(&self.config_dir, log.clone())?;
        Ok(log)
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

    pub async fn save_shares(&self) -> anyhow::Result<()> {
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

    pub async fn insert_grok_device_flow(
        &self,
        device_code: String,
        flow: PendingGrokDeviceFlow,
        now_ms: i64,
    ) {
        self.grok_device_flows
            .write()
            .await
            .insert(device_code, flow, now_ms);
    }

    pub async fn begin_grok_device_poll(
        &self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<GrokDevicePollLease> {
        self.grok_device_flows
            .write()
            .await
            .begin_poll(device_code, now_ms)
    }

    pub async fn finish_grok_device_poll(
        &self,
        device_code: &str,
        result: GrokDevicePollResult,
    ) -> bool {
        self.grok_device_flows
            .write()
            .await
            .finish_poll(device_code, result)
    }

    pub async fn fail_grok_device_poll(&self, device_code: &str, terminal: bool) {
        self.grok_device_flows
            .write()
            .await
            .fail_poll(device_code, terminal);
    }

    pub async fn cancel_grok_device_flow(&self, device_code: &str) -> bool {
        self.grok_device_flows.write().await.cancel(device_code)
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

    pub async fn begin_codex_device_poll(
        &self,
        device_code: &str,
        now_ms: i64,
    ) -> Option<CodexDevicePollLease> {
        self.codex_device_flows
            .write()
            .await
            .begin_poll(device_code, now_ms)
    }

    pub async fn finish_codex_device_poll(
        &self,
        device_code: &str,
        result: CodexDevicePollResult,
    ) -> bool {
        self.codex_device_flows
            .write()
            .await
            .finish_poll(device_code, result)
    }

    pub async fn fail_codex_device_poll(&self, device_code: &str, terminal: bool) {
        self.codex_device_flows
            .write()
            .await
            .fail_poll(device_code, terminal);
    }

    pub async fn cancel_codex_device_flow(&self, device_code: &str) -> bool {
        self.codex_device_flows.write().await.cancel(device_code)
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

    pub async fn delete_share_immediate(
        &self,
        share_id: &str,
    ) -> anyhow::Result<Option<ShareDeleteTombstone>> {
        let config = self.config_snapshot().await;
        let router_target = config.registered_router_identity().and_then(|identity| {
            config.router_api_base().map(|router_api_base| {
                (
                    router_api_base.to_string(),
                    identity.installation_id.clone(),
                )
            })
        });
        self.mutate_shares_immediate(|store| match router_target.as_ref() {
            Some((router_api_base, installation_id)) => {
                store.delete_for_router_target(share_id, router_api_base, installation_id)
            }
            None => store.delete(share_id),
        })
        .await
    }

    pub async fn validate_share_invocation(
        self: &Arc<Self>,
        share_id: &str,
        user_email: Option<&str>,
        now_ms: i64,
    ) -> Result<ShareInvocation, ShareInvocationRejection> {
        let result = self
            .mutate_shares(|shares| shares.validate_for_invocation(share_id, user_email, now_ms))
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
            store.replace_shares(shares);
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

fn new_process_instance_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn identity_for_registration_recovery(
    current: &RouterIdentity,
    response: &client::RegisterInstallationResponse,
) -> anyhow::Result<RouterIdentity> {
    let installation_id = response.installation_id.trim();
    if installation_id.is_empty() {
        anyhow::bail!("router installation discovery returned an empty installation id");
    }
    let mut identity = current.clone();
    if identity.installation_id.trim() != installation_id {
        identity.control_secret = None;
    }
    identity.installation_id = installation_id.to_string();
    if let Some(control_secret) = response.control_secret.as_ref() {
        identity.control_secret = Some(control_secret.clone());
    }
    Ok(identity)
}

fn ensure_complete_router_registration_response(
    recovery_identity: &RouterIdentity,
    response: &client::RegisterInstallationResponse,
) -> anyhow::Result<()> {
    let installation_id = response.installation_id.trim();
    if installation_id.is_empty() {
        anyhow::bail!("router installation register returned an empty installation id");
    }
    let retained_secret_is_valid = recovery_identity.installation_id.trim() == installation_id
        && recovery_identity.control_secret.is_some();
    if response.control_secret.is_none() && !retained_secret_is_valid {
        anyhow::bail!("router installation registration did not return a control secret");
    }
    Ok(())
}

fn preserve_router_identity_from_stale_snapshot(
    current: &ServerConfig,
    incoming: &mut ServerConfig,
) {
    let current_api_base = current
        .router_api_base()
        .map(str::trim)
        .unwrap_or_default()
        .trim_end_matches('/');
    let incoming_api_base = incoming
        .router_api_base()
        .map(str::trim)
        .unwrap_or_default()
        .trim_end_matches('/');
    if current_api_base != incoming_api_base {
        return;
    }
    let Some(current_identity) = current.router.identity.as_ref() else {
        return;
    };

    let stale = match incoming.router.identity.as_mut() {
        None => {
            incoming.router.identity = Some(current_identity.clone());
            true
        }
        Some(incoming_identity)
            if incoming_identity.public_key.trim() == current_identity.public_key.trim() =>
        {
            let stale = incoming_identity.installation_id != current_identity.installation_id
                || incoming_identity.private_key != current_identity.private_key
                || incoming_identity.control_secret != current_identity.control_secret;
            if stale {
                *incoming_identity = current_identity.clone();
            }
            stale
        }
        Some(_) => false,
    };
    if stale {
        incoming.router.last_register_error = current.router.last_register_error.clone();
        incoming.router.last_registered_at_ms = current.router.last_registered_at_ms;
    }
}

fn preserve_setup_completion_from_stale_snapshot(
    current: &ServerConfig,
    incoming: &mut ServerConfig,
) {
    let Some(current_notification) = current.setup_completion_notification.as_ref() else {
        return;
    };
    let Some(incoming_notification) = incoming.setup_completion_notification.as_mut() else {
        incoming.setup_completion_notification = Some(current_notification.clone());
        return;
    };
    if incoming_notification.setup_id != current_notification.setup_id {
        if incoming_notification.created_at_ms <= current_notification.created_at_ms {
            *incoming_notification = current_notification.clone();
        }
        return;
    }

    let current_rank = setup_completion_status_rank(current_notification.status);
    let incoming_rank = setup_completion_status_rank(incoming_notification.status);
    let mut merged = if current_rank >= incoming_rank {
        current_notification.clone()
    } else {
        incoming_notification.clone()
    };
    merged.attempt_count = merged
        .attempt_count
        .max(current_notification.attempt_count)
        .max(incoming_notification.attempt_count);
    merged.created_at_ms = [
        current_notification.created_at_ms,
        incoming_notification.created_at_ms,
    ]
    .into_iter()
    .filter(|value| *value > 0)
    .min()
    .unwrap_or_default();
    merged.updated_at_ms = merged
        .updated_at_ms
        .max(current_notification.updated_at_ms)
        .max(incoming_notification.updated_at_ms);
    merged.last_attempt_at_ms = current_notification
        .last_attempt_at_ms
        .into_iter()
        .chain(incoming_notification.last_attempt_at_ms)
        .max();
    if merged.status == SetupCompletionNotificationStatus::Acknowledged {
        merged.password_hint = None;
        merged.next_attempt_at_ms = None;
        merged.last_error = None;
        merged.router_ack_status = current_notification
            .router_ack_status
            .clone()
            .or_else(|| incoming_notification.router_ack_status.clone());
        merged.acknowledged_at_ms = current_notification
            .acknowledged_at_ms
            .into_iter()
            .chain(incoming_notification.acknowledged_at_ms)
            .max();
    } else if merged.status == SetupCompletionNotificationStatus::TerminalFailed {
        merged.password_hint = None;
        merged.next_attempt_at_ms = None;
        merged.router_ack_status = None;
    } else {
        merged.router_ack_status = None;
        merged.next_attempt_at_ms = current_notification
            .next_attempt_at_ms
            .into_iter()
            .chain(incoming_notification.next_attempt_at_ms)
            .max();
    }
    *incoming_notification = merged;
}

const fn setup_completion_status_rank(status: SetupCompletionNotificationStatus) -> u8 {
    match status {
        SetupCompletionNotificationStatus::WaitingForClaim => 0,
        SetupCompletionNotificationStatus::Pending => 1,
        SetupCompletionNotificationStatus::TerminalFailed => 2,
        SetupCompletionNotificationStatus::Acknowledged => 3,
    }
}

fn setup_completion_retry_delay_ms(setup_id: &str, attempt_count: u32) -> i64 {
    let exponent = attempt_count.saturating_sub(1).min(10);
    let base = SETUP_COMPLETION_RETRY_BASE_MS
        .saturating_mul(1_i64 << exponent)
        .min(SETUP_COMPLETION_RETRY_MAX_MS.saturating_mul(5) / 6);
    let digest = Sha256::digest(format!("{setup_id}\n{attempt_count}").as_bytes());
    let jitter_seed = u64::from_be_bytes(digest[..8].try_into().unwrap_or_default());
    let jitter_limit = (base / 5).max(1) as u64;
    base.saturating_add((jitter_seed % (jitter_limit + 1)) as i64)
}

fn now_ms_i64() -> i64 {
    crate::infra::time::now_ms().min(i64::MAX as u128) as i64
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

pub async fn refresh_router_installation_registration(state: &ServerState) -> bool {
    let config = state.config.read().await.clone();
    if !config.is_setup_complete() || config.router_api_base().is_none() {
        return false;
    }

    match state.register_router_installation().await {
        Ok(registration) => {
            if let Err(error) = state
                .complete_router_registration_control_plane("startup_refresh")
                .await
            {
                tracing::warn!(
                    error = %error,
                    "complete router registration refresh failed"
                );
                return false;
            }
            tracing::info!(
                installation_id = %registration.installation_id,
                app_version = %crate::build_info::router_registration_version(),
                "refreshed router installation registration"
            );
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, "router installation registration refresh failed");
            false
        }
    }
}

pub async fn restore_tunnels(state: ServerState) {
    let registration_completed = refresh_router_installation_registration(&state).await;

    if state.config.read().await.is_setup_complete()
        && should_restore_client_tunnel(
            state
                .tunnels
                .status(&tunnel::client_tunnel_key())
                .await
                .as_ref(),
        )
    {
        ensure_client_tunnel_running(state.clone(), "startup_restore").await;
    }

    let auto_start_share_ids = share_tunnel_restore_ids(&state.shares.read().await.shares);
    for share_id in auto_start_share_ids {
        ensure_share_tunnel_running_for(state.clone(), &share_id, "startup_restore").await;
    }

    state.retry_pending_setup_completion_notification().await;

    if !registration_completed {
        if let Err(error) = reconcile_all_shares_to_router(state.clone()).await {
            tracing::warn!(error = %error, "fallback router share reconcile failed");
        }
        if let Err(error) = reconcile_payout_profile_to_router(state.clone()).await {
            tracing::warn!(error = %error, "fallback router payout profile reconcile failed");
        }
    }
    retry_pending_router_share_deletes(state).await;
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

pub fn spawn_auto_upgrade_scheduler(state: ServerState) {
    tokio::spawn(async move {
        loop {
            let (enabled, interval_minutes) = {
                let config = state.config.read().await;
                (
                    config.upgrade_policy.auto_upgrade_enabled,
                    config
                        .upgrade_policy
                        .auto_upgrade_check_interval_minutes
                        .max(5),
                )
            };
            if enabled {
                if let Err(error) = run_auto_upgrade_tick(&state).await {
                    tracing::warn!(error = %error, "auto upgrade tick failed");
                }
                sleep(Duration::from_secs(interval_minutes * 60)).await;
            } else {
                sleep(Duration::from_secs(60)).await;
            }
        }
    });
}

async fn run_auto_upgrade_tick(state: &ServerState) -> anyhow::Result<()> {
    if let Err(error) = report_installation_upgrade_status(state).await {
        tracing::debug!(error = %error, "auto upgrade status report failed");
    }
    if state.upgrade.is_restart_pending().await {
        return Ok(());
    }
    if let Some(handle) = state.upgrade.current().await {
        if matches!(
            *handle.status.lock().await,
            crate::self_update::upgrade::UpgradeStatus::Running
        ) {
            return Ok(());
        }
    }
    let client = state.http_client().await;
    let latest = crate::self_update::version::fetch_latest_release_meta(&client).await;
    if !latest.update_available {
        return Ok(());
    }
    if crate::self_update::version::ensure_binary_writable().is_err() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-server/0.1 auto-upgrade")
        .build()
        .context("build auto-upgrade client")?;
    state
        .upgrade
        .start(
            client,
            Some("auto-upgrade".to_string()),
            true,
            false,
            state.bind_addr,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

pub fn spawn_periodic_installation_status_report(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(60 * 60)).await;
            if let Err(error) = report_installation_upgrade_status(&state).await {
                tracing::debug!(error = %error, "periodic installation status report failed");
            }
        }
    });
}

const DEFAULT_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const MIN_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 15;
const MAX_ROUTER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS: u64 = 5;
const ROUTER_HEARTBEAT_UNREGISTERED_MAX_RETRY_SECS: u64 = 5 * 60;
const ROUTER_HEARTBEAT_WARNING_INTERVAL: Duration = Duration::from_secs(15 * 60);
const ROUTER_HEARTBEAT_ENDPOINT_WARNING_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const ROUTER_HEARTBEAT_SUSTAINED_FAILURES: u32 = 3;
const ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS: usize = 2 * 1024;

pub fn spawn_installation_heartbeat(state: ServerState) {
    tokio::spawn(async move {
        let mut consecutive_failures = 0_u32;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;
        let mut unregistered_retry_secs = ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS;
        loop {
            if run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
            {
                unregistered_retry_secs = ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS;
                sleep(next_router_heartbeat_delay(router_heartbeat_interval_secs())).await;
            } else {
                sleep(Duration::from_secs(unregistered_retry_secs)).await;
                unregistered_retry_secs =
                    next_router_registration_retry_secs(unregistered_retry_secs);
            }
        }
    });
}

async fn run_installation_heartbeat_once(
    state: &ServerState,
    consecutive_failures: &mut u32,
    last_failure_warning: &mut Option<tokio::time::Instant>,
    last_endpoint_warning: &mut Option<tokio::time::Instant>,
) -> bool {
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        if config.is_local_setup_complete() && config.router_api_base().is_some() {
            match state.register_router_installation().await {
                Ok(_) => {
                    match state
                        .complete_router_registration_control_plane("heartbeat_unregistered_retry")
                        .await
                    {
                        Ok(()) => {
                            *consecutive_failures = 0;
                            *last_failure_warning = None;
                            *last_endpoint_warning = None;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "complete heartbeat registration retry failed");
                        }
                    }
                    return true;
                }
                Err(error) => {
                    *consecutive_failures = consecutive_failures.saturating_add(1);
                    warn_on_sustained_heartbeat_failure(
                        last_failure_warning,
                        *consecutive_failures,
                        &error,
                    );
                    record_installation_heartbeat_failure(state, error.to_string()).await;
                }
            }
        }
        return false;
    }
    let http_client = state.http_client().await;
    match client::send_installation_heartbeat(&http_client, &config, &state.process_instance_id)
        .await
    {
        Ok(()) => {
            *consecutive_failures = 0;
            *last_failure_warning = None;
            *last_endpoint_warning = None;
            record_installation_heartbeat_success(state).await;
            state.retry_pending_setup_completion_notification().await;
        }
        Err(client::InstallationHeartbeatError::EndpointUnavailable { status, body }) => {
            let recovery_error = if config
                .registered_router_identity()
                .is_some_and(|identity| identity.control_secret.is_none())
            {
                match state.register_router_installation().await {
                    Ok(_) => state
                        .complete_router_registration_control_plane(
                            "heartbeat_legacy_secret_recovery",
                        )
                        .await
                        .err()
                        .map(|error| {
                            format!(
                                "complete legacy heartbeat registration recovery failed: {error}"
                            )
                        }),
                    Err(error) => Some(format!(
                        "router heartbeat compatibility registration recovery failed: {error}"
                    )),
                }
            } else {
                None
            };
            if let Some(error) = recovery_error {
                *consecutive_failures = consecutive_failures.saturating_add(1);
                warn_on_sustained_heartbeat_failure(
                    last_failure_warning,
                    *consecutive_failures,
                    &error,
                );
                if *consecutive_failures >= ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                    record_installation_heartbeat_failure(state, error).await;
                }
            } else {
                *consecutive_failures = 0;
                *last_failure_warning = None;
                record_installation_heartbeat_compatible(state).await;
            }
            let now = tokio::time::Instant::now();
            if rate_limited_warning_due(
                last_endpoint_warning,
                now,
                ROUTER_HEARTBEAT_ENDPOINT_WARNING_INTERVAL,
            ) {
                tracing::warn!(
                    %status,
                    response = %body,
                    "router does not support installation heartbeat; upgrade Router to enable offline notifications"
                );
            }
        }
        Err(client::InstallationHeartbeatError::RegistrationRequired { status, body }) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            let heartbeat_error =
                format!("router installation heartbeat requires registration: {status}: {body}");
            tracing::debug!(%status, response = %body, "router installation heartbeat requires registration recovery");
            let recovery_result = match state.register_router_installation().await {
                Ok(_) => match state
                    .complete_router_registration_control_plane("heartbeat_identity_recovery")
                    .await
                {
                    Ok(()) => {
                        let recovered_config = state.config_snapshot().await;
                        let recovered_http_client = state.http_client().await;
                        match client::send_installation_heartbeat(
                            &recovered_http_client,
                            &recovered_config,
                            &state.process_instance_id,
                        )
                        .await
                        {
                            Ok(()) => Ok(true),
                            Err(client::InstallationHeartbeatError::EndpointUnavailable {
                                ..
                            }) => Ok(false),
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Err(error) => Err(format!(
                        "complete heartbeat identity recovery failed: {error}"
                    )),
                },
                Err(error) => Err(format!(
                    "router installation registration recovery failed: {error}"
                )),
            };
            match recovery_result {
                Ok(true) => {
                    *consecutive_failures = 0;
                    *last_failure_warning = None;
                    *last_endpoint_warning = None;
                    record_installation_heartbeat_success(state).await;
                }
                Ok(false) => {
                    *consecutive_failures = 0;
                    *last_failure_warning = None;
                    record_installation_heartbeat_compatible(state).await;
                }
                Err(recovery_error) => {
                    let error = format!("{heartbeat_error}; {recovery_error}");
                    warn_on_sustained_heartbeat_failure(
                        last_failure_warning,
                        *consecutive_failures,
                        &error,
                    );
                    if *consecutive_failures >= ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                        record_installation_heartbeat_failure(state, error).await;
                    }
                }
            }
        }
        Err(error) => {
            *consecutive_failures = consecutive_failures.saturating_add(1);
            tracing::debug!(error = %error, consecutive_failures = *consecutive_failures, "router installation heartbeat failed");
            warn_on_sustained_heartbeat_failure(
                last_failure_warning,
                *consecutive_failures,
                &error,
            );
            if *consecutive_failures >= ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                record_installation_heartbeat_failure(state, error.to_string()).await;
            }
        }
    }
    true
}

async fn record_installation_heartbeat_success(state: &ServerState) {
    state
        .mutate_shares_debounced(|shares| {
            shares.last_router_heartbeat_ms = Some(crate::infra::time::now_ms());
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await;
}

async fn record_installation_heartbeat_compatible(state: &ServerState) {
    state
        .mutate_shares_debounced(|shares| {
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await;
}

async fn record_installation_heartbeat_failure(state: &ServerState, message: String) {
    let message = bounded_router_heartbeat_state_error(message);
    state
        .mutate_shares_debounced(|shares| {
            shares.router_registered = false;
            shares.last_router_error = Some(message);
        })
        .await;
}

fn bounded_router_heartbeat_state_error(message: String) -> String {
    message
        .chars()
        .take(ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS)
        .collect()
}

fn warn_on_sustained_heartbeat_failure(
    last_warning: &mut Option<tokio::time::Instant>,
    consecutive_failures: u32,
    error: &impl std::fmt::Display,
) {
    if consecutive_failures < ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
        return;
    }
    let now = tokio::time::Instant::now();
    if rate_limited_warning_due(last_warning, now, ROUTER_HEARTBEAT_WARNING_INTERVAL) {
        tracing::warn!(
            %error,
            consecutive_failures,
            "router installation heartbeat is persistently failing"
        );
    }
}

fn rate_limited_warning_due(
    last_warning: &mut Option<tokio::time::Instant>,
    now: tokio::time::Instant,
    interval: Duration,
) -> bool {
    if last_warning.is_some_and(|last| now.duration_since(last) < interval) {
        return false;
    }
    *last_warning = Some(now);
    true
}

fn router_heartbeat_interval_secs() -> u64 {
    let value = env::var("CC_SWITCH_SERVER_ROUTER_HEARTBEAT_INTERVAL_SECS").ok();
    normalize_router_heartbeat_interval(value.as_deref())
}

fn normalize_router_heartbeat_interval(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_ROUTER_HEARTBEAT_INTERVAL_SECS)
        .clamp(
            MIN_ROUTER_HEARTBEAT_INTERVAL_SECS,
            MAX_ROUTER_HEARTBEAT_INTERVAL_SECS,
        )
}

fn next_router_heartbeat_delay(interval_secs: u64) -> Duration {
    let jitter = (interval_secs / 10).max(1);
    let width = jitter.saturating_mul(2).saturating_add(1);
    let offset = i128::from(rand::thread_rng().next_u64() % width) - i128::from(jitter);
    Duration::from_secs((i128::from(interval_secs) + offset).max(1) as u64)
}

fn next_router_registration_retry_secs(current_secs: u64) -> u64 {
    current_secs
        .max(ROUTER_HEARTBEAT_UNREGISTERED_RETRY_SECS)
        .saturating_mul(2)
        .min(ROUTER_HEARTBEAT_UNREGISTERED_MAX_RETRY_SECS)
}

pub async fn report_installation_upgrade_status(state: &ServerState) -> anyhow::Result<()> {
    let config = state.config.read().await.clone();
    if !config.has_registered_router_identity() {
        return Ok(());
    }
    let client = state.http_client().await;
    let latest = crate::self_update::version::fetch_latest_release_meta(&client).await;
    let upgrade_capable = crate::self_update::version::ensure_binary_writable().is_ok();
    crate::clients::router::client::report_installation_status(
        &client,
        &config,
        &config.upgrade_policy,
        &latest,
        upgrade_capable,
    )
    .await
}

pub fn spawn_periodic_share_sync_retry(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            run_periodic_share_sync_retry_once(&state).await;
        }
    });
}

async fn run_periodic_share_sync_retry_once(state: &ServerState) {
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        return;
    }
    retry_pending_router_share_deletes(state.clone()).await;
    if state.router_share_prune_retry_requested() {
        if let Err(error) = reconcile_all_shares_to_router(state.clone()).await {
            tracing::warn!(error = %error, "periodic router share prune snapshot retry failed");
        }
        return;
    }
    let pending_ids = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .filter(|share| {
            share.router_synced_revision < share.config_revision
                || share.router_last_sync_error.is_some()
        })
        .map(|share| share.id.clone())
        .collect::<Vec<_>>();
    for share_id in pending_ids {
        if let Err(error) = sync_one_share_to_router(state, &share_id).await {
            tracing::warn!(share_id = %share_id, error = %error, "periodic router share sync retry failed");
        }
    }
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
    let locked_provider_type = account.provider_type;
    let Some(_guard) = state
        .account_refresh_locks
        .try_lock(locked_provider_type, &account.id)
    else {
        return;
    };
    // The periodic scan clones accounts before acquiring the per-account lock.
    // A workspace switch may complete between those two operations, so always
    // re-read under the lock and re-check due state before issuing requests.
    let Some(account) = state.find_account_by_id(&account.id).await else {
        return;
    };
    if account.provider_type != locked_provider_type || !account_quota_refresh_due(&account, now) {
        return;
    }
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
            let account_before_quota_refresh = active_account.clone();
            let account = {
                let mut store = state.accounts.write().await;
                store
                    .mark_refresh_success(&active_account.id, update)
                    .unwrap_or(active_account)
            };
            save_accounts_debounced(state);
            if let Err(error) = state
                .refresh_automatic_subscription_metadata_if_changed(
                    &account_before_quota_refresh,
                    &account,
                )
                .await
            {
                tracing::warn!(
                    account_id = %account.id,
                    %error,
                    "background quota refresh could not persist subscription metadata change"
                );
            }
            emit_oauth_quota_updated(state, &account, true);
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
    ensure_share_tunnel_running_for(state, share_id, "share_state_ensure").await;
}

pub async fn ensure_share_tunnel_running_for(state: ServerState, share_id: &str, reason: &str) {
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
    ensure_share_tunnel_actor(state, share.id, reason).await;
}

pub async fn start_client_tunnel(state: ServerState) {
    ensure_client_tunnel_running(state, "client_tunnel_start").await;
}

pub async fn ensure_client_tunnel_running(state: ServerState, reason: &str) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let spec_id = client_tunnel_spec_id(&state, &local_addr).await;
    let (lease_fn, activate_tunnel_fn, tunnel_state_fn, renew_lease_fn) =
        client_tunnel_callbacks(&state);
    state
        .tunnels
        .ensure_running(
            tunnel::client_tunnel_key(),
            "client-web",
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            reason,
            spec_id,
        )
        .await;
}

pub async fn force_reconnect_client_tunnel(state: ServerState, reason: &str) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let spec_id = client_tunnel_spec_id(&state, &local_addr).await;
    let (lease_fn, activate_tunnel_fn, tunnel_state_fn, renew_lease_fn) =
        client_tunnel_callbacks(&state);
    state
        .tunnels
        .force_reconnect(
            tunnel::client_tunnel_key(),
            "client-web",
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            reason,
            spec_id,
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
    ensure_share_tunnel_running_for(state, &share_id, "share_tunnel_start").await;
}

async fn ensure_share_tunnel_actor(state: ServerState, share_id: String, reason: &str) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let key = tunnel::share_tunnel_key(&share_id);
    let spec_id = share_tunnel_spec_id(&state, &share_id, &local_addr).await;
    let (lease_fn, activate_tunnel_fn, tunnel_state_fn, renew_lease_fn) =
        share_tunnel_callbacks(&state, &share_id);
    state
        .tunnels
        .ensure_running(
            key,
            "share-http",
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            reason,
            spec_id,
        )
        .await;
}

pub async fn force_reconnect_share_tunnel(state: ServerState, share_id: String, reason: &str) {
    let share = {
        let shares = state.shares.read().await;
        shares.get(&share_id).cloned()
    };
    if !share.as_ref().is_some_and(should_restore_share_tunnel) {
        return;
    }
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let key = tunnel::share_tunnel_key(&share_id);
    let spec_id = share_tunnel_spec_id(&state, &share_id, &local_addr).await;
    let (lease_fn, activate_tunnel_fn, tunnel_state_fn, renew_lease_fn) =
        share_tunnel_callbacks(&state, &share_id);
    state
        .tunnels
        .force_reconnect(
            key,
            "share-http",
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            reason,
            spec_id,
        )
        .await;
}

fn client_tunnel_callbacks(
    state: &ServerState,
) -> (LeaseFn, ActivateTunnelFn, TunnelStateFn, RenewLeaseFn) {
    let lease_state = state.clone();
    let lease_fn: LeaseFn = Arc::new(move |request| {
        let lease_state = lease_state.clone();
        Box::pin(async move { issue_client_tunnel_lease(lease_state, request).await })
    });
    (
        lease_fn,
        activate_tunnel_callback(state),
        tunnel_state_callback(state),
        renew_tunnel_callback(state),
    )
}

async fn client_tunnel_spec_id(state: &ServerState, local_addr: &str) -> String {
    let config = state.config.read().await;
    format!(
        "client-web|{}|{}|{}|{local_addr}",
        config.router_api_base().unwrap_or_default(),
        config
            .router
            .identity
            .as_ref()
            .map(|identity| identity.installation_id.as_str())
            .unwrap_or_default(),
        config
            .client
            .tunnel_subdomain
            .as_deref()
            .unwrap_or_default(),
    )
}

async fn share_tunnel_spec_id(state: &ServerState, share_id: &str, local_addr: &str) -> String {
    let config = state.config.read().await;
    let router_api_base = config.router_api_base().unwrap_or_default().to_string();
    let installation_id = config
        .router
        .identity
        .as_ref()
        .map(|identity| identity.installation_id.clone())
        .unwrap_or_default();
    drop(config);
    let subdomain = state
        .shares
        .read()
        .await
        .get(share_id)
        .and_then(|share| share.tunnel_subdomain.clone())
        .unwrap_or_default();
    format!("share-http|{router_api_base}|{installation_id}|{share_id}|{subdomain}|{local_addr}")
}

fn share_tunnel_callbacks(
    state: &ServerState,
    share_id: &str,
) -> (LeaseFn, ActivateTunnelFn, TunnelStateFn, RenewLeaseFn) {
    let lease_state = state.clone();
    let lease_share_id = share_id.to_string();
    let lease_fn: LeaseFn = Arc::new(move |request| {
        let lease_state = lease_state.clone();
        let lease_share_id = lease_share_id.clone();
        Box::pin(
            async move { issue_share_tunnel_lease(lease_state, lease_share_id, request).await },
        )
    });
    (
        lease_fn,
        activate_tunnel_callback(state),
        tunnel_state_callback(state),
        renew_tunnel_callback(state),
    )
}

fn activate_tunnel_callback(state: &ServerState) -> ActivateTunnelFn {
    let activate_state = state.clone();
    Arc::new(move |lease| {
        let activate_state = activate_state.clone();
        Box::pin(async move {
            let config = activate_state.config.read().await.clone();
            let http_client = activate_state.http_client().await;
            client::activate_namespace_tunnel(
                &http_client,
                &config,
                ActivateTunnelPayload {
                    protocol_epoch: PROTOCOL_EPOCH.to_string(),
                    router_id: lease.router_id,
                    lease_id: lease.lease_id,
                    connection_id: lease.connection_id,
                    route_id: lease.route_id,
                    rotation_id: lease.rotation_id,
                    generation: lease.generation,
                    expected_generation: lease.expected_generation,
                },
            )
            .await
        })
    })
}

fn tunnel_state_callback(state: &ServerState) -> TunnelStateFn {
    let query_state = state.clone();
    Arc::new(move |lease| {
        let query_state = query_state.clone();
        Box::pin(async move {
            let config = query_state.config.read().await.clone();
            let http_client = query_state.http_client().await;
            client::namespace_tunnel_state(
                &http_client,
                &config,
                TunnelStatePayload {
                    protocol_epoch: PROTOCOL_EPOCH.to_string(),
                    router_id: lease.router_id,
                    lease_id: lease.lease_id,
                    connection_id: lease.connection_id,
                    route_id: lease.route_id,
                    rotation_id: lease.rotation_id,
                    generation: lease.generation,
                    expected_generation: lease.expected_generation,
                },
            )
            .await
        })
    })
}

fn renew_tunnel_callback(state: &ServerState) -> RenewLeaseFn {
    let renew_state = state.clone();
    Arc::new(move |lease| {
        let renew_state = renew_state.clone();
        Box::pin(async move { renew_router_tunnel_lease(renew_state, lease).await })
    })
}

async fn renew_router_tunnel_lease(
    state: ServerState,
    lease: NamespaceLeaseResponse,
) -> Result<String, TunnelRenewalError> {
    let config = state.config.read().await.clone();
    let configuration_available =
        config.router_api_base().is_some() && config.registered_router_identity().is_some();
    let http_client = state.http_client().await;
    let payload = NamespaceRenewLeasePayload {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        router_id: client::tunnel_router_id(&config).map_err(|error| {
            TunnelRenewalError::FatalConfiguration(format!("resolve router id failed: {error}"))
        })?,
        lease_id: lease.lease_id,
        connection_id: lease.connection_id,
        route_id: lease.route_id,
        rotation_id: lease.rotation_id,
        generation: lease.generation,
        expected_generation: lease.expected_generation,
    };
    client::renew_namespace_lease(&http_client, &config, payload)
        .await
        .map(|renewed| renewed.expires_at)
        .map_err(|error| TunnelRenewalError::from_router(error, configuration_available))
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
    if !config.has_registered_router_identity() {
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
    if !config.has_registered_router_identity() {
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
            let sync_result = sync_one_share_to_router(state, &edit.share_id).await;
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

pub(crate) async fn sync_one_share_to_router(
    state: &ServerState,
    share_id: &str,
) -> anyhow::Result<()> {
    let _sync = state.lock_router_share_sync().await;
    sync_one_share_to_router_locked(state, share_id).await
}

async fn sync_one_share_to_router_locked(
    state: &ServerState,
    share_id: &str,
) -> anyhow::Result<()> {
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        anyhow::bail!("router installation is not registered");
    }
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = {
        let store = state.shares.read().await;
        let blocked_by_delete = store.has_pending_router_delete_for_share(share_id);
        if blocked_by_delete {
            return Ok(());
        }
        let Some(share) = store.shares.iter().find(|share| share.id == share_id) else {
            return Ok(());
        };
        share.clone()
    };
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
    let result = client::push_share_ops(&http_client, &config, vec![op]).await;
    let router_base = config.router_api_base().map(str::to_string);
    {
        let mut store = state.shares.write().await;
        match &result {
            Ok(()) => {
                store.router_registered = true;
                store.last_router_error = None;
                store.mark_router_sync(
                    share_id,
                    share.config_revision,
                    router_base,
                    Ok(crate::infra::time::now_ms()),
                );
            }
            Err(error) => {
                let message = error.to_string();
                store.last_router_error = Some(message.clone());
                store.mark_router_sync(share_id, share.config_revision, router_base, Err(message));
            }
        }
    }
    save_shares_debounced(state);
    result
}

pub(crate) async fn sync_shares_to_router(
    state: &ServerState,
    share_ids: &[String],
) -> anyhow::Result<()> {
    if share_ids.is_empty() {
        return Ok(());
    }
    let _sync = state.lock_router_share_sync().await;
    let config = state.config_snapshot().await;
    if !config.has_registered_router_identity() {
        anyhow::bail!("router installation is not registered");
    }
    let requested_ids = share_ids.iter().cloned().collect::<BTreeSet<_>>();
    let active_ids = {
        let store = state.shares.read().await;
        requested_ids
            .into_iter()
            .filter(|share_id| {
                store.get(share_id).is_some()
                    && !store.has_pending_router_delete_for_share(share_id)
            })
            .collect::<BTreeSet<_>>()
    };
    let operations = build_router_share_upsert_ops(state, &active_ids).await;
    if operations.is_empty() {
        return Ok(());
    }

    let http_client = state.http_client().await;
    for chunk in operations.chunks(ROUTER_SHARE_SYNC_BATCH_SIZE) {
        let chunk = chunk.to_vec();
        match client::push_share_ops(&http_client, &config, chunk.clone()).await {
            Ok(()) => mark_router_share_upserts_synced(state, &config, &chunk).await,
            Err(error) => {
                mark_router_share_upserts_failed(state, &config, &chunk, error.to_string()).await;
                return Err(error);
            }
        }
    }
    Ok(())
}

pub(crate) fn spawn_router_share_delete_retry(state: ServerState, tombstone: ShareDeleteTombstone) {
    tokio::spawn(async move {
        if let Err(error) = retry_router_share_deletes(&state, &[tombstone.clone()]).await {
            tracing::warn!(
                share_id = %tombstone.share_id,
                operation_id = %tombstone.operation_id,
                %error,
                "router share delete remains pending"
            );
        }
    });
}

async fn retry_pending_router_share_deletes(state: ServerState) {
    let pending = state.shares.read().await.pending_router_deletes.clone();
    if pending.is_empty() {
        return;
    }
    if let Err(error) = retry_router_share_deletes(&state, &pending).await {
        tracing::warn!(
            pending_deletes = pending.len(),
            %error,
            "router share delete outbox retry failed"
        );
    }
}

async fn retry_router_share_deletes(
    state: &ServerState,
    tombstones: &[ShareDeleteTombstone],
) -> anyhow::Result<usize> {
    let _sync = state.lock_router_share_sync().await;
    let mut completed = 0;
    for chunk in tombstones.chunks(ROUTER_SHARE_SYNC_BATCH_SIZE) {
        completed += retry_router_share_delete_chunk_locked(state, chunk).await?;
    }
    Ok(completed)
}

async fn retry_router_share_delete_chunk_locked(
    state: &ServerState,
    tombstones: &[ShareDeleteTombstone],
) -> anyhow::Result<usize> {
    let config = state.config_snapshot().await;
    let active = {
        let store = state.shares.read().await;
        tombstones
            .iter()
            .filter(|tombstone| {
                store
                    .pending_router_delete(&tombstone.share_id, &tombstone.operation_id)
                    .is_some()
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    if active.is_empty() {
        return Ok(0);
    }
    if !config.has_registered_router_identity() {
        let error = anyhow::anyhow!("router installation is not registered");
        record_router_share_delete_failures(state, &active, error.to_string()).await?;
        return Err(error);
    }
    let router_api_base = config
        .router_api_base()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?;
    let installation_id = config
        .registered_router_identity()
        .expect("registered identity checked above")
        .installation_id
        .trim()
        .to_string();
    let legacy_operation_ids = active
        .iter()
        .filter(|tombstone| tombstone.has_legacy_router_target())
        .map(|tombstone| tombstone.operation_id.clone())
        .collect::<Vec<_>>();
    if !legacy_operation_ids.is_empty() {
        state
            .mutate_shares_immediate(|store| {
                for operation_id in &legacy_operation_ids {
                    store.bind_legacy_router_delete_target(
                        operation_id,
                        &router_api_base,
                        &installation_id,
                    );
                }
            })
            .await?;
    }
    let requested_operation_ids = active
        .iter()
        .map(|tombstone| tombstone.operation_id.as_str())
        .collect::<BTreeSet<_>>();
    let active = state
        .shares
        .read()
        .await
        .pending_router_deletes
        .iter()
        .filter(|tombstone| {
            requested_operation_ids.contains(tombstone.operation_id.as_str())
                && tombstone.router_target_matches(&router_api_base, &installation_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    if active.is_empty() {
        tracing::debug!(
            router_api_base,
            installation_id,
            "router share delete retry skipped tombstones bound to another Router"
        );
        return Ok(0);
    }

    let current_ids = {
        let store = state.shares.read().await;
        active
            .iter()
            .filter(|tombstone| store.get(&tombstone.share_id).is_some())
            .map(|tombstone| tombstone.share_id.clone())
            .collect::<BTreeSet<_>>()
    };
    let mut ops = active
        .iter()
        .filter(|tombstone| !current_ids.contains(&tombstone.share_id))
        .map(|tombstone| ShareSyncOperation {
            kind: "delete".to_string(),
            share_id: Some(tombstone.share_id.clone()),
            share: None,
        })
        .collect::<Vec<_>>();
    ops.extend(build_router_share_upsert_ops(state, &current_ids).await);
    let http_client = state.http_client().await;
    if let Err(error) = client::push_share_ops(&http_client, &config, ops).await {
        record_router_share_delete_failures(state, &active, error.to_string()).await?;
        return Err(error);
    }

    let compensation_ids = {
        let store = state.shares.read().await;
        active
            .iter()
            .filter(|tombstone| store.get(&tombstone.share_id).is_some())
            .map(|tombstone| tombstone.share_id.clone())
            .collect::<BTreeSet<_>>()
    };
    let compensation_ops = build_router_share_upsert_ops(state, &compensation_ids).await;
    if !compensation_ops.is_empty() {
        if let Err(error) =
            client::push_share_ops(&http_client, &config, compensation_ops.clone()).await
        {
            record_router_share_delete_failures(state, &active, error.to_string()).await?;
            return Err(error);
        }
        mark_router_share_upserts_synced(state, &config, &compensation_ops).await;
    }

    let operation_ids = active
        .iter()
        .map(|tombstone| tombstone.operation_id.clone())
        .collect::<BTreeSet<_>>();
    let completed = state
        .mutate_shares_immediate(|store| {
            operation_ids
                .iter()
                .filter(|operation_id| store.complete_pending_router_delete(operation_id))
                .count()
        })
        .await?;
    Ok(completed)
}

async fn build_router_share_upsert_ops(
    state: &ServerState,
    share_ids: &BTreeSet<String>,
) -> Vec<ShareSyncOperation> {
    if share_ids.is_empty() {
        return Vec::new();
    }
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .filter(|share| share_ids.contains(&share.id))
        .cloned()
        .collect::<Vec<_>>();
    shares
        .iter()
        .map(|share| ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(descriptor_for_share_with_accounts_and_usage(
                share,
                &providers,
                Some(&accounts),
                Some(&usage),
            )),
        })
        .collect()
}

async fn mark_router_share_upserts_synced(
    state: &ServerState,
    config: &ServerConfig,
    operations: &[ShareSyncOperation],
) {
    let revisions = operations
        .iter()
        .filter_map(|operation| {
            operation
                .share
                .as_ref()
                .map(|share| (share.share_id.clone(), share.config_revision))
        })
        .collect::<Vec<_>>();
    let router_base = config.router_api_base().map(str::to_string);
    let now = crate::infra::time::now_ms();
    state
        .mutate_shares_debounced(|store| {
            store.router_registered = true;
            store.last_router_error = None;
            for (share_id, revision) in &revisions {
                store.mark_router_sync(share_id, *revision, router_base.clone(), Ok(now));
            }
        })
        .await;
}

async fn mark_router_share_upserts_failed(
    state: &ServerState,
    config: &ServerConfig,
    operations: &[ShareSyncOperation],
    message: String,
) {
    let revisions = operations
        .iter()
        .filter_map(|operation| {
            operation
                .share
                .as_ref()
                .map(|share| (share.share_id.clone(), share.config_revision))
        })
        .collect::<Vec<_>>();
    let router_base = config.router_api_base().map(str::to_string);
    state
        .mutate_shares_debounced(|store| {
            store.last_router_error = Some(message.clone());
            for (share_id, revision) in &revisions {
                store.mark_router_sync(
                    share_id,
                    *revision,
                    router_base.clone(),
                    Err(message.clone()),
                );
            }
        })
        .await;
}

async fn record_router_share_delete_failures(
    state: &ServerState,
    tombstones: &[ShareDeleteTombstone],
    message: String,
) -> anyhow::Result<()> {
    let operation_ids = tombstones
        .iter()
        .map(|tombstone| tombstone.operation_id.clone())
        .collect::<BTreeSet<_>>();
    state
        .mutate_shares_immediate(|store| {
            for operation_id in operation_ids {
                store.mark_pending_router_delete_failure(&operation_id, message.clone());
            }
        })
        .await
}

/// Reconcile the router's installation-scoped share set with the current local
/// store. This is an internal recovery path used after startup/registration;
/// it is intentionally not exposed as a manual Web API action.
pub async fn reconcile_all_shares_to_router(state: ServerState) -> anyhow::Result<usize> {
    let _sync = state.lock_router_share_sync().await;
    let config = state.config.read().await.clone();
    if !config.has_registered_router_identity() {
        return Ok(0);
    }
    let router_api_base = config
        .router_api_base()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?;
    let installation_id = config
        .registered_router_identity()
        .expect("registered identity checked above")
        .installation_id
        .trim()
        .to_string();

    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let (shares, prune_already_applied) = {
        let store = state.shares.read().await;
        (
            store.shares.clone(),
            store.router_share_prune_applied_for(&router_api_base, &installation_id),
        )
    };
    if prune_already_applied {
        state.clear_router_share_prune_retry();
    } else {
        state.request_router_share_prune_retry();
    }
    let ops = shares
        .iter()
        .map(|share| ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(descriptor_for_share_with_accounts_and_usage(
                share,
                &providers,
                Some(&accounts),
                Some(&usage),
            )),
        })
        .collect::<Vec<_>>();

    let http_client = state.http_client().await;
    let result: anyhow::Result<()> = async {
        for chunk in ops.chunks(ROUTER_SHARE_SYNC_BATCH_SIZE) {
            client::push_share_ops(&http_client, &config, chunk.to_vec()).await?;
        }
        Ok(())
    }
    .await;
    let router_base = config.router_api_base().map(str::to_string);
    let now = crate::infra::time::now_ms();
    state
        .mutate_shares_debounced(|store| match &result {
            Ok(()) => {
                store.router_registered = true;
                store.last_router_error = None;
                for share in &shares {
                    store.mark_router_sync(
                        &share.id,
                        share.config_revision,
                        router_base.clone(),
                        Ok(now),
                    );
                }
            }
            Err(error) => {
                let message = error.to_string();
                store.last_router_error = Some(message.clone());
                for share in &shares {
                    store.mark_router_sync(
                        &share.id,
                        share.config_revision,
                        router_base.clone(),
                        Err(message.clone()),
                    );
                }
            }
        })
        .await;
    result?;

    if !prune_already_applied {
        match client::prune_shares(
            &http_client,
            &config,
            shares.iter().map(|share| share.id.clone()).collect(),
        )
        .await
        {
            Ok(client::SharePruneOutcome::Applied) => {
                state
                    .mutate_shares_immediate(|store| {
                        store.mark_router_share_prune_applied(&router_api_base, &installation_id);
                    })
                    .await?;
                state.clear_router_share_prune_retry();
            }
            Ok(client::SharePruneOutcome::Unsupported) => {
                state.clear_router_share_prune_retry();
                tracing::debug!(
                    installation_id,
                    "router does not support one-time share prune"
                );
            }
            Err(error) => {
                let message = error.to_string();
                state
                    .mutate_shares_debounced(|store| {
                        store.last_router_error = Some(message.clone());
                    })
                    .await;
                return Err(error);
            }
        }
    }

    Ok(shares.len())
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
    if !config.has_registered_router_identity() || state.shares.read().await.shares.is_empty() {
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
    is_prefixed_router_request_id(value) || is_canonical_uuid(value)
}

fn is_prefixed_router_request_id(value: &str) -> bool {
    (8..=80).contains(&value.len())
        && value.starts_with("req_")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
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

pub(crate) async fn ensure_router_installation_owner_bound(
    state: &ServerState,
    config: &ServerConfig,
) -> anyhow::Result<()> {
    let expected_owner = config
        .owner
        .email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("owner email is not configured"))?;
    if !config.has_registered_router_identity() {
        anyhow::bail!("router installation is not registered");
    }

    let http_client = state.http_client().await;
    if installation_owner_matches(config, &http_client, expected_owner).await? {
        return Ok(());
    }

    match crate::clients::router::email_auth::bind_owner_email_at_setup(
        &http_client,
        config,
        expected_owner,
    )
    .await
    {
        Ok(binding)
            if binding.ok
                && binding.owner_verified
                && binding.owner_email.eq_ignore_ascii_case(expected_owner) =>
        {
            return Ok(());
        }
        Ok(binding) if binding.ok && !binding.owner_verified => {
            anyhow::bail!(
                "router installation owner email is not verified; upgrade cc-switch-router to the latest release"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                error = %error.message,
                "bind router owner email during installation bootstrap failed"
            );
        }
    }

    if installation_owner_matches(config, &http_client, expected_owner).await? {
        return Ok(());
    }

    anyhow::bail!("router installation owner email is not bound to {expected_owner}")
}

async fn installation_owner_matches(
    config: &ServerConfig,
    http_client: &reqwest::Client,
    expected_owner: &str,
) -> anyhow::Result<bool> {
    let owner_status = client::get_installation_owner_email_status(http_client, config).await?;
    let Some(bound_owner) = owner_status.owner_email.as_deref() else {
        return Ok(false);
    };
    if bound_owner.eq_ignore_ascii_case(expected_owner) {
        return Ok(owner_status.owner_verified);
    }
    anyhow::bail!(
        "router installation owner email ({bound_owner}) does not match configured owner ({expected_owner})"
    )
}

async fn issue_client_tunnel_lease(
    state: ServerState,
    request: TunnelLeaseRequest,
) -> anyhow::Result<NamespaceLeaseResponse> {
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        anyhow::bail!("setup is incomplete");
    }
    if !config.has_registered_router_identity() {
        if let Err(error) = state.register_router_installation().await {
            return Err(error);
        }
        config = state.config_snapshot().await;
        state
            .complete_router_registration_control_plane("implicit_client_tunnel_registration")
            .await?;
    }

    if let Err(error) = ensure_router_installation_owner_bound(&state, &config).await {
        record_router_error(&state, &config, error.to_string()).await;
        return Err(error);
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
    ClientSubdomain::parse(&subdomain)?;
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
    crate::client_tunnel_provision::mark_claim_success(&state, &mut config).await;
    let installation_id = config
        .registered_router_identity()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?
        .installation_id
        .clone();
    let payload = NamespaceLeasePayload {
        protocol_epoch: PROTOCOL_EPOCH.to_string(),
        router_id: client::tunnel_router_id(&config)?,
        route_id: format!("client:{installation_id}"),
        rotation_id: request.rotation_id,
        generation: request.generation,
        expected_generation: request.expected_generation,
        requested_subdomain: subdomain,
        tunnel_type: "client-web-http".to_string(),
        share: None,
    };
    let lease = match client::issue_namespace_lease(&http_client, &config, payload).await {
        Ok(lease) => lease,
        Err(error) => {
            record_router_error(&state, &config, error.to_string()).await;
            return Err(error);
        }
    };

    let mut next = config;
    next.client.tunnel_status = Some("connected".to_string());
    next.router.ssh_host = Some(lease.ssh_addr.clone());
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
    request: TunnelLeaseRequest,
) -> anyhow::Result<NamespaceLeaseResponse> {
    let config = state.config.read().await.clone();
    if !config.has_registered_router_identity() {
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
    let mut descriptor = descriptor;
    let client_subdomain = config
        .client
        .tunnel_subdomain
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))
        .and_then(|value| ClientSubdomain::parse(value).map_err(Into::into))?;
    descriptor.subdomain = resolve_share_label(&descriptor.subdomain, &client_subdomain)?;
    let requested_subdomain = descriptor.subdomain.clone();
    let http_client = state.http_client().await;
    client::claim_share_subdomain(&http_client, &config, descriptor.clone()).await?;
    client::issue_namespace_lease(
        &http_client,
        &config,
        NamespaceLeasePayload {
            protocol_epoch: PROTOCOL_EPOCH.to_string(),
            router_id: client::tunnel_router_id(&config)?,
            route_id: request.route_id,
            rotation_id: request.rotation_id,
            generation: request.generation,
            expected_generation: request.expected_generation,
            requested_subdomain,
            tunnel_type: "http".to_string(),
            share: Some(descriptor),
        },
    )
    .await
}

fn resolve_share_label(
    configured: &str,
    client_subdomain: &ClientSubdomain,
) -> anyhow::Result<String> {
    if let Some((slug, suffix)) = configured.split_once("--") {
        let slug = ShareSlug::parse(slug)?;
        let suffix = ClientSubdomain::parse(suffix)?;
        if &suffix != client_subdomain {
            anyhow::bail!("share host belongs to another client subdomain");
        }
        return Ok(format!("{}--{}", slug.as_str(), suffix.as_str()));
    }
    let slug = ShareSlug::parse(configured)?;
    Ok(format!("{}--{}", slug.as_str(), client_subdomain.as_str()))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::Cli;
    use crate::clients::router::tunnel::TunnelRuntimeStatus;
    use crate::domain::accounts::store::Account;
    use crate::domain::providers::model::{AppKind, ProviderType};
    use crate::domain::sharing::shares::{Share, ShareAcl, UpsertShareInput};
    use crate::domain::usage::store::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};
    use axum::extract::State as AxumState;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::sync::Mutex as TokioMutex;

    use super::*;

    fn router_sync_share_input(share_id: &str, provider_id: &str) -> UpsertShareInput {
        UpsertShareInput {
            id: Some(share_id.to_string()),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: provider_id.to_string(),
            provider_type: ProviderType::Codex,
            display_name: Some("Router sync share".to_string()),
            enabled: None,
            status: None,
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: Some(format!("{}sync", share_id.replace('-', ""))),
            acl: None,
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            for_sale: None,
            free_access: None,
            sale_market_kind: None,
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: None,
            description: None,
            bindings: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            user_grants: BTreeMap::new(),
        }
    }

    async fn create_router_delete_tombstones(
        state: &ServerState,
        prefix: &str,
        count: usize,
    ) -> Vec<ShareDeleteTombstone> {
        let config = state.config_snapshot().await;
        let router_api_base = config.router_api_base().unwrap().to_string();
        let installation_id = config
            .registered_router_identity()
            .unwrap()
            .installation_id
            .clone();
        state
            .mutate_shares_immediate(|store| {
                (0..count)
                    .map(|index| {
                        let share_id = format!("{prefix}{index}");
                        store
                            .upsert(router_sync_share_input(
                                &share_id,
                                &format!("provider-{prefix}-{index}"),
                            ))
                            .unwrap();
                        store
                            .delete_for_router_target(&share_id, &router_api_base, &installation_id)
                            .unwrap()
                    })
                    .collect()
            })
            .await
            .unwrap()
    }

    async fn create_router_shares(state: &ServerState, prefix: &str, count: usize) {
        state
            .mutate_shares_immediate(|store| {
                for index in 0..count {
                    let share_id = format!("{prefix}{index}");
                    store
                        .upsert(router_sync_share_input(
                            &share_id,
                            &format!("provider-{prefix}-{index}"),
                        ))
                        .unwrap();
                }
            })
            .await
            .unwrap();
    }

    #[derive(Clone)]
    struct SharePruneMockRouter {
        remote_share_ids: Arc<TokioMutex<BTreeSet<String>>>,
        batch_sizes: Arc<TokioMutex<Vec<usize>>>,
        request_order: Arc<TokioMutex<Vec<String>>>,
        prune_requests: Arc<AtomicUsize>,
        prune_status: Arc<TokioMutex<StatusCode>>,
    }

    async fn share_prune_mock_batch_handler(
        AxumState(router): AxumState<SharePruneMockRouter>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        let operations = request["ops"].as_array().unwrap();
        router.batch_sizes.lock().await.push(operations.len());
        router
            .request_order
            .lock()
            .await
            .push(format!("batch:{}", operations.len()));
        let mut remote_share_ids = router.remote_share_ids.lock().await;
        for operation in operations {
            match operation["kind"].as_str() {
                Some("upsert") => {
                    remote_share_ids
                        .insert(operation["share"]["shareId"].as_str().unwrap().to_string());
                }
                Some("delete") => {
                    remote_share_ids.remove(operation["shareId"].as_str().unwrap());
                }
                kind => panic!("unexpected share sync operation: {kind:?}"),
            }
        }
        Json(json!({"ok": true}))
    }

    async fn share_prune_mock_handler(
        AxumState(router): AxumState<SharePruneMockRouter>,
        Json(request): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        router.prune_requests.fetch_add(1, AtomicOrdering::SeqCst);
        let share_ids = request["shareIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>();
        let prune_status = *router.prune_status.lock().await;
        router
            .request_order
            .lock()
            .await
            .push(format!("prune:{}", share_ids.len()));
        if prune_status.is_success() {
            router
                .remote_share_ids
                .lock()
                .await
                .retain(|share_id| share_ids.contains(share_id));
        }
        (prune_status, Json(json!({"ok": true})))
    }

    async fn spawn_share_prune_mock_router(
        remote_share_ids: impl IntoIterator<Item = String>,
        prune_status: StatusCode,
    ) -> (String, SharePruneMockRouter, tokio::task::JoinHandle<()>) {
        let router = SharePruneMockRouter {
            remote_share_ids: Arc::new(TokioMutex::new(remote_share_ids.into_iter().collect())),
            batch_sizes: Arc::new(TokioMutex::new(Vec::new())),
            request_order: Arc::new(TokioMutex::new(Vec::new())),
            prune_requests: Arc::new(AtomicUsize::new(0)),
            prune_status: Arc::new(TokioMutex::new(prune_status)),
        };
        let app = Router::new()
            .route(
                "/v1/shares/batch-sync",
                post(share_prune_mock_batch_handler),
            )
            .route("/v1/shares/prune", post(share_prune_mock_handler))
            .with_state(router.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), router, server)
    }

    async fn configure_registered_test_router(
        state: &ServerState,
        router_url: &str,
        installation_id: &str,
    ) {
        let mut config = state.config_snapshot().await;
        config.router.url = Some(router_url.to_string());
        config.router.api_base = None;
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = installation_id.to_string();
        config.router.identity = Some(identity);
        config.client.tunnel_subdomain = Some("clienttest".to_string());
        state.replace_config(config).await.unwrap();
    }

    #[tokio::test]
    async fn multi_share_metadata_sync_uses_one_router_batch() {
        let (router_url, router, server) =
            spawn_share_prune_mock_router(Vec::<String>::new(), StatusCode::OK).await;
        let state = test_state();
        configure_registered_test_router(&state, &router_url, "inst-metadata-batch").await;
        create_router_shares(&state, "metadata", 2).await;
        let share_ids = vec!["metadata0".to_string(), "metadata1".to_string()];

        sync_shares_to_router(&state, &share_ids).await.unwrap();

        assert_eq!(router.batch_sizes.lock().await.as_slice(), &[2]);
        let shares = state.shares.read().await;
        for share_id in share_ids {
            let share = shares.get(&share_id).unwrap();
            assert_eq!(share.router_synced_revision, share.config_revision);
            assert!(share.router_last_sync_error.is_none());
        }
        server.abort();
    }

    #[tokio::test]
    async fn failed_multi_share_metadata_sync_keeps_all_revisions_pending() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "temporarily unavailable"})),
            )
        }

        let app = Router::new().route("/v1/shares/batch-sync", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        configure_registered_test_router(
            &state,
            &format!("http://{address}"),
            "inst-metadata-failure",
        )
        .await;
        create_router_shares(&state, "pendingmeta", 2).await;
        let share_ids = vec!["pendingmeta0".to_string(), "pendingmeta1".to_string()];

        assert!(sync_shares_to_router(&state, &share_ids).await.is_err());

        let shares = state.shares.read().await;
        for share_id in share_ids {
            let share = shares.get(&share_id).unwrap();
            assert!(share.router_synced_revision < share.config_revision);
            assert!(share.router_last_sync_error.is_some());
        }
        server.abort();
    }

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
            extra_headers: Default::default(),
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
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
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
    async fn health_usage_log_if_due_deduplicates_matching_binding_and_model() {
        let state = test_state();
        let make_log = |request_id: &str, model: &str, created_at_ms: u128| {
            let mut log = UsageLog::new(
                AppKind::Codex,
                "provider-1".to_string(),
                "Provider 1".to_string(),
                ProviderType::Codex,
                429,
                0,
                UsageModelMetadata {
                    model: Some(model.to_string()),
                    requested_model: Some(model.to_string()),
                    ..UsageModelMetadata::default()
                },
                TokenUsage::default(),
            );
            log.apply_context(UsageLogContext {
                request_id: Some(request_id.to_string()),
                share_id: Some("share-1".to_string()),
                data_source: Some("cc-switch-quota".to_string()),
                is_health_check: true,
                ..UsageLogContext::default()
            });
            log.created_at_ms = created_at_ms;
            log
        };

        let first = state
            .push_health_usage_log_if_due(make_log("health-1", "gpt-5.5", 1_000), 10_000)
            .await
            .unwrap();
        let duplicate = state
            .push_health_usage_log_if_due(make_log("health-2", "gpt-5.5", 2_000), 10_000)
            .await
            .unwrap();
        let changed_model = state
            .push_health_usage_log_if_due(make_log("health-3", "gpt-5.6", 3_000), 10_000)
            .await
            .unwrap();

        assert_eq!(first.request_id, "health-1");
        assert_eq!(duplicate.request_id, "health-1");
        assert_eq!(changed_model.request_id, "health-3");
        assert_eq!(state.usage_snapshot().await.logs.len(), 2);
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
                free_access: None,
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
                user_grants: BTreeMap::new(),
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

    #[test]
    fn router_request_id_accepts_prefixed_and_canonical_uuid_forms() {
        assert!(is_router_request_id("req_router_123"));
        assert!(is_router_request_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_router_request_id("550E8400-E29B-41D4-A716-446655440000"));

        assert!(!is_router_request_id("router-request-1"));
        assert!(!is_router_request_id("550e8400e29b41d4a716446655440000"));
        assert!(!is_router_request_id(
            "550e8400-e29b-41d4-a716-44665544000z"
        ));
        assert!(!is_router_request_id("req_bad/value"));
    }

    #[test]
    fn router_heartbeat_interval_defaults_and_clamps_operator_input() {
        assert_eq!(normalize_router_heartbeat_interval(None), 60);
        assert_eq!(normalize_router_heartbeat_interval(Some("invalid")), 60);
        assert_eq!(normalize_router_heartbeat_interval(Some(" 60 ")), 60);
        assert_eq!(normalize_router_heartbeat_interval(Some("90")), 60);
        assert_eq!(normalize_router_heartbeat_interval(Some("1")), 15);
        assert_eq!(normalize_router_heartbeat_interval(Some("99999")), 60);
    }

    #[test]
    fn router_registration_retry_uses_bounded_exponential_backoff() {
        assert_eq!(next_router_registration_retry_secs(5), 10);
        assert_eq!(next_router_registration_retry_secs(10), 20);
        assert_eq!(next_router_registration_retry_secs(200), 300);
        assert_eq!(next_router_registration_retry_secs(300), 300);
    }

    #[test]
    fn router_heartbeat_jitter_stays_within_ten_percent() {
        for _ in 0..256 {
            let delay = next_router_heartbeat_delay(60).as_secs();
            assert!((54..=66).contains(&delay));
        }
    }

    #[test]
    fn heartbeat_warnings_are_rate_limited_per_failure_class() {
        let now = tokio::time::Instant::now();
        let mut last_warning = None;

        assert!(rate_limited_warning_due(
            &mut last_warning,
            now,
            Duration::from_secs(60)
        ));
        assert!(!rate_limited_warning_due(
            &mut last_warning,
            now + Duration::from_secs(59),
            Duration::from_secs(60)
        ));
        assert!(rate_limited_warning_due(
            &mut last_warning,
            now + Duration::from_secs(60),
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn heartbeat_state_error_is_bounded_before_persistence() {
        let bounded = bounded_router_heartbeat_state_error(
            "x".repeat(ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS + 17),
        );
        assert_eq!(
            bounded.chars().count(),
            ROUTER_HEARTBEAT_STATE_ERROR_MAX_CHARS
        );
    }

    #[tokio::test]
    async fn periodic_heartbeat_updates_local_health_on_success_and_failure() {
        async fn handler(
            AxumState(requests): AxumState<Arc<AtomicUsize>>,
        ) -> (StatusCode, Json<Value>) {
            if requests.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                (StatusCode::NO_CONTENT, Json(json!({})))
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "temporarily unavailable"})),
                )
            }
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/installations/heartbeat", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        configure_registered_test_router(&state, &format!("http://{addr}"), "inst-health").await;
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;
        let before = crate::infra::time::now_ms();

        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );
        let successful = state.shares.read().await.clone();
        let last_success = successful.last_router_heartbeat_ms.unwrap();
        assert!(last_success >= before);
        assert!(successful.router_registered);
        assert!(successful.last_router_error.is_none());

        for expected_failures in 1..=ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
            assert!(
                run_installation_heartbeat_once(
                    &state,
                    &mut consecutive_failures,
                    &mut last_failure_warning,
                    &mut last_endpoint_warning,
                )
                .await
            );
            let failed = state.shares.read().await.clone();
            assert_eq!(failed.last_router_heartbeat_ms, Some(last_success));
            assert_eq!(
                failed.router_registered,
                expected_failures < ROUTER_HEARTBEAT_SUSTAINED_FAILURES
            );
            if expected_failures < ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
                assert!(failed.last_router_error.is_none());
            } else {
                assert!(failed
                    .last_router_error
                    .as_deref()
                    .is_some_and(|error| error.contains("temporarily unavailable")));
            }
            assert_eq!(consecutive_failures, expected_failures);
        }
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 4);
        server.abort();
    }

    #[tokio::test]
    async fn unsupported_heartbeat_endpoint_keeps_registered_router_healthy() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"message": "route not found"})),
            )
        }

        let app = Router::new().route("/v1/installations/heartbeat", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-legacy".to_string();
        identity.control_secret = Some("secret".to_string());
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();
        state
            .mutate_shares(|shares| {
                shares.router_registered = false;
                shares.last_router_error = Some("stale".to_string());
                shares.last_router_heartbeat_ms = Some(123);
            })
            .await;
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );

        let shares = state.shares.read().await;
        assert!(shares.router_registered);
        assert!(shares.last_router_error.is_none());
        assert_eq!(shares.last_router_heartbeat_ms, Some(123));
        assert_eq!(consecutive_failures, 0);
        server.abort();
    }

    #[tokio::test]
    async fn registration_recovery_retries_heartbeat_before_marking_success() {
        #[derive(Clone)]
        struct Counts {
            heartbeats: Arc<AtomicUsize>,
            registrations: Arc<AtomicUsize>,
        }

        async fn heartbeat_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            if counts.heartbeats.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"message": "identity expired"})),
                )
            } else {
                (StatusCode::NO_CONTENT, Json(json!({})))
            }
        }

        async fn registration_handler(AxumState(counts): AxumState<Counts>) -> Json<Value> {
            counts.registrations.fetch_add(1, AtomicOrdering::SeqCst);
            Json(json!({
                "installationId": "inst-heartbeat-recovered",
                "controlSecret": "recovered-secret"
            }))
        }

        let counts = Counts {
            heartbeats: Arc::new(AtomicUsize::new(0)),
            registrations: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/installations/heartbeat", post(heartbeat_handler))
            .route("/v1/installations/register", post(registration_handler))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        configure_registered_test_router(
            &state,
            &format!("http://{addr}"),
            "inst-heartbeat-expired",
        )
        .await;
        let mut config = state.config_snapshot().await;
        config.router.identity.as_mut().unwrap().control_secret = Some("old-secret".to_string());
        state.replace_config(config).await.unwrap();
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );

        assert_eq!(counts.heartbeats.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(counts.registrations.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(consecutive_failures, 0);
        let shares = state.shares.read().await;
        assert!(shares.router_registered);
        assert!(shares.last_router_error.is_none());
        assert!(shares.last_router_heartbeat_ms.is_some());
        server.abort();
    }

    #[tokio::test]
    async fn successful_registration_recovery_does_not_hide_repeated_heartbeat_401s() {
        #[derive(Clone)]
        struct Counts {
            heartbeats: Arc<AtomicUsize>,
            registrations: Arc<AtomicUsize>,
            share_syncs: Arc<TokioMutex<Vec<Value>>>,
        }

        async fn heartbeat_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            counts.heartbeats.fetch_add(1, AtomicOrdering::SeqCst);
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": "invalid heartbeat identity"})),
            )
        }

        async fn registration_handler(AxumState(counts): AxumState<Counts>) -> Json<Value> {
            counts.registrations.fetch_add(1, AtomicOrdering::SeqCst);
            Json(json!({
                "installationId": "inst-heartbeat-remapped",
                "controlSecret": "recovered-secret"
            }))
        }

        async fn share_sync_handler(
            AxumState(counts): AxumState<Counts>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            counts.share_syncs.lock().await.push(request);
            Json(json!({"ok": true}))
        }

        let counts = Counts {
            heartbeats: Arc::new(AtomicUsize::new(0)),
            registrations: Arc::new(AtomicUsize::new(0)),
            share_syncs: Arc::new(TokioMutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/v1/installations/heartbeat", post(heartbeat_handler))
            .route("/v1/installations/register", post(registration_handler))
            .route("/v1/shares/batch-sync", post(share_sync_handler))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-heartbeat-401".to_string();
        identity.control_secret = Some("existing-secret".to_string());
        config.router.identity = Some(identity);
        config.client.tunnel_subdomain = Some("clienttest".to_string());
        state.replace_config(config).await.unwrap();
        state
            .shares
            .write()
            .await
            .upsert(UpsertShareInput {
                id: Some("share-heartbeat-remap".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Codex,
                provider_id: "provider-heartbeat-remap".to_string(),
                provider_type: ProviderType::Codex,
                display_name: Some("Heartbeat remap share".to_string()),
                tunnel_subdomain: Some("heartbeatremap".to_string()),
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        for _ in 0..ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
            assert!(
                run_installation_heartbeat_once(
                    &state,
                    &mut consecutive_failures,
                    &mut last_failure_warning,
                    &mut last_endpoint_warning,
                )
                .await
            );
        }

        assert_eq!(consecutive_failures, ROUTER_HEARTBEAT_SUSTAINED_FAILURES);
        assert!(last_failure_warning.is_some());
        assert_eq!(
            counts.heartbeats.load(AtomicOrdering::SeqCst),
            (ROUTER_HEARTBEAT_SUSTAINED_FAILURES * 2) as usize
        );
        assert_eq!(
            counts.registrations.load(AtomicOrdering::SeqCst),
            ROUTER_HEARTBEAT_SUSTAINED_FAILURES as usize
        );
        assert_eq!(
            state
                .config_snapshot()
                .await
                .registered_router_identity()
                .unwrap()
                .installation_id,
            "inst-heartbeat-remapped"
        );
        let share_syncs = counts.share_syncs.lock().await;
        assert_eq!(
            share_syncs.len(),
            ROUTER_HEARTBEAT_SUSTAINED_FAILURES as usize
        );
        assert!(share_syncs.iter().all(|request| {
            let operations = request["ops"].as_array().unwrap();
            operations.len() == 1 && operations[0]["kind"] == "upsert"
        }));
        drop(share_syncs);
        let shares = state.shares.read().await;
        assert!(!shares.router_registered);
        assert!(shares
            .last_router_error
            .as_deref()
            .is_some_and(|error| error.contains("requires registration")));
        server.abort();
    }

    #[tokio::test]
    async fn heartbeat_retries_registration_after_startup_registration_failure() {
        async fn registration_handler(
            AxumState(requests): AxumState<Arc<AtomicUsize>>,
        ) -> Json<Value> {
            requests.fetch_add(1, AtomicOrdering::SeqCst);
            Json(json!({
                "installationId": "inst-startup-recovered",
                "controlSecret": "startup-recovered-secret"
            }))
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/installations/register", post(registration_handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.auth.password_hash = Some("configured".to_string());
        config.owner.email = Some("owner@example.com".to_string());
        config.router.url = Some(format!("http://{addr}"));
        config.client.tunnel_subdomain = Some("startup-recovery".to_string());
        config.router.identity = Some(client::generate_identity_without_installation());
        state.replace_config(config).await.unwrap();
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );

        assert_eq!(requests.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            state
                .config_snapshot()
                .await
                .registered_router_identity()
                .unwrap()
                .installation_id,
            "inst-startup-recovered"
        );
        server.abort();
    }

    #[tokio::test]
    async fn successful_registration_recovery_resets_prior_failure_threshold() {
        #[derive(Clone)]
        struct Counts {
            registrations: Arc<AtomicUsize>,
            heartbeats: Arc<AtomicUsize>,
        }

        async fn registration_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            if counts.registrations.fetch_add(1, AtomicOrdering::SeqCst) < 2 {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "router unavailable"})),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(json!({
                        "installationId": "inst-threshold-recovered",
                        "controlSecret": "threshold-recovered-secret"
                    })),
                )
            }
        }

        async fn heartbeat_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            counts.heartbeats.fetch_add(1, AtomicOrdering::SeqCst);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "heartbeat unavailable"})),
            )
        }

        let counts = Counts {
            registrations: Arc::new(AtomicUsize::new(0)),
            heartbeats: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/installations/register", post(registration_handler))
            .route("/v1/installations/heartbeat", post(heartbeat_handler))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.auth.password_hash = Some("configured".to_string());
        config.owner.email = Some("owner@example.com".to_string());
        config.router.url = Some(format!("http://{addr}"));
        config.client.tunnel_subdomain = Some("threshold-recovery".to_string());
        config.router.identity = Some(client::generate_identity_without_installation());
        state.replace_config(config).await.unwrap();
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        for expected_failures in 1..=2 {
            assert!(
                !run_installation_heartbeat_once(
                    &state,
                    &mut consecutive_failures,
                    &mut last_failure_warning,
                    &mut last_endpoint_warning,
                )
                .await
            );
            assert_eq!(consecutive_failures, expected_failures);
            loop {
                if state.router_registration_flight.lock().await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }

        last_failure_warning = Some(tokio::time::Instant::now());
        last_endpoint_warning = Some(tokio::time::Instant::now());
        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );
        assert_eq!(consecutive_failures, 0);
        assert!(last_failure_warning.is_none());
        assert!(last_endpoint_warning.is_none());
        {
            let shares = state.shares.read().await;
            assert!(shares.router_registered);
            assert!(shares.last_router_error.is_none());
            assert!(shares.last_router_heartbeat_ms.is_none());
        }

        assert!(
            run_installation_heartbeat_once(
                &state,
                &mut consecutive_failures,
                &mut last_failure_warning,
                &mut last_endpoint_warning,
            )
            .await
        );
        assert_eq!(consecutive_failures, 1);
        let shares = state.shares.read().await;
        assert!(shares.router_registered);
        assert!(shares.last_router_error.is_none());
        assert!(shares.last_router_heartbeat_ms.is_none());
        assert_eq!(counts.registrations.load(AtomicOrdering::SeqCst), 3);
        assert_eq!(counts.heartbeats.load(AtomicOrdering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn missing_control_secret_recovery_failures_accumulate_across_endpoint_404s() {
        #[derive(Clone)]
        struct Counts {
            heartbeats: Arc<AtomicUsize>,
            registrations: Arc<AtomicUsize>,
        }

        async fn heartbeat_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            counts.heartbeats.fetch_add(1, AtomicOrdering::SeqCst);
            (
                StatusCode::NOT_FOUND,
                Json(json!({"message": "route /v1/installations/heartbeat not found"})),
            )
        }

        async fn registration_handler(
            AxumState(counts): AxumState<Counts>,
        ) -> (StatusCode, Json<Value>) {
            counts.registrations.fetch_add(1, AtomicOrdering::SeqCst);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "router unavailable"})),
            )
        }

        let counts = Counts {
            heartbeats: Arc::new(AtomicUsize::new(0)),
            registrations: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/installations/heartbeat", post(heartbeat_handler))
            .route("/v1/installations/register", post(registration_handler))
            .with_state(counts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-endpoint-404".to_string();
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();
        let mut consecutive_failures = 0;
        let mut last_failure_warning = None;
        let mut last_endpoint_warning = None;

        for _ in 0..ROUTER_HEARTBEAT_SUSTAINED_FAILURES {
            assert!(
                run_installation_heartbeat_once(
                    &state,
                    &mut consecutive_failures,
                    &mut last_failure_warning,
                    &mut last_endpoint_warning,
                )
                .await
            );
        }

        assert_eq!(consecutive_failures, ROUTER_HEARTBEAT_SUSTAINED_FAILURES);
        assert!(last_failure_warning.is_some());
        assert_eq!(
            counts.heartbeats.load(AtomicOrdering::SeqCst),
            ROUTER_HEARTBEAT_SUSTAINED_FAILURES as usize
        );
        assert_eq!(
            counts.registrations.load(AtomicOrdering::SeqCst),
            ROUTER_HEARTBEAT_SUSTAINED_FAILURES as usize
        );
        server.abort();
    }

    #[test]
    fn stale_config_snapshot_cannot_regress_router_identity_fields() {
        let mut current = ServerConfig::empty();
        current.router.url = Some("https://router.example.com".to_string());
        current.router.identity = Some(RouterIdentity {
            installation_id: "inst-new".to_string(),
            public_key: "same-public-key".to_string(),
            private_key: "current-private-key".to_string(),
            control_secret: Some("current-control-secret".to_string()),
        });
        current.router.last_registered_at_ms = Some(200);
        let mut stale = current.clone();
        stale.owner.email = Some("new-owner@example.com".to_string());
        stale.router.identity = Some(RouterIdentity {
            installation_id: "inst-old".to_string(),
            public_key: "same-public-key".to_string(),
            private_key: "current-private-key".to_string(),
            control_secret: Some("old-control-secret".to_string()),
        });
        stale.router.last_registered_at_ms = Some(100);

        preserve_router_identity_from_stale_snapshot(&current, &mut stale);

        assert_eq!(stale.owner.email.as_deref(), Some("new-owner@example.com"));
        assert_eq!(stale.router.identity, current.router.identity);
        assert_eq!(stale.router.last_registered_at_ms, Some(200));
    }

    #[test]
    fn stale_config_snapshot_cannot_regress_setup_completion_state() {
        use crate::domain::settings::config::SetupCompletionNotificationState;

        let mut current = ServerConfig::empty();
        let mut acknowledged = SetupCompletionNotificationState::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "p******w".to_string(),
            100,
        );
        acknowledged.status = SetupCompletionNotificationStatus::Acknowledged;
        acknowledged.attempt_count = 3;
        acknowledged.updated_at_ms = 300;
        acknowledged.last_attempt_at_ms = Some(250);
        acknowledged.acknowledged_at_ms = Some(300);
        acknowledged.router_ack_status = Some("suppressed_disabled".to_string());
        acknowledged.password_hint = None;
        current.setup_completion_notification = Some(acknowledged.clone());

        let mut stale = current.clone();
        let mut pending = SetupCompletionNotificationState::new(
            acknowledged.setup_id.clone(),
            "p******w".to_string(),
            100,
        );
        pending.status = SetupCompletionNotificationStatus::Pending;
        pending.attempt_count = 1;
        pending.updated_at_ms = 150;
        pending.last_attempt_at_ms = Some(150);
        pending.next_attempt_at_ms = Some(180);
        stale.setup_completion_notification = Some(pending);

        preserve_setup_completion_from_stale_snapshot(&current, &mut stale);

        let merged = stale.setup_completion_notification.unwrap();
        assert_eq!(
            merged.status,
            SetupCompletionNotificationStatus::Acknowledged
        );
        assert_eq!(merged.attempt_count, 3);
        assert_eq!(merged.last_attempt_at_ms, Some(250));
        assert_eq!(merged.acknowledged_at_ms, Some(300));
        assert_eq!(
            merged.router_ack_status.as_deref(),
            Some("suppressed_disabled")
        );
        assert!(merged.password_hint.is_none());
        assert!(merged.next_attempt_at_ms.is_none());
        assert!(merged.last_error.is_none());
    }

    #[test]
    fn older_different_setup_id_cannot_replace_newer_notification() {
        use crate::domain::settings::config::SetupCompletionNotificationState;

        let mut current = ServerConfig::empty();
        current.setup_completion_notification = Some(SetupCompletionNotificationState::new(
            "123e4567-e89b-42d3-a456-426614174000".to_string(),
            "n******w".to_string(),
            200,
        ));
        let mut stale = current.clone();
        stale.setup_completion_notification = Some(SetupCompletionNotificationState::new(
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            "o******d".to_string(),
            100,
        ));

        preserve_setup_completion_from_stale_snapshot(&current, &mut stale);

        assert_eq!(
            stale
                .setup_completion_notification
                .as_ref()
                .unwrap()
                .setup_id,
            "123e4567-e89b-42d3-a456-426614174000"
        );
    }

    #[test]
    fn setup_completion_retry_backoff_is_bounded_and_jitter_is_stable() {
        let setup_id = "123e4567-e89b-42d3-a456-426614174000";
        let first = setup_completion_retry_delay_ms(setup_id, 1);
        assert_eq!(first, setup_completion_retry_delay_ms(setup_id, 1));
        assert!(first >= SETUP_COMPLETION_RETRY_BASE_MS);
        assert!(first <= SETUP_COMPLETION_RETRY_BASE_MS * 6 / 5);
        let capped = setup_completion_retry_delay_ms(setup_id, u32::MAX);
        assert!(capped <= SETUP_COMPLETION_RETRY_MAX_MS);
        assert!(capped >= SETUP_COMPLETION_RETRY_MAX_MS * 5 / 6);
    }

    #[tokio::test]
    async fn setup_completion_transient_failure_stays_pending_without_exposing_password() {
        async fn unavailable(
            Json(request): Json<Value>,
        ) -> (
            StatusCode,
            [(axum::http::HeaderName, &'static str); 1],
            Json<Value>,
        ) {
            assert_eq!(request["setup"]["passwordHint"], "s******9");
            assert!(request["setup"].get("passwordLength").is_none());
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::RETRY_AFTER, "3600")],
                Json(json!({"message": "temporarily unavailable"})),
            )
        }

        let app = Router::new().route("/v1/installations/setup-completed", post(unavailable));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation".to_string();
        config.router.identity = Some(identity);
        config.set_password("supersecret9").unwrap();
        config.setup_completion_notification = Some(
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "123e4567-e89b-42d3-a456-426614174000".to_string(),
                "s******9".to_string(),
                100,
            ),
        );
        state.replace_config(config).await.unwrap();

        state
            .deliver_setup_completion_notification(true)
            .await
            .unwrap();

        let config = state.config_snapshot().await;
        let notification = config.setup_completion_notification.unwrap();
        assert_eq!(
            notification.status,
            SetupCompletionNotificationStatus::Pending
        );
        assert_eq!(notification.attempt_count, 1);
        assert!(notification
            .next_attempt_at_ms
            .zip(notification.last_attempt_at_ms)
            .is_some_and(|(next, last)| next.saturating_sub(last) >= 60 * 60 * 1_000));
        assert!(notification
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("429")));
        let persisted = std::fs::read_to_string(crate::domain::settings::config::config_path(
            &state.config_dir,
        ))
        .unwrap();
        assert!(!persisted.contains("supersecret9"));
        assert!(persisted.contains("s******9"));
        server.abort();
    }

    #[tokio::test]
    async fn setup_completion_terminal_rejection_clears_persisted_hint() {
        async fn rejected(Json(request): Json<Value>) -> (StatusCode, Json<Value>) {
            assert_eq!(request["setup"]["passwordHint"], "p******w");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"message": "invalid setup completion"})),
            )
        }

        let app = Router::new().route("/v1/installations/setup-completed", post(rejected));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation".to_string();
        config.router.identity = Some(identity);
        config.setup_completion_notification = Some(
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "123e4567-e89b-42d3-a456-426614174000".to_string(),
                "p******w".to_string(),
                100,
            ),
        );
        state.replace_config(config).await.unwrap();

        state
            .deliver_setup_completion_notification(true)
            .await
            .unwrap();

        let notification = state
            .config_snapshot()
            .await
            .setup_completion_notification
            .unwrap();
        assert_eq!(
            notification.status,
            SetupCompletionNotificationStatus::TerminalFailed
        );
        assert!(notification.password_hint.is_none());
        assert!(notification.next_attempt_at_ms.is_none());
        assert!(notification
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("422")));
        let persisted = std::fs::read_to_string(crate::domain::settings::config::config_path(
            &state.config_dir,
        ))
        .unwrap();
        assert!(!persisted.contains("p******w"));
        server.abort();
    }

    #[tokio::test]
    async fn setup_completion_acknowledgement_clears_persisted_hint() {
        async fn suppressed(Json(request): Json<Value>) -> Json<Value> {
            Json(json!({
                "ok": true,
                "status": "suppressed_disabled",
                "setupId": request["setup"]["setupId"]
            }))
        }

        let app = Router::new().route("/v1/installations/setup-completed", post(suppressed));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation".to_string();
        config.router.identity = Some(identity);
        config.setup_completion_notification = Some(
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "123e4567-e89b-42d3-a456-426614174000".to_string(),
                "p******w".to_string(),
                100,
            ),
        );
        state.replace_config(config).await.unwrap();

        state
            .deliver_setup_completion_notification(true)
            .await
            .unwrap();

        let config = state.config_snapshot().await;
        let notification = config.setup_completion_notification.unwrap();
        assert_eq!(
            notification.status,
            SetupCompletionNotificationStatus::Acknowledged
        );
        assert!(notification.password_hint.is_none());
        assert!(notification.acknowledged_at_ms.is_some());
        assert_eq!(
            notification.router_ack_status.as_deref(),
            Some("suppressed_disabled")
        );
        assert!(notification.next_attempt_at_ms.is_none());
        let persisted = std::fs::read_to_string(crate::domain::settings::config::config_path(
            &state.config_dir,
        ))
        .unwrap();
        assert!(!persisted.contains("p******w"));
        server.abort();
    }

    #[tokio::test]
    async fn restart_recovers_waiting_notification_from_persisted_authoritative_claim() {
        async fn already_suppressed(
            AxumState(requests): AxumState<Arc<AtomicUsize>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            requests.fetch_add(1, AtomicOrdering::SeqCst);
            Json(json!({
                "ok": true,
                "status": "suppressed_disabled",
                "setupId": request["setup"]["setupId"]
            }))
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/installations/setup-completed",
                post(already_suppressed),
            )
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation".to_string();
        config.router.identity = Some(identity);
        config.client.tunnel_status = Some("claimed_remote".to_string());
        config.setup_completion_notification = Some(
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "123e4567-e89b-42d3-a456-426614174000".to_string(),
                "p******w".to_string(),
                100,
            ),
        );
        state.replace_config(config).await.unwrap();
        let config_dir = state.config_dir.clone();
        drop(state);

        let restarted = test_state_at(config_dir);
        restarted
            .retry_pending_setup_completion_notification()
            .await;

        let notification = restarted
            .config_snapshot()
            .await
            .setup_completion_notification
            .unwrap();
        assert_eq!(
            notification.status,
            SetupCompletionNotificationStatus::Acknowledged
        );
        assert!(notification.password_hint.is_none());
        assert_eq!(
            notification.router_ack_status.as_deref(),
            Some("suppressed_disabled")
        );
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 1);

        let skipped_state = test_state();
        let mut skipped = skipped_state.config_snapshot().await;
        skipped.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation-skipped".to_string();
        skipped.router.identity = Some(identity);
        skipped.client.tunnel_status = Some("claim_skipped".to_string());
        skipped.setup_completion_notification = Some(
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
                "p******w".to_string(),
                100,
            ),
        );
        skipped_state.replace_config(skipped).await.unwrap();
        let config_dir = skipped_state.config_dir.clone();
        drop(skipped_state);
        let restarted_skipped = test_state_at(config_dir);

        restarted_skipped
            .retry_pending_setup_completion_notification()
            .await;

        assert_eq!(
            restarted_skipped
                .config_snapshot()
                .await
                .setup_completion_notification
                .unwrap()
                .status,
            SetupCompletionNotificationStatus::WaitingForClaim
        );
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 1);

        let lost_response_state = test_state();
        let mut lost_response = lost_response_state.config_snapshot().await;
        lost_response.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "fixture-installation-lost-response".to_string();
        lost_response.router.identity = Some(identity);
        lost_response.client.tunnel_status = Some("claimed_remote".to_string());
        let mut notification =
            crate::domain::settings::config::SetupCompletionNotificationState::new(
                "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
                "p******w".to_string(),
                100,
            );
        notification.status = SetupCompletionNotificationStatus::Pending;
        notification.attempt_count = 1;
        notification.last_attempt_at_ms = Some(100);
        notification.next_attempt_at_ms = Some(100);
        lost_response.setup_completion_notification = Some(notification);
        lost_response_state
            .replace_config(lost_response)
            .await
            .unwrap();
        let config_dir = lost_response_state.config_dir.clone();
        drop(lost_response_state);
        let restarted_lost_response = test_state_at(config_dir);

        restarted_lost_response
            .retry_pending_setup_completion_notification()
            .await;

        let notification = restarted_lost_response
            .config_snapshot()
            .await
            .setup_completion_notification
            .unwrap();
        assert_eq!(
            notification.status,
            SetupCompletionNotificationStatus::Acknowledged
        );
        assert_eq!(
            notification.router_ack_status.as_deref(),
            Some("suppressed_disabled")
        );
        assert!(notification.password_hint.is_none());
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn pending_router_identity_survives_lost_response_and_restart() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dropped_response = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await;
        });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        state
            .register_router_installation()
            .await
            .expect_err("a dropped registration response must fail");
        dropped_response.await.unwrap();
        let pending = state
            .config_snapshot()
            .await
            .router
            .identity
            .expect("pending identity must be persisted before the request");
        assert!(pending.installation_id.is_empty());
        let config_dir = state.config_dir.clone();
        drop(state);

        let restarted = test_state_at(config_dir);
        let reloaded = restarted
            .config_snapshot()
            .await
            .router
            .identity
            .expect("restart must reload the pending identity");
        assert_eq!(reloaded.public_key, pending.public_key);
        assert_eq!(reloaded.private_key, pending.private_key);

        let requests = Arc::new(TokioMutex::new(Vec::<Value>::new()));
        let app = Router::new().route(
            "/v1/installations/register",
            post({
                let requests = requests.clone();
                move |Json(request): Json<Value>| {
                    let requests = requests.clone();
                    async move {
                        requests.lock().await.push(request);
                        Json(json!({
                            "installationId": "inst-after-restart",
                            "controlSecret": "secret-after-restart"
                        }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        set_test_router_url(&restarted, format!("http://{addr}")).await;

        restarted.register_router_installation().await.unwrap();

        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["publicKey"], pending.public_key);
        server.abort();
    }

    #[tokio::test]
    async fn unverified_owner_email_match_blocks_owner_gate() {
        async fn owner_status() -> Json<Value> {
            Json(json!({
                "ok": true,
                "ownerEmail": "owner@example.com",
                "ownerVerified": false
            }))
        }

        let app = Router::new().route("/v1/installations/owner-email", get(owner_status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        configure_registered_test_router(
            &state,
            &format!("http://{addr}"),
            "inst-unverified-owner",
        )
        .await;
        let mut config = state.config_snapshot().await;
        config.owner.email = Some("owner@example.com".to_string());
        state.replace_config(config.clone()).await.unwrap();

        let error = ensure_router_installation_owner_bound(&state, &config)
            .await
            .expect_err("unverified owner must block owner gate");
        assert!(
            error
                .to_string()
                .contains("router installation owner email is not bound"),
            "unexpected error: {error}"
        );

        server.abort();
    }

    #[tokio::test]
    async fn legacy_router_owner_response_remains_verified_compatible() {
        async fn owner_status() -> Json<Value> {
            Json(json!({
                "ok": true,
                "ownerEmail": "owner@example.com"
            }))
        }

        let app = Router::new().route("/v1/installations/owner-email", get(owner_status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        configure_registered_test_router(&state, &format!("http://{addr}"), "inst-legacy-owner")
            .await;
        let mut config = state.config_snapshot().await;
        config.owner.email = Some("owner@example.com".to_string());
        state.replace_config(config.clone()).await.unwrap();

        ensure_router_installation_owner_bound(&state, &config)
            .await
            .unwrap();

        server.abort();
    }

    #[tokio::test]
    async fn concurrent_router_registration_is_singleflight_with_one_identity() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            requests.lock().await.push(request);
            tokio::time::sleep(Duration::from_millis(30)).await;
            Json(json!({
                "installationId": "inst-singleflight",
                "controlSecret": "singleflight-secret"
            }))
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let (first, second) = tokio::join!(
            state.register_router_installation(),
            state.register_router_installation()
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(first.installation_id, "inst-singleflight");
        assert_eq!(second.installation_id, first.installation_id);
        assert_eq!(second.public_key, first.public_key);
        assert_eq!(requests.lock().await.len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn concurrent_failed_router_registration_shares_one_attempt() {
        async fn handler(
            AxumState(requests): AxumState<Arc<AtomicUsize>>,
        ) -> (StatusCode, Json<Value>) {
            requests.fetch_add(1, AtomicOrdering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": "router unavailable"})),
            )
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let (first, second) = tokio::join!(
            state.register_router_installation(),
            state.register_router_installation()
        );

        let first = first.expect_err("first registration must fail").to_string();
        let second = second
            .expect_err("concurrent registration must share failure")
            .to_string();
        assert_eq!(first, second);
        assert_eq!(requests.load(AtomicOrdering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn registration_failure_is_persisted_before_waiters_are_notified() {
        #[derive(Clone)]
        struct Gate {
            request_started: Arc<tokio::sync::Notify>,
            release_response: Arc<tokio::sync::Notify>,
        }

        async fn handler(AxumState(gate): AxumState<Gate>) -> (StatusCode, Json<Value>) {
            gate.request_started.notify_one();
            gate.release_response.notified().await;
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "router unavailable"})),
            )
        }

        let gate = Gate {
            request_started: Arc::new(tokio::sync::Notify::new()),
            release_response: Arc::new(tokio::sync::Notify::new()),
        };
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;
        let registration_state = state.clone();
        let mut registration =
            tokio::spawn(async move { registration_state.register_router_installation().await });

        gate.request_started.notified().await;
        let config_guard = state.config.write().await;
        gate.release_response.notify_one();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut registration)
                .await
                .is_err(),
            "registration waiter completed before its error could be persisted"
        );
        drop(config_guard);

        let error = registration.await.unwrap().unwrap_err().to_string();
        assert_eq!(
            state
                .config_snapshot()
                .await
                .router
                .last_register_error
                .as_deref(),
            Some(error.as_str())
        );
        assert_eq!(
            ServerConfig::load_or_default(&state.config_dir)
                .unwrap()
                .router
                .last_register_error
                .as_deref(),
            Some(error.as_str())
        );
        server.abort();
    }

    #[tokio::test]
    async fn registration_singleflight_preserves_unreachable_classification() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let error = state
            .register_router_installation()
            .await
            .expect_err("registration against a closed listener must fail");

        assert!(
            crate::client_tunnel_provision::is_router_unreachable_error(&error),
            "error lost its unreachable classification: {error:#}"
        );
    }

    #[tokio::test]
    async fn registration_singleflight_classifies_503_as_transient() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"message": "retry later"})),
            )
        }

        let app = Router::new().route("/v1/installations/register", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let error = state.register_router_installation().await.unwrap_err();

        assert!(crate::client_tunnel_provision::is_router_unreachable_error(
            &error
        ));
        server.abort();
    }

    #[tokio::test]
    async fn registration_4xx_body_wording_cannot_enable_offline_fallback() {
        async fn handler() -> (StatusCode, Json<Value>) {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": "connection policy is invalid"})),
            )
        }

        let app = Router::new().route("/v1/installations/register", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let error = state.register_router_installation().await.unwrap_err();

        assert!(!crate::client_tunnel_provision::is_router_unreachable_error(&error));
        server.abort();
    }

    #[tokio::test]
    async fn registration_response_merge_preserves_concurrent_config_updates() {
        #[derive(Clone)]
        struct Gate {
            request_received: Arc<tokio::sync::Notify>,
            send_response: Arc<tokio::sync::Notify>,
        }

        async fn handler(AxumState(gate): AxumState<Gate>) -> Json<Value> {
            gate.request_received.notify_one();
            gate.send_response.notified().await;
            Json(json!({
                "installationId": "inst-merge",
                "controlSecret": "merge-secret"
            }))
        }

        let gate = Gate {
            request_received: Arc::new(tokio::sync::Notify::new()),
            send_response: Arc::new(tokio::sync::Notify::new()),
        };
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;
        let mut stale_snapshot = state.config_snapshot().await;
        let registration_state = state.clone();
        let registration =
            tokio::spawn(async move { registration_state.register_router_installation().await });

        gate.request_received.notified().await;
        let pending = state.config_snapshot().await.router.identity.unwrap();
        assert!(pending.has_keypair());
        assert!(pending.installation_id.is_empty());
        stale_snapshot.owner.email = Some("concurrent@example.com".to_string());
        state.replace_config(stale_snapshot).await.unwrap();
        gate.send_response.notify_one();
        registration.await.unwrap().unwrap();

        let config = state.config_snapshot().await;
        assert_eq!(
            config.owner.email.as_deref(),
            Some("concurrent@example.com")
        );
        assert_eq!(
            config.registered_router_identity().unwrap().installation_id,
            "inst-merge"
        );
        server.abort();
    }

    #[tokio::test]
    async fn old_router_rejection_does_not_trigger_legacy_registration() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut requests = requests.lock().await;
            requests.push(request);
            match requests.len() {
                1 => (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"message": "old protocol only"})),
                ),
                2 => (
                    StatusCode::OK,
                    Json(json!({"installationId": "inst-discovered"})),
                ),
                _ => (
                    StatusCode::OK,
                    Json(json!({
                        "installationId": "inst-discovered",
                        "controlSecret": "legacy-control-secret"
                    })),
                ),
            }
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;
        let mut stale_config = state.config_snapshot().await;
        let mut stale_identity = client::generate_identity_without_installation();
        stale_identity.installation_id = "inst-from-another-router".to_string();
        stale_identity.control_secret = Some("stale-control-secret".to_string());
        stale_config.router.identity = Some(stale_identity);
        state.replace_config(stale_config).await.unwrap();

        let error = state.register_router_installation().await.unwrap_err();

        assert!(error.to_string().contains("401 Unauthorized"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["proofVersion"], 2);
        server.abort();
    }

    #[tokio::test]
    async fn registration_rejection_returns_without_legacy_fallback() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let request_number = {
                let mut requests = requests.lock().await;
                requests.push(request);
                requests.len()
            };
            match request_number {
                1 => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"message": "legacy only"})),
                    )
                }
                2 => (
                    StatusCode::OK,
                    Json(json!({"installationId": "discovered-before-timeout"})),
                ),
                _ => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    (
                        StatusCode::OK,
                        Json(json!({
                            "installationId": "discovered-before-timeout",
                            "controlSecret": "too-late"
                        })),
                    )
                }
            }
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        set_test_router_url(&state, format!("http://{addr}")).await;

        let started = tokio::time::Instant::now();
        let error = state
            .register_router_installation_locked_with_timeout(Duration::from_millis(200))
            .await
            .expect_err("registration rejection must be terminal");

        assert!(started.elapsed() < Duration::from_millis(750));
        assert!(error.to_string().contains("401 Unauthorized"));
        assert!(!crate::client_tunnel_provision::is_router_unreachable_error(&error));
        assert_eq!(requests.lock().await.len(), 1);
        assert!(state
            .config_snapshot()
            .await
            .router
            .identity
            .unwrap()
            .installation_id
            .is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn registered_identity_without_control_secret_recovers_via_legacy_signature() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut requests = requests.lock().await;
            requests.push(request);
            if requests.len() == 1 {
                (
                    StatusCode::OK,
                    Json(json!({"installationId": "inst-known"})),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(json!({
                        "installationId": "inst-known",
                        "controlSecret": "recovered-control-secret"
                    })),
                )
            }
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/installations/register", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-known".to_string();
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();

        let result = state.register_router_installation().await.unwrap();

        assert!(result.control_secret_present);
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["proofVersion"], 2);
        assert!(requests[1].get("proofVersion").is_none());
        assert!(requests[1]["timestampMs"].is_number());
        assert!(requests[1]["signature"].is_string());
        drop(requests);
        let identity = state.config_snapshot().await.router.identity.unwrap();
        assert_eq!(
            identity.control_secret.as_deref(),
            Some("recovered-control-secret")
        );
        server.abort();
    }

    #[tokio::test]
    async fn legacy_router_share_prune_is_compatible_without_persisting_marker() {
        let (router_url, router, server) =
            spawn_share_prune_mock_router(["ghost-share".to_string()], StatusCode::NOT_FOUND).await;
        let state = test_state();
        configure_registered_test_router(&state, &router_url, "inst-prune-legacy").await;
        create_router_shares(&state, "legacyshare", 1).await;

        assert_eq!(
            reconcile_all_shares_to_router(state.clone()).await.unwrap(),
            1
        );
        run_periodic_share_sync_retry_once(&state).await;
        assert_eq!(
            router.prune_requests.load(AtomicOrdering::SeqCst),
            1,
            "unsupported prune must not enter the periodic retry loop"
        );
        assert_eq!(
            reconcile_all_shares_to_router(state.clone()).await.unwrap(),
            1
        );

        assert_eq!(router.prune_requests.load(AtomicOrdering::SeqCst), 2);
        assert!(state
            .shares
            .read()
            .await
            .router_share_prune_marker
            .is_none());
        assert!(ShareStore::load_or_default(&state.config_dir)
            .unwrap()
            .router_share_prune_marker
            .is_none());
        assert!(router.remote_share_ids.lock().await.contains("ghost-share"));
        server.abort();
    }

    #[tokio::test]
    async fn successful_share_prune_clears_ghost_once_and_target_change_runs_again() {
        let (first_url, first_router, first_server) =
            spawn_share_prune_mock_router(["ghost-first".to_string()], StatusCode::NO_CONTENT)
                .await;
        let state = test_state();
        configure_registered_test_router(&state, &first_url, "inst-prune-target").await;
        create_router_shares(&state, "localshare", 1).await;

        reconcile_all_shares_to_router(state.clone()).await.unwrap();
        assert_eq!(
            first_router
                .remote_share_ids
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["localshare0".to_string()]
        );
        assert_eq!(first_router.prune_requests.load(AtomicOrdering::SeqCst), 1);
        assert!(state
            .shares
            .read()
            .await
            .router_share_prune_applied_for(&first_url, "inst-prune-target"));

        reconcile_all_shares_to_router(state.clone()).await.unwrap();
        assert_eq!(
            first_router.prune_requests.load(AtomicOrdering::SeqCst),
            1,
            "the same target must not be pruned twice after the marker is durable"
        );

        let (second_url, second_router, second_server) =
            spawn_share_prune_mock_router(["ghost-second".to_string()], StatusCode::NO_CONTENT)
                .await;
        let mut config = state.config_snapshot().await;
        config.router.url = Some(second_url.clone());
        config.router.api_base = None;
        state.replace_config(config).await.unwrap();

        reconcile_all_shares_to_router(state.clone()).await.unwrap();
        assert_eq!(second_router.prune_requests.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            second_router
                .remote_share_ids
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["localshare0".to_string()]
        );
        let persisted = ShareStore::load_or_default(&state.config_dir).unwrap();
        assert!(persisted.router_share_prune_applied_for(&second_url, "inst-prune-target"));
        assert!(!persisted.router_share_prune_applied_for(&first_url, "inst-prune-target"));

        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn router_share_reconcile_chunks_upserts_before_one_time_prune() {
        let (router_url, router, server) =
            spawn_share_prune_mock_router(Vec::new(), StatusCode::NO_CONTENT).await;
        let state = test_state();
        configure_registered_test_router(&state, &router_url, "inst-prune-chunks").await;
        let share_count = ROUTER_SHARE_SYNC_BATCH_SIZE * 2 + 3;
        create_router_shares(&state, "chunkedshare", share_count).await;

        assert_eq!(
            reconcile_all_shares_to_router(state.clone()).await.unwrap(),
            share_count
        );

        assert_eq!(
            *router.batch_sizes.lock().await,
            vec![
                ROUTER_SHARE_SYNC_BATCH_SIZE,
                ROUTER_SHARE_SYNC_BATCH_SIZE,
                3
            ]
        );
        assert_eq!(
            *router.request_order.lock().await,
            vec![
                format!("batch:{}", ROUTER_SHARE_SYNC_BATCH_SIZE),
                format!("batch:{}", ROUTER_SHARE_SYNC_BATCH_SIZE),
                "batch:3".to_string(),
                format!("prune:{share_count}"),
            ]
        );
        assert_eq!(router.remote_share_ids.lock().await.len(), share_count);
        assert!(state
            .shares
            .read()
            .await
            .router_share_prune_applied_for(&router_url, "inst-prune-chunks"));
        server.abort();
    }

    #[tokio::test]
    async fn periodic_retry_replays_full_snapshot_after_prune_failure() {
        let (router_url, router, server) = spawn_share_prune_mock_router(
            ["ghost-share".to_string()],
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        let state = test_state();
        configure_registered_test_router(&state, &router_url, "inst-prune-retry").await;
        create_router_shares(&state, "retryshare", 2).await;

        reconcile_all_shares_to_router(state.clone())
            .await
            .expect_err("the initial prune must fail");
        assert_eq!(router.prune_requests.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(*router.batch_sizes.lock().await, vec![2]);
        assert!(state.shares.read().await.shares.iter().all(|share| {
            share.router_synced_revision == share.config_revision
                && share.router_last_sync_error.is_none()
        }));

        *router.prune_status.lock().await = StatusCode::NO_CONTENT;
        run_periodic_share_sync_retry_once(&state).await;

        assert_eq!(router.prune_requests.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(*router.batch_sizes.lock().await, vec![2, 2]);
        assert_eq!(
            router
                .remote_share_ids
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["retryshare0".to_string(), "retryshare1".to_string()]
        );
        assert!(state
            .shares
            .read()
            .await
            .router_share_prune_applied_for(&router_url, "inst-prune-retry"));

        run_periodic_share_sync_retry_once(&state).await;
        assert_eq!(router.prune_requests.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(*router.batch_sizes.lock().await, vec![2, 2]);
        server.abort();
    }

    #[tokio::test]
    async fn failed_router_share_delete_survives_restart_and_success_clears_it() {
        async fn handler(
            AxumState(attempts): AxumState<Arc<AtomicUsize>>,
            Json(_request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            if attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "retry later"})),
                )
            } else {
                (StatusCode::OK, Json(json!({"ok": true})))
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/shares/batch-sync", post(handler))
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-delete-restart".to_string();
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();
        state
            .mutate_shares_immediate(|store| {
                store.upsert(router_sync_share_input("sharedelete", "provider-delete"))
            })
            .await
            .unwrap()
            .unwrap();
        let tombstone = state
            .mutate_shares_immediate(|store| store.delete("sharedelete"))
            .await
            .unwrap()
            .unwrap();
        assert!(tombstone.has_legacy_router_target());

        retry_router_share_deletes(&state, &[tombstone.clone()])
            .await
            .expect_err("the first Router delete must remain pending");
        let persisted = ShareStore::load_or_default(&state.config_dir).unwrap();
        assert_eq!(persisted.pending_router_deletes.len(), 1);
        assert!(persisted.pending_router_deletes[0].last_error.is_some());
        assert_eq!(
            persisted.pending_router_deletes[0]
                .router_api_base
                .as_deref(),
            Some(format!("http://{addr}").as_str())
        );
        assert_eq!(
            persisted.pending_router_deletes[0]
                .installation_id
                .as_deref(),
            Some("inst-delete-restart")
        );
        let config_dir = state.config_dir.clone();
        drop(state);

        let restarted = test_state_at(config_dir.clone());
        let pending = restarted.shares.read().await.pending_router_deletes.clone();
        assert_eq!(pending.len(), 1);
        retry_router_share_deletes(&restarted, &pending)
            .await
            .unwrap();

        assert!(restarted
            .shares
            .read()
            .await
            .pending_router_deletes
            .is_empty());
        assert!(ShareStore::load_or_default(&config_dir)
            .unwrap()
            .pending_router_deletes
            .is_empty());
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn router_share_delete_retry_waits_for_its_original_target() {
        async fn handler(
            AxumState(requests): AxumState<Arc<AtomicUsize>>,
            Json(_request): Json<Value>,
        ) -> Json<Value> {
            requests.fetch_add(1, AtomicOrdering::SeqCst);
            Json(json!({"ok": true}))
        }

        async fn spawn_router() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
            let requests = Arc::new(AtomicUsize::new(0));
            let app = Router::new()
                .route("/v1/shares/batch-sync", post(handler))
                .with_state(requests.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (format!("http://{addr}"), requests, server)
        }

        let (first_url, first_requests, first_server) = spawn_router().await;
        let (second_url, second_requests, second_server) = spawn_router().await;
        let state = test_state();
        let mut first_config = state.config_snapshot().await;
        first_config.router.url = Some(first_url.clone());
        let mut first_identity = client::generate_identity_without_installation();
        first_identity.installation_id = "inst-delete-first".to_string();
        first_config.router.identity = Some(first_identity.clone());
        state.replace_config(first_config).await.unwrap();
        state
            .mutate_shares_immediate(|store| {
                store.upsert(router_sync_share_input(
                    "targeteddelete",
                    "provider-targeted-delete",
                ))
            })
            .await
            .unwrap()
            .unwrap();
        let tombstone = state
            .delete_share_immediate("targeteddelete")
            .await
            .unwrap()
            .unwrap();
        assert!(tombstone.router_target_matches(&first_url, "inst-delete-first"));

        let mut second_config = state.config_snapshot().await;
        second_config.router.url = Some(second_url);
        let mut second_identity = client::generate_identity_without_installation();
        second_identity.installation_id = "inst-delete-second".to_string();
        second_config.router.identity = Some(second_identity);
        state.replace_config(second_config).await.unwrap();

        assert_eq!(
            retry_router_share_deletes(&state, &[tombstone.clone()])
                .await
                .unwrap(),
            0
        );
        assert_eq!(first_requests.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(second_requests.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(state.shares.read().await.pending_router_deletes.len(), 1);

        let mut restored_config = state.config_snapshot().await;
        restored_config.router.url = Some(first_url);
        restored_config.router.identity = Some(first_identity);
        state.replace_config(restored_config).await.unwrap();
        assert_eq!(
            retry_router_share_deletes(&state, &[tombstone])
                .await
                .unwrap(),
            1
        );
        assert_eq!(first_requests.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(second_requests.load(AtomicOrdering::SeqCst), 0);
        assert!(state.shares.read().await.pending_router_deletes.is_empty());

        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn pending_router_share_deletes_are_replayed_in_bounded_batches() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            requests.lock().await.push(request);
            Json(json!({"ok": true}))
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/shares/batch-sync", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-delete-batch".to_string();
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();

        let tombstone_count = ROUTER_SHARE_SYNC_BATCH_SIZE * 2 + 3;
        let tombstones =
            create_router_delete_tombstones(&state, "batchshare", tombstone_count).await;

        assert_eq!(
            retry_router_share_deletes(&state, &tombstones)
                .await
                .unwrap(),
            tombstone_count
        );

        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests
                .iter()
                .map(|request| request["ops"].as_array().unwrap().len())
                .collect::<Vec<_>>(),
            vec![
                ROUTER_SHARE_SYNC_BATCH_SIZE,
                ROUTER_SHARE_SYNC_BATCH_SIZE,
                3
            ]
        );
        assert!(requests
            .iter()
            .flat_map(|request| request["ops"].as_array().unwrap())
            .all(|operation| operation["kind"] == "delete"));
        assert!(state.shares.read().await.pending_router_deletes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn pending_router_share_delete_replay_confirms_each_batch_before_stopping_on_failure() {
        async fn handler(
            AxumState(requests): AxumState<Arc<TokioMutex<Vec<Value>>>>,
            Json(request): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let attempt = {
                let mut requests = requests.lock().await;
                requests.push(request);
                requests.len()
            };
            if attempt == 2 {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"message": "retry later"})),
                )
            } else {
                (StatusCode::OK, Json(json!({"ok": true})))
            }
        }

        let requests = Arc::new(TokioMutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/shares/batch-sync", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-delete-batch-failure".to_string();
        config.router.identity = Some(identity);
        state.replace_config(config).await.unwrap();

        let tombstone_count = ROUTER_SHARE_SYNC_BATCH_SIZE * 2 + 7;
        let tombstones =
            create_router_delete_tombstones(&state, "failedbatchshare", tombstone_count).await;

        retry_router_share_deletes(&state, &tombstones)
            .await
            .expect_err("the second Router delete batch must remain pending");

        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            request["ops"].as_array().unwrap().len() == ROUTER_SHARE_SYNC_BATCH_SIZE
        }));
        drop(requests);

        let persisted = ShareStore::load_or_default(&state.config_dir).unwrap();
        assert_eq!(
            persisted.pending_router_deletes.len(),
            tombstone_count - ROUTER_SHARE_SYNC_BATCH_SIZE
        );
        assert!(
            persisted.pending_router_deletes[..ROUTER_SHARE_SYNC_BATCH_SIZE]
                .iter()
                .all(|tombstone| tombstone.last_error.is_some())
        );
        assert!(
            persisted.pending_router_deletes[ROUTER_SHARE_SYNC_BATCH_SIZE..]
                .iter()
                .all(|tombstone| tombstone.last_error.is_none())
        );
        assert_eq!(
            persisted.pending_router_deletes[0].share_id,
            format!("failedbatchshare{}", ROUTER_SHARE_SYNC_BATCH_SIZE)
        );
        assert_eq!(
            persisted.pending_router_deletes[ROUTER_SHARE_SYNC_BATCH_SIZE].share_id,
            format!("failedbatchshare{}", ROUTER_SHARE_SYNC_BATCH_SIZE * 2)
        );
        assert_eq!(
            state.shares.read().await.pending_router_deletes,
            persisted.pending_router_deletes
        );
        server.abort();
    }

    #[tokio::test]
    async fn in_flight_delete_is_followed_by_upsert_when_same_id_is_recreated() {
        #[derive(Clone)]
        struct Gate {
            requests: Arc<TokioMutex<Vec<Value>>>,
            delete_received: Arc<tokio::sync::Notify>,
            release_delete: Arc<tokio::sync::Notify>,
        }

        async fn handler(
            AxumState(gate): AxumState<Gate>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            let kind = request["ops"][0]["kind"].as_str().unwrap().to_string();
            gate.requests.lock().await.push(request);
            if kind == "delete" {
                gate.delete_received.notify_one();
                gate.release_delete.notified().await;
            }
            Json(json!({"ok": true}))
        }

        let gate = Gate {
            requests: Arc::new(TokioMutex::new(Vec::new())),
            delete_received: Arc::new(tokio::sync::Notify::new()),
            release_delete: Arc::new(tokio::sync::Notify::new()),
        };
        let app = Router::new()
            .route("/v1/shares/batch-sync", post(handler))
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = "inst-delete-recreate".to_string();
        config.router.identity = Some(identity);
        config.client.tunnel_subdomain = Some("clienttest".to_string());
        state.replace_config(config).await.unwrap();
        state
            .mutate_shares_immediate(|store| {
                store.upsert(router_sync_share_input(
                    "recreatedshare",
                    "provider-before-delete",
                ))
            })
            .await
            .unwrap()
            .unwrap();
        let tombstone = state
            .delete_share_immediate("recreatedshare")
            .await
            .unwrap()
            .unwrap();
        let retry_state = state.clone();
        let retry_tombstone = tombstone.clone();
        let retry = tokio::spawn(async move {
            retry_router_share_deletes(&retry_state, &[retry_tombstone]).await
        });

        gate.delete_received.notified().await;
        state
            .mutate_shares_immediate(|store| {
                store.upsert(router_sync_share_input(
                    "recreatedshare",
                    "provider-after-delete",
                ))
            })
            .await
            .unwrap()
            .unwrap();
        assert!(state.shares.read().await.pending_router_deletes.is_empty());
        gate.release_delete.notify_one();
        retry.await.unwrap().unwrap();

        let requests = gate.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["ops"][0]["kind"], "delete");
        assert_eq!(requests[1]["ops"][0]["kind"], "upsert");
        assert_eq!(
            requests[1]["ops"][0]["share"]["providerId"],
            "provider-after-delete"
        );
        drop(requests);
        let share = state
            .shares
            .read()
            .await
            .get("recreatedshare")
            .cloned()
            .unwrap();
        assert_eq!(share.router_synced_revision, share.config_revision);
        server.abort();
    }

    async fn set_test_router_url(state: &ServerState, url: String) {
        let mut config = state.config_snapshot().await;
        config.router.url = Some(url);
        config.router.api_base = None;
        state.replace_config(config).await.unwrap();
    }

    fn test_state() -> ServerState {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir = std::env::temp_dir().join(format!("cc-switch-server-state-test-{nanos}"));
        test_state_at(config_dir)
    }

    fn test_state_at(config_dir: PathBuf) -> ServerState {
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
    fn share_in_flight_tracker_intersects_share_and_user_limits() {
        let tracker = Arc::new(ShareInFlightTracker::default());
        let alice = tracker
            .try_acquire_for_user("share-1", Some(2), Some("Alice@Example.com"), Some(1))
            .expect("alice should acquire her first slot");
        assert!(matches!(
            tracker.try_acquire_for_user("share-1", Some(2), Some("alice@example.com"), Some(1)),
            Err(ShareInFlightAcquireError::UserLimit)
        ));

        let bob = tracker
            .try_acquire_for_user("share-1", Some(2), Some("bob@example.com"), Some(2))
            .expect("bob should use the remaining total slot");
        assert!(matches!(
            tracker.try_acquire_for_user("share-1", Some(2), Some("charlie@example.com"), Some(2)),
            Err(ShareInFlightAcquireError::ShareLimit)
        ));

        drop(alice);
        assert!(tracker
            .try_acquire_for_user("share-1", Some(2), Some("charlie@example.com"), Some(1))
            .is_ok());
        drop(bob);
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
            free_access: false,
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
            config_revision: 0,
            router_synced_revision: 0,
            user_grants: std::collections::BTreeMap::new(),
        }
    }
}
