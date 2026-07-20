use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex};
use tracing::warn;

use crate::self_update::restart::schedule_upgrade_restart;
use crate::self_update::version::{
    backup_installed_binary, commits_equal, fetch_release_checksum,
    release_binary_url_for_cache_key, request_release_asset, SelfUpdateError, BINARY_ROLLBACK_PATH,
    BINARY_STAGING_PATH,
};

const LOG_CHANNEL_CAPACITY: usize = 256;
const TOTAL_STEPS: usize = 7;
const DOWNLOAD_PROGRESS_PERCENT_STEP: u8 = 20;
const DOWNLOAD_PROGRESS_UNKNOWN_BYTES: u64 = 8 * 1024 * 1024;
const SANITY_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const STATE_FILENAME: &str = "upgrade-state.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeLogLevel {
    Info,
    Progress,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeLogEntry {
    pub task_id: String,
    pub step: usize,
    pub total_steps: usize,
    pub level: UpgradeLogLevel,
    pub message: String,
    pub progress: Option<u8>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStatusSnapshot {
    pub task_id: String,
    pub status: UpgradeStatus,
    pub restart_pending: bool,
    pub logs: Vec<UpgradeLogEntry>,
    #[serde(default)]
    pub target_commit_id: Option<String>,
    #[serde(default)]
    pub restart_after: bool,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Clone)]
pub struct UpgradeHandle {
    pub task_id: String,
    pub status: Arc<Mutex<UpgradeStatus>>,
    pub sender: broadcast::Sender<UpgradeLogEntry>,
    pub history: Arc<Mutex<Vec<UpgradeLogEntry>>>,
    pub restart_pending: Arc<Mutex<bool>>,
    target_commit_id: Arc<Mutex<Option<String>>>,
    restart_after: bool,
}

pub struct UpgradeRegistry {
    inner: Mutex<Option<UpgradeHandle>>,
    state_path: PathBuf,
}

impl UpgradeRegistry {
    pub fn load(config_dir: &Path) -> Result<Self, SelfUpdateError> {
        let state_path = config_dir.join(STATE_FILENAME);
        let handle = if state_path.exists() {
            match std::fs::read(&state_path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<UpgradeStatusSnapshot>(&bytes)
                        .map_err(|error| error.to_string())
                }) {
                Ok(mut snapshot) => {
                    reconcile_snapshot_with_running_binary(&mut snapshot);
                    if let Err(error) = write_snapshot_atomic(&state_path, &snapshot) {
                        warn!(error = %error, "persist reconciled upgrade state failed");
                    }
                    Some(handle_from_snapshot(snapshot))
                }
                Err(error) => {
                    warn!(path = %state_path.display(), error = %error, "ignoring unreadable upgrade state");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            inner: Mutex::new(handle),
            state_path,
        })
    }

    pub async fn is_restart_pending(&self) -> bool {
        if let Some(handle) = self.inner.lock().await.as_ref() {
            return *handle.restart_pending.lock().await;
        }
        false
    }

    pub async fn start(
        self: &Arc<Self>,
        client: reqwest::Client,
        actor: Option<String>,
        restart_after: bool,
        force: bool,
        health_addr: SocketAddr,
    ) -> Result<UpgradeHandle, SelfUpdateError> {
        let mut guard = self.inner.lock().await;
        if let Some(handle) = guard.as_ref() {
            if matches!(*handle.status.lock().await, UpgradeStatus::Running) {
                return Ok(handle.clone());
            }
        }
        let task_id = new_task_id();
        let (sender, _receiver) = broadcast::channel(LOG_CHANNEL_CAPACITY);
        let handle = UpgradeHandle {
            task_id,
            status: Arc::new(Mutex::new(UpgradeStatus::Running)),
            sender,
            history: Arc::new(Mutex::new(Vec::new())),
            restart_pending: Arc::new(Mutex::new(false)),
            target_commit_id: Arc::new(Mutex::new(None)),
            restart_after,
        };
        *guard = Some(handle.clone());
        drop(guard);
        self.persist(&handle).await?;

        let registry = self.clone();
        let task_handle = handle.clone();
        tokio::spawn(async move {
            let result = run_upgrade(
                &registry,
                client,
                &task_handle,
                actor,
                restart_after,
                force,
                health_addr,
            )
            .await;
            match result {
                Ok(UpgradeRunOutcome::AwaitingRestart) => {}
                Ok(UpgradeRunOutcome::Complete) => {
                    *task_handle.status.lock().await = UpgradeStatus::Success;
                    if let Err(error) = registry.persist(&task_handle).await {
                        warn!(error = %error, "persist completed upgrade failed");
                    }
                }
                Err(error) => {
                    *task_handle.status.lock().await = UpgradeStatus::Failed;
                    if let Err(persist_error) = registry.persist(&task_handle).await {
                        warn!(error = %persist_error, "persist failed upgrade failed");
                    }
                    warn!(error = %error, "self-update task failed");
                }
            }
        });
        Ok(handle)
    }

