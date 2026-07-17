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
use rand::{Rng, RngCore};
use russh::client;
use russh::keys::key::PublicKey;
use russh::{Channel, Disconnect};
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinSet;

use crate::clients::router::client::{
    NamespaceLeaseResponse, RenewLeaseError, TunnelStateResponse,
};
use crate::infra::time::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelLeaseRequest {
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

pub type LeaseFuture = Pin<Box<dyn Future<Output = anyhow::Result<NamespaceLeaseResponse>> + Send>>;
pub type LeaseFn = Arc<dyn Fn(TunnelLeaseRequest) -> LeaseFuture + Send + Sync>;
pub type RenewLeaseFuture =
    Pin<Box<dyn Future<Output = Result<String, TunnelRenewalError>> + Send>>;
pub type RenewLeaseFn = Arc<dyn Fn(NamespaceLeaseResponse) -> RenewLeaseFuture + Send + Sync>;
pub type TunnelControlFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<TunnelStateResponse>> + Send>>;
pub type ActivateTunnelFn =
    Arc<dyn Fn(NamespaceLeaseResponse) -> TunnelControlFuture + Send + Sync>;
pub type TunnelStateFn = Arc<dyn Fn(NamespaceLeaseResponse) -> TunnelControlFuture + Send + Sync>;

const TUNNELS_FILE_NAME: &str = "tunnels.json";
const LEASE_RENEW_MIN_DELAY: Duration = Duration::from_secs(5);
const LEASE_RENEW_RETRY_MAX_DELAY: Duration = Duration::from_secs(15);
const LEASE_ISSUE_TIMEOUT: Duration = Duration::from_secs(15);
const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SSH_AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const REMOTE_FORWARD_TIMEOUT: Duration = Duration::from_secs(15);
const TUNNEL_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const LEASE_RENEW_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const SSH_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_secs(1);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(60);
const STABLE_CONNECTION_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, thiserror::Error)]
pub enum TunnelRenewalError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    ReplaceRequired(String),
    #[error("{0}")]
    FatalConfiguration(String),
}

impl TunnelRenewalError {
    pub fn from_router(error: RenewLeaseError, configuration_available: bool) -> Self {
        match error {
            RenewLeaseError::Retryable(message) => Self::Retryable(message),
            RenewLeaseError::Terminal(message)
                if configuration_available && renewal_rejection_requires_replacement(&message) =>
            {
                Self::ReplaceRequired(message)
            }
            RenewLeaseError::Terminal(message) => Self::FatalConfiguration(message),
        }
    }
}

