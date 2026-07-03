mod admin;
mod build_info;
mod cli;
mod core;
mod coverage;
mod http;
mod proxy;
mod state;

use anyhow::Context;
use cli::{Cli, Command};
use state::ServerStateInner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_args();
    init_tracing(&cli.log_level);

    match cli.effective_command() {
        Command::Serve => serve(cli).await,
        Command::Config { command } => admin::run_config_command(&cli, command),
        Command::Doctor { check_port } => admin::run_doctor(&cli, check_port),
        Command::Version { json } => print_version(json),
    }
}

async fn serve(cli: Cli) -> anyhow::Result<()> {
    let state = ServerStateInner::load(cli.clone()).context("initialize server state")?;
    state::restore_tunnels(state.clone()).await;
    state::spawn_periodic_backups(state.clone());
    state::spawn_account_quota_refresh(state.clone());
    state::spawn_share_edit_event_listener(state.clone());
    http::serve(state).await
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
    let info = build_info::build_info();
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("{}", info.format_human());
    }
    Ok(())
}
