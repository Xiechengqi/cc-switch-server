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
        Command::Doctor { check_port } => cc_switch_server::admin::run_doctor(&cli, check_port),
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
    let state =
        ServerStateInner::load(cli.clone(), log_capture).context("initialize server state")?;
    state.sync_log_config_from_ui_settings().await;
    cc_switch_server::state::restore_tunnels(state.clone()).await;
    cc_switch_server::state::spawn_public_ip_discovery(state.clone());
    cc_switch_server::state::spawn_installation_heartbeat(state.clone());
    cc_switch_server::state::spawn_periodic_backups(state.clone());
    cc_switch_server::state::spawn_periodic_share_sync_retry(state.clone());
    cc_switch_server::state::spawn_auto_upgrade_scheduler(state.clone());
    cc_switch_server::state::spawn_periodic_installation_status_report(state.clone());
    cc_switch_server::state::spawn_account_quota_refresh(state.clone());
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
