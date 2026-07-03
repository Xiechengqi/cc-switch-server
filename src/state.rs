use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use futures_util::StreamExt;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{sleep, Duration};

use crate::cli::Cli;
use crate::core::account_managers::AccountRefreshLocks;
use crate::core::account_refresh::{account_needs_native_refresh, execute_native_account_refresh};
use crate::core::accounts::{Account, AccountRefreshUpdate, AccountStore};
use crate::core::config::{mask_proxy_url, ServerConfig};
use crate::core::failover::FailoverStore;
use crate::core::kiro_device::KiroDeviceFlowStore;
use crate::core::oauth_login::OAuthLoginStore;
use crate::core::pricing::ModelPricingStore;
use crate::core::provider::AppKind;
use crate::core::providers::ProviderStore;
use crate::core::quota::{refresh_account_quota, QuotaRefreshResult};
use crate::core::router_client::{
    self, IssueLeaseResponse, ShareEditAckPayload, ShareEditView, ShareRequestLogEntry,
};
use crate::core::shares::{ShareMarketGrantStatus, ShareStore};
use crate::core::tunnel::{self, LeaseFn, TunnelSupervisor};
use crate::core::universal_providers::UniversalProviderStore;
use crate::core::usage::{UsageLog, UsageStore};
use crate::coverage::ProviderCoverage;
use crate::proxy::cursor::cursor_session::CursorSessionManager;

#[derive(Debug)]
pub struct ServerStateInner {
    pub bind_addr: SocketAddr,
    pub config_dir: PathBuf,
    pub web_dist_dir: Option<PathBuf>,
    pub provider_coverage: ProviderCoverage,
    pub config: RwLock<ServerConfig>,
    pub providers: RwLock<ProviderStore>,
    pub universal_providers: RwLock<UniversalProviderStore>,
    pub accounts: RwLock<AccountStore>,
    pub failover: RwLock<FailoverStore>,
    pub pricing: RwLock<ModelPricingStore>,
    pub usage: RwLock<UsageStore>,
    pub shares: RwLock<ShareStore>,
    pub sessions: RwLock<Vec<Session>>,
    pub oauth_logins: RwLock<OAuthLoginStore>,
    pub kiro_device_flows: RwLock<KiroDeviceFlowStore>,
    pub cursor_sessions: CursorSessionManager,
    pub account_refresh_locks: AccountRefreshLocks,
    pub share_in_flight: Arc<ShareInFlightTracker>,
    pub control_nonces: Arc<ControlNonceCache>,
    pub http_client: RwLock<reqwest::Client>,
    pub events: broadcast::Sender<ServerEvent>,
    pub tunnels: Arc<TunnelSupervisor>,
    pub debounced_saves: Arc<DebouncedStoreSaves>,
}

pub type ServerState = Arc<ServerStateInner>;

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
            created_at_ms: crate::core::usage::now_ms(),
        }
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

pub fn save_accounts_debounced(state: &ServerState) {
    schedule_debounced_save(state.clone(), DebouncedStoreKind::Accounts);
}

