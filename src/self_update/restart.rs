use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::self_update::version::{
    is_containerized, service_cc_switch_started, SelfUpdateError, BINARY_INSTALL_PATH,
    BINARY_ROLLBACK_PATH, BINARY_STAGING_PATH, SERVICE_LOG_PATH, SERVICE_UNIT,
};

const HELPER_SPEC_FILENAME: &str = "upgrade-helper.json";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartStrategy {
    Service,
    Standalone,
}

impl RestartStrategy {
    pub fn label(&self) -> &'static str {
        match self {
            RestartStrategy::Service => "service",
            RestartStrategy::Standalone => "standalone",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HelperMode {
    InstallStaged,
    RestartOnly,
    Rollback,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateHelperSpec {
    task_id: Option<String>,
    mode: HelperMode,
    strategy: RestartStrategy,
    parent_pid: u32,
    health_addr: SocketAddr,
    expected_commit: Option<String>,
    config_dir: PathBuf,
    server_args: Vec<String>,
    install_path: PathBuf,
    staging_path: PathBuf,
    rollback_path: PathBuf,
    log_path: PathBuf,
}

pub fn detect_restart_strategy() -> RestartStrategy {
    if service_cc_switch_started() {
        RestartStrategy::Service
    } else {
        RestartStrategy::Standalone
    }
}

pub fn schedule_upgrade_restart(
    task_id: &str,
    target_commit: &str,
    config_dir: &Path,
    health_addr: SocketAddr,
) -> Result<String, SelfUpdateError> {
    launch_helper(UpdateHelperSpec {
        task_id: Some(task_id.to_string()),
        mode: HelperMode::InstallStaged,
        strategy: detect_restart_strategy(),
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: Some(target_commit.to_string()),
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
        log_path: SERVICE_LOG_PATH.into(),
    })
}

pub fn restart_from_detected_service(
    config_dir: &Path,
    health_addr: SocketAddr,
) -> Result<String, SelfUpdateError> {
    if is_containerized() {
        return Err(SelfUpdateError::Forbidden(
            "in-process restart is disabled in containers; restart the container instead".into(),
        ));
    }
    let pending = pending_upgrade(config_dir);
    launch_helper(UpdateHelperSpec {
        task_id: pending.as_ref().map(|value| value.0.clone()),
        mode: if pending.is_some() {
            HelperMode::InstallStaged
        } else {
            HelperMode::RestartOnly
        },
        strategy: detect_restart_strategy(),
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: pending
            .map(|value| value.1)
            .or_else(|| Some(crate::build_info::build_info().commit_id.to_string())),
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
        log_path: SERVICE_LOG_PATH.into(),
    })
}

fn pending_upgrade(config_dir: &Path) -> Option<(String, String)> {
    if !Path::new(BINARY_STAGING_PATH).is_file() {
        return None;
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_dir.join("upgrade-state.json")).ok()?).ok()?;
    let task_id = value.get("taskId")?.as_str()?.to_string();
    let target_commit = value.get("targetCommitId")?.as_str()?.to_string();
    (!task_id.is_empty() && !target_commit.is_empty()).then_some((task_id, target_commit))
}

pub fn rollback_from_backup_and_restart(
    config_dir: &Path,
    health_addr: SocketAddr,
) -> Result<String, SelfUpdateError> {
    if !Path::new(BINARY_ROLLBACK_PATH).exists() {
        return Err(SelfUpdateError::Forbidden(format!(
            "rollback backup not found at {BINARY_ROLLBACK_PATH}"
        )));
    }
    launch_helper(UpdateHelperSpec {
        task_id: None,
        mode: HelperMode::Rollback,
        strategy: detect_restart_strategy(),
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: None,
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
        log_path: SERVICE_LOG_PATH.into(),
    })
}

fn launch_helper(spec: UpdateHelperSpec) -> Result<String, SelfUpdateError> {
    let spec_path = spec.config_dir.join(HELPER_SPEC_FILENAME);
    write_json_atomic(&spec_path, &spec)?;
    let current_exe = std::env::current_exe().map_err(|err| {
        SelfUpdateError::Internal(format!("resolve current executable failed: {err}"))
    })?;
    let command_label = format!(
        "{} self-update-helper --spec {}",
        current_exe.display(),
        spec_path.display()
    );

    match spec.strategy {
        RestartStrategy::Service => {
            let suffix = spec
                .task_id
                .as_deref()
                .unwrap_or("manual")
                .chars()
                .take(12)
                .collect::<String>();
            let status = Command::new("systemd-run")
                .args(["--quiet", "--collect", "--property=Type=exec"])
                .arg(format!("--unit=cc-switch-server-update-{suffix}"))
                .arg(&current_exe)
                .arg("self-update-helper")
                .arg("--spec")
                .arg(&spec_path)
                .status()
                .map_err(|err| {
                    SelfUpdateError::Internal(format!("launch systemd update helper failed: {err}"))
                })?;
            if !status.success() {
                return Err(SelfUpdateError::Internal(format!(
                    "systemd-run update helper exited with {status}"
                )));
            }
        }
        RestartStrategy::Standalone => {
            let spawned = Command::new("setsid")
                .args(["-f"])
                .arg(&current_exe)
                .arg("self-update-helper")
                .arg("--spec")
                .arg(&spec_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Err(err) = spawned {
                if err.kind() != std::io::ErrorKind::NotFound {
                    return Err(SelfUpdateError::Internal(format!(
                        "launch standalone update helper failed: {err}"
                    )));
                }
                Command::new(&current_exe)
                    .arg("self-update-helper")
                    .arg("--spec")
                    .arg(&spec_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|err| {
                        SelfUpdateError::Internal(format!("spawn update helper failed: {err}"))
                    })?;
            }
        }
    }
    Ok(command_label)
}

pub fn run_update_helper(spec_path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(spec_path)?;
    let spec: UpdateHelperSpec = serde_json::from_slice(&bytes)?;
    let result = run_update_helper_inner(spec_path, &spec);
    if let Err(error) = &result {
        if let Some(task_id) = spec.task_id.as_deref() {
            let _ = crate::self_update::upgrade::record_helper_outcome(
                &spec.config_dir,
                task_id,
                false,
                &format!("update helper failed: {error}"),
            );
        }
    }
    result
}

fn run_update_helper_inner(spec_path: &Path, spec: &UpdateHelperSpec) -> anyhow::Result<()> {
    std::thread::sleep(Duration::from_secs(2));

    let rollback_source = match spec.mode {
        HelperMode::InstallStaged => Some(spec.rollback_path.clone()),
        HelperMode::Rollback => {
            let current_backup = spec.staging_path.with_extension("rollback-current");
            std::fs::copy(&spec.install_path, &current_backup)?;
            std::fs::copy(&spec.rollback_path, &spec.staging_path)?;
            Some(current_backup)
        }
        HelperMode::RestartOnly => None,
    };

    if !matches!(spec.mode, HelperMode::RestartOnly) {
        install_staged_binary(&spec.staging_path, &spec.install_path)?;
    }
    let (mut started_process, restart_error) = match restart_process(spec) {
        Ok(child) => (child, None),
        Err(error) => (None, Some(error)),
    };

    if restart_error.is_none()
        && wait_for_expected_version(
            spec.health_addr,
            spec.expected_commit.as_deref(),
            HEALTH_TIMEOUT,
        )
    {
        if let Some(task_id) = spec.task_id.as_deref() {
            crate::self_update::upgrade::record_helper_outcome(
                &spec.config_dir,
                task_id,
                true,
                "new binary passed health and version checks",
            )?;
        }
        let _ = std::fs::remove_file(spec_path);
        return Ok(());
    }

    if let Some(child) = started_process.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some(source) = rollback_source.as_deref().filter(|path| path.exists()) {
        std::fs::copy(source, &spec.staging_path)?;
        install_staged_binary(&spec.staging_path, &spec.install_path)?;
        let _ = restart_process(spec)?;
        let _ = wait_for_expected_version(spec.health_addr, None, HEALTH_TIMEOUT);
    }
    if let Some(task_id) = spec.task_id.as_deref() {
        crate::self_update::upgrade::record_helper_outcome(
            &spec.config_dir,
            task_id,
            false,
            "new binary failed health/version checks; rollback was attempted",
        )?;
    }
    match restart_error {
        Some(error) => anyhow::bail!("restart failed: {error}"),
        None => anyhow::bail!("updated service did not pass health/version checks"),
    }
}

fn restart_process(spec: &UpdateHelperSpec) -> anyhow::Result<Option<std::process::Child>> {
    match spec.strategy {
        RestartStrategy::Service => {
            let status = Command::new("systemctl")
                .args(["restart", SERVICE_UNIT])
                .status()?;
            anyhow::ensure!(status.success(), "systemctl restart exited with {status}");
            Ok(None)
        }
        RestartStrategy::Standalone => {
            let parent_proc = PathBuf::from(format!("/proc/{}", spec.parent_pid));
            if parent_proc.exists() {
                let status = Command::new("kill")
                    .args(["-TERM", &spec.parent_pid.to_string()])
                    .status()?;
                anyhow::ensure!(
                    status.success(),
                    "failed to terminate current server process"
                );
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && parent_proc.exists() {
                std::thread::sleep(Duration::from_millis(200));
            }
            anyhow::ensure!(
                !parent_proc.exists(),
                "current server process did not exit after SIGTERM"
            );
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&spec.log_path)?;
            let err_log = log.try_clone()?;
            let child = Command::new(&spec.install_path)
                .args(&spec.server_args)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(err_log))
                .spawn()?;
            Ok(Some(child))
        }
    }
}

fn install_staged_binary(staging: &Path, install: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    anyhow::ensure!(staging.exists(), "staged binary is missing");
    let mut permissions = std::fs::metadata(staging)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(staging, permissions)?;
    std::fs::rename(staging, install)?;
    if let Some(parent) = install.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn wait_for_expected_version(
    addr: SocketAddr,
    expected_commit: Option<&str>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe_version(addr, expected_commit).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    false
}

fn probe_version(addr: SocketAddr, expected_commit: Option<&str>) -> std::io::Result<bool> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET /version HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response = String::from_utf8_lossy(&response);
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return Ok(false);
    };
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Ok(false);
    }
    let Some(expected) = expected_commit else {
        return Ok(true);
    };
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    Ok(value
        .get("commitId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|actual| commits_match(actual, expected)))
}

fn commits_match(actual: &str, expected: &str) -> bool {
    let actual = actual.trim().to_ascii_lowercase();
    let expected = expected.trim().to_ascii_lowercase();
    let prefix = actual.len().min(expected.len()).min(12);
    prefix >= 7 && actual[..prefix] == expected[..prefix]
}

fn loopback_health_addr(addr: SocketAddr) -> SocketAddr {
    if addr.ip().is_unspecified() {
        let ip = match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        };
        SocketAddr::new(ip, addr.port())
    } else {
        addr
    }
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), SelfUpdateError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|err| SelfUpdateError::Internal(format!("serialize helper spec failed: {err}")))?;
    let tmp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|err| SelfUpdateError::Internal(format!("write helper spec failed: {err}")))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|err| SelfUpdateError::Internal(format!("flush helper spec failed: {err}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|err| SelfUpdateError::Internal(format!("commit helper spec failed: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn loopback_health_address_replaces_unspecified_ip() {
        assert_eq!(
            loopback_health_addr("0.0.0.0:15721".parse().unwrap()),
            "127.0.0.1:15721".parse().unwrap()
        );
    }

    #[test]
    fn commit_matching_accepts_full_and_short_ids() {
        assert!(commits_match(
            "c276bd37b4b6a31bd0d41e99c9b1feef388faf8f",
            "c276bd37b4b6"
        ));
        assert!(!commits_match("c276bd37b4b6", "aaaaaaaaaaaa"));
    }

    #[test]
    fn staged_install_renames_on_same_filesystem() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "cc-switch-install-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let install = dir.join("cc-switch-server");
        let staging = dir.join(".cc-switch-server.new");
        std::fs::write(&install, b"old").unwrap();
        std::fs::write(&staging, b"new").unwrap();

        install_staged_binary(&staging, &install).unwrap();

        assert_eq!(std::fs::read(&install).unwrap(), b"new");
        assert!(!staging.exists());
        assert_ne!(
            std::fs::metadata(&install).unwrap().permissions().mode() & 0o100,
            0
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn version_probe_requires_expected_commit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request);
            let body = r#"{"commitId":"c276bd37b4b6a31bd0d41e99c9b1feef388faf8f"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        assert!(probe_version(addr, Some("c276bd37b4b6")).unwrap());
        server.join().unwrap();
    }
}
