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
        default_value = "127.0.0.1",
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
    /// Manage the local web admin password stored in server.json.
    Password {
        #[command(subcommand)]
        command: PasswordCommand,
    },
    /// Initialize server.json for first-time setup without starting HTTP.
    Init(InitArgs),
    #[command(hide = true)]
    SelfUpdateHelper {
        #[arg(long)]
        spec: PathBuf,
    },
}

#[derive(Debug, Clone, Parser)]
pub struct InitArgs {
    /// Owner email for this server installation.
    #[arg(long)]
    pub owner_email: String,
    /// Router API base URL, for example https://sgptokenswitch.cc
    #[arg(long)]
    pub router_url: String,
    /// Client tunnel subdomain. Leave unset to auto-generate a memorable subdomain.
    #[arg(long)]
    pub client_subdomain: Option<String>,
    /// Web admin password (at least 8 characters).
    #[arg(long)]
    pub password: Option<String>,
    /// Read the web admin password from stdin.
    #[arg(long)]
    pub password_stdin: bool,
    /// Validate input without writing server.json.
    #[arg(long)]
    pub dry_run: bool,
    /// Continue when Router is unreachable (skip client tunnel claim).
    #[arg(long, default_value_t = false)]
    pub allow_offline: bool,
    /// Write server.json but skip Router registration and client tunnel claim.
    #[arg(long)]
    pub skip_router_claim: bool,
    /// Fail when setup is already complete instead of exiting successfully.
    #[arg(long)]
    pub force: bool,
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

#[derive(Debug, Clone, Subcommand)]
pub enum PasswordCommand {
    /// Set a new web admin password and revoke active web sessions.
    Reset {
        /// New password (at least 8 characters).
        #[arg(long)]
        password: Option<String>,
        /// Read the new password from stdin instead of --password.
        #[arg(long)]
        stdin: bool,
    },
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
        self.web_dist_dir.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;

    #[test]
    fn default_host_is_loopback() {
        let cli = Cli::try_parse_from(["cc-switch-server"]).unwrap();
        assert_eq!(cli.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

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
    fn web_dist_dir_is_only_explicit_override() {
        let cli = Cli::try_parse_from(["cc-switch-server"]).unwrap();
        assert_eq!(cli.resolved_web_dist_dir(), None);

        let cli = Cli::try_parse_from([
            "cc-switch-server",
            "--web-dist-dir",
            "/tmp/cc-switch-server-web",
        ])
        .unwrap();
        assert_eq!(
            cli.resolved_web_dist_dir(),
            Some(PathBuf::from("/tmp/cc-switch-server-web"))
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
    fn parses_internal_self_update_helper_subcommand() {
        let cli = Cli::try_parse_from([
            "cc-switch-server",
            "self-update-helper",
            "--spec",
            "/tmp/upgrade-helper.json",
        ])
        .unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::SelfUpdateHelper { spec }
                if spec.as_path() == std::path::Path::new("/tmp/upgrade-helper.json")
        ));
    }

    #[test]
    fn parses_password_reset_subcommand() {
        let cli = Cli::try_parse_from([
            "cc-switch-server",
            "password",
            "reset",
            "--password",
            "new-password-123",
        ])
        .unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Password {
                command: PasswordCommand::Reset {
                    password: Some(value),
                    stdin: false,
                }
            } if value == "new-password-123"
        ));
    }

    #[test]
    fn parses_init_subcommand() {
        let cli = Cli::try_parse_from([
            "cc-switch-server",
            "--config-dir",
            "/tmp/cc-switch-server-init",
            "init",
            "--owner-email",
            "owner@example.com",
            "--router-url",
            "https://sgptokenswitch.cc",
            "--password",
            "password123",
        ])
        .unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Init(args)
                if args.owner_email == "owner@example.com"
                && args.router_url == "https://sgptokenswitch.cc"
                && args.password.as_deref() == Some("password123")
        ));
    }

    #[test]
    fn parses_password_reset_stdin_subcommand() {
        let cli =
            Cli::try_parse_from(["cc-switch-server", "password", "reset", "--stdin"]).unwrap();

        assert!(matches!(
            cli.effective_command(),
            Command::Password {
                command: PasswordCommand::Reset {
                    password: None,
                    stdin: true,
                }
            }
        ));
    }
}