pub fn save_shares_debounced(state: &ServerState) {
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
    pub fn load(cli: Cli) -> anyhow::Result<ServerState> {
        let config_dir = cli.resolved_config_dir()?;

        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;

        let provider_coverage = ProviderCoverage::load_embedded()?;
        let config = ServerConfig::load_or_default(&config_dir)?;
        let providers = ProviderStore::load_or_default(&config_dir)?;
        let universal_providers = UniversalProviderStore::load_or_default(&config_dir)?;
        let accounts = AccountStore::load_or_default(&config_dir)?;
        let failover = FailoverStore::load_or_default(&config_dir)?;
        let pricing = ModelPricingStore::load_or_default(&config_dir)?;
        let usage = UsageStore::load_or_default(&config_dir)?;
        let shares = ShareStore::load_or_default(&config_dir)?;
        let bind_addr = SocketAddr::new(cli.host, cli.port);
        let http_client = build_http_client(&config, bind_addr)?;
        let (events, _) = broadcast::channel(256);

        let tunnels = TunnelSupervisor::load_or_default(&config_dir)?;

        Ok(Arc::new(Self {
            bind_addr,
            config_dir,
            web_dist_dir: cli.resolved_web_dist_dir(),
            provider_coverage,
            config: RwLock::new(config),
            providers: RwLock::new(providers),
            universal_providers: RwLock::new(universal_providers),
            accounts: RwLock::new(accounts),
            failover: RwLock::new(failover),
            pricing: RwLock::new(pricing),
            usage: RwLock::new(usage),
            shares: RwLock::new(shares),
            sessions: RwLock::new(Vec::new()),
            oauth_logins: RwLock::new(OAuthLoginStore::default()),
            kiro_device_flows: RwLock::new(KiroDeviceFlowStore::default()),
            cursor_sessions: CursorSessionManager::default(),
            account_refresh_locks: AccountRefreshLocks::default(),
            share_in_flight: Arc::new(ShareInFlightTracker::default()),
            control_nonces: Arc::new(ControlNonceCache::default()),
            http_client: RwLock::new(http_client),
            events,
            tunnels,
            debounced_saves: Arc::new(DebouncedStoreSaves::default()),
        }))
    }

    pub async fn replace_config(&self, config: ServerConfig) -> anyhow::Result<()> {
        let http_client = build_http_client(&config, self.bind_addr)?;
        config.save(&self.config_dir)?;
        *self.http_client.write().await = http_client;
        *self.config.write().await = config;
        Ok(())
    }

    pub async fn reload_persistent_stores(&self) -> anyhow::Result<()> {
        let config = ServerConfig::load_or_default(&self.config_dir)?;
        let http_client = build_http_client(&config, self.bind_addr)?;
        let providers = ProviderStore::load_or_default(&self.config_dir)?;
        let universal_providers = UniversalProviderStore::load_or_default(&self.config_dir)?;
        let accounts = AccountStore::load_or_default(&self.config_dir)?;
        let failover = FailoverStore::load_or_default(&self.config_dir)?;
        let pricing = ModelPricingStore::load_or_default(&self.config_dir)?;
        let usage = UsageStore::load_or_default(&self.config_dir)?;
        let shares = ShareStore::load_or_default(&self.config_dir)?;

        *self.http_client.write().await = http_client;
        *self.config.write().await = config;
        *self.providers.write().await = providers;
        *self.universal_providers.write().await = universal_providers;
        *self.accounts.write().await = accounts;
        *self.failover.write().await = failover;
        *self.pricing.write().await = pricing;
        *self.usage.write().await = usage;
        *self.shares.write().await = shares;
        self.tunnels.reload_statuses().await?;
        Ok(())
    }

    pub async fn http_client(&self) -> reqwest::Client {
        self.http_client.read().await.clone()
    }

    pub fn emit_event(&self, event: ServerEvent) {
        let _ = self.events.send(event);
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    pub async fn save_providers(&self) -> anyhow::Result<()> {
        self.providers.read().await.save(&self.config_dir)
    }

    pub async fn save_universal_providers(&self) -> anyhow::Result<()> {
        self.universal_providers.read().await.save(&self.config_dir)
    }

    pub async fn save_accounts(&self) -> anyhow::Result<()> {
        self.accounts.read().await.save(&self.config_dir)
    }

    pub async fn save_failover(&self) -> anyhow::Result<()> {
        self.failover.read().await.save(&self.config_dir)
    }

    pub async fn save_pricing(&self) -> anyhow::Result<()> {
        self.pricing.read().await.save(&self.config_dir)
    }

    pub async fn save_usage(&self) -> anyhow::Result<()> {
        self.usage.read().await.save(&self.config_dir)
    }

    pub async fn save_shares(&self) -> anyhow::Result<()> {
        self.shares.read().await.save(&self.config_dir)
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
        crate::core::config::validate_proxy_url(proxy_url)?;
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

    let auto_start_share_ids = auto_start_share_ids(&state.shares.read().await.shares);
    for share_id in auto_start_share_ids {
        start_share_tunnel(state.clone(), share_id).await;
    }
}

pub fn spawn_periodic_backups(state: ServerState) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(6 * 60 * 60)).await;
            match crate::core::backup::create_backup(
                &state.config_dir,
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
            refresh_due_account_quotas(&state).await;
            sleep(Duration::from_secs(5 * 60)).await;
        }
    });
}

