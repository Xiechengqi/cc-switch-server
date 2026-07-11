use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::self_update::version::{
    is_containerized, SelfUpdateError, BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH,
    BINARY_STAGING_PATH, SERVICE_LOG_PATH, SERVICE_UNIT,
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
    #[serde(default)]
    service_unit: Option<String>,
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
    if current_service_unit().is_some() {
        RestartStrategy::Service
    } else {
        RestartStrategy::Standalone
    }
}

fn current_service_unit() -> Option<String> {
    std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|cgroup| service_unit_from_cgroup(&cgroup))
        .or_else(|| {
            service_main_pid(SERVICE_UNIT)
                .is_some_and(|pid| pid == std::process::id())
                .then(|| SERVICE_UNIT.to_string())
        })
}

fn service_unit_from_cgroup(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .filter_map(|line| line.rsplit_once(':').map(|(_, path)| path))
        .filter_map(|path| path.rsplit('/').find(|component| !component.is_empty()))
        .find(|component| component.ends_with(".service"))
        .map(str::to_string)
}

fn service_main_pid(unit: &str) -> Option<u32> {
    let output = Command::new("systemctl")
        .args(["show", "--property=MainPID", "--value", unit])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
}

pub fn schedule_upgrade_restart(
    task_id: &str,
    target_commit: &str,
    config_dir: &Path,
    health_addr: SocketAddr,
) -> Result<String, SelfUpdateError> {
    let service_unit = current_service_unit();
    launch_helper(UpdateHelperSpec {
        task_id: Some(task_id.to_string()),
        mode: HelperMode::InstallStaged,
        strategy: if service_unit.is_some() {
            RestartStrategy::Service
        } else {
            RestartStrategy::Standalone
        },
        service_unit,
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
    if pending.is_none() {
        if let Some(unit) = current_service_unit() {
            return schedule_systemd_restart(&unit).or_else(|systemd_error| {
                schedule_exec_restart().map(|command| {
                    format!("{command}; systemd scheduling fallback: {systemd_error}")
                })
            });
        }
        return schedule_exec_restart();
    }

    let service_unit = current_service_unit();
    launch_helper(UpdateHelperSpec {
        task_id: pending.as_ref().map(|value| value.0.clone()),
        mode: HelperMode::InstallStaged,
        strategy: if service_unit.is_some() {
            RestartStrategy::Service
        } else {
            RestartStrategy::Standalone
        },
        service_unit,
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: pending.map(|value| value.1),
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
        log_path: SERVICE_LOG_PATH.into(),
    })
}

fn schedule_systemd_restart(unit: &str) -> Result<String, SelfUpdateError> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let transient_unit = format!("cc-switch-server-restart-{}-{nonce}", std::process::id());
    let status = systemd_restart_command(unit, &transient_unit)
        .status()
        .map_err(|error| {
            SelfUpdateError::Internal(format!("schedule systemd restart failed: {error}"))
        })?;
    if !status.success() {
        return Err(SelfUpdateError::Internal(format!(
            "systemd-run restart scheduling exited with {status}"
        )));
    }
    Ok(format!(
        "systemd-run --unit {transient_unit} --on-active=1s systemctl restart --no-block {unit}"
    ))
}

fn systemd_restart_command(unit: &str, transient_unit: &str) -> Command {
    let mut command = Command::new("systemd-run");
    command
        .arg("--quiet")
        .arg("--collect")
        .arg("--unit")
        .arg(transient_unit)
        .arg("--on-active=1s")
        .arg("systemctl")
        .arg("restart")
        .arg("--no-block")
        .arg(unit);
    command
}

fn schedule_exec_restart() -> Result<String, SelfUpdateError> {
    let current_exe = std::env::current_exe().map_err(|error| {
        SelfUpdateError::Internal(format!("resolve current executable failed: {error}"))
    })?;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let command_label = format!("{} [current arguments] (exec)", current_exe.display());
    std::thread::Builder::new()
        .name("cc-switch-server-restart".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(750));
            use std::os::unix::process::CommandExt;
            let error = Command::new(&current_exe).args(&args).exec();
            tracing::error!(
                executable = %current_exe.display(),
                error = %error,
                "exec restart failed"
            );
        })
        .map_err(|error| {
            SelfUpdateError::Internal(format!("schedule exec restart failed: {error}"))
        })?;
    Ok(command_label)
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
    let service_unit = current_service_unit();
    launch_helper(UpdateHelperSpec {
        task_id: None,
        mode: HelperMode::Rollback,
        strategy: if service_unit.is_some() {
            RestartStrategy::Service
        } else {
            RestartStrategy::Standalone
        },
        service_unit,
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
    run_update_helper_inner_with_timeout(spec_path, spec, HEALTH_TIMEOUT)
}

fn run_update_helper_inner_with_timeout(
    spec_path: &Path,
    spec: &UpdateHelperSpec,
    health_timeout: Duration,
) -> anyhow::Result<()> {
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

    let replacement_result = match restart_error.as_ref() {
        Some(error) => Err(format!("restart failed: {error}")),
        None => wait_for_expected_version(
            spec.health_addr,
            spec.expected_commit.as_deref(),
            Some(spec.parent_pid),
            health_timeout,
        ),
    };
    if replacement_result.is_ok() {
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
    let replacement_error =
        replacement_result.expect_err("failed replacement must carry probe or restart diagnostics");
    if let Some(task_id) = spec.task_id.as_deref() {
        crate::self_update::upgrade::record_helper_outcome(
            &spec.config_dir,
            task_id,
            false,
            &format!("replacement failed: {replacement_error}; attempting rollback"),
        )?;
    }
    let rollback_result = match rollback_source.as_deref().filter(|path| path.exists()) {
        Some(source) => (|| -> anyhow::Result<()> {
            std::fs::copy(source, &spec.staging_path)?;
            install_staged_binary(&spec.staging_path, &spec.install_path)?;
            let _ = restart_process(spec)?;
            wait_for_expected_version(spec.health_addr, None, None, health_timeout)
                .map_err(anyhow::Error::msg)
        })()
        .map(|_| "rollback passed health checks".to_string())
        .unwrap_or_else(|error| format!("rollback failed: {error}")),
        None => "rollback was not available".to_string(),
    };
    if let Some(task_id) = spec.task_id.as_deref() {
        crate::self_update::upgrade::record_helper_outcome(
            &spec.config_dir,
            task_id,
            false,
            &format!("replacement failed: {replacement_error}; {rollback_result}"),
        )?;
    }
    anyhow::bail!(replacement_error)
}

fn restart_process(spec: &UpdateHelperSpec) -> anyhow::Result<Option<std::process::Child>> {
    match spec.strategy {
        RestartStrategy::Service => {
            let unit = spec
                .service_unit
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("systemd restart target is missing"))?;
            let status = Command::new("systemctl").args(["restart", unit]).status()?;
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
    replaced_pid: Option<u32>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut last_error = "version endpoint did not respond".to_string();
    while Instant::now() < deadline {
        match probe_version(addr, expected_commit, replaced_pid) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!(
        "health/version checks timed out after {}s; last probe: {last_error}",
        timeout.as_secs()
    ))
}

fn probe_version(
    addr: SocketAddr,
    expected_commit: Option<&str>,
    replaced_pid: Option<u32>,
) -> Result<(), String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|error| format!("connect {addr} failed: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("set probe timeout failed: {error}"))?;
    stream
        .write_all(b"GET /version HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("write version probe failed: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("read version probe failed: {error}"))?;
    let response = String::from_utf8_lossy(&response);
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return Err("version probe returned an invalid HTTP response".into());
    };
    let status_line = headers.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        return Err(format!("version probe returned {status_line}"));
    }
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("parse version response failed: {error}"))?;
    if let Some(replaced_pid) = replaced_pid {
        let actual_pid = value
            .get("processId")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| "version response is missing processId".to_string())?;
        if actual_pid == replaced_pid {
            return Err(format!(
                "version endpoint is still served by previous process {replaced_pid}"
            ));
        }
    }
    if let Some(expected) = expected_commit {
        let actual = value
            .get("commitId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !commits_match(actual, expected) {
            return Err(format!(
                "version commit mismatch: expected {expected}, got {}",
                if actual.is_empty() {
                    "missing commitId"
                } else {
                    actual
                }
            ));
        }
    }
    Ok(())
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
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
    fn service_unit_is_read_from_current_process_cgroup() {
        assert_eq!(
            service_unit_from_cgroup("0::/system.slice/cc-switch-server.service\n"),
            Some("cc-switch-server.service".into())
        );
        assert_eq!(
            service_unit_from_cgroup("0::/user.slice/user-1000.slice/session-3.scope\n"),
            None
        );
        assert_eq!(
            service_unit_from_cgroup(
                "0::/user.slice/user-1000.slice/user@1000.service/session.slice/app.scope\n"
            ),
            None
        );
    }

    #[test]
    fn systemd_restart_is_delayed_and_non_blocking() {
        let command =
            systemd_restart_command("cc-switch-server.service", "cc-switch-server-restart-test");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--quiet",
                "--collect",
                "--unit",
                "cc-switch-server-restart-test",
                "--on-active=1s",
                "systemctl",
                "restart",
                "--no-block",
                "cc-switch-server.service",
            ]
        );
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
            let body = r#"{"commitId":"c276bd37b4b6a31bd0d41e99c9b1feef388faf8f","processId":456}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        probe_version(addr, Some("c276bd37b4b6"), Some(123)).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn version_probe_reports_actual_commit_on_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request);
            let body = r#"{"commitId":"aaaaaaaaaaaa"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let error = probe_version(addr, Some("bbbbbbbbbbbb"), None)
            .expect_err("mismatched replacement must fail its version probe");
        assert!(error.contains("expected bbbbbbbbbbbb, got aaaaaaaaaaaa"));
        server.join().unwrap();
    }

    #[test]
    fn version_probe_rejects_previous_process_pid() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request);
            let body = r#"{"commitId":"c276bd37b4b6","processId":123}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let error = probe_version(addr, Some("c276bd37b4b6"), Some(123))
            .expect_err("the previous process must not satisfy restart health checks");
        assert!(error.contains("still served by previous process 123"));
        server.join().unwrap();
    }

    #[test]
    fn standalone_helper_rolls_back_mismatched_replacement_and_persists_diagnostics() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "cc-switch-helper-rollback-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let install_path = dir.join("cc-switch-server");
        let staging_path = dir.join(".cc-switch-server.new");
        let rollback_path = dir.join("cc-switch-server.bak");
        let spec_path = dir.join(HELPER_SPEC_FILENAME);
        let executable = b"#!/bin/sh\nexit 0\n";
        for path in [&install_path, &staging_path, &rollback_path] {
            std::fs::write(path, executable).unwrap();
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }
        let task_id = "rollback-diagnostics";
        let snapshot = crate::self_update::upgrade::UpgradeStatusSnapshot {
            task_id: task_id.into(),
            status: crate::self_update::upgrade::UpgradeStatus::Running,
            restart_pending: false,
            logs: Vec::new(),
            target_commit_id: Some("bbbbbbbbbbbb".into()),
            restart_after: true,
            updated_at: String::new(),
        };
        std::fs::write(
            dir.join("upgrade-state.json"),
            serde_json::to_vec_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let health_addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_server = stop.clone();
        let server = thread::spawn(move || {
            while !stop_server.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0u8; 512];
                        let _ = stream.read(&mut request);
                        let body = r#"{"commitId":"aaaaaaaaaaaa","processId":42}"#;
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("health listener failed: {error}"),
                }
            }
        });
        let spec = UpdateHelperSpec {
            task_id: Some(task_id.into()),
            mode: HelperMode::InstallStaged,
            strategy: RestartStrategy::Standalone,
            service_unit: None,
            parent_pid: u32::MAX,
            health_addr,
            expected_commit: Some("bbbbbbbbbbbb".into()),
            config_dir: dir.clone(),
            server_args: Vec::new(),
            install_path: install_path.clone(),
            staging_path,
            rollback_path,
            log_path: dir.join("server.log"),
        };

        let error =
            run_update_helper_inner_with_timeout(&spec_path, &spec, Duration::from_millis(100))
                .expect_err("mismatched replacement must roll back");
        assert!(error.to_string().contains("version commit mismatch"));
        let persisted: crate::self_update::upgrade::UpgradeStatusSnapshot =
            serde_json::from_slice(&std::fs::read(dir.join("upgrade-state.json")).unwrap())
                .unwrap();
        assert_eq!(
            persisted.status,
            crate::self_update::upgrade::UpgradeStatus::Failed
        );
        assert!(persisted.logs.iter().any(|entry| {
            entry
                .message
                .contains("expected bbbbbbbbbbbb, got aaaaaaaaaaaa; rollback passed health checks")
        }));
        assert_eq!(std::fs::read(&install_path).unwrap(), executable);

        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
