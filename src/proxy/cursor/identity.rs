use rand::RngCore;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cursor_client_contract::{
    sdk_client_version, CLIENT_TYPE_HEADER, CLIENT_VERSION_HEADER, GHOST_MODE_ENABLED,
    GHOST_MODE_HEADER, SDK_CLIENT_TYPE,
};
use crate::domain::accounts::cursor_import::normalize_cursor_access_token;
use crate::domain::accounts::store::Account;

use super::h2_client::agent_connect_headers;
use super::profile::CursorProtocolRail;

pub const DEFAULT_CURSOR_CLI_VERSION: &str = "cli-2026.07.08-0c04a8a";
const CURSOR_CLIENT_ID: &str = "cc-switch-server";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorAccountData {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_service_machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_client_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_config_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_client_id: Option<String>,
}

impl CursorAccountData {
    pub fn machine_id(&self) -> &str {
        self.cursor_service_machine_id
            .as_deref()
            .unwrap_or(self.account_id.as_str())
    }

    pub fn resolved_client_version(&self) -> String {
        self.cursor_client_version
            .as_deref()
            .and_then(normalize_cursor_cli_version)
            .or_else(detect_cursor_cli_version)
            .unwrap_or_else(|| DEFAULT_CURSOR_CLI_VERSION.to_string())
    }

    pub fn config_version(&self) -> String {
        self.cursor_config_version
            .clone()
            .unwrap_or_else(random_uuid_like)
    }

    pub fn client_id(&self) -> &str {
        self.cursor_client_id.as_deref().unwrap_or(CURSOR_CLIENT_ID)
    }
}

pub fn cursor_account_from_managed_account(account: &Account) -> CursorAccountData {
    CursorAccountData {
        account_id: account.id.clone(),
        email: account.email.clone(),
        refresh_token: account.refresh_token.clone(),
        id_token: account.id_token.clone(),
        cursor_service_machine_id: string_path(account, &MACHINE_ID_PATHS),
        cursor_client_version: string_path(account, &CLIENT_VERSION_PATHS),
        cursor_config_version: string_path(account, &CONFIG_VERSION_PATHS),
        cursor_client_id: string_path(account, &CLIENT_ID_PATHS),
    }
}

pub fn cursor_account_for_api_key(
    api_key: &str,
    verified_account_id: Option<&str>,
) -> CursorAccountData {
    let key_hash = sha256_hex(api_key);
    let stable_seed = verified_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&key_hash);
    let identity_hash = sha256_hex(stable_seed);
    CursorAccountData {
        account_id: verified_account_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("cursor_apikey_{}", &key_hash[..24])),
        email: None,
        refresh_token: None,
        id_token: None,
        cursor_service_machine_id: Some(identity_hash.clone()),
        cursor_client_version: None,
        cursor_config_version: Some(stable_uuid_like(&format!("cursor-config:{identity_hash}"))),
        cursor_client_id: None,
    }
}

pub fn cursor_agentservice_headers(
    rail: CursorProtocolRail,
    account: &CursorAccountData,
    token: &str,
) -> Vec<(String, String)> {
    let mut headers = agent_connect_headers(matches!(rail, CursorProtocolRail::OAuthCli));
    let request_id = random_uuid_like();
    headers.extend([
        (
            "authorization".to_string(),
            format!("Bearer {}", normalize_cursor_access_token(token)),
        ),
        (
            CLIENT_TYPE_HEADER.to_string(),
            match rail {
                CursorProtocolRail::OAuthCli => "cli",
                CursorProtocolRail::ApiKeySdk => SDK_CLIENT_TYPE,
            }
            .to_string(),
        ),
        (
            CLIENT_VERSION_HEADER.to_string(),
            cursor_client_version_for_rail(rail, account),
        ),
        (
            GHOST_MODE_HEADER.to_string(),
            GHOST_MODE_ENABLED.to_string(),
        ),
        ("x-request-id".to_string(), request_id.clone()),
        ("x-original-request-id".to_string(), request_id),
    ]);
    if rail == CursorProtocolRail::OAuthCli {
        let traceparent = random_traceparent();
        headers.extend([
            ("traceparent".to_string(), traceparent.clone()),
            ("backend-traceparent".to_string(), traceparent),
        ]);
    }
    headers
}

