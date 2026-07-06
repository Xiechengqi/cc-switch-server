use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, ConfigCommand, PasswordCommand};
use crate::core::accounts::{accounts_path, AccountStore};
use crate::core::config::{config_path, RouterIdentity, ServerConfig};
use crate::core::providers::{providers_path, ProviderStore};
use crate::core::shares::{shares_path, ShareStore};
use crate::core::tunnel::{tunnels_path, TunnelRuntimeStatus};
use crate::core::usage::{usage_path, UsageStore};
use crate::coverage::ProviderCoverage;
use crate::web_assets;

pub fn run_config_command(cli: &Cli, command: ConfigCommand) -> anyhow::Result<()> {
    match command {
        ConfigCommand::Path => print_config_paths(cli),
        ConfigCommand::Print => {
            println!("{}", config_print_json(cli)?);
            Ok(())
        }
        ConfigCommand::Validate => {
            let snapshot = validate_config_stores(cli)?;
            println!("{}", validation_report(&snapshot));
            Ok(())
        }
    }
}

pub fn run_password_command(cli: &Cli, command: PasswordCommand) -> anyhow::Result<()> {
    match command {
        PasswordCommand::Reset { password, stdin } => reset_password(cli, password, stdin),
    }
}

fn reset_password(cli: &Cli, password: Option<String>, stdin: bool) -> anyhow::Result<()> {
    if password.is_some() && stdin {
        anyhow::bail!("use either --password or --stdin, not both");
    }

    let new_password = if stdin {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("read password from stdin")?;
        line.trim_end_matches(['\n', '\r']).to_string()
    } else {
        password.context("password is required; pass --password or --stdin")?
    };

    if new_password.is_empty() {
        anyhow::bail!("password must not be empty");
    }

    let config_dir = cli.resolved_config_dir()?;
    let mut config = ServerConfig::load_or_default(&config_dir)?;
    config
        .set_password(&new_password)
        .context("hash new web admin password")?;
    config
        .save(&config_dir)
        .context("save server.json with new password")?;
    crate::core::web_auth::WebAuthStore::load(config_dir.clone())
        .revoke_all_sessions()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .context("revoke active web auth sessions")?;

    println!(
        "web admin password reset in {}; active web sessions revoked",
        config_dir.display()
    );
    Ok(())
}

pub fn run_doctor(cli: &Cli, check_port: bool) -> anyhow::Result<()> {
    let report = doctor_report(cli, check_port);
    println!("{}", report.format());
    report.into_result()
}

fn print_config_paths(cli: &Cli) -> anyhow::Result<()> {
    let config_dir = cli.resolved_config_dir()?;
    let web_dist_dir = cli.resolved_web_dist_dir();

    println!("configDir={}", config_dir.display());
    println!(
        "webDistDir={}",
        web_dist_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<embedded>".to_string())
    );
    println!("bindAddr={}", cli.bind_addr());
    Ok(())
}

pub(crate) fn config_print_json(cli: &Cli) -> anyhow::Result<String> {
    let snapshot = validate_config_stores(cli)?;
    let summary = ConfigPrintSummary::from(snapshot);
    serde_json::to_string_pretty(&summary).context("serialize redacted config summary")
}

pub(crate) fn validate_config_stores(cli: &Cli) -> anyhow::Result<ConfigSnapshot> {
    let config_dir = cli.resolved_config_dir()?;
    let web_dist_dir = cli.resolved_web_dist_dir();
    let config = ServerConfig::load_or_default(&config_dir)?;
    let providers = ProviderStore::load_or_default(&config_dir)?;
    let accounts = AccountStore::load_or_default(&config_dir)?;
    let shares = ShareStore::load_or_default(&config_dir)?;
    let usage = UsageStore::load_or_default(&config_dir)?;
    let tunnels = TunnelRuntimeStoreForCli::load_or_default(&config_dir)?;

    let stores = vec![
        store_summary("server", config_path(&config_dir), None)?,
        store_summary(
            "providers",
            providers_path(&config_dir),
            Some(providers.providers.len()),
        )?,
        store_summary(
            "accounts",
            accounts_path(&config_dir),
            Some(accounts.accounts.len()),
        )?,
        store_summary(
            "shares",
            shares_path(&config_dir),
            Some(shares.shares.len()),
        )?,
        store_summary("usage", usage_path(&config_dir), Some(usage.logs.len()))?,
        store_summary(
            "tunnels",
            tunnels_path(&config_dir),
            Some(tunnels.statuses.len()),
        )?,
    ];

    Ok(ConfigSnapshot {
        config_dir,
        web_dist_dir,
        bind_addr: cli.bind_addr().to_string(),
        config,
        providers,
        shares,
        stores,
    })
}

