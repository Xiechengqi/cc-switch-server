use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::{json, Value};

const ANTIGRAVITY_VERSION_ENV: &str = "CC_SWITCH_ANTIGRAVITY_CLIENT_VERSION";
const ANTIGRAVITY_FALLBACK_VERSION: &str = "2.2.1";
const ANTIGRAVITY_MANIFEST_URL: &str =
    "https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml";
const ANTIGRAVITY_HUB_PLATFORM: &str = "darwin/arm64";
const ANTIGRAVITY_CLIENT_METADATA_PLATFORM: i32 = 2;
const ANTIGRAVITY_UPDATER_USER_AGENT: &str = "electron-builder";
const ANTIGRAVITY_REFRESH_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);
const ANTIGRAVITY_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.54.0";
pub const COPILOT_EDITOR_VERSION: &str = "vscode/1.126.0";
pub const COPILOT_PLUGIN_VERSION: &str = "copilot-chat/0.54.0";
pub const COPILOT_API_VERSION: &str = "2026-06-01";
pub const GEMINI_CLI_X_GOOG_API_CLIENT: &str = "google-genai-sdk/1.41.0 gl-node/v22.19.0";

static ANTIGRAVITY_VERSION: OnceLock<RwLock<String>> = OnceLock::new();
static ANTIGRAVITY_UPDATER_STARTED: OnceLock<()> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct AntigravityManifest {
    version: String,
}

pub fn server_user_agent() -> String {
    format!("cc-switch-server/{}", env!("CARGO_PKG_VERSION"))
}

pub fn antigravity_version() -> String {
    if let Some(version) = configured_antigravity_version() {
        return version;
    }
    ANTIGRAVITY_VERSION
        .get_or_init(|| RwLock::new(ANTIGRAVITY_FALLBACK_VERSION.to_string()))
        .read()
        .map(|version| version.clone())
        .unwrap_or_else(|_| ANTIGRAVITY_FALLBACK_VERSION.to_string())
}

pub fn antigravity_user_agent() -> String {
    format!(
        "antigravity/hub/{} {}",
        antigravity_version(),
        ANTIGRAVITY_HUB_PLATFORM
    )
}

pub fn antigravity_client_metadata() -> Value {
    json!({
        "ideType": 9,
        "platform": ANTIGRAVITY_CLIENT_METADATA_PLATFORM,
        "pluginType": 2,
    })
}

pub fn gemini_cli_user_agent() -> String {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "x86",
        other => other,
    };
    format!("GeminiCLI/0.31.0/unknown ({platform}; {arch})")
}

pub fn gemini_cli_code_assist_metadata() -> Value {
    json!({
        "ideType": "GEMINI_CLI",
        "pluginType": "GEMINI",
    })
}

pub fn spawn_antigravity_version_updater() {
    if configured_antigravity_version().is_some() || ANTIGRAVITY_UPDATER_STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            match fetch_antigravity_version().await {
                Ok(version) => {
                    if let Ok(mut cached) = ANTIGRAVITY_VERSION
                        .get_or_init(|| RwLock::new(ANTIGRAVITY_FALLBACK_VERSION.to_string()))
                        .write()
                    {
                        *cached = version.clone();
                    }
                    tracing::debug!(version, "refreshed Antigravity client version");
                }
                Err(error) => {
                    tracing::debug!(error = %error, "Antigravity version refresh failed; keeping cached version");
                }
            }
            tokio::time::sleep(ANTIGRAVITY_REFRESH_INTERVAL).await;
        }
    });
}

fn configured_antigravity_version() -> Option<String> {
    std::env::var(ANTIGRAVITY_VERSION_ENV)
        .ok()
        .map(|version| version.trim().to_string())
        .filter(|version| valid_antigravity_version(version))
}

async fn fetch_antigravity_version() -> anyhow::Result<String> {
    let client = crate::infra::http::direct_client_builder()
        .user_agent(server_user_agent())
        .build()
        .context("build Antigravity version client")?;
    let manifest = client
        .get(ANTIGRAVITY_MANIFEST_URL)
        .header(reqwest::header::USER_AGENT, ANTIGRAVITY_UPDATER_USER_AGENT)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .timeout(ANTIGRAVITY_FETCH_TIMEOUT)
        .send()
        .await
        .context("fetch Antigravity version manifest")?
        .error_for_status()
        .context("Antigravity version manifest returned an error")?
        .text()
        .await
        .context("read Antigravity version manifest")?;
    parse_antigravity_manifest(&manifest)
}

fn parse_antigravity_manifest(manifest: &str) -> anyhow::Result<String> {
    let parsed: AntigravityManifest =
        serde_yaml::from_str(manifest).context("parse Antigravity version manifest")?;
    let version = parsed.version.trim();
    if !valid_antigravity_version(version) {
        bail!("Antigravity manifest contains an invalid version");
    }
    Ok(version.to_string())
}

fn valid_antigravity_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_antigravity_manifest() {
        assert_eq!(
            parse_antigravity_manifest("version: 2.3.4\nfiles: []\n").unwrap(),
            "2.3.4"
        );
        assert!(parse_antigravity_manifest("version: 'bad value'\n").is_err());
    }

    #[test]
    fn server_identity_contains_the_package_version() {
        assert_eq!(
            server_user_agent(),
            format!("cc-switch-server/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn antigravity_hub_identity_and_metadata_use_the_same_platform() {
        let user_agent = antigravity_user_agent();
        assert!(user_agent.starts_with("antigravity/hub/"));
        assert!(user_agent.ends_with(" darwin/arm64"));

        let metadata = antigravity_client_metadata();
        assert_eq!(metadata["ideType"], 9);
        assert_eq!(metadata["pluginType"], 2);
        assert_eq!(metadata["platform"], ANTIGRAVITY_CLIENT_METADATA_PLATFORM);
    }

    #[test]
    fn gemini_cli_identity_is_centralized_with_code_assist_metadata() {
        let user_agent = gemini_cli_user_agent();
        assert!(user_agent.starts_with("GeminiCLI/0.31.0/unknown ("));
        assert!(user_agent.ends_with(')'));
        assert_eq!(
            GEMINI_CLI_X_GOOG_API_CLIENT,
            "google-genai-sdk/1.41.0 gl-node/v22.19.0"
        );
        assert_eq!(
            gemini_cli_code_assist_metadata(),
            json!({"ideType": "GEMINI_CLI", "pluginType": "GEMINI"})
        );
    }

    #[test]
    fn copilot_identity_is_one_consistent_editor_plugin_release() {
        assert_eq!(COPILOT_USER_AGENT, "GitHubCopilotChat/0.54.0");
        assert_eq!(COPILOT_EDITOR_VERSION, "vscode/1.126.0");
        assert_eq!(COPILOT_PLUGIN_VERSION, "copilot-chat/0.54.0");
        assert_eq!(COPILOT_API_VERSION, "2026-06-01");
        assert_eq!(
            COPILOT_USER_AGENT.strip_prefix("GitHubCopilotChat/"),
            COPILOT_PLUGIN_VERSION.strip_prefix("copilot-chat/")
        );
    }
}