    pub async fn current(&self) -> Option<UpgradeHandle> {
        self.inner.lock().await.clone()
    }

    pub async fn status_snapshot(&self) -> Option<UpgradeStatusSnapshot> {
        self.refresh_from_disk().await;
        let handle = self.inner.lock().await.clone()?;
        Some(snapshot_from_handle(&handle).await)
    }

    pub async fn refresh_from_disk(&self) {
        let bytes = match std::fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let snapshot: UpgradeStatusSnapshot = match serde_json::from_slice(&bytes) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(error = %error, "refresh persisted upgrade state failed");
                return;
            }
        };
        let Some(handle) = self.inner.lock().await.clone() else {
            return;
        };
        if handle.task_id != snapshot.task_id {
            return;
        }
        let new_entries = {
            let mut history = handle.history.lock().await;
            let entries = snapshot
                .logs
                .iter()
                .skip(history.len())
                .cloned()
                .collect::<Vec<_>>();
            history.extend(entries.iter().cloned());
            entries
        };
        for entry in new_entries {
            let _ = handle.sender.send(entry);
        }
        *handle.status.lock().await = snapshot.status;
        *handle.restart_pending.lock().await = snapshot.restart_pending;
        *handle.target_commit_id.lock().await = snapshot.target_commit_id;
    }

    pub async fn clear_restart_pending(&self) {
        if let Some(handle) = self.inner.lock().await.clone() {
            *handle.restart_pending.lock().await = false;
            if let Err(error) = self.persist(&handle).await {
                warn!(error = %error, "persist restart-pending state failed");
            }
        }
    }

    async fn persist(&self, handle: &UpgradeHandle) -> Result<(), SelfUpdateError> {
        let snapshot = snapshot_from_handle(handle).await;
        write_snapshot_atomic(&self.state_path, &snapshot)
    }
}

pub type SharedUpgradeRegistry = Arc<UpgradeRegistry>;

impl std::fmt::Debug for UpgradeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpgradeRegistry")
            .field("state_path", &self.state_path)
            .finish_non_exhaustive()
    }
}

enum UpgradeRunOutcome {
    Complete,
    AwaitingRestart,
}

fn new_task_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn emit(
    registry: &UpgradeRegistry,
    handle: &UpgradeHandle,
    step: usize,
    level: UpgradeLogLevel,
    message: impl Into<String>,
    progress: Option<u8>,
) {
    let entry = UpgradeLogEntry {
        task_id: handle.task_id.clone(),
        step,
        total_steps: TOTAL_STEPS,
        level,
        message: message.into(),
        progress,
        at: Utc::now().to_rfc3339(),
    };
    handle.history.lock().await.push(entry.clone());
    let _ = handle.sender.send(entry);
    if let Err(error) = registry.persist(handle).await {
        warn!(error = %error, "persist upgrade log failed");
    }
}