fn renewal_rejection_requires_replacement(message: &str) -> bool {
    ["404 Not Found", "409 Conflict", "410 Gone"]
        .iter()
        .any(|status| message.contains(status))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStartOutcome {
    Started { generation: u64 },
    AlreadyRunning { generation: u64 },
}

impl TunnelStartOutcome {
    pub fn started(self) -> bool {
        matches!(self, Self::Started { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportState {
    Candidate,
    Active,
    Draining,
    Retired,
}

impl TransportState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Retired => "retired",
        }
    }
}

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
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub desired_generation: u64,
    #[serde(default)]
    pub router_generation: u64,
    #[serde(default)]
    pub router_active_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,
    pub updated_at_ms: u128,
}

pub struct TunnelSupervisor {
    slots: Mutex<BTreeMap<String, Arc<Mutex<TunnelTaskSlot>>>>,
    statuses: Arc<RwLock<BTreeMap<String, TunnelRuntimeStatus>>>,
    store_path: PathBuf,
}

#[derive(Debug, Default)]
struct TunnelTaskSlot {
    generation: u64,
    desired_generation: u64,
    spec_id: Option<String>,
    handle: Option<tokio::task::JoinHandle<()>>,
    retiring_handles: Vec<tokio::task::JoinHandle<()>>,
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
            slots: Mutex::new(BTreeMap::new()),
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

    pub async fn ensure_running(
        self: &Arc<Self>,
        key: impl Into<String>,
        kind: impl Into<String>,
        local_addr: String,
        lease_fn: LeaseFn,
        activate_tunnel_fn: ActivateTunnelFn,
        tunnel_state_fn: TunnelStateFn,
        renew_lease_fn: RenewLeaseFn,
        reason: impl Into<String>,
        spec_id: impl Into<String>,
    ) -> TunnelStartOutcome {
        let key = key.into();
        let kind = kind.into();
        let reason = reason.into();
        let spec_id = spec_id.into();
        let slot = self.slot_for(&key).await;
        let mut slot = slot.lock().await;
        slot.retiring_handles.retain(|handle| !handle.is_finished());
        let mut replacement_generation = None;
        if let Some(handle) = slot.handle.as_ref() {
            if !handle.is_finished() && slot.spec_id.as_deref() == Some(spec_id.as_str()) {
                tracing::debug!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    generation = slot.generation,
                    reason = %reason,
                    "tunnel ensure reused running actor"
                );
                return TunnelStartOutcome::AlreadyRunning {
                    generation: slot.generation,
                };
            }
        }
        if let Some(handle) = slot.handle.take() {
            let operation = if slot.spec_id.as_deref() == Some(spec_id.as_str()) {
                "replace_finished"
            } else {
                "replace_changed_spec"
            };
            let next_generation = slot
                .generation
                .max(slot.desired_generation)
                .saturating_add(1);
            slot.desired_generation = next_generation;
            replacement_generation = Some(next_generation);
            self.set_transition_status(
                &key,
                &kind,
                next_generation,
                "reconnecting",
                TransportState::Draining,
                &reason,
                &spec_id,
            )
            .await;
            if handle.is_finished() {
                let _ = handle.await;
            } else {
                slot.retiring_handles.push(handle);
            }
            tracing::info!(
                tunnel_key = %key,
                tunnel_kind = %kind,
                generation = next_generation,
                reason = %reason,
                operation,
                "tunnel actor replacement requested"
            );
        }
        let generation = replacement_generation.unwrap_or_else(|| {
            slot.generation
                .max(slot.desired_generation)
                .saturating_add(1)
        });
        slot.generation = generation;
        slot.desired_generation = generation;
        slot.spec_id = Some(spec_id.clone());
        let (router_generation, router_active_generation) = self.next_router_generation(&key).await;
        self.set_starting_status(&key, &kind, generation, &reason, &spec_id)
            .await;
        slot.handle = Some(self.spawn_actor(
            key.clone(),
            kind.clone(),
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            generation,
            router_generation,
            router_active_generation,
            reason.clone(),
            spec_id,
        ));
        tracing::info!(
            tunnel_key = %key,
            tunnel_kind = %kind,
            generation,
            reason = %reason,
            operation = "ensure",
            "tunnel actor started"
        );
        TunnelStartOutcome::Started { generation }
    }

    pub async fn force_reconnect(
        self: &Arc<Self>,
        key: impl Into<String>,
        kind: impl Into<String>,
        local_addr: String,
        lease_fn: LeaseFn,
        activate_tunnel_fn: ActivateTunnelFn,
        tunnel_state_fn: TunnelStateFn,
        renew_lease_fn: RenewLeaseFn,
        reason: impl Into<String>,
        spec_id: impl Into<String>,
    ) -> TunnelStartOutcome {
        let key = key.into();
        let kind = kind.into();
        let reason = reason.into();
        let spec_id = spec_id.into();
        let slot = self.slot_for(&key).await;
        let mut slot = slot.lock().await;
        slot.retiring_handles.retain(|handle| !handle.is_finished());
        let generation = slot
            .generation
            .max(slot.desired_generation)
            .saturating_add(1);
        slot.desired_generation = generation;
        if let Some(handle) = slot.handle.take() {
            self.set_transition_status(
                &key,
                &kind,
                generation,
                "reconnecting",
                TransportState::Draining,
                &reason,
                &spec_id,
            )
            .await;
            if handle.is_finished() {
                let _ = handle.await;
            } else {
                slot.retiring_handles.push(handle);
            }
        }
        slot.generation = generation;
        slot.spec_id = Some(spec_id.clone());
        let (router_generation, router_active_generation) = self.next_router_generation(&key).await;
        self.set_starting_status(&key, &kind, generation, &reason, &spec_id)
            .await;
        slot.handle = Some(self.spawn_actor(
            key.clone(),
            kind.clone(),
            local_addr,
            lease_fn,
            activate_tunnel_fn,
            tunnel_state_fn,
            renew_lease_fn,
            generation,
            router_generation,
            router_active_generation,
            reason.clone(),
            spec_id,
        ));
        tracing::info!(
            tunnel_key = %key,
            tunnel_kind = %kind,
            generation,
            reason = %reason,
            operation = "force_reconnect",
            "tunnel actor replaced"
        );
        TunnelStartOutcome::Started { generation }
    }

    pub async fn stop(&self, key: &str, status: &str) {
        let slot = self.slot_for(key).await;
        let mut slot = slot.lock().await;
        let generation = slot
            .generation
            .max(slot.desired_generation)
            .saturating_add(1);
        slot.desired_generation = generation;
        let kind = self
            .statuses
            .read()
            .await
            .get(key)
            .map(|item| item.kind.clone())
            .unwrap_or_default();
        self.set_transition_status(
            key,
            &kind,
            generation,
            status,
            TransportState::Retired,
            "stop",
            slot.spec_id.as_deref().unwrap_or("unknown"),
        )
        .await;
        if let Some(handle) = slot.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        for handle in slot.retiring_handles.drain(..) {
            handle.abort();
            let _ = handle.await;
        }
        slot.generation = generation;
        tracing::info!(tunnel_key = %key, generation, status, "tunnel actor stopped");
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

    async fn slot_for(&self, key: &str) -> Arc<Mutex<TunnelTaskSlot>> {
        if let Some(slot) = self.slots.lock().await.get(key).cloned() {
            return slot;
        }
        let persisted_generation = self
            .statuses
            .read()
            .await
            .get(key)
            .map(|status| status.generation.max(status.desired_generation))
            .unwrap_or_default();
        let mut slots = self.slots.lock().await;
        slots
            .entry(key.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(TunnelTaskSlot {
                    generation: persisted_generation,
                    desired_generation: persisted_generation,
                    spec_id: None,
                    handle: None,
                    retiring_handles: Vec::new(),
                }))
            })
            .clone()
    }

    async fn set_starting_status(
        &self,
        key: &str,
        kind: &str,
        generation: u64,
        reason: &str,
        spec_id: &str,
    ) {
        let mut next = self.status(key).await.unwrap_or_default();
        next.key = key.to_string();
        next.kind = kind.to_string();
        next.status = "starting".to_string();
        next.generation = generation;
        next.desired_generation = generation;
        next.transport_state = Some(TransportState::Candidate.as_str().to_string());
        next.start_reason = Some(reason.to_string());
        next.spec_id = Some(spec_id.to_string());
        next.updated_at_ms = now_ms();
        self.set_status(next).await;
    }

    async fn next_router_generation(&self, key: &str) -> (u64, u64) {
        let status = self.status(key).await.unwrap_or_default();
        let active = status.router_active_generation;
        (
            status
                .router_generation
                .max(active)
                .saturating_add(1)
                .max(1),
            active,
        )
    }

    async fn set_transition_status(
        &self,
        key: &str,
        kind: &str,
        generation: u64,
        status: &str,
        transport_state: TransportState,
        reason: &str,
        spec_id: &str,
    ) {
        let mut next = self.status(key).await.unwrap_or_default();
        next.key = key.to_string();
        if !kind.is_empty() {
            next.kind = kind.to_string();
        }
        next.status = status.to_string();
        next.generation = generation;
        next.desired_generation = generation;
        next.transport_state = Some(transport_state.as_str().to_string());
        next.start_reason = Some(reason.to_string());
        next.spec_id = Some(spec_id.to_string());
        next.updated_at_ms = now_ms();
        self.set_status(next).await;
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_actor(
        &self,
        key: String,
        kind: String,
        local_addr: String,
        lease_fn: LeaseFn,
        activate_tunnel_fn: ActivateTunnelFn,
        tunnel_state_fn: TunnelStateFn,
        renew_lease_fn: RenewLeaseFn,
        generation: u64,
        router_generation: u64,
        router_active_generation: u64,
        reason: String,
        spec_id: String,
    ) -> tokio::task::JoinHandle<()> {
        let statuses = self.statuses.clone();
        let store_path = self.store_path.clone();
        tokio::spawn(async move {
            run_tunnel_actor(
                statuses,
                store_path,
                key,
                kind,
                local_addr,
                lease_fn,
                activate_tunnel_fn,
                tunnel_state_fn,
                renew_lease_fn,
                generation,
                router_generation,
                router_active_generation,
                reason,
                spec_id,
            )
            .await;
        })
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

#[allow(clippy::too_many_arguments)]
async fn run_tunnel_actor(
    statuses: Arc<RwLock<BTreeMap<String, TunnelRuntimeStatus>>>,
    store_path: PathBuf,
    key: String,
    kind: String,
    local_addr: String,
    lease_fn: LeaseFn,
    activate_tunnel_fn: ActivateTunnelFn,
    tunnel_state_fn: TunnelStateFn,
    renew_lease_fn: RenewLeaseFn,
    generation: u64,
    mut router_generation: u64,
    mut router_active_generation: u64,
    reason: String,
    spec_id: String,
) {
    let mut retry_cap = INITIAL_RECONNECT_BACKOFF;
    let mut rotation_id = new_rotation_id();
    loop {
        if actor_generation_is_stale(&statuses, &key, generation).await {
            tracing::debug!(
                tunnel_key = %key,
                tunnel_kind = %kind,
                generation,
                "retiring tunnel actor stopped after replacement"
            );
            break;
        }
        set_status_for_generation(
            &statuses,
            &store_path,
            TunnelRuntimeStatus {
                key: key.clone(),
                kind: kind.clone(),
                status: "leasing".to_string(),
                generation,
                desired_generation: generation,
                router_generation,
                router_active_generation,
                transport_state: Some(TransportState::Candidate.as_str().to_string()),
                start_reason: Some(reason.clone()),
                spec_id: Some(spec_id.clone()),
                updated_at_ms: now_ms(),
                ..TunnelRuntimeStatus::default()
            },
        )
        .await;

        let attempt_started = tokio::time::Instant::now();
        let lease_request = TunnelLeaseRequest {
            route_id: key.clone(),
            rotation_id: rotation_id.clone(),
            generation: router_generation,
            expected_generation: router_active_generation,
        };
        let lease = match tokio::time::timeout(LEASE_ISSUE_TIMEOUT, lease_fn(lease_request)).await {
            Ok(Ok(lease)) => lease,
            Ok(Err(error)) => {
                let error_message = error.to_string();
                let previous_router_generation = router_generation;
                let previous_router_active_generation = router_active_generation;
                (router_generation, router_active_generation) =
                    sync_router_generations_after_lease_error(
                        &error_message,
                        router_generation,
                        router_active_generation,
                    );
                if router_generation != previous_router_generation
                    || router_active_generation != previous_router_active_generation
                    || lease_issue_requires_new_rotation(&error)
                {
                    rotation_id = new_rotation_id();
                }
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    TunnelRuntimeStatus {
                        key: key.clone(),
                        kind: kind.clone(),
                        status: "retrying".to_string(),
                        last_error: Some(error_message),
                        generation,
                        desired_generation: generation,
                        router_generation,
                        router_active_generation,
                        transport_state: Some(TransportState::Candidate.as_str().to_string()),
                        start_reason: Some(reason.clone()),
                        spec_id: Some(spec_id.clone()),
                        updated_at_ms: now_ms(),
                        ..TunnelRuntimeStatus::default()
                    },
                )
                .await;
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    generation,
                    reason = %reason,
                    error = %error,
                    "tunnel lease attempt failed"
                );
                sleep_with_full_jitter(retry_cap).await;
                retry_cap = next_backoff(retry_cap);
                continue;
            }
            Err(_) => {
                let error = format!(
                    "router tunnel lease request timed out after {}s",
                    LEASE_ISSUE_TIMEOUT.as_secs()
                );
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    TunnelRuntimeStatus {
                        key: key.clone(),
                        kind: kind.clone(),
                        status: "retrying".to_string(),
                        last_error: Some(error.clone()),
                        generation,
                        desired_generation: generation,
                        transport_state: Some(TransportState::Candidate.as_str().to_string()),
                        start_reason: Some(reason.clone()),
                        spec_id: Some(spec_id.clone()),
                        updated_at_ms: now_ms(),
                        ..TunnelRuntimeStatus::default()
                    },
                )
                .await;
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    generation,
                    reason = %reason,
                    timeout_seconds = LEASE_ISSUE_TIMEOUT.as_secs(),
                    "tunnel lease attempt timed out"
                );
                sleep_with_full_jitter(retry_cap).await;
                retry_cap = next_backoff(retry_cap);
                continue;
            }
        };

        set_status_for_generation(
            &statuses,
            &store_path,
            status_from_lease(
                &key,
                &kind,
                "connecting",
                &lease,
                None,
                generation,
                &reason,
                &spec_id,
                TransportState::Candidate,
            ),
        )
        .await;

        let outcome = connect_and_forward(
            &lease,
            &local_addr,
            &statuses,
            &store_path,
            &key,
            &kind,
            &activate_tunnel_fn,
            &tunnel_state_fn,
            &renew_lease_fn,
            generation,
            &reason,
            &spec_id,
        )
        .await;
        let active_long_enough = attempt_started.elapsed() >= STABLE_CONNECTION_WINDOW;
        match outcome {
            Ok(TunnelConnectionEnd::Ended) => {
                router_active_generation = lease.generation;
                router_generation = lease.generation.saturating_add(1);
                rotation_id = new_rotation_id();
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    status_from_lease(
                        &key,
                        &kind,
                        "retrying",
                        &lease,
                        Some("router ssh forward ended".to_string()),
                        generation,
                        &reason,
                        &spec_id,
                        TransportState::Draining,
                    ),
                )
                .await;
            }
            Ok(TunnelConnectionEnd::ReplaceRequired(error)) => {
                router_active_generation = lease.generation;
                router_generation = lease.generation.saturating_add(1);
                rotation_id = new_rotation_id();
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    status_from_lease(
                        &key,
                        &kind,
                        "replacement_required",
                        &lease,
                        Some(error.clone()),
                        generation,
                        &reason,
                        &spec_id,
                        TransportState::Draining,
                    ),
                )
                .await;
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    generation,
                    error = %error,
                    "router requires a replacement tunnel lease"
                );
            }
            Ok(TunnelConnectionEnd::FatalConfiguration(error)) => {
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    status_from_lease(
                        &key,
                        &kind,
                        "configuration_error",
                        &lease,
                        Some(error.clone()),
                        generation,
                        &reason,
                        &spec_id,
                        TransportState::Retired,
                    ),
                )
                .await;
                tracing::error!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    generation,
                    error = %error,
                    "tunnel actor stopped on fatal configuration error"
                );
                break;
            }
            Err(error) => {
                set_status_for_generation(
                    &statuses,
                    &store_path,
                    status_from_lease(
                        &key,
                        &kind,
                        "retrying",
                        &lease,
                        Some(error.to_string()),
                        generation,
                        &reason,
                        &spec_id,
                        TransportState::Candidate,
                    ),
                )
                .await;
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    actor_generation = generation,
                    lease_generation = lease.generation,
                    expected_generation = lease.expected_generation,
                    error = %format!("{error:#}"),
                    "tunnel connection cycle failed"
                );
            }
        }

        retry_cap = if active_long_enough {
            INITIAL_RECONNECT_BACKOFF
        } else {
            next_backoff(retry_cap)
        };
        if actor_generation_is_stale(&statuses, &key, generation).await {
            break;
        }
        sleep_with_full_jitter(retry_cap).await;
    }
}

