use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use russh::client;
use russh::keys::key::PublicKey;
use russh::{Channel, Disconnect};
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::core::router_client::IssueLeaseResponse;
use crate::core::usage::now_ms;

pub type LeaseFuture = Pin<Box<dyn Future<Output = anyhow::Result<IssueLeaseResponse>> + Send>>;
pub type LeaseFn = Arc<dyn Fn() -> LeaseFuture + Send + Sync>;

const TUNNELS_FILE_NAME: &str = "tunnels.json";
const LEASE_RENEW_BEFORE: Duration = Duration::from_secs(60);
const LEASE_RENEW_SHORT_BEFORE: Duration = Duration::from_secs(10);
const LEASE_RENEW_MIN_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelRuntimeStatus {
    pub key: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_at_ms: Option<u128>,
    pub updated_at_ms: u128,
}

pub struct TunnelSupervisor {
    tasks: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    statuses: Arc<RwLock<BTreeMap<String, TunnelRuntimeStatus>>>,
    store_path: PathBuf,
}

impl std::fmt::Debug for TunnelSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TunnelSupervisor")
    }
}

impl TunnelSupervisor {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Arc<Self>> {
        fs::create_dir_all(config_dir)?;
        let store_path = tunnels_path(config_dir);
        let statuses = if store_path.exists() {
            let content = fs::read_to_string(&store_path)?;
            serde_json::from_str::<TunnelRuntimeStore>(&content)?.statuses
        } else {
            BTreeMap::new()
        };
        Ok(Arc::new(Self {
            tasks: Mutex::new(BTreeMap::new()),
            statuses: Arc::new(RwLock::new(statuses)),
            store_path,
        }))
    }

    pub async fn reload_statuses(&self) -> anyhow::Result<()> {
        let statuses = if self.store_path.exists() {
            let content = fs::read_to_string(&self.store_path)?;
            serde_json::from_str::<TunnelRuntimeStore>(&content)?.statuses
        } else {
            BTreeMap::new()
        };
        *self.statuses.write().await = statuses;
        Ok(())
    }