async fn run_upgrade(
    registry: &UpgradeRegistry,
    client: reqwest::Client,
    handle: &UpgradeHandle,
    actor: Option<String>,
    restart_after: bool,
    force: bool,
    health_addr: SocketAddr,
) -> Result<UpgradeRunOutcome, SelfUpdateError> {
    let actor = actor.unwrap_or_else(|| "unknown".to_string());
    emit(
        registry,
        handle,
        1,
        UpgradeLogLevel::Info,
        format!("upgrade requested by {actor}; fetching release checksum"),
        Some(progress_pct(1, 0)),
    )
    .await;
    let release_cache_key = handle.task_id.as_str();
    let expected_checksum = match fetch_release_checksum(&client, release_cache_key).await {
        Ok(checksum) => checksum,
        Err(error) => {
            emit(
                registry,
                handle,
                1,
                UpgradeLogLevel::Error,
                error.to_string(),
                None,
            )
            .await;
            return Err(error);
        }
    };
    emit(
        registry,
        handle,
        1,
        UpgradeLogLevel::Success,
        "release checksum fetched and parsed",
        Some(progress_pct(1, 100)),
    )
    .await;

    let target = Path::new(BINARY_STAGING_PATH);
    let binary_url = release_binary_url_for_cache_key(release_cache_key);
    cleanup_tmp(target);
    emit(
        registry,
        handle,
        2,
        UpgradeLogLevel::Info,
        format!("downloading {} to {}", binary_url, target.display()),
        Some(progress_pct(2, 0)),
    )
    .await;
    let downloaded_bytes =
        match download_with_progress(&client, &binary_url, target, registry, handle).await {
            Ok(downloaded) => downloaded,
            Err(error) => {
                cleanup_tmp(target);
                emit(
                    registry,
                    handle,
                    2,
                    UpgradeLogLevel::Error,
                    format!("download failed: {error}"),
                    None,
                )
                .await;
                return Err(error);
            }
        };
    emit(
        registry,
        handle,
        2,
        UpgradeLogLevel::Success,
        format!(
            "download complete: {:.1} MiB",
            downloaded_bytes as f64 / 1024.0 / 1024.0
        ),
        Some(progress_pct(2, 100)),
    )
    .await;

    let downloaded_checksum = match sha256_of_file(target) {
        Ok(checksum) => checksum,
        Err(error) => {
            cleanup_tmp(target);
            emit(
                registry,
                handle,
                3,
                UpgradeLogLevel::Error,
                format!("checksum calculation failed: {error}"),
                None,
            )
            .await;
            return Err(error);
        }
    };
    if downloaded_checksum != expected_checksum {
        cleanup_tmp(target);
        let error = SelfUpdateError::Internal(format!(
            "release checksum mismatch: expected {expected_checksum}, got {downloaded_checksum}"
        ));
        emit(
            registry,
            handle,
            3,
            UpgradeLogLevel::Error,
            error.to_string(),
            None,
        )
        .await;
        return Err(error);
    }
    emit(
        registry,
        handle,
        3,
        UpgradeLogLevel::Success,
        format!("sha256 verified: {downloaded_checksum}"),
        Some(progress_pct(3, 100)),
    )
    .await;

    if let Err(error) = chmod_exec(target) {
        cleanup_tmp(target);
        emit(
            registry,
            handle,
            4,
            UpgradeLogLevel::Error,
            format!("make staged binary executable failed: {error}"),
            None,
        )
        .await;
        return Err(error);
    }
    emit(
        registry,
        handle,
        4,
        UpgradeLogLevel::Info,
        "running staged binary sanity and version checks",
        Some(progress_pct(4, 40)),
    )
    .await;
    let target_commit = match sanity_exec(target).await {
        Ok(commit) => commit,
        Err(error) => {
            cleanup_tmp(target);
            emit(
                registry,
                handle,
                4,
                UpgradeLogLevel::Error,
                format!("sanity check failed: {error}"),
                None,
            )
            .await;
            return Err(error);
        }
    };
    *handle.target_commit_id.lock().await = Some(target_commit.clone());
    emit(
        registry,
        handle,
        4,
        UpgradeLogLevel::Success,
        format!("staged binary passed sanity check at commit {target_commit}"),
        Some(progress_pct(4, 100)),
    )
    .await;

    if !force && commits_equal(&target_commit, crate::build_info::build_info().commit_id) {
        cleanup_tmp(target);
        emit(
            registry,
            handle,
            7,
            UpgradeLogLevel::Success,
            format!("already running release {target_commit}; no upgrade needed"),
            Some(100),
        )
        .await;
        return Ok(UpgradeRunOutcome::Complete);
    }

    if let Err(error) = backup_installed_binary() {
        cleanup_tmp(target);
        emit(
            registry,
            handle,
            5,
            UpgradeLogLevel::Error,
            format!("backup failed: {error}"),
            None,
        )
        .await;
        return Err(error);
    }
    emit(
        registry,
        handle,
        5,
        UpgradeLogLevel::Success,
        format!("rollback backup saved at {BINARY_ROLLBACK_PATH}"),
        Some(progress_pct(5, 100)),
    )
    .await;

    if restart_after {
        let command = registry
            .state_path
            .parent()
            .ok_or_else(|| SelfUpdateError::Internal("upgrade state path has no parent".into()))
            .and_then(|config_dir| {
                schedule_upgrade_restart(&handle.task_id, &target_commit, config_dir, health_addr)
            });
        let command = match command {
            Ok(command) => command,
            Err(error) => {
                cleanup_tmp(target);
                emit(
                    registry,
                    handle,
                    6,
                    UpgradeLogLevel::Error,
                    format!("schedule restart failed: {error}"),
                    None,
                )
                .await;
                return Err(error);
            }
        };
        emit(
            registry,
            handle,
            6,
            UpgradeLogLevel::Success,
            format!("restart helper scheduled: {command}"),
            Some(progress_pct(6, 100)),
        )
        .await;
        emit(
            registry,
            handle,
            7,
            UpgradeLogLevel::Info,
            "waiting for the replacement process to pass health and version checks",
            Some(95),
        )
        .await;
        Ok(UpgradeRunOutcome::AwaitingRestart)
    } else {
        *handle.restart_pending.lock().await = true;
        emit(
            registry,
            handle,
            6,
            UpgradeLogLevel::Success,
            format!("upgrade staged at {BINARY_STAGING_PATH}; restart is pending"),
            Some(progress_pct(6, 100)),
        )
        .await;
        emit(
            registry,
            handle,
            7,
            UpgradeLogLevel::Success,
            "upgrade package is ready to install",
            Some(100),
        )
        .await;
        Ok(UpgradeRunOutcome::Complete)
    }
}