async fn actor_generation_is_stale(
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    key: &str,
    generation: u64,
) -> bool {
    statuses
        .read()
        .await
        .get(key)
        .is_some_and(|status| status.desired_generation != generation)
}

async fn set_status(
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    status: TunnelRuntimeStatus,
) {
    let mut statuses = statuses.write().await;
    statuses.insert(status.key.clone(), status);
    persist_statuses(store_path, &statuses);
}

async fn set_status_for_generation(
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    status: TunnelRuntimeStatus,
) -> bool {
    let mut statuses = statuses.write().await;
    if statuses
        .get(&status.key)
        .is_some_and(|current| current.desired_generation != status.generation)
    {
        return false;
    }
    statuses.insert(status.key.clone(), status);
    persist_statuses(store_path, &statuses);
    true
}

fn status_from_lease(
    key: &str,
    kind: &str,
    status: &str,
    lease: &NamespaceLeaseResponse,
    error: Option<String>,
    generation: u64,
    start_reason: &str,
    spec_id: &str,
    transport_state: TransportState,
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
        generation,
        desired_generation: generation,
        router_generation: lease.generation,
        router_active_generation: match transport_state {
            TransportState::Active | TransportState::Draining | TransportState::Retired => {
                lease.generation
            }
            TransportState::Candidate => lease.expected_generation,
        },
        transport_state: Some(transport_state.as_str().to_string()),
        start_reason: Some(start_reason.to_string()),
        spec_id: Some(spec_id.to_string()),
        updated_at_ms: now_ms(),
        ..TunnelRuntimeStatus::default()
    }
}