fn detect_cursor_cli_version() -> Option<String> {
    for name in [
        "CC_SWITCH_CURSOR_AGENT_CLI_VERSION",
        "CURSOR_AGENT_CLI_VERSION",
    ] {
        if let Some(version) = std::env::var(name)
            .ok()
            .and_then(|value| normalize_cursor_cli_version(&value))
        {
            return Some(version);
        }
    }
    detect_cursor_cli_version_from_fs()
}

fn normalize_cursor_cli_version(value: &str) -> Option<String> {
    let value = value.trim();
    let build = value.strip_prefix("cli-").unwrap_or(value);
    let (date, revision) = build.split_once('-')?;
    let mut date_parts = date.split('.');
    let valid_date = date_parts
        .next()
        .is_some_and(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_digit()))
        && date_parts.clone().count() == 2
        && date_parts.all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()));
    let valid_revision = revision.len() >= 6
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    (valid_date && valid_revision).then(|| format!("cli-{build}"))
}

fn detect_cursor_cli_version_from_fs() -> Option<String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)?;
    for binary in ["agent", "cursor-agent"] {
        let path = home.join(".local/bin").join(binary);
        if let Ok(resolved) = std::fs::canonicalize(path) {
            if let Some(version) = cursor_cli_version_from_path(&resolved) {
                return Some(version);
            }
        }
    }
    let versions_dir = std::env::var_os("CURSOR_DATA_DIR")
        .map(std::path::PathBuf::from)
        .map(|path| path.join("versions"))
        .unwrap_or_else(|| default_cursor_cli_versions_dir(&home));
    let mut versions = std::fs::read_dir(versions_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|value| normalize_cursor_cli_version(&value))
        .collect::<Vec<_>>();
    versions.sort();
    versions.pop()
}

fn cursor_cli_version_from_path(path: &std::path::Path) -> Option<String> {
    let parts = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    parts.windows(2).rev().find_map(|parts| {
        (parts[0] == "versions")
            .then(|| normalize_cursor_cli_version(&parts[1]))
            .flatten()
    })
}

fn default_cursor_cli_versions_dir(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
            .join("cursor-agent/versions")
    } else {
        home.join(".local/share/cursor-agent/versions")
    }
}

pub(crate) fn cursor_client_version_for_rail(
    rail: CursorProtocolRail,
    account: &CursorAccountData,
) -> String {
    match rail {
        CursorProtocolRail::OAuthCli => account.resolved_client_version(),
        CursorProtocolRail::ApiKeySdk => sdk_client_version(),
    }
}

fn string_path(account: &Account, paths: &[&str]) -> Option<String> {
    account
        .raw
        .as_ref()
        .and_then(|value| string_in_value(value, paths))
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| string_in_value(value, paths))
        })
}

