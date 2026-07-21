use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_HISTORY_BYTES: usize = 256 * 1024;
const MIN_HISTORY_BYTES: usize = 64 * 1024;
const MAX_HISTORY_BYTES: usize = 1024 * 1024;
const DEFAULT_IDLE_DETACH_SECS: u64 = 900;
const DEFAULT_MAX_LIFETIME_SECS: u64 = 7200;
const REPLAY_CHUNK_BYTES: usize = 16 * 1024;
const LIVE_QUEUE_CAP: usize = 32;
const READ_BUF_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct TerminalRuntimeOptions {
    pub shell: Vec<String>,
    pub cwd: PathBuf,
    pub history_bytes: usize,
    pub idle_detach: Duration,
    pub max_lifetime: Duration,
    pub permit_write: bool,
    pub replay_chunk_bytes: usize,
    pub live_queue_cap: usize,
    pub read_buf_bytes: usize,
}

impl TerminalRuntimeOptions {
    pub(crate) fn resolve(config_dir: &Path) -> Self {
        let shell = resolve_shell();
        let cwd = std::env::var("CC_SWITCH_TERMINAL_CWD")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir.to_path_buf());
        Self {
            shell,
            cwd,
            history_bytes: clamp_usize(
                env_usize("CC_SWITCH_TERMINAL_HISTORY_BYTES", DEFAULT_HISTORY_BYTES),
                MIN_HISTORY_BYTES,
                MAX_HISTORY_BYTES,
            ),
            idle_detach: Duration::from_secs(env_u64(
                "CC_SWITCH_TERMINAL_IDLE_DETACH_SECS",
                DEFAULT_IDLE_DETACH_SECS,
            )),
            max_lifetime: Duration::from_secs(env_u64(
                "CC_SWITCH_TERMINAL_MAX_LIFETIME_SECS",
                DEFAULT_MAX_LIFETIME_SECS,
            )),
            permit_write: env_bool("CC_SWITCH_TERMINAL_PERMIT_WRITE", true),
            replay_chunk_bytes: REPLAY_CHUNK_BYTES,
            live_queue_cap: LIVE_QUEUE_CAP,
            read_buf_bytes: READ_BUF_BYTES,
        }
    }
}

fn resolve_shell() -> Vec<String> {
    if let Ok(value) = std::env::var("CC_SWITCH_TERMINAL_SHELL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return shell_words(trimmed);
        }
    }
    if path_exists("/bin/bash") {
        return vec!["/bin/bash".to_string()];
    }
    if path_exists("/usr/bin/bash") {
        return vec!["/usr/bin/bash".to_string()];
    }
    if let Ok(shell) = std::env::var("SHELL") {
        let trimmed = shell.trim();
        if !trimmed.is_empty() && path_exists(trimmed) {
            return vec![trimmed.to_string()];
        }
    }
    if path_exists("/bin/sh") {
        return vec!["/bin/sh".to_string()];
    }
    vec!["sh".to_string()]
}

fn shell_words(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|part| part.to_string())
        .collect()
}

fn path_exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn clamp_usize(value: usize, min: usize, max: usize) -> usize {
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::clamp_usize;

    #[test]
    fn history_bytes_clamp() {
        assert_eq!(clamp_usize(1, 64 * 1024, 1024 * 1024), 64 * 1024);
        assert_eq!(
            clamp_usize(2 * 1024 * 1024, 64 * 1024, 1024 * 1024),
            1024 * 1024
        );
    }
}