async fn connect_and_forward(
    lease: &NamespaceLeaseResponse,
    local_addr: &str,
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    key: &str,
    kind: &str,
    activate_tunnel_fn: &ActivateTunnelFn,
    tunnel_state_fn: &TunnelStateFn,
    renew_lease_fn: &RenewLeaseFn,
    generation: u64,
    start_reason: &str,
    spec_id: &str,
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
    let mut handle = tokio::time::timeout(
        SSH_CONNECT_TIMEOUT,
        client::connect(ssh_config, &lease.ssh_addr, handler),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "router ssh connection timed out after {}s",
            SSH_CONNECT_TIMEOUT.as_secs()
        )
    })??;
    let auth_ok = tokio::time::timeout(
        SSH_AUTH_TIMEOUT,
        handle.authenticate_password(&lease.ssh_username, &lease.ssh_password),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "router ssh authentication timed out after {}s",
            SSH_AUTH_TIMEOUT.as_secs()
        )
    })??;
    if !auth_ok {
        anyhow::bail!("router ssh authentication failed");
    }

    let result = async {
        let remote_port =
            tokio::time::timeout(REMOTE_FORWARD_TIMEOUT, request_forward(&mut handle))
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "router remote forward request timed out after {}s",
                        REMOTE_FORWARD_TIMEOUT.as_secs()
                    )
                })??;

        // Router activation probes the candidate through this reverse forward.
        // Keep the pump live while control-plane activation is in flight.
        let forward = accept_loop(fwd_rx, local_addr);
        tokio::pin!(forward);
        let activation = poll_control_with_forward(
            forward.as_mut(),
            tokio::time::timeout(TUNNEL_CONTROL_TIMEOUT, activate_tunnel_fn(lease.clone())),
            "tunnel forward ended before activation completed",
        )
        .await?;
        let activation_confirmed = match activation {
            Ok(Ok(state)) => tunnel_state_is_active(&state, lease),
            Ok(Err(error)) => {
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    actor_generation = generation,
                    lease_generation = lease.generation,
                    expected_generation = lease.expected_generation,
                    error = %format!("{error:#}"),
                    "tunnel activation failed; checking authoritative state"
                );
                query_tunnel_activation_state(forward.as_mut(), tunnel_state_fn, lease).await?
            }
            Err(_) => {
                tracing::warn!(
                    tunnel_key = %key,
                    tunnel_kind = %kind,
                    actor_generation = generation,
                    lease_generation = lease.generation,
                    expected_generation = lease.expected_generation,
                    timeout_seconds = TUNNEL_CONTROL_TIMEOUT.as_secs(),
                    "tunnel activation timed out; checking authoritative state"
                );
                query_tunnel_activation_state(forward.as_mut(), tunnel_state_fn, lease).await?
            }
        };
        if !activation_confirmed {
            anyhow::bail!(
                "router did not confirm lease generation {} as active (expected active generation {})",
                lease.generation,
                lease.expected_generation
            );
        }

        let mut connected = status_from_lease(
            key,
            kind,
            "connected",
            lease,
            None,
            generation,
            start_reason,
            spec_id,
            TransportState::Active,
        );
        connected.remote_port = Some(remote_port);
        connected.connected_at_ms = Some(now_ms());
        set_status_for_generation(statuses, store_path, connected).await;

        maintain_forward_with_pump(
            forward.as_mut(),
            lease,
            remote_port,
            statuses,
            store_path,
            key,
            kind,
            renew_lease_fn,
            generation,
            start_reason,
            spec_id,
        )
        .await
    }
    .await;
    let _ = tokio::time::timeout(
        SSH_DISCONNECT_TIMEOUT,
        handle.disconnect(Disconnect::ByApplication, "", "en"),
    )
    .await;
    result
}

