use std::path::Path;
use std::process::{Command, Stdio};

use crate::self_update::version::{
    service_cc_switch_started, SelfUpdateError, BINARY_INSTALL_PATH, BINARY_ROLLBACK_PATH,
    BINARY_STAGING_PATH, SERVICE_NAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartStrategy {
    Service,
    Nohup,
}

impl RestartStrategy {
    pub fn label(&self) -> &'static str {
        match self {
            RestartStrategy::Service => "service",
            RestartStrategy::Nohup => "nohup",
        }
    }
}

pub fn detect_restart_strategy() -> RestartStrategy {
    if service_cc_switch_started() {
        RestartStrategy::Service
    } else {
        RestartStrategy::Nohup
    }
}

pub fn schedule_restart(strategy: RestartStrategy) -> Result<String, SelfUpdateError> {
    let script = render_restart_script(strategy);
    spawn_detached(&script)?;
    Ok(script)
}

pub fn restart_from_detected_service() -> Result<String, SelfUpdateError> {
    schedule_restart(detect_restart_strategy())
}

pub fn rollback_from_backup_and_restart() -> Result<String, SelfUpdateError> {
    if !Path::new(BINARY_ROLLBACK_PATH).exists() {
        return Err(SelfUpdateError::Forbidden(
            "rollback backup not found at /tmp/cc-switch-server.bak".into(),
        ));
    }
    std::fs::copy(BINARY_ROLLBACK_PATH, BINARY_INSTALL_PATH).map_err(|err| {
        SelfUpdateError::Internal(format!(
            "restore rollback backup to {BINARY_INSTALL_PATH} failed: {err}"
        ))
    })?;
    chmod_install_binary()?;
    let strategy = detect_restart_strategy();
    let script = render_rollback_restart_script(strategy);
    spawn_detached(&script)?;
    Ok(script)
}

fn render_restart_script(strategy: RestartStrategy) -> String {
    match strategy {
        RestartStrategy::Service => format!(
            "sleep 3; \
             if [ -f {staging} ]; then mv -f {staging} {bin}; fi; \
             service {service} restart",
            staging = BINARY_STAGING_PATH,
            bin = BINARY_INSTALL_PATH,
            service = SERVICE_NAME,
        ),
        RestartStrategy::Nohup => format!(
            "sleep 3; \
             pkill -9 cc-switch-server 2>/dev/null || true; \
             if [ -f {staging} ]; then mv -f {staging} {bin}; fi; \
             chmod +x {bin} 2>/dev/null || true; \
             nohup {bin} >> {log} 2>&1 &",
            staging = BINARY_STAGING_PATH,
            bin = BINARY_INSTALL_PATH,
            log = crate::self_update::version::SERVICE_LOG_PATH,
        ),
    }
}

fn render_rollback_restart_script(strategy: RestartStrategy) -> String {
    match strategy {
        RestartStrategy::Service => format!("service {service} restart", service = SERVICE_NAME),
        RestartStrategy::Nohup => format!(
            "sleep 3; \
             pkill -9 cc-switch-server 2>/dev/null || true; \
             nohup {bin} >> {log} 2>&1 &",
            bin = BINARY_INSTALL_PATH,
            log = crate::self_update::version::SERVICE_LOG_PATH,
        ),
    }
}

fn chmod_install_binary() -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(BINARY_INSTALL_PATH).map_err(|err| {
        SelfUpdateError::Internal(format!("stat {BINARY_INSTALL_PATH} failed: {err}"))
    })?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(BINARY_INSTALL_PATH, perms).map_err(|err| {
        SelfUpdateError::Internal(format!("chmod {BINARY_INSTALL_PATH} failed: {err}"))
    })
}

fn spawn_detached(script: &str) -> Result<(), SelfUpdateError> {
    let result = Command::new("setsid")
        .args(["-f", "bash", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match result {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Command::new("bash")
                .args(["-c", &format!("({script}) </dev/null >/dev/null 2>&1 &")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| {
                    SelfUpdateError::Internal(format!("spawn restart child failed: {err}"))
                })?;
            Ok(())
        }
        Err(err) => Err(SelfUpdateError::Internal(format!(
            "spawn setsid restart child failed: {err}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_restart_script_moves_staging_and_restarts_service() {
        let script = render_restart_script(RestartStrategy::Service);
        assert!(script.contains("sleep 3"));
        assert!(script.contains("/tmp/cc-switch-server"));
        assert!(script.contains("service cc-switch-server restart"));
    }

    #[test]
    fn nohup_restart_script_matches_manual_ops_flow() {
        let script = render_restart_script(RestartStrategy::Nohup);
        assert!(script.contains("sleep 3"));
        assert!(script.contains("pkill -9 cc-switch-server"));
        assert!(script.contains("mv -f /tmp/cc-switch-server /usr/local/bin/cc-switch-server"));
        assert!(script.contains("nohup /usr/local/bin/cc-switch-server"));
        assert!(script.contains("/var/log/cc-switch-server.log"));
    }

    #[test]
    fn rollback_nohup_script_restarts_installed_binary() {
        let script = render_rollback_restart_script(RestartStrategy::Nohup);
        assert!(script.contains("pkill -9 cc-switch-server"));
        assert!(script.contains("nohup /usr/local/bin/cc-switch-server"));
        assert!(script.contains("/var/log/cc-switch-server.log"));
    }
}
