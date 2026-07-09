use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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

use crate::self_update::restart::{schedule_restart, RestartStrategy};
use crate::self_update::version::{
    detect_service_status, release_binary_url, SelfUpdateError, BINARY_INSTALL_PATH,
    BINARY_ROLLBACK_PATH,
};

const LOG_CHANNEL_CAPACITY: usize = 256;
const TOTAL_STEPS: usize = 7;
const DOWNLOAD_BUFFER_TICK_BYTES: u64 = 256 * 1024;
const SANITY_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Running,
    Success,
    Failed,
}

#[derive(Clone)]
pub struct UpgradeHandle {
    pub task_id: String,
    pub status: Arc<Mutex<UpgradeStatus>>,
    pub sender: broadcast::Sender<UpgradeLogEntry>,
    pub history: Arc<Mutex<Vec<UpgradeLogEntry>>>,
    pub restart_pending: Arc<Mutex<bool>>,
}

#[derive(Default)]
pub struct UpgradeRegistry {
    inner: Mutex<Option<UpgradeHandle>>,
}

impl UpgradeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn is_restart_pending(&self) -> bool {
        if let Some(handle) = self.inner.lock().await.as_ref() {
            return *handle.restart_pending.lock().await;
        }
        false
    }

    pub async fn start(
        &self,
        client: reqwest::Client,
        actor: Option<String>,
        restart_after: bool,
    ) -> Result<UpgradeHandle, SelfUpdateError> {
        let mut guard = self.inner.lock().await;
        if let Some(handle) = guard.as_ref() {
            let status = *handle.status.lock().await;
            if matches!(status, UpgradeStatus::Running) {
                return Err(SelfUpdateError::Internal(
                    "an upgrade is already in progress".into(),
                ));
            }
        }
        let task_id = new_task_id();
        let (tx, _rx) = broadcast::channel(LOG_CHANNEL_CAPACITY);
        let handle = UpgradeHandle {
            task_id: task_id.clone(),
            status: Arc::new(Mutex::new(UpgradeStatus::Running)),
            sender: tx,
            history: Arc::new(Mutex::new(Vec::new())),
            restart_pending: Arc::new(Mutex::new(false)),
        };
        *guard = Some(handle.clone());
        drop(guard);

        let handle_for_task = handle.clone();
        tokio::spawn(async move {
            let outcome = run_upgrade(client, &handle_for_task, actor, restart_after).await;
            let mut status_guard = handle_for_task.status.lock().await;
            *status_guard = match outcome {
                Ok(()) => UpgradeStatus::Success,
                Err(_) => UpgradeStatus::Failed,
            };
        });
        Ok(handle)
    }

    pub async fn current(&self) -> Option<UpgradeHandle> {
        self.inner.lock().await.clone()
    }

    pub async fn clear_restart_pending(&self) {
        if let Some(handle) = self.inner.lock().await.as_ref() {
            *handle.restart_pending.lock().await = false;
        }
    }
}

pub type SharedUpgradeRegistry = Arc<UpgradeRegistry>;

impl std::fmt::Debug for UpgradeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpgradeRegistry").finish_non_exhaustive()
    }
}

fn new_task_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn emit(
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
}