async fn query_tunnel_activation_state(
    forward: Pin<&mut impl Future<Output = anyhow::Result<()>>>,
    tunnel_state_fn: &TunnelStateFn,
    lease: &NamespaceLeaseResponse,
) -> anyhow::Result<bool> {
    let state = poll_control_with_forward(
        forward,
        tokio::time::timeout(TUNNEL_CONTROL_TIMEOUT, tunnel_state_fn(lease.clone())),
        "tunnel forward ended before activation state was confirmed",
    )
    .await?
    .map_err(|_| {
        anyhow::anyhow!(
            "router tunnel state request timed out after {}s",
            TUNNEL_CONTROL_TIMEOUT.as_secs()
        )
    })??;
    Ok(tunnel_state_is_active(&state, lease))
}

async fn poll_control_with_forward<F, C, T>(
    mut forward: Pin<&mut F>,
    control: C,
    forward_ended_message: &'static str,
) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<()>>,
    C: Future<Output = T>,
{
    tokio::pin!(control);
    tokio::select! {
        result = forward.as_mut() => {
            result?;
            anyhow::bail!(forward_ended_message)
        }
        result = &mut control => Ok(result),
    }
}

fn tunnel_state_is_active(state: &TunnelStateResponse, lease: &NamespaceLeaseResponse) -> bool {
    state.protocol_epoch == lease.protocol_epoch
        && state.router_id == lease.router_id
        && state.route_id == lease.route_id
        && state.rotation_id == lease.rotation_id
        && state.generation == lease.generation
        && state.expected_generation == lease.expected_generation
        && state.state == "active"
        && state.active_generation == Some(lease.generation)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn maintain_forward(
    fwd_rx: mpsc::UnboundedReceiver<Channel<client::Msg>>,
    local_addr: &str,
    lease: &NamespaceLeaseResponse,
    remote_port: u16,
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    key: &str,
    kind: &str,
    renew_lease_fn: &RenewLeaseFn,
    generation: u64,
    start_reason: &str,
    spec_id: &str,
) -> anyhow::Result<TunnelConnectionEnd> {
    let accept = accept_loop(fwd_rx, local_addr);
    tokio::pin!(accept);
    maintain_forward_with_pump(
        accept.as_mut(),
        lease,
        remote_port,
        statuses,
        store_path,
        key,
        kind,
        renew_lease_fn,
        generation,
        start_reason,
        spec_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn maintain_forward_with_pump<F>(
    mut accept: Pin<&mut F>,
    lease: &NamespaceLeaseResponse,
    remote_port: u16,
    statuses: &RwLock<BTreeMap<String, TunnelRuntimeStatus>>,
    store_path: &Path,
    key: &str,
    kind: &str,
    renew_lease_fn: &RenewLeaseFn,
    generation: u64,
    start_reason: &str,
    spec_id: &str,
) -> anyhow::Result<TunnelConnectionEnd>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let connected_at_ms = now_ms();
    let mut active_lease = lease.clone();
    let mut retry_delay = None;
    let mut retry_cap = INITIAL_RECONNECT_BACKOFF;

    loop {
        let delay = match retry_delay.take() {
            Some(delay) => delay,
            None => renewal_delay(&active_lease.expires_at)
                .ok_or_else(|| anyhow::anyhow!("router lease expiration is invalid"))?,
        };
        tokio::select! {
            result = accept.as_mut() => return result.map(|_| TunnelConnectionEnd::Ended),
            _ = tokio::time::sleep(delay) => {}
        }

        let mut renewing = status_from_lease(
            key,
            kind,
            "renewing",
            &active_lease,
            None,
            generation,
            start_reason,
            spec_id,
            TransportState::Active,
        );
        renewing.remote_port = Some(remote_port);
        renewing.connected_at_ms = Some(connected_at_ms);
        set_status_for_generation(statuses, store_path, renewing).await;

        let renewal = renew_lease_fn(active_lease.clone());
        tokio::pin!(renewal);
        let renewed = tokio::select! {
            result = accept.as_mut() => return result.map(|_| TunnelConnectionEnd::Ended),
            result = tokio::time::timeout(LEASE_RENEW_TIMEOUT, &mut renewal) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(TunnelRenewalError::Retryable(format!(
                        "router tunnel lease renewal timed out after {}s",
                        LEASE_RENEW_TIMEOUT.as_secs()
                    ))),
                }
            },
        };
        match renewed {
            Ok(expires_at) => {
                active_lease.expires_at = expires_at;
                retry_cap = INITIAL_RECONNECT_BACKOFF;
                let mut connected = status_from_lease(
                    key,
                    kind,
                    "connected",
                    &active_lease,
                    None,
                    generation,
                    start_reason,
                    spec_id,
                    TransportState::Active,
                );
                connected.remote_port = Some(remote_port);
                connected.connected_at_ms = Some(connected_at_ms);
                set_status_for_generation(statuses, store_path, connected).await;
            }
            Err(TunnelRenewalError::Retryable(error)) => {
                let mut retrying = status_from_lease(
                    key,
                    kind,
                    "renewal_retrying",
                    &active_lease,
                    Some(error),
                    generation,
                    start_reason,
                    spec_id,
                    TransportState::Active,
                );
                retrying.remote_port = Some(remote_port);
                retrying.connected_at_ms = Some(connected_at_ms);
                set_status_for_generation(statuses, store_path, retrying).await;
                retry_delay = Some(full_jitter_delay(retry_cap));
                retry_cap = (retry_cap * 2).min(LEASE_RENEW_RETRY_MAX_DELAY);
            }
            Err(TunnelRenewalError::ReplaceRequired(error)) => {
                return Ok(TunnelConnectionEnd::ReplaceRequired(error));
            }
            Err(TunnelRenewalError::FatalConfiguration(error)) => {
                return Ok(TunnelConnectionEnd::FatalConfiguration(error));
            }
        }
    }
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
    let mut bridges = JoinSet::new();
    loop {
        tokio::select! {
            channel = fwd_rx.recv() => {
                let Some(channel) = channel else {
                    while bridges.join_next().await.is_some() {}
                    return Ok(());
                };
                let local_addr = local_addr.to_string();
                bridges.spawn(async move {
                    let stream = channel.into_stream();
                    if let Err(error) = forward_tcp(stream, &local_addr).await {
                        tracing::debug!(error = %error, "tunnel tcp forward failed");
                    }
                });
            }
            result = bridges.join_next(), if !bridges.is_empty() => {
                if let Some(Err(error)) = result {
                    tracing::debug!(error = %error, "tunnel tcp bridge task failed");
                }
            }
        }
    }
}