async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    registry: &UpgradeRegistry,
    handle: &UpgradeHandle,
) -> Result<u64, SelfUpdateError> {
    let response = request_release_asset(client, url, DOWNLOAD_TIMEOUT, "binary download").await?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("open staged file failed: {err}")))?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    let mut next_percent = DOWNLOAD_PROGRESS_PERCENT_STEP;
    let mut next_unknown_bytes = DOWNLOAD_PROGRESS_UNKNOWN_BYTES;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|err| SelfUpdateError::Internal(format!("download chunk failed: {err}")))?;
        file.write_all(&chunk)
            .await
            .map_err(|err| SelfUpdateError::Internal(format!("write staged file failed: {err}")))?;
        downloaded += chunk.len() as u64;
        if let Some(within_step) = download_progress_milestone(
            downloaded,
            total,
            &mut next_percent,
            &mut next_unknown_bytes,
        ) {
            let message = total.map_or_else(
                || format!("downloaded {:.1} MiB", downloaded as f64 / 1024.0 / 1024.0),
                |value| {
                    format!(
                        "downloaded {:.1} MiB / {:.1} MiB",
                        downloaded as f64 / 1024.0 / 1024.0,
                        value as f64 / 1024.0 / 1024.0
                    )
                },
            );
            emit(
                registry,
                handle,
                2,
                UpgradeLogLevel::Progress,
                message,
                Some(progress_pct(2, within_step)),
            )
            .await;
        }
    }
    file.flush()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("flush staged file failed: {err}")))?;
    file.sync_all()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("sync staged file failed: {err}")))?;
    Ok(downloaded)
}

fn download_progress_milestone(
    downloaded: u64,
    total: Option<u64>,
    next_percent: &mut u8,
    next_unknown_bytes: &mut u64,
) -> Option<u8> {
    if let Some(total) = total.filter(|value| *value > 0) {
        let percent = ((downloaded.saturating_mul(100) / total).min(100)) as u8;
        if percent >= 100 || *next_percent >= 100 || percent < *next_percent {
            return None;
        }
        while *next_percent <= percent {
            *next_percent = next_percent.saturating_add(DOWNLOAD_PROGRESS_PERCENT_STEP);
        }
        return Some(percent);
    }

    if downloaded < *next_unknown_bytes {
        return None;
    }
    while *next_unknown_bytes <= downloaded {
        *next_unknown_bytes = next_unknown_bytes.saturating_add(DOWNLOAD_PROGRESS_UNKNOWN_BYTES);
    }
    Some(0)
}

