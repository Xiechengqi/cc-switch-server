use std::process::Command;
use std::time::Duration;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Forbidden(String),
}

pub const SERVICE_UNIT: &str = "cc-switch-server.service";
pub const BINARY_INSTALL_PATH: &str = "/usr/local/bin/cc-switch-server";
pub const BINARY_ROLLBACK_PATH: &str = "/usr/local/bin/cc-switch-server.bak";
pub const SERVICE_LOG_PATH: &str = "/var/log/cc-switch-server.log";

pub fn release_binary_url() -> &'static str {
    let target = env!("CC_SWITCH_BUILD_TARGET");
    if target.contains("aarch64") || target.contains("arm64") {
        "https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-arm64"
    } else {
        "https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-amd64"
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Systemd,
    Nohup,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub manager: ServiceManager,
    pub active: bool,
    pub unit_name: Option<&'static str>,
    pub active_state: Option<String>,
    pub unit_file_state: Option<String>,
}

pub fn detect_service_status() -> ServiceStatus {
    let output = Command::new("systemctl")
        .args([
            "--no-pager",
            "show",
            "--property=ActiveState",
            "--property=UnitFileState",
            "--property=LoadState",
            SERVICE_UNIT,
        ])
        .output();

    let Ok(output) = output else {
        return nohup_status();
    };
    if !output.status.success() {
        return nohup_status();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut active_state = None;
    let mut unit_file_state = None;
    let mut load_state = None;
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("ActiveState=") {
            active_state = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("UnitFileState=") {
            unit_file_state = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("LoadState=") {
            load_state = Some(value.to_string());
        }
    }
    let loaded_known = load_state
        .as_deref()
        .map(|state| state != "not-found" && state != "masked")
        .unwrap_or(false);
    if !loaded_known {
        return nohup_status();
    }
    let active = active_state.as_deref() == Some("active");
    ServiceStatus {
        manager: ServiceManager::Systemd,
        active,
        unit_name: Some(SERVICE_UNIT),
        active_state,
        unit_file_state,
    }
}

fn nohup_status() -> ServiceStatus {
    ServiceStatus {
        manager: ServiceManager::Nohup,
        active: true,
        unit_name: None,
        active_state: Some("running".into()),
        unit_file_state: None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestReleaseMeta {
    pub binary_url: String,
    pub available: bool,
    pub etag: Option<String>,
    pub content_length: Option<u64>,
    pub error: Option<String>,
}

pub async fn fetch_latest_release_meta(client: &reqwest::Client) -> LatestReleaseMeta {
    let url = release_binary_url();
    let mut meta = LatestReleaseMeta {
        binary_url: url.to_string(),
        available: false,
        etag: None,
        content_length: None,
        error: None,
    };
    match client.head(url).timeout(Duration::from_secs(8)).send().await {
        Ok(resp) => {
            if resp.status().is_success() || resp.status().is_redirection() {
                meta.available = true;
                if let Some(value) = resp.headers().get("etag") {
                    meta.etag = value.to_str().ok().map(str::to_string);
                }
                if let Some(value) = resp.headers().get("content-length") {
                    meta.content_length = value.to_str().ok().and_then(|v| v.trim().parse().ok());
                }
            } else {
                meta.error = Some(format!("HTTP {}", resp.status()));
            }
        }
        Err(err) => meta.error = Some(err.to_string()),
    }
    meta
}

pub fn ensure_binary_writable() -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = match std::fs::metadata(BINARY_INSTALL_PATH) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(SelfUpdateError::Internal(format!(
                "stat {BINARY_INSTALL_PATH} failed: {err}"
            )));
        }
    };
    let mode = metadata.permissions().mode();
    if mode & 0o200 == 0 {
        return Err(SelfUpdateError::Forbidden(format!(
            "binary at {BINARY_INSTALL_PATH} is not writable by this process"
        )));
    }
    Ok(())
}

pub fn rollback_available() -> bool {
    std::path::Path::new(BINARY_ROLLBACK_PATH).exists()
}