fn validation_report(snapshot: &ConfigSnapshot) -> String {
    let mut lines = vec![
        format!("configDir={}", snapshot.config_dir.display()),
        format!(
            "webDistDir={}",
            snapshot
                .web_dist_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<embedded>".to_string())
        ),
        format!("bindAddr={}", snapshot.bind_addr),
    ];

    for store in &snapshot.stores {
        let state = if store.exists {
            "present"
        } else {
            "missing/default"
        };
        let items = store
            .items
            .map(|count| format!(", items={count}"))
            .unwrap_or_default();
        let bytes = store
            .bytes
            .map(|count| format!(", bytes={count}"))
            .unwrap_or_default();
        lines.push(format!(
            "[ok] {}: {}{}{} ({})",
            store.name, state, items, bytes, store.path
        ));
    }

    lines.push("configuration is valid".to_string());
    lines.join("\n")
}

fn doctor_report(cli: &Cli, check_port: bool) -> DoctorReport {
    let mut report = DoctorReport::default();

    let config_dir = match cli.resolved_config_dir() {
        Ok(path) => path,
        Err(error) => {
            report.fail("config-dir", error.to_string());
            return report;
        }
    };

    check_config_dir(&mut report, &config_dir);
    check_web_dist_dir(&mut report, cli.resolved_web_dist_dir().as_deref());
    check_provider_coverage(&mut report);

    match validate_config_stores(cli) {
        Ok(snapshot) => {
            report.ok(
                "stores",
                format!(
                    "all configured JSON stores parsed under {}",
                    config_dir.display()
                ),
            );
            check_setup(&mut report, &snapshot.config);
            check_share_provider_links(&mut report, &snapshot);
        }
        Err(error) => {
            report.fail("stores", error.to_string());
        }
    }

    if check_port {
        check_bind_addr(&mut report, cli);
    }

    report
}