fn chmod_exec(path: &Path) -> Result<(), SelfUpdateError> {
    let mut permissions = std::fs::metadata(path)
        .map_err(|err| SelfUpdateError::Internal(format!("stat staged file failed: {err}")))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|err| SelfUpdateError::Internal(format!("chmod staged file failed: {err}")))
}

async fn sanity_exec(path: &Path) -> Result<String, SelfUpdateError> {
    let output = tokio::time::timeout(SANITY_TIMEOUT, Command::new(path).arg("--help").output())
        .await
        .map_err(|_| SelfUpdateError::Internal("sanity --help timed out".into()))?
        .map_err(|err| SelfUpdateError::Internal(format!("sanity exec failed: {err}")))?;
    if !output.status.success() {
        return Err(SelfUpdateError::Internal(format!(
            "sanity --help exited with status {}",
            output.status
        )));
    }
    let output = tokio::time::timeout(
        SANITY_TIMEOUT,
        Command::new(path).args(["version", "--json"]).output(),
    )
    .await
    .map_err(|_| SelfUpdateError::Internal("sanity version check timed out".into()))?
    .map_err(|err| SelfUpdateError::Internal(format!("sanity version exec failed: {err}")))?;
    if !output.status.success() {
        return Err(SelfUpdateError::Internal(format!(
            "sanity version check exited with status {}",
            output.status
        )));
    }
    validate_staged_version_output(&output.stdout)
}

fn validate_staged_version_output(stdout: &[u8]) -> Result<String, SelfUpdateError> {
    let value: serde_json::Value = serde_json::from_slice(stdout).map_err(|err| {
        SelfUpdateError::Internal(format!("parse staged binary version output failed: {err}"))
    })?;
    let actual_commit = value
        .get("commitId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !(7..=40).contains(&actual_commit.len())
        || !actual_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SelfUpdateError::Internal(
            "staged binary version output has an invalid commitId".into(),
        ));
    }
    Ok(actual_commit)
}

fn sha256_of_file(path: &Path) -> Result<String, SelfUpdateError> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)
        .map_err(|err| SelfUpdateError::Internal(format!("open for sha256 failed: {err}")))?;
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| SelfUpdateError::Internal(format!("read for sha256 failed: {err}")))?;
    Ok(hex::encode(hasher.finalize()))
}

fn cleanup_tmp(file: &Path) {
    if let Err(error) = std::fs::remove_file(file) {
        if !matches!(error.kind(), std::io::ErrorKind::NotFound) {
            warn!(path = %file.display(), error = %error, "cleanup staged upgrade file failed");
        }
    }
}

fn progress_pct(step: usize, within_step: u8) -> u8 {
    let completed_steps = step.saturating_sub(1) as u32;
    let scaled = completed_steps * 100 + within_step as u32;
    (scaled / TOTAL_STEPS as u32).min(100) as u8
}

async fn snapshot_from_handle(handle: &UpgradeHandle) -> UpgradeStatusSnapshot {
    UpgradeStatusSnapshot {
        task_id: handle.task_id.clone(),
        status: *handle.status.lock().await,
        restart_pending: *handle.restart_pending.lock().await,
        logs: handle.history.lock().await.clone(),
        target_commit_id: handle.target_commit_id.lock().await.clone(),
        restart_after: handle.restart_after,
        updated_at: Utc::now().to_rfc3339(),
    }
}

fn handle_from_snapshot(snapshot: UpgradeStatusSnapshot) -> UpgradeHandle {
    let (sender, _receiver) = broadcast::channel(LOG_CHANNEL_CAPACITY);
    UpgradeHandle {
        task_id: snapshot.task_id,
        status: Arc::new(Mutex::new(snapshot.status)),
        sender,
        history: Arc::new(Mutex::new(snapshot.logs)),
        restart_pending: Arc::new(Mutex::new(snapshot.restart_pending)),
        target_commit_id: Arc::new(Mutex::new(snapshot.target_commit_id)),
        restart_after: snapshot.restart_after,
    }
}

