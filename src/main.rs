use anyhow::Context;
use cc_switch_server::cli::{Cli, Command};
use cc_switch_server::state::ServerStateInner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_args();
    init_tracing(&cli.log_level);

    match cli.effective_command() {
        Command::Serve => serve(cli).await,
        Command::Config { command } => cc_switch_server::admin::run_config_command(&cli, command),
        Command::Doctor { check_port } => cc_switch_server::admin::run_doctor(&cli, check_port),
        Command::Version { json } => print_version(json),
        Command::Password { command } => {
            cc_switch_server::admin::run_password_command(&cli, command)
        }
    }
}

async fn serve(cli: Cli) -> anyhow::Result<()> {
    cc_switch_server::metrics::init()?;
    let state = ServerStateInner::load(cli.clone()).context("initialize server state")?;
    cc_switch_server::state::restore_tunnels(state.clone()).await;
    cc_switch_server::state::spawn_periodic_backups(state.clone());
    cc_switch_server::state::spawn_account_quota_refresh(state.clone());
    cc_switch_server::state::spawn_share_edit_event_listener(state.clone());
    cc_switch_server::api::serve(state).await
}

fn init_tracing(log_level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
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