fn check_config_dir(report: &mut DoctorReport, config_dir: &Path) {
    match fs::metadata(config_dir) {
        Ok(metadata) if metadata.is_dir() => {
            if metadata.permissions().readonly() {
                report.fail(
                    "config-dir",
                    format!("{} exists but is read-only", config_dir.display()),
                );
            } else {
                report.ok("config-dir", format!("{} exists", config_dir.display()));
            }
        }
        Ok(_) => report.fail(
            "config-dir",
            format!("{} exists but is not a directory", config_dir.display()),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => report.warn(
            "config-dir",
            format!(
                "{} does not exist yet; serve/setup will create it",
                config_dir.display()
            ),
        ),
        Err(error) => report.fail(
            "config-dir",
            format!("cannot inspect {}: {error}", config_dir.display()),
        ),
    }
}

fn check_web_dist_dir(report: &mut DoctorReport, web_dist_dir: Option<&Path>) {
    let Some(web_dist_dir) = web_dist_dir else {
        if web_assets::asset_count() == 0 {
            report.fail(
                "web-dist",
                "no embedded web assets were compiled into this binary".to_string(),
            );
            return;
        }
        report.ok(
            "web-dist",
            format!("using {} embedded web asset(s)", web_assets::asset_count()),
        );
        return;
    };

    match fs::metadata(web_dist_dir) {
        Ok(metadata) if metadata.is_dir() => {
            report.ok("web-dist", format!("{} exists", web_dist_dir.display()));
        }
        Ok(_) => report.fail(
            "web-dist",
            format!("{} exists but is not a directory", web_dist_dir.display()),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => report.warn(
            "web-dist",
            format!("{} is missing; API still works", web_dist_dir.display()),
        ),
        Err(error) => report.fail(
            "web-dist",
            format!("cannot inspect {}: {error}", web_dist_dir.display()),
        ),
    }
}

fn check_provider_coverage(report: &mut DoctorReport) {
    match ProviderCoverage::load_embedded() {
        Ok(coverage) => report.ok(
            "provider-coverage",
            format!(
                "{} provider type coverage entries loaded",
                coverage.provider_types.len()
            ),
        ),
        Err(error) => report.fail("provider-coverage", error.to_string()),
    }
}

fn check_setup(report: &mut DoctorReport, config: &ServerConfig) {
    if config.is_setup_complete() {
        report.ok("setup", "setup is complete".to_string());
    } else {
        report.warn(
            "setup",
            "setup is incomplete; Web/API setup is required before routing traffic".to_string(),
        );
    }

    if config.router_api_base().is_some() {
        report.ok("router", "router API base is configured".to_string());
    } else {
        report.warn(
            "router",
            "router URL/API base is not configured".to_string(),
        );
    }

    if config
        .client
        .tunnel_subdomain
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        report.ok(
            "client-tunnel",
            "client tunnel subdomain is configured".to_string(),
        );
    } else {
        report.warn(
            "client-tunnel",
            "client tunnel subdomain is not configured".to_string(),
        );
    }
}

fn check_share_provider_links(report: &mut DoctorReport, snapshot: &ConfigSnapshot) {
    let missing = snapshot
        .shares
        .shares
        .iter()
        .filter(|share| {
            !snapshot.providers.providers.iter().any(|provider| {
                provider.app == share.app && provider.provider.id == share.provider_id
            })
        })
        .map(|share| {
            format!(
                "{}:{}:{}",
                share.id,
                app_label(share.app),
                share.provider_id
            )
        })
        .collect::<Vec<_>>();

    if missing.is_empty() {
        report.ok(
            "share-provider-links",
            format!(
                "{} share(s) reference existing providers",
                snapshot.shares.shares.len()
            ),
        );
    } else {
        report.fail(
            "share-provider-links",
            format!("missing provider references: {}", missing.join(", ")),
        );
    }
}

fn app_label(app: crate::core::provider::AppKind) -> &'static str {
    match app {
        crate::core::provider::AppKind::Claude => "claude",
        crate::core::provider::AppKind::Codex => "codex",
        crate::core::provider::AppKind::Gemini => "gemini",
    }
}

fn check_bind_addr(report: &mut DoctorReport, cli: &Cli) {
    match TcpListener::bind(cli.bind_addr()) {
        Ok(listener) => {
            drop(listener);
            report.ok("bind-addr", format!("{} is available", cli.bind_addr()));
        }
        Err(error) => report.fail(
            "bind-addr",
            format!("{} is not bindable: {error}", cli.bind_addr()),
        ),
    }
}

fn store_summary(
    name: &'static str,
    path: PathBuf,
    items: Option<usize>,
) -> anyhow::Result<StoreSummary> {
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            anyhow::bail!("inspect {} store {}: {error}", name, path.display());
        }
    };

    Ok(StoreSummary {
        name,
        path: path.display().to_string(),
        exists: metadata.is_some(),
        bytes: metadata.as_ref().map(|metadata| metadata.len()),
        items,
    })
}

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    config_dir: PathBuf,
    web_dist_dir: Option<PathBuf>,
    bind_addr: String,
    config: ServerConfig,
    providers: ProviderStore,
    shares: ShareStore,
    stores: Vec<StoreSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigPrintSummary {
    config_dir: String,
    web_dist_dir: Option<String>,
    bind_addr: String,
    setup_complete: bool,
    server: RedactedServerConfigSummary,
    stores: Vec<StoreSummary>,
}