fn reconcile_snapshot_with_running_binary(snapshot: &mut UpgradeStatusSnapshot) {
    let running = crate::build_info::build_info().commit_id;
    let target_matches = snapshot
        .target_commit_id
        .as_deref()
        .is_some_and(|target| crate::self_update::version::commits_equal(target, running));
    if target_matches && !snapshot.restart_pending {
        snapshot.status = UpgradeStatus::Success;
        snapshot.restart_pending = false;
        append_recovery_log(
            snapshot,
            UpgradeLogLevel::Success,
            "replacement binary started successfully",
        );
    } else if snapshot.status == UpgradeStatus::Running {
        snapshot.status = UpgradeStatus::Failed;
        append_recovery_log(
            snapshot,
            UpgradeLogLevel::Error,
            "upgrade was interrupted before the replacement binary became healthy",
        );
    } else if snapshot.status == UpgradeStatus::Success
        && snapshot.target_commit_id.is_some()
        && !snapshot.restart_pending
        && !target_matches
    {
        snapshot.status = UpgradeStatus::Failed;
        append_recovery_log(
            snapshot,
            UpgradeLogLevel::Error,
            "running binary does not match the completed upgrade target",
        );
    }
    snapshot.updated_at = Utc::now().to_rfc3339();
}

fn append_recovery_log(
    snapshot: &mut UpgradeStatusSnapshot,
    level: UpgradeLogLevel,
    message: &str,
) {
    if snapshot
        .logs
        .last()
        .is_some_and(|entry| entry.message == message)
    {
        return;
    }
    snapshot.logs.push(UpgradeLogEntry {
        task_id: snapshot.task_id.clone(),
        step: TOTAL_STEPS,
        total_steps: TOTAL_STEPS,
        level,
        message: message.to_string(),
        progress: Some(if matches!(level, UpgradeLogLevel::Success) {
            100
        } else {
            95
        }),
        at: Utc::now().to_rfc3339(),
    });
}

