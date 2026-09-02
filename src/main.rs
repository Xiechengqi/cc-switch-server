use std::sync::Arc;

use anyhow::Context;
use cc_switch_server::cli::{Cli, Command};
use cc_switch_server::logging::{LogCapture, RING_BUFFER_CAPACITY};
use cc_switch_server::state::ServerStateInner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_args();
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
    let state =
        ServerStateInner::load(cli.clone(), log_capture).context("initialize server state")?;
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
    cc_switch_server::state::spawn_audit_log_uploader(state.clone());
    cc_switch_server::state::spawn_router_share_log_sync_worker(state.clone());
    cc_switch_server::state::spawn_periodic_backups(state.clone());
    cc_switch_server::state::spawn_periodic_share_sync_retry(state.clone());
    cc_switch_server::state::spawn_auto_upgrade_scheduler(state.clone());
    cc_switch_server::state::spawn_periodic_installation_status_report(state.clone());
    cc_switch_server::state::spawn_installation_upgrade_task_reporter(state.clone());
    cc_switch_server::state::spawn_account_quota_refresh(state.clone());
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
    cc_switch_server::api::serve(state).await
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
