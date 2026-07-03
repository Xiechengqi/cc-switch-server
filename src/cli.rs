use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Parser)]
#[command(name = "cc-switch-server")]
#[command(version = crate::build_info::VERSION_LINE, long_version = crate::build_info::LONG_VERSION)]
#[command(about = "Headless code-agent token server for Claude, Codex and Gemini")]
pub struct Cli {
    #[arg(
        long,
        env = "CC_SWITCH_SERVER_HOST",
        default_value = "0.0.0.0",
        global = true
    )]
    pub host: IpAddr,

    #[arg(
        long,
        env = "CC_SWITCH_SERVER_PORT",
        default_value_t = 15721,
        global = true
    )]
    pub port: u16,

    #[arg(long, env = "CC_SWITCH_SERVER_CONFIG_DIR", global = true)]
    pub config_dir: Option<PathBuf>,

    #[arg(long, env = "CC_SWITCH_SERVER_WEB_DIST_DIR", global = true)]
    pub web_dist_dir: Option<PathBuf>,

    #[arg(
        long,
        env = "CC_SWITCH_SERVER_LOG",
        default_value = "info",
        global = true
    )]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Run the HTTP server.
    Serve,
    /// Inspect and validate local configuration without starting the server.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run deployment and configuration diagnostics without starting tunnels.
    Doctor {
        /// Also check whether the configured host:port can be bound.
        #[arg(long)]
        check_port: bool,
    },
    /// Print binary version and build metadata.
    Version {
        /// Print build metadata as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// Print resolved config paths and bind address.
    Path,
    /// Print a redacted JSON summary of server configuration and stores.
    Print,
    /// Parse and validate local JSON stores.
    Validate,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub fn effective_command(&self) -> Command {
        self.command.clone().unwrap_or(Command::Serve)
    }

    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    pub fn resolved_config_dir(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = self.config_dir.clone() {
            return Ok(path);
        }

        if let Ok(value) = std::env::var("CC_SWITCH_SERVER_CONFIG_DIR") {
            return Ok(PathBuf::from(value));
        }

        let home = std::env::var_os("HOME").context("HOME is not set; pass --config-dir")?;
        Ok(PathBuf::from(home).join(".cc-switch-server"))
    }

    pub fn resolved_web_dist_dir(&self) -> Option<PathBuf> {
        self.web_dist_dir.clone().or_else(default_web_dist_dir)
    }
}

fn default_web_dist_dir() -> Option<PathBuf> {
    let cwd_dist = std::env::current_dir().ok()?.join("web-dist");
    cwd_dist.is_dir().then_some(cwd_dist)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;

    #[test]
    fn parses_legacy_startup_without_subcommand_as_serve() {
        let cli =
            Cli::try_parse_from(["cc-switch-server", "--host", "127.0.0.1", "--port", "15722"])
                .unwrap();

        assert_eq!(cli.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cli.port, 15722);
        assert!(matches!(cli.effective_command(), Command::Serve));
    }

    #[test]
    fn parses_explicit_serve_with_global_options_after_subcommand() {
        let cli = Cli::try_parse_from([
            "cc-switch-server",
            "serve",
            "--host",
            "127.0.0.1",
            "--config-dir",
            "/tmp/cc-switch-server-test",
        ])
        .unwrap();

        assert!(matches!(cli.effective_command(), Command::Serve));
        assert_eq!(cli.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            cli.config_dir,
            Some(PathBuf::from("/tmp/cc-switch-server-test"))
        );
    }

    #[test]
    fn parses_config_validate_subcommand() {
        let cli = Cli::try_parse_from(["cc-switch-server", "config", "validate"]).unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Config {
                command: ConfigCommand::Validate
            }
        ));
    }

    #[test]
    fn parses_version_subcommand() {
        let cli = Cli::try_parse_from(["cc-switch-server", "version"]).unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Version { json: false }
        ));
    }

    #[test]
    fn parses_version_json_subcommand() {
        let cli = Cli::try_parse_from(["cc-switch-server", "version", "--json"]).unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Version { json: true }
        ));
    }
}