    pub async fn start(
        self: &Arc<Self>,
        key: impl Into<String>,
        kind: impl Into<String>,
        local_addr: String,
        lease_fn: LeaseFn,
    ) {
        let key = key.into();
        let kind = kind.into();
        self.stop(&key, "restarting").await;
        self.set_status(TunnelRuntimeStatus {
            key: key.clone(),
            kind: kind.clone(),
            status: "starting".to_string(),
            updated_at_ms: now_ms(),
            ..TunnelRuntimeStatus::default()
        })
        .await;

        let statuses = self.statuses.clone();
        let store_path = self.store_path.clone();
        let task_key = key.clone();
        let task_kind = kind.clone();
        let handle = tokio::spawn(async move {
            let mut delay = Duration::from_secs(1);
            loop {
                set_status(
                    &statuses,
                    &store_path,
                    TunnelRuntimeStatus {
                        key: task_key.clone(),
                        kind: task_kind.clone(),
                        status: "leasing".to_string(),
                        updated_at_ms: now_ms(),
                        ..TunnelRuntimeStatus::default()
                    },
                )
                .await;

                match lease_fn().await {
                    Ok(lease) => {
                        delay = Duration::from_secs(1);
                        set_status(
                            &statuses,
                            &store_path,
                            status_from_lease(&task_key, &task_kind, "connecting", &lease, None),
                        )
                        .await;
                        match connect_and_forward(
                            &lease,
                            &local_addr,
                            &statuses,
                            &store_path,
                            &task_key,
                            &task_kind,
                        )
                        .await
                        {
                            Ok(TunnelConnectionEnd::Renewing) => {
                                continue;
                            }
                            Ok(TunnelConnectionEnd::Ended) => {
                                set_status(
                                    &statuses,
                                    &store_path,
                                    status_from_lease(&task_key, &task_kind, "ended", &lease, None),
                                )
                                .await;
                            }
                            Err(error) => {
                                set_status(
                                    &statuses,
                                    &store_path,
                                    status_from_lease(
                                        &task_key,
                                        &task_kind,
                                        "retrying",
                                        &lease,
                                        Some(error.to_string()),
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                    Err(error) => {
                        set_status(
                            &statuses,
                            &store_path,
                            TunnelRuntimeStatus {
                                key: task_key.clone(),
                                kind: task_kind.clone(),
                                status: "retrying".to_string(),
                                last_error: Some(error.to_string()),
                                updated_at_ms: now_ms(),
                                ..TunnelRuntimeStatus::default()
                            },
                        )
                        .await;
                    }
                }

                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        });

        self.tasks.lock().await.insert(key, handle);
    }

    pub async fn stop(&self, key: &str, status: &str) {
        if let Some(handle) = self.tasks.lock().await.remove(key) {
            handle.abort();
        }
        let snapshot = {
            let mut statuses = self.statuses.write().await;
            if let Some(existing) = statuses.get_mut(key) {
                existing.status = status.to_string();
                existing.updated_at_ms = now_ms();
            }
            statuses.clone()
        };
        persist_statuses(&self.store_path, &snapshot);
    }

    pub async fn status(&self, key: &str) -> Option<TunnelRuntimeStatus> {
        self.statuses.read().await.get(key).cloned()
    }

    pub async fn statuses(&self) -> Vec<TunnelRuntimeStatus> {
        self.statuses.read().await.values().cloned().collect()
    }

    async fn set_status(&self, status: TunnelRuntimeStatus) {
        set_status(&self.statuses, &self.store_path, status).await;
    }
}

pub fn client_tunnel_key() -> String {
    "client-web".to_string()
}

pub fn share_tunnel_key(share_id: &str) -> String {
    format!("share:{share_id}")
}

pub fn local_forward_addr(bind_addr: SocketAddr) -> String {
    if bind_addr.ip().is_unspecified() {
        format!("127.0.0.1:{}", bind_addr.port())
    } else {
        bind_addr.to_string()
    }
}

async fn set_status(
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    status: TunnelRuntimeStatus,
) {
    let snapshot = {
        let mut statuses = statuses.write().await;
        statuses.insert(status.key.clone(), status);
        statuses.clone()
    };
    persist_statuses(store_path, &snapshot);
}

fn status_from_lease(
    key: &str,
    kind: &str,
    status: &str,
    lease: &IssueLeaseResponse,
    error: Option<String>,
) -> TunnelRuntimeStatus {
    TunnelRuntimeStatus {
        key: key.to_string(),
        kind: kind.to_string(),
        status: status.to_string(),
        tunnel_url: Some(lease.tunnel_url.clone()),
        subdomain: Some(lease.subdomain.clone()),
        lease_id: Some(lease.lease_id.clone()),
        connection_id: Some(lease.connection_id.clone()),
        lease_expires_at: Some(lease.expires_at.clone()),
        last_error: error,
        updated_at_ms: now_ms(),
        ..TunnelRuntimeStatus::default()
    }
}

async fn connect_and_forward(
    lease: &IssueLeaseResponse,
    local_addr: &str,
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    key: &str,
    kind: &str,
) -> anyhow::Result<TunnelConnectionEnd> {
    let ssh_config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel();
    let handler = TunnelHandler {
        fwd_tx,
        expected_fingerprint: lease.ssh_host_fingerprint.clone(),
        ssh_addr: lease.ssh_addr.clone(),
    };
    let mut handle = client::connect(ssh_config, &lease.ssh_addr, handler).await?;
    let auth_ok = handle
        .authenticate_password(&lease.ssh_username, &lease.ssh_password)
        .await?;
    if !auth_ok {
        anyhow::bail!("router ssh authentication failed");
    }

    let remote_port = request_forward(&mut handle).await?;
    let mut connected = status_from_lease(key, kind, "connected", lease, None);
    connected.remote_port = Some(remote_port);
    connected.connected_at_ms = Some(now_ms());
    set_status(statuses, store_path, connected).await;

    let result = if let Some(delay) = renewal_delay(&lease.expires_at) {
        tokio::select! {
            result = accept_loop(fwd_rx, local_addr) => result.map(|_| TunnelConnectionEnd::Ended),
            _ = tokio::time::sleep(delay) => {
                let mut renewing = status_from_lease(key, kind, "renewing", lease, None);
                renewing.remote_port = Some(remote_port);
                set_status(statuses, store_path, renewing).await;
                Ok(TunnelConnectionEnd::Renewing)
            }
        }
    } else {
        accept_loop(fwd_rx, local_addr)
            .await
            .map(|_| TunnelConnectionEnd::Ended)
    };
    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
    result
}

async fn request_forward(handle: &mut client::Handle<TunnelHandler>) -> anyhow::Result<u16> {
    for _ in 0..10 {
        let port: u16 = rand::thread_rng().gen_range(20000..30000);
        match handle.tcpip_forward("0.0.0.0", port as u32).await {
            Ok(bound_port) => {
                return Ok(if bound_port == 0 {
                    port
                } else {
                    bound_port as u16
                });
            }
            Err(error) => {
                tracing::debug!(port, error = %error, "remote forward port rejected");
            }
        }
    }
    anyhow::bail!("all remote forward port attempts failed")
}

async fn accept_loop(
    mut fwd_rx: mpsc::UnboundedReceiver<Channel<client::Msg>>,
    local_addr: &str,
) -> anyhow::Result<()> {
    while let Some(channel) = fwd_rx.recv().await {
        let local_addr = local_addr.to_string();
        tokio::spawn(async move {
            let stream = channel.into_stream();
            if let Err(error) = forward_tcp(stream, &local_addr).await {
                tracing::debug!(error = %error, "tunnel tcp forward failed");
            }
        });
    }
    Ok(())
}

async fn forward_tcp<S>(mut remote: S, local_addr: &str) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut local = TcpStream::connect(local_addr).await?;
    io::copy_bidirectional(&mut remote, &mut local).await?;
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelConnectionEnd {
    Ended,
    Renewing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelRuntimeStore {
    #[serde(default)]
    statuses: BTreeMap<String, TunnelRuntimeStatus>,
}

pub fn tunnels_path(config_dir: &Path) -> PathBuf {
    config_dir.join(TUNNELS_FILE_NAME)
}

pub fn renewal_delay(expires_at: &str) -> Option<Duration> {
    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .ok()?
        .with_timezone(&Utc);
    let now = Utc::now();
    if expires_at <= now {
        return Some(Duration::from_secs(0));
    }
    let remaining = (expires_at - now).to_std().ok()?;
    if remaining <= LEASE_RENEW_MIN_DELAY {
        return Some(Duration::from_secs(0));
    }

    let renew_before = if remaining > LEASE_RENEW_BEFORE * 2 {
        LEASE_RENEW_BEFORE
    } else {
        let half_remaining = Duration::from_secs((remaining.as_secs() / 2).max(1));
        LEASE_RENEW_SHORT_BEFORE.min(half_remaining)
    };
    let delay = remaining.saturating_sub(renew_before);
    Some(if delay < LEASE_RENEW_MIN_DELAY {
        LEASE_RENEW_MIN_DELAY.min(remaining)
    } else {
        delay
    })
}

fn persist_statuses(store_path: &Path, statuses: &BTreeMap<String, TunnelRuntimeStatus>) {
    let store = TunnelRuntimeStore {
        statuses: statuses.clone(),
    };
    if let Err(error) = crate::core::storage::write_json_pretty(store_path, &store) {
        tracing::warn!(error = %error, path = %store_path.display(), "write tunnel runtime store failed");
    }
}

struct TunnelHandler {
    fwd_tx: mpsc::UnboundedSender<Channel<client::Msg>>,
    expected_fingerprint: Option<String>,
    ssh_addr: String,
}

#[async_trait]
impl client::Handler for TunnelHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let actual = format!("SHA256:{}", server_public_key.fingerprint());
        match &self.expected_fingerprint {
            Some(expected) if constant_time_eq(expected.as_bytes(), actual.as_bytes()) => {
                tracing::info!(ssh_addr = %self.ssh_addr, fingerprint = %actual, "router ssh host key verified");
                Ok(true)
            }
            Some(expected) => {
                tracing::error!(
                    ssh_addr = %self.ssh_addr,
                    expected = %expected,
                    actual = %actual,
                    "router ssh host key mismatch"
                );
                Ok(false)
            }
            None => {
                tracing::warn!(
                    ssh_addr = %self.ssh_addr,
                    actual = %actual,
                    "router did not return ssh host fingerprint; accepting key"
                );
                Ok(true)
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let _ = self.fwd_tx.send(channel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_config_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cc-switch-server-{name}-{nanos}"))
    }

    #[tokio::test]
    async fn persists_runtime_status_and_loads_it_back() {
        let dir = temp_config_dir("tunnel-persist");
        let supervisor = TunnelSupervisor::load_or_default(&dir).unwrap();
        supervisor
            .set_status(TunnelRuntimeStatus {
                key: "client-web".to_string(),
                kind: "client-web".to_string(),
                status: "connected".to_string(),
                tunnel_url: Some("https://example.test".to_string()),
                lease_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
                updated_at_ms: now_ms(),
                ..TunnelRuntimeStatus::default()
            })
            .await;

        let loaded = TunnelSupervisor::load_or_default(&dir).unwrap();
        let status = loaded.status("client-web").await.unwrap();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(status.status, "connected");
        assert_eq!(status.tunnel_url.as_deref(), Some("https://example.test"));
    }

    #[tokio::test]
    async fn stop_status_is_persisted() {
        let dir = temp_config_dir("tunnel-stop");
        let supervisor = TunnelSupervisor::load_or_default(&dir).unwrap();
        supervisor
            .set_status(TunnelRuntimeStatus {
                key: "share:s1".to_string(),
                kind: "share-http".to_string(),
                status: "connected".to_string(),
                updated_at_ms: now_ms(),
                ..TunnelRuntimeStatus::default()
            })
            .await;
        supervisor.stop("share:s1", "stopped").await;

        let loaded = TunnelSupervisor::load_or_default(&dir).unwrap();
        let status = loaded.status("share:s1").await.unwrap();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(status.status, "stopped");
    }

    #[test]
    fn renewal_delay_is_zero_only_for_imminent_expiry() {
        let soon = (Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
        assert_eq!(renewal_delay(&soon), Some(Duration::from_secs(0)));
    }

    #[test]
    fn renewal_delay_keeps_short_router_leases_connected() {
        let future = (Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        let delay = renewal_delay(&future).unwrap();
        assert!(delay <= Duration::from_secs(51));
        assert!(delay >= Duration::from_secs(45));
    }

    #[test]
    fn renewal_delay_is_before_expiry() {
        let future = (Utc::now() + chrono::Duration::seconds(180)).to_rfc3339();
        let delay = renewal_delay(&future).unwrap();
        assert!(delay <= Duration::from_secs(121));
        assert!(delay >= Duration::from_secs(100));
    }
}