async fn run_upgrade(
    client: reqwest::Client,
    handle: &UpgradeHandle,
    actor: Option<String>,
    restart_after: bool,
) -> Result<(), SelfUpdateError> {
    let actor = actor.unwrap_or_else(|| "unknown".to_string());
    emit(
        handle,
        1,
        UpgradeLogLevel::Info,
        format!("upgrade requested by {actor}"),
        Some(progress_pct(1, 0)),
    )
    .await;

    let target = Path::new(BINARY_INSTALL_PATH);
    let target_parent = target.parent().ok_or_else(|| {
        SelfUpdateError::Internal(format!(
            "install target has no parent: {}",
            target.display()
        ))
    })?;
    if let Err(err) = std::fs::create_dir_all(target_parent) {
        emit(
            handle,
            1,
            UpgradeLogLevel::Error,
            format!("ensure install dir failed: {err}"),
            None,
        )
        .await;
        return Err(SelfUpdateError::Internal(format!(
            "ensure install dir failed: {err}"
        )));
    }
    let tmp_path = target_parent.join(format!("cc-switch-server.upgrade-{}", handle.task_id));
    emit(
        handle,
        1,
        UpgradeLogLevel::Info,
        format!("staging tmp file at {}", tmp_path.display()),
        Some(progress_pct(1, 100)),
    )
    .await;

    let release_url = release_binary_url();
    emit(
        handle,
        2,
        UpgradeLogLevel::Info,
        format!("downloading {release_url}"),
        Some(progress_pct(2, 0)),
    )
    .await;
    if let Err(err) =
        download_with_progress(&client, release_url, &tmp_path, handle).await
    {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            2,
            UpgradeLogLevel::Error,
            format!("download failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }

    emit(
        handle,
        3,
        UpgradeLogLevel::Info,
        "setting executable permissions",
        Some(progress_pct(3, 0)),
    )
    .await;
    if let Err(err) = chmod_exec(&tmp_path) {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            3,
            UpgradeLogLevel::Error,
            format!("chmod failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }

    emit(
        handle,
        3,
        UpgradeLogLevel::Info,
        "running sanity check (--help)",
        Some(progress_pct(3, 50)),
    )
    .await;
    if let Err(err) = sanity_exec(&tmp_path).await {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            3,
            UpgradeLogLevel::Error,
            format!("sanity check failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }

    let new_sha = match sha256_of_file(&tmp_path) {
        Ok(value) => value,
        Err(err) => {
            cleanup_tmp(&tmp_path);
            emit(
                handle,
                4,
                UpgradeLogLevel::Error,
                format!("sha256 failed: {err}"),
                None,
            )
            .await;
            return Err(err);
        }
    };
    let current_sha = sha256_of_file(Path::new(BINARY_INSTALL_PATH)).ok();
    emit(
        handle,
        4,
        UpgradeLogLevel::Info,
        format!(
            "new sha256: {new_sha}; current: {}",
            current_sha.as_deref().unwrap_or("(missing)")
        ),
        Some(progress_pct(4, 100)),
    )
    .await;
    if current_sha.as_deref() == Some(new_sha.as_str()) {
        emit(
            handle,
            4,
            UpgradeLogLevel::Warn,
            "downloaded binary matches the running one; restart will still pick up env changes",
            None,
        )
        .await;
    }

    let bak_path = BINARY_ROLLBACK_PATH.to_string();
    if let Err(err) = swap_binary(&tmp_path, target, Path::new(&bak_path)) {
        cleanup_tmp(&tmp_path);
        emit(
            handle,
            5,
            UpgradeLogLevel::Error,
            format!("swap failed: {err}"),
            None,
        )
        .await;
        return Err(err);
    }
    cleanup_tmp(&tmp_path);
    emit(
        handle,
        5,
        UpgradeLogLevel::Success,
        format!("installed new binary at {BINARY_INSTALL_PATH} (backup at {bak_path})"),
        Some(progress_pct(5, 100)),
    )
    .await;

    if restart_after {
        let manager = detect_service_status().manager;
        let strategy = RestartStrategy::from_manager(manager);
        emit(
            handle,
            6,
            UpgradeLogLevel::Info,
            format!("triggering restart via {} mode", strategy.label()),
            Some(progress_pct(6, 30)),
        )
        .await;
        let restart_script = match schedule_restart(strategy) {
            Ok(script) => script,
            Err(err) => {
                emit(
                    handle,
                    6,
                    UpgradeLogLevel::Error,
                    format!("restart spawn failed: {err}"),
                    None,
                )
                .await;
                return Err(err);
            }
        };
        *handle.restart_pending.lock().await = false;
        emit(
            handle,
            6,
            UpgradeLogLevel::Success,
            format!("restart scheduled: {restart_script}"),
            Some(progress_pct(6, 100)),
        )
        .await;
        emit(
            handle,
            7,
            UpgradeLogLevel::Success,
            "process will exit shortly; reload once /health succeeds",
            Some(progress_pct(7, 100)),
        )
        .await;
    } else {
        *handle.restart_pending.lock().await = true;
        emit(
            handle,
            6,
            UpgradeLogLevel::Success,
            "upgrade complete; restart is required to run the new binary",
            Some(progress_pct(6, 100)),
        )
        .await;
        emit(
            handle,
            7,
            UpgradeLogLevel::Info,
            "use the pending-restart action when you are ready to apply the upgrade",
            Some(progress_pct(7, 100)),
        )
        .await;
    }
    Ok(())
}

async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    handle: &UpgradeHandle,
) -> Result<u64, SelfUpdateError> {
    let response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("download request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(SelfUpdateError::Internal(format!(
            "download HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("open tmp file failed: {err}")))?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut next_tick: u64 = DOWNLOAD_BUFFER_TICK_BYTES;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|err| SelfUpdateError::Internal(format!("download chunk failed: {err}")))?;
        file.write_all(&chunk)
            .await
            .map_err(|err| SelfUpdateError::Internal(format!("write tmp failed: {err}")))?;
        downloaded += chunk.len() as u64;
        if downloaded >= next_tick {
            next_tick = downloaded + DOWNLOAD_BUFFER_TICK_BYTES;
            let pct = match total {
                Some(total_bytes) if total_bytes > 0 => Some(
                    ((downloaded as f64 / total_bytes as f64) * 100.0).clamp(0.0, 100.0) as u8,
                ),
                _ => None,
            };
            let msg = match total {
                Some(total_bytes) => format!(
                    "downloaded {:.1} MiB / {:.1} MiB",
                    downloaded as f64 / 1024.0 / 1024.0,
                    total_bytes as f64 / 1024.0 / 1024.0
                ),
                None => format!("downloaded {:.1} MiB", downloaded as f64 / 1024.0 / 1024.0),
            };
            emit(handle, 2, UpgradeLogLevel::Progress, msg, pct).await;
        }
    }
    file.flush()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("flush tmp failed: {err}")))?;
    Ok(downloaded)
}

fn chmod_exec(path: &Path) -> Result<(), SelfUpdateError> {
    let mut perms = std::fs::metadata(path)
        .map_err(|err| SelfUpdateError::Internal(format!("stat tmp failed: {err}")))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|err| SelfUpdateError::Internal(format!("chmod failed: {err}")))
}

async fn sanity_exec(path: &Path) -> Result<(), SelfUpdateError> {
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
    Ok(())
}

fn sha256_of_file(path: &Path) -> Result<String, SelfUpdateError> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)
        .map_err(|err| SelfUpdateError::Internal(format!("open for sha256 failed: {err}")))?;
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| SelfUpdateError::Internal(format!("read for sha256 failed: {err}")))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn swap_binary(new_path: &Path, target: &Path, bak: &Path) -> Result<(), SelfUpdateError> {
    if target.exists() {
        if bak.exists() {
            let _ = std::fs::remove_file(bak);
        }
        std::fs::rename(target, bak).map_err(|err| {
            SelfUpdateError::Internal(format!("backup current binary failed: {err}"))
        })?;
    }
    if let Err(err) = std::fs::rename(new_path, target) {
        if bak.exists() {
            let _ = std::fs::rename(bak, target);
        }
        return Err(SelfUpdateError::Internal(format!(
            "install new binary failed: {err}"
        )));
    }
    Ok(())
}

fn cleanup_tmp(file: &Path) {
    if let Err(err) = std::fs::remove_file(file) {
        if !matches!(err.kind(), std::io::ErrorKind::NotFound) {
            warn!(path = %file.display(), error = %err, "cleanup tmp upgrade file failed");
        }
    }
}

fn progress_pct(step: usize, within_step: u8) -> u8 {
    let base = (step.saturating_sub(1) * 100 / TOTAL_STEPS) as u32;
    let inc = (within_step as u32) * 100 / (TOTAL_STEPS as u32) / 100;
    base.saturating_add(inc).min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_pct_monotonic() {
        let mut last = 0u8;
        for step in 1..=TOTAL_STEPS {
            for pct in [0u8, 50, 100] {
                let value = progress_pct(step, pct);
                assert!(value >= last, "step {step} pct {pct}: {value} < {last}");
                last = value;
            }
        }
    }
}