async fn refresh_due_account_quotas(state: &ServerState) {
    let now = crate::core::usage::now_ms() as i64;
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
    let mut active_account = account;
    let mut account_mutated = false;
    if account_needs_native_refresh(&active_account, now) {
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(&http_client, &active_account, now).await
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
            match store.mark_refresh_success(&active_account.id, update) {
                Some(account) => account,
                None => return,
            }
        };
        account_mutated = true;
    }

    let http_client = state.http_client().await;
    match refresh_account_quota(&http_client, &active_account, now, false).await {
        Ok(QuotaRefreshResult::Updated { update, .. }) => {
            {
                let mut store = state.accounts.write().await;
                store.mark_refresh_success(&active_account.id, update);
            }
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
                update.quota_next_refresh_at =
                    Some(now.saturating_add(crate::core::quota::QUOTA_FAILURE_COOLDOWN_MS));
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
                    now.saturating_add(crate::core::quota::QUOTA_FAILURE_COOLDOWN_MS),
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
    match account.provider_type {
        crate::core::provider::ProviderType::CodexOAuth
        | crate::core::provider::ProviderType::ClaudeOAuth
        | crate::core::provider::ProviderType::GeminiCli => {
            account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || account
                    .refresh_token
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        }
        crate::core::provider::ProviderType::OllamaCloud => {
            account
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || account
                    .access_token
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        }
        _ => false,
    }
}

fn should_restore_client_tunnel(status: Option<&crate::core::tunnel::TunnelRuntimeStatus>) -> bool {
    status.is_none_or(|status| status.status != "stopped")
}

fn auto_start_share_ids(shares: &[crate::core::shares::Share]) -> Vec<String> {
    shares
        .iter()
        .filter(|share| share.auto_start && share.enabled && share.status == "active")
        .map(|share| share.id.clone())
        .collect()
}

pub async fn start_client_tunnel(state: ServerState) {
    let local_addr = tunnel::local_forward_addr(state.bind_addr);
    let lease_state = state.clone();
    let lease_fn: LeaseFn = Arc::new(move || {
        let lease_state = lease_state.clone();
        Box::pin(async move { issue_client_tunnel_lease(lease_state).await })
    });
    state
        .tunnels
        .start(
            tunnel::client_tunnel_key(),
            "client-web",
            local_addr,
            lease_fn,
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
    state
        .tunnels
        .start(key, "share-http", local_addr, lease_fn)
        .await;
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
        let result =
            router_client::batch_sync_share_request_logs(&http_client, &config, vec![entry])
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
    let edits = match router_client::pending_share_edits(&http_client, &config, share_ids).await {
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
                    updated_at_ms: Some(crate::core::usage::now_ms()),
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
    match router_client::ack_share_edit(&http_client, config, ack).await {
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
                updated_at_ms: Some(crate::core::usage::now_ms()),
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
    let descriptor = router_client::descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let op = router_client::ShareSyncOperation {
        kind: "upsert".to_string(),
        share_id: None,
        share: Some(descriptor),
    };
    let http_client = state.http_client().await;
    let result = router_client::batch_sync_shares(&http_client, config, vec![op]).await;
    let router_base = config.router_api_base().map(str::to_string);
    {
        let mut store = state.shares.write().await;
        match &result {
            Ok(()) => {
                store.router_registered = true;
                store.last_router_error = None;
                store.mark_router_sync(share_id, router_base, Ok(crate::core::usage::now_ms()));
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
    let url = router_client::share_edit_events_url(&config)?;
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
                        log.router_last_synced_at_ms = Some(crate::core::usage::now_ms());
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
        if let Err(error) = router_client::register_installation(&http_client, &mut config).await {
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
    if let Err(error) = router_client::claim_client_tunnel(
        &http_client,
        &config,
        router_client::ClientTunnelConfig {
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
    let lease = match router_client::issue_client_web_lease(&http_client, &config, subdomain).await
    {
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
    let descriptor = router_client::descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let requested_subdomain = descriptor.subdomain.clone();
    let http_client = state.http_client().await;
    router_client::claim_share_subdomain(&http_client, &config, descriptor.clone()).await?;
    router_client::issue_share_lease(&http_client, &config, requested_subdomain, descriptor).await
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::Cli;
    use crate::core::provider::{AppKind, ProviderType};
    use crate::core::shares::{Share, ShareAcl, UpsertShareInput};
    use crate::core::tunnel::TunnelRuntimeStatus;
    use crate::core::usage::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};

    use super::*;

    #[tokio::test]
    async fn share_request_log_entry_preserves_router_sync_fields() {
        let state = test_state();
        state.shares.write().await.upsert(UpsertShareInput {
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
        });

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
        ServerStateInner::load(Cli {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        })
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
    fn control_nonce_cache_rejects_replay_within_window() {
        let cache = ControlNonceCache::default();

        assert!(cache.register("inst-1", "nonce-1", 10_000, 300_000));
        assert!(!cache.register("inst-1", "nonce-1", 11_000, 300_000));
        assert!(cache.register("inst-2", "nonce-1", 11_000, 300_000));
        assert!(cache.register("inst-1", "nonce-1", 400_001, 300_000));
    }

    #[test]
    fn restore_tunnel_logic_selects_only_active_auto_start_shares() {
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