impl From<ConfigSnapshot> for ConfigPrintSummary {
    fn from(snapshot: ConfigSnapshot) -> Self {
        Self {
            config_dir: snapshot.config_dir.display().to_string(),
            web_dist_dir: snapshot.web_dist_dir.map(|path| path.display().to_string()),
            bind_addr: snapshot.bind_addr,
            setup_complete: snapshot.config.is_setup_complete(),
            server: RedactedServerConfigSummary::from(snapshot.config),
            stores: snapshot.stores,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedServerConfigSummary {
    auth: RedactedAuthSummary,
    owner: RedactedOwnerSummary,
    router: RedactedRouterSummary,
    client: RedactedClientSummary,
}

impl From<ServerConfig> for RedactedServerConfigSummary {
    fn from(config: ServerConfig) -> Self {
        Self {
            auth: RedactedAuthSummary {
                password_configured: config.auth.password_hash.is_some(),
                api_token_configured: config.auth.api_token_hash.is_some(),
            },
            owner: RedactedOwnerSummary {
                email: config.owner.email,
            },
            router: RedactedRouterSummary {
                url: config.router.url,
                api_base: config.router.api_base,
                domain: config.router.domain,
                region: config.router.region,
                ssh_host: config.router.ssh_host,
                ssh_user: config.router.ssh_user,
                custom: config.router.custom,
                identity: config
                    .router
                    .identity
                    .as_ref()
                    .map(RedactedRouterIdentitySummary::from),
                last_register_error: config.router.last_register_error,
                last_registered_at_ms: config.router.last_registered_at_ms,
            },
            client: RedactedClientSummary {
                tunnel_subdomain: config.client.tunnel_subdomain,
                tunnel_status: config.client.tunnel_status,
                last_heartbeat_ms: config.client.last_heartbeat_ms,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedAuthSummary {
    password_configured: bool,
    api_token_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedOwnerSummary {
    email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedRouterSummary {
    url: Option<String>,
    api_base: Option<String>,
    domain: Option<String>,
    region: Option<String>,
    ssh_host: Option<String>,
    ssh_user: Option<String>,
    custom: bool,
    identity: Option<RedactedRouterIdentitySummary>,
    last_register_error: Option<String>,
    last_registered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedRouterIdentitySummary {
    installation_id: String,
    public_key_configured: bool,
    private_key_configured: bool,
    control_secret_configured: bool,
}

impl From<&RouterIdentity> for RedactedRouterIdentitySummary {
    fn from(identity: &RouterIdentity) -> Self {
        Self {
            installation_id: identity.installation_id.clone(),
            public_key_configured: !identity.public_key.is_empty(),
            private_key_configured: !identity.private_key.is_empty(),
            control_secret_configured: identity
                .control_secret
                .as_deref()
                .is_some_and(|value| !value.is_empty()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedactedClientSummary {
    tunnel_subdomain: Option<String>,
    tunnel_status: Option<String>,
    last_heartbeat_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreSummary {
    name: &'static str,
    path: String,
    exists: bool,
    bytes: Option<u64>,
    items: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelRuntimeStoreForCli {
    #[serde(default)]
    statuses: BTreeMap<String, TunnelRuntimeStatus>,
}

impl TunnelRuntimeStoreForCli {
    fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = tunnels_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read tunnels {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse tunnels {}", path.display()))
    }
}

#[derive(Debug, Clone, Default)]
struct DoctorReport {
    checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn ok(&mut self, name: impl Into<String>, message: impl Into<String>) {
        self.checks.push(DoctorCheck {
            level: DoctorLevel::Ok,
            name: name.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, name: impl Into<String>, message: impl Into<String>) {
        self.checks.push(DoctorCheck {
            level: DoctorLevel::Warn,
            name: name.into(),
            message: message.into(),
        });
    }

    fn fail(&mut self, name: impl Into<String>, message: impl Into<String>) {
        self.checks.push(DoctorCheck {
            level: DoctorLevel::Fail,
            name: name.into(),
            message: message.into(),
        });
    }

    fn format(&self) -> String {
        let mut lines = self
            .checks
            .iter()
            .map(|check| {
                format!(
                    "[{}] {}: {}",
                    check.level.as_str(),
                    check.name,
                    check.message
                )
            })
            .collect::<Vec<_>>();

        let failures = self
            .checks
            .iter()
            .filter(|check| check.level == DoctorLevel::Fail)
            .count();
        let warnings = self
            .checks
            .iter()
            .filter(|check| check.level == DoctorLevel::Warn)
            .count();

        lines.push(format!(
            "doctor summary: {} ok, {} warning(s), {} failure(s)",
            self.checks.len() - warnings - failures,
            warnings,
            failures
        ));
        lines.join("\n")
    }

    fn into_result(self) -> anyhow::Result<()> {
        let failures = self
            .checks
            .iter()
            .filter(|check| check.level == DoctorLevel::Fail)
            .count();

        if failures > 0 {
            anyhow::bail!("doctor found {failures} failing check(s)");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct DoctorCheck {
    level: DoctorLevel,
    name: String,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorLevel {
    Ok,
    Warn,
    Fail,
}

impl DoctorLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn config_print_redacts_secret_fields() {
        let config_dir = temp_config_dir("config-print-redacts");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_path(&config_dir),
            serde_json::to_vec_pretty(&json!({
                "auth": {
                    "passwordHash": "secret-password-hash",
                    "apiTokenHash": "secret-api-token-hash"
                },
                "owner": {
                    "email": "owner@example.com"
                },
                "router": {
                    "url": "https://router.example.com",
                    "identity": {
                        "installationId": "installation-1",
                        "publicKey": "public-key-value",
                        "privateKey": "secret-private-key",
                        "controlSecret": "secret-control"
                    }
                },
                "client": {
                    "tunnelSubdomain": "ownerabcde"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let cli = test_cli(config_dir.clone());
        let output = config_print_json(&cli).unwrap();
        fs::remove_dir_all(config_dir).unwrap();

        assert!(!output.contains("secret-password-hash"));
        assert!(!output.contains("secret-api-token-hash"));
        assert!(!output.contains("secret-private-key"));
        assert!(!output.contains("secret-control"));
        assert!(!output.contains("public-key-value"));
        assert!(output.contains("\"passwordConfigured\": true"));
        assert!(output.contains("\"privateKeyConfigured\": true"));
        assert!(output.contains("\"controlSecretConfigured\": true"));
    }

    #[test]
    fn validate_config_stores_rejects_malformed_json() {
        let config_dir = temp_config_dir("config-validate-malformed");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(providers_path(&config_dir), "{not json").unwrap();

        let cli = test_cli(config_dir.clone());
        let error = validate_config_stores(&cli).unwrap_err().to_string();
        fs::remove_dir_all(config_dir).unwrap();

        assert!(error.contains("parse providers"));
    }

    #[test]
    fn reset_password_updates_hash_and_revokes_web_sessions() {
        let config_dir = temp_config_dir("password-reset");
        fs::create_dir_all(&config_dir).unwrap();
        let mut config = ServerConfig::load_or_default(&config_dir).unwrap();
        config.set_password("old-password-1").unwrap();
        config.save(&config_dir).unwrap();
        let sessions_path = config_dir.join("web-auth-sessions.json");
        fs::write(
            &sessions_path,
            r#"[{"id":"1","accessTokenHash":"abc","refreshTokenHash":"def","accessExpiresAt":"2999-01-01T00:00:00Z","refreshExpiresAt":"2999-01-01T00:00:00Z","createdAt":"2026-01-01T00:00:00Z","lastUsedAt":"2026-01-01T00:00:00Z","revokedAt":null}]"#,
        )
        .unwrap();

        let cli = test_cli(config_dir.clone());
        reset_password(&cli, Some("new-password-2".to_string()), false).unwrap();

        let updated = ServerConfig::load_or_default(&config_dir).unwrap();
        assert!(updated.verify_password("new-password-2"));
        assert!(!updated.verify_password("old-password-1"));
        let raw = fs::read_to_string(&sessions_path).unwrap();
        assert!(raw.contains(r#""revokedAt":"#));
        fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn doctor_warns_for_fresh_config_without_failing() {
        let config_dir = temp_config_dir("doctor-fresh");
        let cli = test_cli(config_dir.clone());

        let report = doctor_report(&cli, false);

        assert!(report
            .checks
            .iter()
            .any(|check| check.level == DoctorLevel::Warn && check.name == "config-dir"));
        assert!(report
            .checks
            .iter()
            .all(|check| check.level != DoctorLevel::Fail));
    }

    fn test_cli(config_dir: PathBuf) -> Cli {
        Cli {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        }
    }

    fn temp_config_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cc-switch-server-{name}-{nanos}"))
    }
}
