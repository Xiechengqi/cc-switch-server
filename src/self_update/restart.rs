use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::self_update::version::{
    is_containerized, run_command_with_timeout, SelfUpdateError, BINARY_INSTALL_PATH,
    BINARY_ROLLBACK_PATH, BINARY_STAGING_PATH, SERVICE_NAME, SERVICE_UNIT,
};

const HELPER_SPEC_FILENAME: &str = "upgrade-helper.json";
const RESTART_OPERATION_FILENAME: &str = "restart-operation.json";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(45);
const PROCESS_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(45);
const PROCESS_KILL_TIMEOUT: Duration = Duration::from_secs(5);
const SERVICE_DETECTION_TIMEOUT: Duration = Duration::from_secs(3);
const SERVICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(55);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartStrategy {
    Systemd,
    OpenRc,
    Standalone,
}

impl RestartStrategy {
    pub fn label(&self) -> &'static str {
        match self {
            RestartStrategy::Systemd => "systemd",
            RestartStrategy::OpenRc => "openrc",
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
    operation_id: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartOperationSnapshot {
    pub operation_id: String,
    pub status: String,
    pub stage: String,
    pub strategy: RestartStrategy,
    pub old_pid: u32,
    pub new_pid: Option<u32>,
    pub requested_at: String,
    pub updated_at: String,
    pub message: String,
}

#[derive(Debug)]
pub struct RestartSchedule {
    pub operation_id: String,
    pub command: String,
}

pub fn read_restart_operation(config_dir: &Path) -> Option<RestartOperationSnapshot> {
    serde_json::from_slice(&std::fs::read(config_dir.join(RESTART_OPERATION_FILENAME)).ok()?).ok()
}

fn update_restart_operation(
    config_dir: &Path,
    operation_id: &str,
    status: &str,
    stage: &str,
    message: impl Into<String>,
    new_pid: Option<u32>,
) -> anyhow::Result<()> {
    let path = config_dir.join(RESTART_OPERATION_FILENAME);
    let mut snapshot: RestartOperationSnapshot = serde_json::from_slice(&std::fs::read(&path)?)?;
    if snapshot.operation_id != operation_id {
        return Ok(());
    }
    snapshot.status = status.to_string();
    snapshot.stage = stage.to_string();
    snapshot.message = message.into();
    snapshot.updated_at = chrono::Utc::now().to_rfc3339();
    if new_pid.is_some() {
        snapshot.new_pid = new_pid;
    }
    write_json_atomic(&path, &snapshot).map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn new_operation_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn detect_restart_strategy() -> RestartStrategy {
    detect_restart_target().0
}

fn detect_restart_target() -> (RestartStrategy, Option<String>) {
    choose_restart_target(current_systemd_service_unit(), openrc_service_running())
}

fn choose_restart_target(
    systemd_unit: Option<String>,
    openrc_available: bool,
) -> (RestartStrategy, Option<String>) {
    if let Some(unit) = systemd_unit {
        return (RestartStrategy::Systemd, Some(unit));
    }
    if openrc_available {
        return (RestartStrategy::OpenRc, Some(SERVICE_NAME.to_string()));
    }
    (RestartStrategy::Standalone, None)
}

fn current_systemd_service_unit() -> Option<String> {
    std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|cgroup| service_unit_from_cgroup(&cgroup))
        .or_else(|| {
            service_main_pid(SERVICE_UNIT)
                .is_some_and(|pid| pid == std::process::id())
                .then(|| SERVICE_UNIT.to_string())
        })
}

fn openrc_service_running() -> bool {
    if !Path::new("/etc/init.d").join(SERVICE_NAME).is_file() || !command_exists("rc-service") {
        return false;
    }
    let mut command = Command::new("rc-service");
    command.args([SERVICE_NAME, "status"]);
    run_command_with_timeout(command, SERVICE_DETECTION_TIMEOUT)
        .is_ok_and(|output| output.status.success())
}

fn command_exists(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(command).is_file())
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
    let mut command = Command::new("systemctl");
    command.args(["show", "--property=MainPID", "--value", unit]);
    let output = run_command_with_timeout(command, SERVICE_DETECTION_TIMEOUT).ok()?;
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
    let (strategy, service_unit) = detect_restart_target();
    launch_helper(UpdateHelperSpec {
        operation_id: new_operation_id(),
        task_id: Some(task_id.to_string()),
        mode: HelperMode::InstallStaged,
        strategy,
        service_unit,
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: Some(target_commit.to_string()),
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
    })
}

pub fn restart_from_detected_service(
    config_dir: &Path,
    health_addr: SocketAddr,
) -> Result<RestartSchedule, SelfUpdateError> {
    if is_containerized() {
        return Err(SelfUpdateError::Forbidden(
            "in-process restart is disabled in containers; restart the container instead".into(),
        ));
    }
    let pending = pending_upgrade(config_dir);
    let current_exe = std::env::current_exe().map_err(|error| {
        SelfUpdateError::Internal(format!("resolve current executable failed: {error}"))
    })?;
    let install_path = restart_install_path(pending.is_some(), current_exe);
    let (strategy, service_unit) = detect_restart_target();
    let operation_id = new_operation_id();
    let command = launch_helper(UpdateHelperSpec {
        operation_id: operation_id.clone(),
        task_id: pending.as_ref().map(|value| value.0.clone()),
        mode: if pending.is_some() {
            HelperMode::InstallStaged
        } else {
            HelperMode::RestartOnly
        },
        strategy,
        service_unit,
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: pending
            .map(|value| value.1)
            .or_else(|| Some(crate::build_info::build_info().commit_id.to_string())),
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path,
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
    })?;
    Ok(RestartSchedule {
        operation_id,
        command,
    })
}

fn restart_install_path(has_pending_upgrade: bool, current_exe: PathBuf) -> PathBuf {
    if has_pending_upgrade {
        PathBuf::from(BINARY_INSTALL_PATH)
    } else {
        current_exe
    }
}

fn pending_upgrade(config_dir: &Path) -> Option<(String, String)> {
    pending_upgrade_at(config_dir, Path::new(BINARY_STAGING_PATH))
}

fn pending_upgrade_at(config_dir: &Path, staging_path: &Path) -> Option<(String, String)> {
    if !staging_path.is_file() {
        return None;
    }
    let snapshot: crate::self_update::upgrade::UpgradeStatusSnapshot =
        serde_json::from_slice(&std::fs::read(config_dir.join("upgrade-state.json")).ok()?).ok()?;
    if snapshot.status != crate::self_update::upgrade::UpgradeStatus::Success
        || !snapshot.restart_pending
    {
        return None;
    }
    let task_id = snapshot.task_id;
    let target_commit = snapshot.target_commit_id?;
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
    let (strategy, service_unit) = detect_restart_target();
    launch_helper(UpdateHelperSpec {
        operation_id: new_operation_id(),
        task_id: None,
        mode: HelperMode::Rollback,
        strategy,
        service_unit,
        parent_pid: std::process::id(),
        health_addr: loopback_health_addr(health_addr),
        expected_commit: None,
        config_dir: config_dir.to_path_buf(),
        server_args: std::env::args().skip(1).collect(),
        install_path: BINARY_INSTALL_PATH.into(),
        staging_path: BINARY_STAGING_PATH.into(),
        rollback_path: BINARY_ROLLBACK_PATH.into(),
    })
}

fn launch_helper(spec: UpdateHelperSpec) -> Result<String, SelfUpdateError> {
    crate::logging::ensure_log_dir(&spec.config_dir).map_err(|error| {
        SelfUpdateError::Internal(format!(
            "initialize restart log directory under {} failed: {error}",
            spec.config_dir.display()
        ))
    })?;
    let now = chrono::Utc::now().to_rfc3339();
    write_json_atomic(
        &spec.config_dir.join(RESTART_OPERATION_FILENAME),
        &RestartOperationSnapshot {
            operation_id: spec.operation_id.clone(),
            status: "running".into(),
            stage: "helper_spawning".into(),
            strategy: spec.strategy,
            old_pid: spec.parent_pid,
            new_pid: None,
            requested_at: now.clone(),
            updated_at: now,
            message: "restart helper is being launched".into(),
        },
    )?;
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

    if let Err(error) = spawn_detached_helper(&spec, &current_exe, &spec_path) {
        if read_restart_operation(&spec.config_dir).is_some_and(|operation| {
            operation.operation_id == spec.operation_id && operation.status == "running"
        }) {
            let _ = update_restart_operation(
                &spec.config_dir,
                &spec.operation_id,
                "failed",
                "helper_spawn_failed",
                error.to_string(),
                None,
            );
        }
        return Err(error);
    }
    Ok(command_label)
}

fn spawn_detached_helper(
    spec: &UpdateHelperSpec,
    current_exe: &Path,
    spec_path: &Path,
) -> Result<(), SelfUpdateError> {
    match spec.strategy {
        RestartStrategy::Systemd => {
            let suffix = spec
                .task_id
                .as_deref()
                .unwrap_or("manual")
                .chars()
                .take(12)
                .collect::<String>();
            let transient_unit = format!("cc-switch-server-update-{suffix}");
            let mut command = Command::new("systemd-run");
            command
                .args(["--quiet", "--collect", "--property=Type=exec"])
                .arg(format!("--unit={transient_unit}"))
                .arg(current_exe)
                .arg("self-update-helper")
                .arg("--spec")
                .arg(spec_path);
            let launch = run_command_with_timeout(command, SERVICE_COMMAND_TIMEOUT);
            if launch.as_ref().is_ok_and(|output| output.status.success())
                && wait_for_helper_start_confirmation(spec, Duration::from_secs(2))
            {
                return Ok(());
            }
            if helper_start_confirmed(spec) {
                return Ok(());
            }
            stop_transient_helper(&transient_unit);
            Err(match launch {
                Ok(output) if output.status.success() => SelfUpdateError::Internal(
                    "systemd update helper did not confirm startup within 2s".into(),
                ),
                Ok(output) => SelfUpdateError::Internal(format!(
                    "systemd-run update helper exited with {}",
                    output.status
                )),
                Err(error) => SelfUpdateError::Internal(format!(
                    "launch systemd update helper failed: {error}"
                )),
            })
        }
        RestartStrategy::OpenRc | RestartStrategy::Standalone => {
            use std::os::unix::process::CommandExt;

            let helper_log_path = crate::logging::restart_helper_log_path(&spec.config_dir);
            let helper_log = crate::logging::open_log_append(&helper_log_path).map_err(|err| {
                SelfUpdateError::Internal(format!(
                    "open restart helper log {} failed: {err}",
                    helper_log_path.display()
                ))
            })?;
            let helper_error_log = helper_log.try_clone().map_err(|err| {
                SelfUpdateError::Internal(format!("clone restart helper log failed: {err}"))
            })?;
            let mut command = Command::new(current_exe);
            command
                .arg("self-update-helper")
                .arg("--spec")
                .arg(spec_path)
                .stdin(Stdio::null())
                .stdout(Stdio::from(helper_log))
                .stderr(Stdio::from(helper_error_log));
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().map_err(|err| {
                SelfUpdateError::Internal(format!("spawn update helper failed: {err}"))
            })?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if helper_start_confirmed(spec) {
                    return Ok(());
                }
                if let Some(status) = child.try_wait().map_err(|err| {
                    SelfUpdateError::Internal(format!("inspect update helper failed: {err}"))
                })? {
                    return Err(SelfUpdateError::Internal(format!(
                        "update helper exited before startup confirmation with {status}; see {}",
                        helper_log_path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = child.kill();
            Err(SelfUpdateError::Internal(format!(
                "update helper did not confirm startup within 2s; see {}",
                helper_log_path.display()
            )))
        }
    }
}

fn helper_start_confirmed(spec: &UpdateHelperSpec) -> bool {
    read_restart_operation(&spec.config_dir).is_some_and(|operation| {
        operation.operation_id == spec.operation_id
            && operation.status == "running"
            && operation.stage != "helper_spawning"
    })
}

fn wait_for_helper_start_confirmation(spec: &UpdateHelperSpec, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if helper_start_confirmed(spec) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    helper_start_confirmed(spec)
}

fn stop_transient_helper(unit: &str) {
    let mut command = Command::new("systemctl");
    command.args(["stop", "--no-block", unit]);
    let _ = run_command_with_timeout(command, SERVICE_DETECTION_TIMEOUT);
}

pub fn run_update_helper(spec_path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(spec_path)?;
    let spec: UpdateHelperSpec = serde_json::from_slice(&bytes)?;
    let result = run_update_helper_inner(spec_path, &spec);
    if let Err(error) = &result {
        if read_restart_operation(&spec.config_dir)
            .is_some_and(|operation| operation.status == "running")
        {
            let _ = update_restart_operation(
                &spec.config_dir,
                &spec.operation_id,
                "failed",
                "helper_failed",
                format!("restart helper failed: {error}"),
                None,
            );
        }
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
    update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "running",
        "helper_started",
        "detached restart helper started",
        None,
    )?;
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

    stop_process(spec)?;
    if !matches!(spec.mode, HelperMode::RestartOnly) {
        if let Err(error) = install_staged_binary(&spec.staging_path, &spec.install_path) {
            let recovery = start_process(spec)
                .map(|_| "previous binary restarted".to_string())
                .unwrap_or_else(|restart_error| {
                    format!("previous binary restart also failed: {restart_error}")
                });
            anyhow::bail!("install staged binary failed: {error}; {recovery}");
        }
    }
    let (mut started_process, restart_error) = match start_process(spec) {
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
    if let Ok(new_pid) = replacement_result {
        update_restart_operation(
            &spec.config_dir,
            &spec.operation_id,
            "success",
            "health_check_passed",
            "replacement process passed health and version checks",
            Some(new_pid),
        )?;
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

    stop_replacement_process(spec, started_process.as_mut());
    let replacement_error =
        replacement_result.expect_err("failed replacement must carry probe or restart diagnostics");
    update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "failed",
        "replacement_failed",
        &replacement_error,
        None,
    )?;
    if let Some(task_id) = spec.task_id.as_deref() {
        crate::self_update::upgrade::record_helper_outcome(
            &spec.config_dir,
            task_id,
            false,
            &format!("replacement failed: {replacement_error}; attempting rollback"),
        )?;
    }
    let (rollback_stage, rollback_result) =
        match rollback_source.as_deref().filter(|path| path.exists()) {
            Some(source) => match (|| -> anyhow::Result<()> {
                std::fs::copy(source, &spec.staging_path)?;
                install_staged_binary(&spec.staging_path, &spec.install_path)?;
                let _ = start_process(spec)?;
                wait_for_expected_version(spec.health_addr, None, None, health_timeout)
                    .map(|_| ())
                    .map_err(anyhow::Error::msg)
            })() {
                Ok(()) => (
                    "rollback_succeeded",
                    "rollback passed health checks".to_string(),
                ),
                Err(error) => ("rollback_failed", format!("rollback failed: {error}")),
            },
            None => (
                "rollback_unavailable",
                "rollback was not available".to_string(),
            ),
        };
    let final_message = format!("replacement failed: {replacement_error}; {rollback_result}");
    let _ = update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "failed",
        rollback_stage,
        &final_message,
        None,
    );
    if let Some(task_id) = spec.task_id.as_deref() {
        crate::self_update::upgrade::record_helper_outcome(
            &spec.config_dir,
            task_id,
            false,
            &final_message,
        )?;
    }
    anyhow::bail!(final_message)
}

fn stop_process(spec: &UpdateHelperSpec) -> anyhow::Result<()> {
    update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "running",
        "old_process_exiting",
        format!("requesting process {} to exit", spec.parent_pid),
        None,
    )?;
    match spec.strategy {
        RestartStrategy::Systemd => {
            let unit = spec
                .service_unit
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("systemd stop target is missing"))?;
            run_service_command("systemctl", &["stop", unit])?;
        }
        RestartStrategy::OpenRc => {
            let service = spec
                .service_unit
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("OpenRC stop target is missing"))?;
            run_service_command("rc-service", &[service, "stop"])?;
        }
        RestartStrategy::Standalone => {
            signal_process(spec.parent_pid, libc::SIGTERM)?;
        }
    }
    let forced = wait_for_process_exit(spec.parent_pid)?;
    update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "running",
        "old_process_stopped",
        if forced {
            format!(
                "process {} exceeded the graceful shutdown deadline and was killed",
                spec.parent_pid
            )
        } else {
            format!("process {} stopped cleanly", spec.parent_pid)
        },
        None,
    )?;
    Ok(())
}

fn start_process(spec: &UpdateHelperSpec) -> anyhow::Result<Option<std::process::Child>> {
    let child = match spec.strategy {
        RestartStrategy::Systemd => {
            let unit = spec
                .service_unit
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("systemd start target is missing"))?;
            run_service_command("systemctl", &["start", unit])?;
            None
        }
        RestartStrategy::OpenRc => {
            let service = spec
                .service_unit
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("OpenRC start target is missing"))?;
            run_service_command("rc-service", &[service, "start"])?;
            None
        }
        RestartStrategy::Standalone => {
            crate::logging::ensure_log_dir(&spec.config_dir)?;
            let log_path = crate::logging::process_log_path(&spec.config_dir);
            let log = crate::logging::open_log_append(&log_path)?;
            let err_log = log.try_clone()?;
            let child = Command::new(&spec.install_path)
                .args(&spec.server_args)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(err_log))
                .spawn()?;
            Some(child)
        }
    };
    let child_pid = child.as_ref().map(std::process::Child::id);
    update_restart_operation(
        &spec.config_dir,
        &spec.operation_id,
        "running",
        "replacement_started",
        match spec.strategy {
            RestartStrategy::Systemd => "systemd service started; waiting for health checks".into(),
            RestartStrategy::OpenRc => "OpenRC service started; waiting for health checks".into(),
            RestartStrategy::Standalone => format!(
                "replacement process {} started; waiting for health checks",
                child_pid.unwrap_or_default()
            ),
        },
        child_pid,
    )?;
    Ok(child)
}