fn write_snapshot_atomic(
    path: &Path,
    snapshot: &UpgradeStatusSnapshot,
) -> Result<(), SelfUpdateError> {
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(|err| {
        SelfUpdateError::Internal(format!("serialize upgrade state failed: {err}"))
    })?;
    let tmp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|err| SelfUpdateError::Internal(format!("write upgrade state failed: {err}")))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|err| SelfUpdateError::Internal(format!("flush upgrade state failed: {err}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|err| SelfUpdateError::Internal(format!("commit upgrade state failed: {err}")))?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

pub fn record_helper_outcome(
    config_dir: &Path,
    task_id: &str,
    success: bool,
    message: &str,
) -> anyhow::Result<()> {
    let path = config_dir.join(STATE_FILENAME);
    let bytes = std::fs::read(&path)?;
    let mut snapshot: UpgradeStatusSnapshot = serde_json::from_slice(&bytes)?;
    if snapshot.task_id != task_id {
        anyhow::bail!("upgrade helper task id does not match persisted task");
    }
    snapshot.status = if success {
        UpgradeStatus::Success
    } else {
        UpgradeStatus::Failed
    };
    snapshot.restart_pending = false;
    append_recovery_log(
        &mut snapshot,
        if success {
            UpgradeLogLevel::Success
        } else {
            UpgradeLogLevel::Error
        },
        message,
    );
    snapshot.updated_at = Utc::now().to_rfc3339();
    write_snapshot_atomic(&path, &snapshot).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_pct_is_monotonic_and_finishes_at_100() {
        let mut last = 0u8;
        for step in 1..=TOTAL_STEPS {
            for percent in [0u8, 50, 100] {
                let value = progress_pct(step, percent);
                assert!(
                    value >= last,
                    "step {step} percent {percent}: {value} < {last}"
                );
                last = value;
            }
        }
        assert_eq!(last, 100);
    }

    #[test]
    fn download_progress_is_reduced_to_coarse_milestones() {
        let total = 28 * 1024 * 1024;
        let mut next_percent = DOWNLOAD_PROGRESS_PERCENT_STEP;
        let mut next_unknown_bytes = DOWNLOAD_PROGRESS_UNKNOWN_BYTES;
        let milestones = (1..=112)
            .filter_map(|chunk| {
                download_progress_milestone(
                    chunk * 256 * 1024,
                    Some(total),
                    &mut next_percent,
                    &mut next_unknown_bytes,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(milestones, vec![20, 40, 60, 80]);

        let mut next_percent = DOWNLOAD_PROGRESS_PERCENT_STEP;
        let mut next_unknown_bytes = DOWNLOAD_PROGRESS_UNKNOWN_BYTES;
        let unknown_milestones = (1..=28)
            .filter_map(|mib| {
                download_progress_milestone(
                    mib * 1024 * 1024,
                    None,
                    &mut next_percent,
                    &mut next_unknown_bytes,
                )
            })
            .count();
        assert_eq!(unknown_milestones, 3);
    }

    #[test]
    fn staged_version_must_contain_a_valid_commit() {
        let expected = "aabbccddeeff00112233445566778899aabbccdd";
        let matching = serde_json::json!({ "commitId": expected.to_ascii_uppercase() });
        assert_eq!(
            validate_staged_version_output(matching.to_string().as_bytes()).unwrap(),
            expected
        );

        for invalid in ["", "main", "not-a-commit", "abc123"] {
            let output = serde_json::json!({ "commitId": invalid });
            let error = validate_staged_version_output(output.to_string().as_bytes())
                .expect_err("invalid staged commit must be rejected before restart");
            assert!(error.to_string().contains("invalid commitId"));
        }
    }

    #[test]
    fn persisted_snapshot_round_trips() {
        let dir = std::env::temp_dir().join(format!("cc-switch-upgrade-test-{}", new_task_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STATE_FILENAME);
        let snapshot = UpgradeStatusSnapshot {
            task_id: "task-1".into(),
            status: UpgradeStatus::Success,
            restart_pending: true,
            logs: Vec::new(),
            target_commit_id: Some("abc1234".into()),
            restart_after: false,
            updated_at: Utc::now().to_rfc3339(),
        };
        write_snapshot_atomic(&path, &snapshot).unwrap();
        let loaded: UpgradeStatusSnapshot =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded.task_id, snapshot.task_id);
        assert_eq!(loaded.status, UpgradeStatus::Success);
        assert!(loaded.restart_pending);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn startup_reconciles_upgrade_against_running_commit() {
        let mut applied = UpgradeStatusSnapshot {
            task_id: "applied".into(),
            status: UpgradeStatus::Running,
            restart_pending: false,
            logs: Vec::new(),
            target_commit_id: Some(crate::build_info::build_info().commit_id.to_string()),
            restart_after: true,
            updated_at: String::new(),
        };
        reconcile_snapshot_with_running_binary(&mut applied);
        assert_eq!(applied.status, UpgradeStatus::Success);
        assert_eq!(applied.logs.last().unwrap().progress, Some(100));

        let mut staged = UpgradeStatusSnapshot {
            task_id: "staged".into(),
            status: UpgradeStatus::Success,
            restart_pending: true,
            logs: Vec::new(),
            target_commit_id: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            restart_after: false,
            updated_at: String::new(),
        };
        reconcile_snapshot_with_running_binary(&mut staged);
        assert_eq!(staged.status, UpgradeStatus::Success);
        assert!(staged.restart_pending);

        let mut interrupted = UpgradeStatusSnapshot {
            task_id: "interrupted".into(),
            status: UpgradeStatus::Running,
            restart_pending: false,
            logs: Vec::new(),
            target_commit_id: None,
            restart_after: true,
            updated_at: String::new(),
        };
        reconcile_snapshot_with_running_binary(&mut interrupted);
        assert_eq!(interrupted.status, UpgradeStatus::Failed);
    }

    #[test]
    fn unreadable_upgrade_state_does_not_block_server_startup() {
        let dir = std::env::temp_dir().join(format!("cc-switch-upgrade-corrupt-{}", new_task_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STATE_FILENAME), b"{not-json").unwrap();
        assert!(UpgradeRegistry::load(&dir).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
