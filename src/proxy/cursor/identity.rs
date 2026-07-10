use rand::RngCore;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::accounts::cursor_import::detect_cursor_ide_version;
use crate::domain::accounts::store::Account;

use super::h2_client::agent_connect_headers;

pub const DEFAULT_CURSOR_CLIENT_VERSION: &str = "cli-2026.01.09-231024f";
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
            .clone()
            .or_else(detect_cursor_cli_version)
            .unwrap_or_else(|| DEFAULT_CURSOR_CLIENT_VERSION.to_string())
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

pub fn cursor_account_for_api_key(api_key: &str) -> CursorAccountData {
    let hash = sha256_hex(api_key);
    CursorAccountData {
        account_id: format!("cursor_apikey_{}", &hash[..24]),
        email: None,
        refresh_token: None,
        id_token: None,
        cursor_service_machine_id: Some(hash.clone()),
        cursor_client_version: Some(DEFAULT_CURSOR_CLIENT_VERSION.to_string()),
        cursor_config_version: Some(stable_uuid_like(&format!("cursor-config:{hash}"))),
        cursor_client_id: None,
    }
}

pub fn cursor_agent_headers(account: &CursorAccountData, token: &str) -> Vec<(String, String)> {
    let mut headers = vec![
        ("authorization".to_string(), format!("Bearer {token}")),
        (
            "accept".to_string(),
            "application/connect+proto".to_string(),
        ),
        ("accept-encoding".to_string(), "gzip".to_string()),
        ("x-amzn-trace-id".to_string(), random_uuid_like()),
    ];
    headers.extend(agent_connect_headers());
    headers.extend(identity_headers(account, token));
    headers
}

pub fn cursor_agentservice_headers(
    account: &CursorAccountData,
    token: &str,
) -> Vec<(String, String)> {
    let mut headers = agent_connect_headers();
    let request_id = random_uuid_like();
    let traceparent = random_traceparent();
    headers.extend([
        ("authorization".to_string(), format!("Bearer {token}")),
        (
            "x-cursor-client-version".to_string(),
            account.resolved_client_version(),
        ),
        ("x-cursor-client-type".to_string(), "cli".to_string()),
        ("x-cursor-client-os".to_string(), cursor_os().to_string()),
        (
            "x-cursor-client-arch".to_string(),
            std::env::consts::ARCH.to_string(),
        ),
        ("x-ghost-mode".to_string(), "true".to_string()),
        ("traceparent".to_string(), traceparent.clone()),
        ("backend-traceparent".to_string(), traceparent),
        ("x-request-id".to_string(), request_id.clone()),
        ("x-original-request-id".to_string(), request_id),
    ]);
    headers
}

pub fn identity_headers(account: &CursorAccountData, token: &str) -> Vec<(String, String)> {
    let machine_id = account.machine_id();
    vec![
        ("x-client-key".to_string(), sha256_hex(token)),
        (
            "x-cursor-checksum".to_string(),
            build_cursor_checksum(token, machine_id),
        ),
        (
            "x-cursor-client-version".to_string(),
            account.resolved_client_version(),
        ),
        ("x-cursor-client-type".to_string(), "ide".to_string()),
        ("x-cursor-client-os".to_string(), cursor_os().to_string()),
        (
            "x-cursor-client-arch".to_string(),
            std::env::consts::ARCH.to_string(),
        ),
        (
            "x-cursor-client-device-type".to_string(),
            "desktop".to_string(),
        ),
        (
            "x-cursor-config-version".to_string(),
            account.config_version(),
        ),
        (
            "x-cursor-client-id".to_string(),
            account.client_id().to_string(),
        ),
        ("x-cursor-timezone".to_string(), cursor_timezone()),
        ("x-ghost-mode".to_string(), "true".to_string()),
        ("x-session-id".to_string(), random_uuid_like()),
        ("x-request-id".to_string(), random_uuid_like()),
    ]
}

fn cursor_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

fn cursor_timezone() -> String {
    std::env::var("TZ")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "UTC".to_string())
}

fn detect_cursor_cli_version() -> Option<String> {
    let version = detect_cursor_ide_version()?;
    Some(if version.starts_with("cli-") {
        version
    } else {
        format!("cli-{version}")
    })
}

fn build_cursor_checksum(token: &str, machine_id: &str) -> String {
    let stable_machine_id = if machine_id.is_empty() {
        sha256_hex(&format!("{token}machineId"))
    } else {
        machine_id.to_string()
    };
    let timestamp = (chrono::Utc::now().timestamp_millis() / 1_000_000) as u64;
    let mut buf = [
        ((timestamp >> 40) & 0xff) as u8,
        ((timestamp >> 32) & 0xff) as u8,
        ((timestamp >> 24) & 0xff) as u8,
        ((timestamp >> 16) & 0xff) as u8,
        ((timestamp >> 8) & 0xff) as u8,
        (timestamp & 0xff) as u8,
    ];
    let mut previous = 165u8;
    for (index, byte) in buf.iter_mut().enumerate() {
        *byte = (*byte ^ previous).wrapping_add((index % 256) as u8);
        previous = *byte;
    }
    format!("{}{}", jyh_encode(&buf), stable_machine_id)
}

fn jyh_encode(bytes: &[u8]) -> String {
    const URL_SAFE_BASE64: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let a = bytes[index];
        let b = if index + 1 < bytes.len() {
            bytes[index + 1]
        } else {
            0
        };
        let c = if index + 2 < bytes.len() {
            bytes[index + 2]
        } else {
            0
        };
        out.push(URL_SAFE_BASE64[(a >> 2) as usize] as char);
        out.push(URL_SAFE_BASE64[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        if index + 1 < bytes.len() {
            out.push(URL_SAFE_BASE64[(((b & 15) << 2) | (c >> 6)) as usize] as char);
        }
        if index + 2 < bytes.len() {
            out.push(URL_SAFE_BASE64[(c & 63) as usize] as char);
        }
        index += 3;
    }
    out
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
        let account = cursor_account_for_api_key("cursor-key");
        assert!(account.account_id.starts_with("cursor_apikey_"));
        assert_eq!(account.machine_id().len(), 64);
        assert_eq!(
            account.resolved_client_version(),
            DEFAULT_CURSOR_CLIENT_VERSION
        );
    }

    #[test]
    fn managed_account_reads_cursor_raw_metadata() {
        let account = Account {
            id: "cursor_1".to_string(),
            provider_type: ProviderType::CursorOAuth,
            email: Some("u@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: None,
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: Some(json!({
                "cursorServiceMachineId": "machine",
                "cursorClientVersion": "cli-test",
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
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        };
        let cursor = cursor_account_from_managed_account(&account);
        assert_eq!(cursor.machine_id(), "machine");
        assert_eq!(cursor.resolved_client_version(), "cli-test");
        assert_eq!(cursor.config_version(), "config");
        assert_eq!(cursor.client_id(), "client");
    }

    #[test]
    fn agent_headers_include_connect_and_identity() {
        let account = cursor_account_for_api_key("cursor-key");
        let headers = cursor_agent_headers(&account, "access-token");
        assert!(headers.iter().any(|(key, _)| key == "authorization"));
        assert!(headers.iter().any(|(key, _)| key == "content-type"));
        assert!(headers.iter().any(|(key, _)| key == "x-cursor-checksum"));
    }
}
