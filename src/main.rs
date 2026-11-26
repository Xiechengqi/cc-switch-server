use std::sync::Arc;

use anyhow::Context;
use cc_switch_server::cli::{Cli, Command};
use cc_switch_server::logging::{LogCapture, RING_BUFFER_CAPACITY};
use cc_switch_server::state::ServerStateInner;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_args();
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    // The unoptimized shared forwarder alone uses over 2 MiB per poll. Give
    // debug workers enough headroom without changing release stack sizes or
    // overriding an explicitly larger RUST_MIN_STACK used by local fixtures.
    #[cfg(debug_assertions)]
    runtime.thread_stack_size(
        std::env::var("RUST_MIN_STACK")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default()
            .max(8 * 1024 * 1024),
    );
    runtime
        .build()
        .context("initialize async runtime")?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let log_capture = Arc::new(LogCapture::new(RING_BUFFER_CAPACITY));
    init_tracing(&cli.log_level, log_capture.clone());

    match cli.effective_command() {
        Command::Serve => serve(cli, log_capture).await,
        Command::Config { command } => cc_switch_server::admin::run_config_command(&cli, command),
        Command::Doctor {
            check_port,
            startup_contracts_only,
        } => cc_switch_server::admin::run_doctor(&cli, check_port, startup_contracts_only),
        Command::Version { json } => print_version(json),
        Command::Password { command } => {
            cc_switch_server::admin::run_password_command(&cli, command)
        }
        Command::Init(args) => cc_switch_server::setup::run_cli_init(&cli, args).await,
        Command::SelfUpdateHelper { spec } => {
            cc_switch_server::self_update::restart::run_update_helper(&spec)
        }
    }
}

async fn serve(cli: Cli, log_capture: Arc<LogCapture>) -> anyhow::Result<()> {
    cc_switch_server::metrics::init()?;
    cc_switch_server::provider_identity::spawn_antigravity_version_updater();
    cc_switch_server::state::enable_fresh_install_sqlite_authority();
    let state =
        ServerStateInner::load(cli.clone(), log_capture).context("initialize server state")?;
    if let Some(id) = &state.market_workspace.boot_lease {
        let _ = cc_switch_server::market_lease::PROCESS_LEASE.set(id.clone());
        let sealing = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tick.tick().await;
                if let Err(error) = sealing
                    .market_workspace
                    .seal_expired(chrono::Utc::now().timestamp())
                {
                    tracing::error!(%error,"persist workspace expiry");
                }
                let now = chrono::Utc::now().timestamp();
                if sealing
                    .market_workspace
                    .access()
                    .1
                    .is_some_and(|g| g.expires_at.saturating_add(300) <= now)
                {
                    match sealing.reap_expired_workspace(now).await {
                        Ok(true) => {
                            sealing.begin_shutdown();
                            break;
                        }
                        Err(e) => tracing::warn!(error=%e,"defer expired workspace cleanup"),
                        _ => {}
                    }
                }
            }
        });
    }
    state.sync_log_config_from_ui_settings().await;
    let config = state.config_snapshot().await;
    let installation_id = config
        .registered_router_identity()
        .map(|identity| identity.installation_id.as_str())
        .unwrap_or("unregistered");
    tracing::info!(
        process_id = std::process::id(),
        process_instance_id = %state.process_instance_id,
        installation_id,
        "server process log started"
    );
    let claude_identity = cc_switch_server::domain::claude_cli::claude_cli_identity();
    tracing::info!(
        wire_profile_id = cc_switch_server::domain::claude_cli::CLAUDE_WIRE_PROFILE.id,
        profile_cli_version = cc_switch_server::domain::claude_cli::CLAUDE_WIRE_PROFILE.claude_code_version,
        effective_cli_version = %claude_identity.version,
        identity_source = claude_identity.source,
        stale_override_rejected = claude_identity.stale_override_rejected,
        "resolved Claude OAuth wire identity"
    );
    if claude_identity.override_conflict {
        tracing::warn!(
            identity_source = claude_identity.source,
            "both CC_SWITCH_CLI_UA and CC_SWITCH_CLI_UA_VERSION are set; resolved source is shown above"
        );
    }
    if claude_identity.stale_override_rejected {
        tracing::warn!(
            profile_cli_version = cc_switch_server::domain::claude_cli::CLAUDE_WIRE_PROFILE.claude_code_version,
            effective_cli_version = %claude_identity.version,
            "ignored a Claude CLI identity override older than the audited wire profile"
        );
    }
    cc_switch_server::state::restore_tunnels(state.clone()).await;
    cc_switch_server::state::spawn_public_ip_discovery(state.clone());
    cc_switch_server::state::spawn_installation_heartbeat(state.clone());
    if state.market_workspace.boot_lease.is_none() {
        cc_switch_server::state::spawn_audit_log_uploader(state.clone());
    }
    cc_switch_server::state::spawn_router_share_log_sync_worker(state.clone());
    cc_switch_server::state::spawn_periodic_backups(state.clone());
    cc_switch_server::state::spawn_periodic_share_sync_retry(state.clone());
    cc_switch_server::state::spawn_value_quota_sweeper(state.clone());
    cc_switch_server::state::spawn_auto_upgrade_scheduler(state.clone());
    cc_switch_server::state::spawn_periodic_installation_status_report(state.clone());
    cc_switch_server::state::spawn_installation_upgrade_task_reporter(state.clone());
    cc_switch_server::state::spawn_account_quota_refresh(state.clone());
    cc_switch_server::state::spawn_account_checkin(state.clone());
    cc_switch_server::state::spawn_cursor_account_refresh(state.clone());
    cc_switch_server::state::spawn_codex_cli_version_sync(state.clone());
    let status_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) =
            cc_switch_server::state::report_installation_upgrade_status(&status_state).await
        {
            tracing::warn!(error = %error, "initial installation upgrade status report failed");
        }
    });
    cc_switch_server::state::spawn_share_edit_event_listener(state.clone());
    cc_switch_server::api::serve(state.clone()).await?;
    if state.market_workspace.switch_required() {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(workspace_switch_executable()?)
            .args(std::env::args_os().skip(1))
            .exec();
        return Err(error.into());
    }
    Ok(())
}

/// An upgrade that replaced the binary in place without restarting leaves
/// `/proc/self/exe` pointing at "<path> (deleted)". The replacement at the
/// original path is the installed version, so the switch executes that file.
fn workspace_switch_executable() -> anyhow::Result<std::path::PathBuf> {
    let current = std::env::current_exe()?;
    Ok(replaced_executable_path(&current)
        .filter(|path| path.is_file())
        .unwrap_or(current))
}

fn replaced_executable_path(current: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    current
        .as_os_str()
        .as_bytes()
        .strip_suffix(b" (deleted)")
        .map(|path| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(path)))
}

fn init_tracing(log_level: &str, capture: Arc<LogCapture>) {
    cc_switch_server::logging::init_tracing(log_level, capture);
}

fn print_version(json: bool) -> anyhow::Result<()> {
    let info = cc_switch_server::build_info::build_info();
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("{}", info.format_human());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_switch_follows_an_in_place_upgrade() {
        assert_eq!(
            super::replaced_executable_path(std::path::Path::new(
                "/usr/local/bin/cc-switch-server (deleted)"
            )),
            Some("/usr/local/bin/cc-switch-server".into())
        );
        assert_eq!(
            super::replaced_executable_path(std::path::Path::new(
                "/usr/local/bin/cc-switch-server"
            )),
            None
        );
    }
}