fn string_in_value(value: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|path| {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn stable_uuid_like(input: &str) -> String {
    let hash = sha256_hex(input);
    format!(
        "{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn random_traceparent() -> String {
    let mut trace_id = [0u8; 16];
    let mut parent_id = [0u8; 8];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut trace_id);
    rng.fill_bytes(&mut parent_id);
    if trace_id.iter().all(|byte| *byte == 0) {
        trace_id[15] = 1;
    }
    if parent_id.iter().all(|byte| *byte == 0) {
        parent_id[7] = 1;
    }
    format!("00-{}-{}-01", hex_lower(&trace_id), hex_lower(&parent_id))
}

const MACHINE_ID_PATHS: [&str; 8] = [
    "/cursorServiceMachineId",
    "/cursor_service_machine_id",
    "/machineId",
    "/machine_id",
    "/cursor/serviceMachineId",
    "/cursor/service_machine_id",
    "/account/cursorServiceMachineId",
    "/account/cursor_service_machine_id",
];
const CLIENT_VERSION_PATHS: [&str; 6] = [
    "/cursorClientVersion",
    "/cursor_client_version",
    "/clientVersion",
    "/client_version",
    "/cursor/clientVersion",
    "/cursor/client_version",
];
const CONFIG_VERSION_PATHS: [&str; 6] = [
    "/cursorConfigVersion",
    "/cursor_config_version",
    "/configVersion",
    "/config_version",
    "/cursor/configVersion",
    "/cursor/config_version",
];
const CLIENT_ID_PATHS: [&str; 6] = [
    "/cursorClientId",
    "/cursor_client_id",
    "/clientId",
    "/client_id",
    "/cursor/clientId",
    "/cursor/client_id",
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::accounts::store::Account;
    use crate::domain::providers::model::ProviderType;

    use super::*;

    #[test]
    fn api_key_account_uses_stable_hash_identity() {
        let account = cursor_account_for_api_key("cursor-key", None);
        let repeated = cursor_account_for_api_key("cursor-key", None);
        assert!(account.account_id.starts_with("cursor_apikey_"));
        assert_eq!(account.machine_id().len(), 64);
        assert_eq!(account.account_id, repeated.account_id);
        assert_eq!(account.machine_id(), repeated.machine_id());
        assert_eq!(account.config_version(), repeated.config_version());
    }

    #[test]
    fn verified_principal_stabilizes_machine_identity_across_key_rotation() {
        let first = cursor_account_for_api_key("cursor-key-one", Some("cursor_apikey_account"));
        let rotated = cursor_account_for_api_key("cursor-key-two", Some("cursor_apikey_account"));
        let other = cursor_account_for_api_key("cursor-key-two", Some("cursor_apikey_other"));

        assert_eq!(first.account_id, rotated.account_id);
        assert_eq!(first.machine_id(), rotated.machine_id());
        assert_eq!(first.config_version(), rotated.config_version());
        assert_ne!(first.machine_id(), other.machine_id());
    }

    #[test]
    fn managed_account_reads_cursor_raw_metadata() {
        let account = Account {
            id: "cursor_1".to_string(),
            provider_type: ProviderType::CursorOAuth,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("u@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: Some(json!({
                "cursorServiceMachineId": "machine",
                "cursorClientVersion": "cli-2026.07.08-0c04a8a",
                "cursorConfigVersion": "config",
                "cursorClientId": "client"
            })),
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capability_observations: Default::default(),
        };
        let cursor = cursor_account_from_managed_account(&account);
        assert_eq!(cursor.machine_id(), "machine");
        assert_eq!(cursor.resolved_client_version(), "cli-2026.07.08-0c04a8a");
        assert_eq!(cursor.config_version(), "config");
        assert_eq!(cursor.client_id(), "client");
    }

    #[test]
    fn agentservice_headers_are_rail_specific_and_strip_composite_tokens() {
        let account = cursor_account_for_api_key("cursor-key", None);
        let expected_sdk_version = sdk_client_version();
        let cli = cursor_agentservice_headers(
            CursorProtocolRail::OAuthCli,
            &account,
            "subject::access-token",
        );
        let sdk =
            cursor_agentservice_headers(CursorProtocolRail::ApiKeySdk, &account, "access-token");
        assert!(cli
            .iter()
            .any(|(key, value)| { key == "authorization" && value == "Bearer access-token" }));
        assert!(cli
            .iter()
            .any(|(key, value)| key == CLIENT_TYPE_HEADER && value == "cli"));
        assert!(cli.iter().any(|(key, _)| key == "traceparent"));
        assert!(cli.iter().any(|(key, _)| key == "backend-traceparent"));
        assert!(cli
            .iter()
            .any(|(key, value)| key == "connect-accept-encoding" && value == "gzip"));
        assert!(sdk
            .iter()
            .any(|(key, value)| key == CLIENT_TYPE_HEADER && value == SDK_CLIENT_TYPE));
        assert!(sdk.iter().any(|(key, value)| {
            key == CLIENT_VERSION_HEADER && value == &expected_sdk_version
        }));
        assert!(!sdk.iter().any(|(key, _)| key == "traceparent"));
        assert!(!sdk.iter().any(|(key, _)| key == "backend-traceparent"));
        assert!(!sdk.iter().any(|(key, _)| key == "connect-accept-encoding"));
        for headers in [&cli, &sdk] {
            assert!(!headers.iter().any(|(key, _)| {
                matches!(
                    key.as_str(),
                    "x-cursor-checksum"
                        | "x-client-key"
                        | "x-cursor-config-version"
                        | "x-cursor-client-id"
                        | "x-amzn-trace-id"
                )
            }));
        }
    }

    #[test]
    fn cli_version_rejects_ide_semver_and_accepts_agent_build_ids() {
        assert_eq!(normalize_cursor_cli_version("3.1.2"), None);
        assert_eq!(
            normalize_cursor_cli_version("2026.07.08-0c04a8a").as_deref(),
            Some("cli-2026.07.08-0c04a8a")
        );
        assert_eq!(
            normalize_cursor_cli_version("cli-2026.07.08-0c04a8a").as_deref(),
            Some("cli-2026.07.08-0c04a8a")
        );
    }
}
