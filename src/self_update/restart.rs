use std::process::{Command, Stdio};

use crate::self_update::version::{
    detect_service_status, SelfUpdateError, ServiceManager, BINARY_INSTALL_PATH, SERVICE_LOG_PATH,
    SERVICE_UNIT,
};

#[derive(Debug, Clone, Copy)]
pub enum RestartStrategy {
    Systemd,
    Nohup,
}

impl RestartStrategy {
    pub fn from_manager(manager: ServiceManager) -> Self {
        match manager {
            ServiceManager::Systemd => RestartStrategy::Systemd,
            ServiceManager::Nohup => RestartStrategy::Nohup,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            RestartStrategy::Systemd => "systemd",
            RestartStrategy::Nohup => "nohup",
        }
    }
}

pub fn schedule_restart(strategy: RestartStrategy) -> Result<String, SelfUpdateError> {
    let script = render_restart_script(strategy);
    spawn_detached(&script)?;
    Ok(script)
}

fn render_restart_script(strategy: RestartStrategy) -> String {
    let pid = std::process::id();
    match strategy {
        RestartStrategy::Systemd => format!(
            "sleep 1; \
             mkdir -p $(dirname {log}) 2>/dev/null || true; \
             : > {log} 2>/dev/null || true; \
             /bin/systemctl restart {unit}",
            unit = SERVICE_UNIT,
            log = SERVICE_LOG_PATH,
        ),
        RestartStrategy::Nohup => format!(
            "sleep 1; \
             mkdir -p $(dirname {log}) 2>/dev/null || true; \
             : > {log} 2>/dev/null || true; \
             kill -TERM {pid} 2>/dev/null; \
             for i in $(seq 1 60); do \
                 if ! kill -0 {pid} 2>/dev/null; then break; fi; \
                 sleep 0.2; \
             done; \
             touch {log} 2>/dev/null || true; \
             nohup {bin} >> {log} 2>&1 &",
            pid = pid,
            log = SERVICE_LOG_PATH,
            bin = BINARY_INSTALL_PATH,
        ),
    }
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

pub fn restart_from_detected_service() -> Result<String, SelfUpdateError> {
    let strategy = RestartStrategy::from_manager(detect_service_status().manager);
    schedule_restart(strategy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_script_references_unit() {
        let script = render_restart_script(RestartStrategy::Systemd);
        assert!(script.contains("systemctl restart cc-switch-server.service"));
        assert!(script.contains(": > /var/log/cc-switch-server.log"));
    }

    #[test]
    fn nohup_script_kills_and_reexecs() {
        let script = render_restart_script(RestartStrategy::Nohup);
        assert!(script.contains("kill -TERM"));
        assert!(script.contains("/usr/local/bin/cc-switch-server"));
    }
}
