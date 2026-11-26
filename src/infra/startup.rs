use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const PROGRESS_FILE: &str = "startup-state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupProgress {
    pub process_id: u32,
    pub process_start_ticks: Option<u64>,
    pub instance_id: String,
    pub commit_id: String,
    pub started_at_ms: u128,
    pub stage: String,
    pub completed: u64,
    pub elapsed_ms: u128,
    pub status: String,
}

struct ActiveStartup {
    path: PathBuf,
    progress: StartupProgress,
    started: Instant,
    phase_started: Instant,
    last_write: Instant,
    owns_directory: bool,
}

fn active() -> &'static Mutex<Option<ActiveStartup>> {
    static ACTIVE: OnceLock<Mutex<Option<ActiveStartup>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

pub struct StartupGuard;

impl StartupGuard {
    pub fn begin(root: &Path) -> Self {
        let now = Instant::now();
        let state = ActiveStartup {
            path: root.join(PROGRESS_FILE),
            progress: StartupProgress {
                process_id: std::process::id(),
                process_start_ticks: process_start_ticks(std::process::id()),
                instance_id: format!("{:032x}", rand::random::<u128>()),
                commit_id: crate::build_info::build_info().commit_id.to_string(),
                started_at_ms: crate::infra::time::now_ms(),
                stage: "data_directory".into(),
                completed: 0,
                elapsed_ms: 0,
                status: "starting".into(),
            },
            started: now,
            phase_started: now,
            last_write: now,
            owns_directory: false,
        };
        *active().lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        tracing::info!(
            process_id = std::process::id(),
            "server initialization started"
        );
        Self
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        let mut active = active().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut state) = active.take() {
            if state.progress.status == "starting" {
                state.progress.status = "failed".into();
                state.progress.elapsed_ms = state.started.elapsed().as_millis();
                if state.owns_directory {
                    let _ = crate::infra::storage::write_json_pretty(&state.path, &state.progress);
                }
                tracing::error!(stage = %state.progress.stage, elapsed_ms = state.progress.elapsed_ms, "server initialization failed");
            }
        }
    }
}

pub fn advance(stage: &str, completed: u64) {
    let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return;
    };
    if state.progress.status != "starting" {
        return;
    }
    if stage == "workspace_authorization" {
        state.owns_directory = true;
    }
    let changed = state.progress.stage != stage;
    if changed {
        tracing::info!(stage = %state.progress.stage, elapsed_ms = state.phase_started.elapsed().as_millis(), completed = state.progress.completed, "server initialization phase completed");
        state.progress.stage = stage.into();
        state.progress.completed = 0;
        state.phase_started = Instant::now();
    }
    state.progress.completed = state.progress.completed.saturating_add(completed);
    state.progress.elapsed_ms = state.started.elapsed().as_millis();
    if changed || state.last_write.elapsed() >= Duration::from_secs(1) {
        if let Err(error) = crate::infra::storage::write_json_pretty(&state.path, &state.progress) {
            tracing::warn!(%error, "write server initialization progress failed");
        }
        state.last_write = Instant::now();
    }
}

pub fn ready() {
    advance("ready", 0);
    let mut guard = active().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.progress.status = "ready".into();
        state.progress.elapsed_ms = state.started.elapsed().as_millis();
        let _ = crate::infra::storage::write_json_pretty(&state.path, &state.progress);
        tracing::info!(
            elapsed_ms = state.progress.elapsed_ms,
            "server initialization completed"
        );
    }
}

pub fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}