fn stop_replacement_process(spec: &UpdateHelperSpec, child: Option<&mut std::process::Child>) {
    match spec.strategy {
        RestartStrategy::Systemd => {
            if let Some(unit) = spec.service_unit.as_deref() {
                let _ = run_service_command("systemctl", &["stop", unit]);
            }
        }
        RestartStrategy::OpenRc => {
            if let Some(service) = spec.service_unit.as_deref() {
                let _ = run_service_command("rc-service", &[service, "stop"]);
            }
        }
        RestartStrategy::Standalone => {
            if let Some(child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn run_service_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let mut child = Command::new(program).args(args).spawn()?;
    let deadline = Instant::now() + SERVICE_COMMAND_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::ensure!(
                status.success(),
                "{program} {} exited with {status}",
                args.join(" ")
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "{program} {} timed out after {}s",
                args.join(" "),
                SERVICE_COMMAND_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn signal_process(pid: u32, signal: i32) -> anyhow::Result<()> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    if !process_is_active(&proc_dir) {
        return Ok(());
    }
    let pid = i32::try_from(pid).map_err(|_| anyhow::anyhow!("process id is out of range"))?;
    let result = unsafe { libc::kill(pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error.into())
}

fn wait_for_process_exit(pid: u32) -> anyhow::Result<bool> {
    wait_for_process_exit_with_timeouts(pid, PROCESS_GRACEFUL_TIMEOUT, PROCESS_KILL_TIMEOUT)
}

fn wait_for_process_exit_with_timeouts(
    pid: u32,
    graceful_timeout: Duration,
    kill_timeout: Duration,
) -> anyhow::Result<bool> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let graceful_deadline = Instant::now() + graceful_timeout;
    while Instant::now() < graceful_deadline && process_is_active(&proc_dir) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if !process_is_active(&proc_dir) {
        return Ok(false);
    }

    signal_process(pid, libc::SIGKILL)?;
    let kill_deadline = Instant::now() + kill_timeout;
    while Instant::now() < kill_deadline && process_is_active(&proc_dir) {
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::ensure!(
        !process_is_active(&proc_dir),
        "current server process remained active after SIGKILL"
    );
    Ok(true)
}

fn process_is_active(proc_dir: &Path) -> bool {
    match std::fs::read_to_string(proc_dir.join("stat")) {
        Ok(stat) => process_stat_is_active(&stat),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => proc_dir.exists(),
    }
}

fn process_stat_is_active(stat: &str) -> bool {
    stat.rsplit_once(") ")
        .and_then(|(_, rest)| rest.chars().next())
        .is_some_and(|state| !matches!(state, 'Z' | 'X' | 'x'))
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
) -> Result<u32, String> {
    let deadline = Instant::now() + timeout;
    let mut last_error = "version endpoint did not respond".to_string();
    while Instant::now() < deadline {
        match probe_version_pid(addr, expected_commit, replaced_pid) {
            Ok(pid) => return Ok(pid),
            Err(error) => last_error = error,
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!(
        "health/version checks timed out after {}s; last probe: {last_error}",
        timeout.as_secs()
    ))
}

#[cfg(test)]
fn probe_version(
    addr: SocketAddr,
    expected_commit: Option<&str>,
    replaced_pid: Option<u32>,
) -> Result<(), String> {
    probe_version_pid(addr, expected_commit, replaced_pid).map(|_| ())
}

fn probe_version_pid(
    addr: SocketAddr,
    expected_commit: Option<&str>,
    replaced_pid: Option<u32>,
) -> Result<u32, String> {
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
    let actual_pid = value
        .get("processId")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| "version response is missing processId".to_string())?;
    if let Some(replaced_pid) = replaced_pid {
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
    Ok(actual_pid)
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
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
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
    fn restart_target_prefers_systemd_then_openrc() {
        assert_eq!(
            choose_restart_target(Some("custom.service".into()), true),
            (RestartStrategy::Systemd, Some("custom.service".into()))
        );
        assert_eq!(
            choose_restart_target(None, true),
            (RestartStrategy::OpenRc, Some(SERVICE_NAME.into()))
        );
        assert_eq!(
            choose_restart_target(None, false),
            (RestartStrategy::Standalone, None)
        );
    }

    #[test]
    fn stuck_process_is_killed_after_graceful_deadline() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(50));
        signal_process(pid, libc::SIGTERM).unwrap();

        let forced = wait_for_process_exit_with_timeouts(
            pid,
            Duration::from_millis(50),
            Duration::from_secs(1),
        )
        .unwrap();

        assert!(forced);
        let _ = child.wait();
    }

    #[test]
    fn external_command_timeout_is_bounded() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 5"]);
        let started = Instant::now();

        let error = run_command_with_timeout(command, Duration::from_millis(50))
            .expect_err("stalled command must time out");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn external_command_timeout_preserves_success_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf helper-ready"]);

        let output = run_command_with_timeout(command, Duration::from_secs(1)).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"helper-ready");
    }

    #[test]
    fn process_state_treats_zombies_as_stopped() {
        assert!(process_stat_is_active("123 (cc-switch-server) S 1 2 3"));
        assert!(!process_stat_is_active("123 (cc-switch-server) Z 1 2 3"));
        assert!(!process_stat_is_active("123 (cc-switch-server) X 1 2 3"));
    }

    #[test]
    fn pending_upgrade_requires_success_and_restart_pending() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-pending-upgrade-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let staging = dir.join("cc-switch-server.new");
        std::fs::write(&staging, b"staged").unwrap();
        let mut snapshot = crate::self_update::upgrade::UpgradeStatusSnapshot {
            task_id: "pending-task".into(),
            status: crate::self_update::upgrade::UpgradeStatus::Failed,
            restart_pending: true,
            logs: Vec::new(),
            target_commit_id: Some("abcdef012345".into()),
            restart_after: false,
            updated_at: String::new(),
        };
        let state_path = dir.join("upgrade-state.json");
        std::fs::write(&state_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(pending_upgrade_at(&dir, &staging), None);

        snapshot.status = crate::self_update::upgrade::UpgradeStatus::Success;
        snapshot.restart_pending = false;
        std::fs::write(&state_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(pending_upgrade_at(&dir, &staging), None);

        snapshot.restart_pending = true;
        std::fs::write(&state_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(
            pending_upgrade_at(&dir, &staging),
            Some(("pending-task".into(), "abcdef012345".into()))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_restart_reexecutes_the_running_binary_path() {
        let running = PathBuf::from("/root/cc-switch-server");
        assert_eq!(restart_install_path(false, running.clone()), running);
        assert_eq!(
            restart_install_path(true, PathBuf::from("/tmp/dev-binary")),
            PathBuf::from(BINARY_INSTALL_PATH)
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
            let body = r#"{"commitId":"aaaaaaaaaaaa","processId":456}"#;
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
        let operation_id = "test-restart-operation".to_string();
        write_json_atomic(
            &dir.join(RESTART_OPERATION_FILENAME),
            &RestartOperationSnapshot {
                operation_id: operation_id.clone(),
                status: "running".into(),
                stage: "helper_spawning".into(),
                strategy: RestartStrategy::Standalone,
                old_pid: u32::MAX,
                new_pid: None,
                requested_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                message: "test".into(),
            },
        )
        .unwrap();
        let spec = UpdateHelperSpec {
            operation_id,
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
        let operation = read_restart_operation(&dir).unwrap();
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.stage, "rollback_succeeded");
        assert!(operation.message.contains("rollback passed health checks"));
        assert_eq!(std::fs::read(&install_path).unwrap(), executable);

        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