async fn forward_tcp<S>(mut remote: S, local_addr: &str) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut local = tokio::time::timeout(LOCAL_CONNECT_TIMEOUT, TcpStream::connect(local_addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "local tunnel connect timed out"))??;
    io::copy_bidirectional(&mut remote, &mut local).await?;
    Ok(())
}

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_RECONNECT_BACKOFF)
}

fn lease_issue_message_requires_new_rotation(message: &str) -> bool {
    message.contains("rotationId belongs to an expired or retired lease")
        || message.contains("generation must be newer than persisted generation")
        || message.contains("route already has a non-expired candidate rotation")
        || message.contains("route generation changed: expected ")
}

fn lease_issue_requires_new_rotation(error: &anyhow::Error) -> bool {
    lease_issue_message_requires_new_rotation(&error.to_string())
}

fn parse_route_active_generation_from_lease_error(message: &str) -> Option<u64> {
    message
        .split("route generation changed: expected ")
        .nth(1)?
        .split(", active ")
        .nth(1)?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn sync_router_generations_after_lease_error(
    message: &str,
    router_generation: u64,
    router_active_generation: u64,
) -> (u64, u64) {
    if let Some(active_generation) = parse_route_active_generation_from_lease_error(message) {
        return (
            active_generation.saturating_add(1).max(router_generation),
            active_generation,
        );
    }
    if lease_issue_message_requires_new_rotation(message) {
        return (
            next_router_generation_after_lease_error(message, router_generation),
            router_active_generation,
        );
    }
    (router_generation, router_active_generation)
}

fn next_router_generation_after_lease_error(message: &str, current: u64) -> u64 {
    if let Some(rest) = message.strip_prefix("generation must be newer than persisted generation ")
    {
        if let Ok(max_generation) = rest.trim().parse::<u64>() {
            return max_generation.saturating_add(1).max(current);
        }
    }
    current.saturating_add(1).max(1)
}

fn new_rotation_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn full_jitter_delay(cap: Duration) -> Duration {
    if cap.is_zero() {
        return Duration::ZERO;
    }
    let max_millis = cap.as_millis().min(u64::MAX as u128) as u64;
    Duration::from_millis(rand::thread_rng().gen_range(0..=max_millis))
}

async fn sleep_with_full_jitter(cap: Duration) {
    tokio::time::sleep(full_jitter_delay(cap)).await;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum TunnelConnectionEnd {
    Ended,
    ReplaceRequired(String),
    FatalConfiguration(String),
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

    // Without an issued-at field, half of the current remaining lifetime is the
    // earliest stable renewal point available to the client.
    let delay = Duration::from_secs((remaining.as_secs() / 2).max(1));
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
    if let Err(error) = crate::infra::storage::write_json_pretty(store_path, &store) {
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
                tracing::debug!(ssh_addr = %self.ssh_addr, fingerprint = %actual, "router ssh host key verified");
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_config_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cc-switch-server-{name}-{nanos}"))
    }

    fn test_lease(expires_at: String) -> NamespaceLeaseResponse {
        NamespaceLeaseResponse {
            protocol_epoch: "namespace-flat-1".into(),
            router_id: "router.example.test".into(),
            lease_id: "lease-1".into(),
            connection_id: "connection-1".into(),
            route_id: "client-web".into(),
            rotation_id: "client-web:1".into(),
            generation: 1,
            expected_generation: 0,
            ssh_username: "connection-1".into(),
            ssh_password: "password".into(),
            ssh_addr: "127.0.0.1:22".into(),
            expires_at,
            tunnel_url: "https://client.example.test".into(),
            subdomain: "client".into(),
            ssh_host_fingerprint: None,
        }
    }

    fn pending_lease_fn(calls: Arc<AtomicUsize>) -> LeaseFn {
        Arc::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        })
    }

    fn pending_renew_fn() -> RenewLeaseFn {
        Arc::new(|_| Box::pin(std::future::pending()))
    }

    fn pending_tunnel_control_fn() -> ActivateTunnelFn {
        Arc::new(|_| Box::pin(std::future::pending()))
    }

    #[tokio::test]
    async fn activation_control_poll_drives_the_candidate_forward_pump() {
        let pump_polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pump_polled_by_forward = pump_polled.clone();
        let forward = std::future::poll_fn(move |_| {
            pump_polled_by_forward.store(true, Ordering::SeqCst);
            Poll::<anyhow::Result<()>>::Pending
        });
        tokio::pin!(forward);
        let pump_polled_by_control = pump_polled.clone();
        let control = async move {
            while !pump_polled_by_control.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            42_u8
        };

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            poll_control_with_forward(forward.as_mut(), control, "forward ended before activation"),
        )
        .await
        .expect("activation must not deadlock waiting for the forward pump")
        .expect("forward remains available");

        assert_eq!(result, 42);
        assert!(pump_polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn activation_control_fails_if_the_forward_pump_ends_first() {
        let forward = async { anyhow::bail!("candidate bridge failed") };
        tokio::pin!(forward);

        let error = poll_control_with_forward(
            forward.as_mut(),
            std::future::pending::<()>(),
            "forward ended before activation",
        )
        .await
        .expect_err("a dead candidate must not be activated");

        assert!(error.to_string().contains("candidate bridge failed"));
    }

    #[test]
    fn authoritative_active_state_recovers_a_lost_activation_response() {
        let lease = test_lease("2099-01-01T00:00:00Z".to_string());
        let mut state = TunnelStateResponse {
            protocol_epoch: lease.protocol_epoch.clone(),
            router_id: lease.router_id.clone(),
            route_id: lease.route_id.clone(),
            rotation_id: lease.rotation_id.clone(),
            generation: lease.generation,
            expected_generation: lease.expected_generation,
            state: "active".into(),
            active_generation: Some(lease.generation),
            candidate_generations: Vec::new(),
            draining_generations: Vec::new(),
        };

        assert!(tunnel_state_is_active(&state, &lease));
        state.active_generation = Some(lease.generation + 1);
        assert!(!tunnel_state_is_active(&state, &lease));
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
        assert_eq!(status.transport_state.as_deref(), Some("retired"));
    }

    #[tokio::test]
    async fn concurrent_ensure_creates_one_actor_and_one_lease_attempt() {
        let dir = temp_config_dir("tunnel-concurrent-ensure");
        let supervisor = TunnelSupervisor::load_or_default(&dir).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let lease = pending_lease_fn(calls.clone());
        let renew = pending_renew_fn();
        let activate = pending_tunnel_control_fn();
        let state = pending_tunnel_control_fn();
        let mut ensures = JoinSet::new();

        for index in 0..100 {
            let supervisor = supervisor.clone();
            let lease = lease.clone();
            let renew = renew.clone();
            let activate = activate.clone();
            let state = state.clone();
            ensures.spawn(async move {
                supervisor
                    .ensure_running(
                        "share:concurrent",
                        "share-http",
                        "127.0.0.1:9".to_string(),
                        lease,
                        activate,
                        state,
                        renew,
                        format!("ensure-{index}"),
                        "share-spec",
                    )
                    .await
            });
        }

        let mut started = 0;
        while let Some(result) = ensures.join_next().await {
            if result.unwrap().started() {
                started += 1;
            }
        }
        tokio::task::yield_now().await;

        assert_eq!(started, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let status = supervisor.status("share:concurrent").await.unwrap();
        assert_eq!(status.generation, 1);
        assert_eq!(status.desired_generation, 1);
        supervisor.stop("share:concurrent", "stopped").await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn force_reconnect_advances_generation_and_fences_stale_status() {
        let dir = temp_config_dir("tunnel-generation-fence");
        let supervisor = TunnelSupervisor::load_or_default(&dir).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let lease = pending_lease_fn(calls.clone());
        let renew = pending_renew_fn();
        let activate = pending_tunnel_control_fn();
        let state = pending_tunnel_control_fn();
        supervisor
            .ensure_running(
                "share:fenced",
                "share-http",
                "127.0.0.1:9".to_string(),
                lease.clone(),
                activate.clone(),
                state.clone(),
                renew.clone(),
                "initial",
                "share-spec-v1",
            )
            .await;
        tokio::task::yield_now().await;
        supervisor
            .force_reconnect(
                "share:fenced",
                "share-http",
                "127.0.0.1:9".to_string(),
                lease,
                activate,
                state,
                renew,
                "configuration_changed",
                "share-spec-v2",
            )
            .await;
        tokio::task::yield_now().await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let current = supervisor.status("share:fenced").await.unwrap();
        assert_eq!(current.generation, 2);
        let accepted = set_status_for_generation(
            &supervisor.statuses,
            &supervisor.store_path,
            TunnelRuntimeStatus {
                key: "share:fenced".to_string(),
                kind: "share-http".to_string(),
                status: "connected".to_string(),
                generation: 1,
                desired_generation: 1,
                updated_at_ms: now_ms(),
                ..TunnelRuntimeStatus::default()
            },
        )
        .await;
        assert!(!accepted);
        assert_eq!(
            supervisor.status("share:fenced").await.unwrap().generation,
            2
        );
        supervisor.stop("share:fenced", "stopped").await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renewal_delay_is_zero_only_for_imminent_expiry() {
        let soon = (Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
        assert_eq!(renewal_delay(&soon), Some(Duration::from_secs(0)));
    }

    #[test]
    fn terminal_renewal_errors_distinguish_replacement_from_configuration() {
        let replacement = TunnelRenewalError::from_router(
            RenewLeaseError::Terminal("renew failed: 409 Conflict".into()),
            true,
        );
        let fatal = TunnelRenewalError::from_router(
            RenewLeaseError::Terminal("renew failed: 401 Unauthorized".into()),
            true,
        );
        let missing_config = TunnelRenewalError::from_router(
            RenewLeaseError::Terminal("router api base is not configured".into()),
            false,
        );

        assert!(matches!(
            replacement,
            TunnelRenewalError::ReplaceRequired(_)
        ));
        assert!(matches!(fatal, TunnelRenewalError::FatalConfiguration(_)));
        assert!(matches!(
            missing_config,
            TunnelRenewalError::FatalConfiguration(_)
        ));
    }

    #[test]
    fn renewal_delay_keeps_short_router_leases_connected() {
        let future = (Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        let delay = renewal_delay(&future).unwrap();
        assert!(delay <= Duration::from_secs(31));
        assert!(delay >= Duration::from_secs(28));
    }

    #[test]
    fn renewal_delay_is_before_expiry() {
        let future = (Utc::now() + chrono::Duration::seconds(180)).to_rfc3339();
        let delay = renewal_delay(&future).unwrap();
        assert!(delay <= Duration::from_secs(91));
        assert!(delay >= Duration::from_secs(88));
    }

    #[tokio::test]
    async fn successful_renewal_keeps_same_forward_pending() {
        let dir = temp_config_dir("tunnel-renew-success");
        fs::create_dir_all(&dir).unwrap();
        let store_path = dir.join(TUNNELS_FILE_NAME);
        let statuses = RwLock::new(BTreeMap::new());
        let lease = test_lease((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_renewal = calls.clone();
        let renew: RenewLeaseFn = Arc::new(move |lease| {
            assert_eq!(lease.lease_id, "lease-1");
            assert_eq!(lease.connection_id, "connection-1");
            calls_for_renewal.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok((Utc::now() + chrono::Duration::seconds(60)).to_rfc3339()) })
        });
        let (_forward_tx, forward_rx) = mpsc::unbounded_channel();

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            maintain_forward(
                forward_rx,
                "127.0.0.1:9",
                &lease,
                23456,
                &statuses,
                &store_path,
                "client-web",
                "client-web",
                &renew,
                1,
                "test",
                "test-spec",
            ),
        )
        .await;

        assert!(result.is_err(), "forward must remain active after renewal");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let status = statuses.read().await.get("client-web").cloned().unwrap();
        assert_eq!(status.status, "connected");
        assert_eq!(status.connection_id.as_deref(), Some("connection-1"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn retryable_renewal_error_keeps_same_forward_pending() {
        let dir = temp_config_dir("tunnel-renew-retry");
        fs::create_dir_all(&dir).unwrap();
        let store_path = dir.join(TUNNELS_FILE_NAME);
        let statuses = RwLock::new(BTreeMap::new());
        let lease = test_lease((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let renew: RenewLeaseFn = Arc::new(|_| {
            Box::pin(async { Err(TunnelRenewalError::Retryable("router unavailable".into())) })
        });
        let (_forward_tx, forward_rx) = mpsc::unbounded_channel();

        let result = tokio::time::timeout(
            Duration::from_millis(100),
            maintain_forward(
                forward_rx,
                "127.0.0.1:9",
                &lease,
                23456,
                &statuses,
                &store_path,
                "client-web",
                "client-web",
                &renew,
                1,
                "test",
                "test-spec",
            ),
        )
        .await;

        assert!(
            result.is_err(),
            "transient failure must not end the forward"
        );
        let status = statuses.read().await.get("client-web").cloned().unwrap();
        assert_eq!(status.status, "renewal_retrying");
        assert_eq!(status.last_error.as_deref(), Some("router unavailable"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn replacement_required_renewal_ends_forward_for_fallback_reconnect() {
        let dir = temp_config_dir("tunnel-renew-terminal");
        fs::create_dir_all(&dir).unwrap();
        let store_path = dir.join(TUNNELS_FILE_NAME);
        let statuses = RwLock::new(BTreeMap::new());
        let lease = test_lease((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
        let renew: RenewLeaseFn = Arc::new(|_| {
            Box::pin(async {
                Err(TunnelRenewalError::ReplaceRequired(
                    "lease not found".into(),
                ))
            })
        });
        let (_forward_tx, forward_rx) = mpsc::unbounded_channel();

        let outcome = maintain_forward(
            forward_rx,
            "127.0.0.1:9",
            &lease,
            23456,
            &statuses,
            &store_path,
            "client-web",
            "client-web",
            &renew,
            1,
            "test",
            "test-spec",
        )
        .await
        .expect("replacement rejection should be a classified outcome");

        assert_eq!(
            outcome,
            TunnelConnectionEnd::ReplaceRequired("lease not found".into())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn next_router_generation_after_lease_error_reads_persisted_max() {
        assert_eq!(
            next_router_generation_after_lease_error(
                "generation must be newer than persisted generation 7",
                3,
            ),
            8,
        );
        assert_eq!(
            next_router_generation_after_lease_error(
                "route already has a non-expired candidate rotation",
                4,
            ),
            5,
        );
    }

    #[test]
    fn sync_router_generations_after_route_generation_changed_error() {
        let message = "router namespace tunnel lease failed: 409 Conflict: {\"message\":\"route generation changed: expected 6, active 0\"}";
        assert_eq!(
            sync_router_generations_after_lease_error(message, 7, 6),
            (7, 0),
        );
        assert_eq!(
            sync_router_generations_after_lease_error(message, 2, 6),
            (2, 0),
        );
    }

    #[test]
    fn sync_router_generations_after_persisted_generation_error() {
        assert_eq!(
            sync_router_generations_after_lease_error(
                "generation must be newer than persisted generation 5",
                2,
                4,
            ),
            (6, 4),
        );
    }
}
