//! Shared Cursor client identity contract.
//!
//! Public API verification and AgentService transport must present the same
//! SDK identity. Keeping these constants outside `clients` and `proxy` avoids
//! reversing either dependency boundary.

pub(crate) const CLIENT_TYPE_HEADER: &str = "x-cursor-client-type";
pub(crate) const CLIENT_VERSION_HEADER: &str = "x-cursor-client-version";
pub(crate) const GHOST_MODE_HEADER: &str = "x-ghost-mode";
pub(crate) const SDK_CLIENT_TYPE: &str = "sdk";
pub(crate) const GHOST_MODE_ENABLED: &str = "true";
pub(crate) const PUBLIC_API_CLIENT_VERSION: &str = "composer-api-0.1.0";
pub(crate) const DEFAULT_SDK_CLIENT_VERSION: &str = "sdk-1.0.13";
pub(crate) const DEFAULT_API_KEY_EXCHANGE_URL: &str =
    "https://api2.cursor.sh/auth/exchange_user_api_key";
pub(crate) const DEFAULT_DASHBOARD_PROFILE_URL: &str = "https://cursor.com/api/auth/me";
pub(crate) const DASHBOARD_ORIGIN: &str = "https://cursor.com";
pub(crate) const DASHBOARD_REFERER: &str = "https://cursor.com/dashboard";
pub(crate) const DASHBOARD_USER_AGENT: &str = "Cursor/1.1.6 (cc-switch browser login)";

pub(crate) fn cursor_membership_label(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let label = match lower.as_str() {
        "free" => "Cursor Free".to_string(),
        "pro" => "Cursor Pro".to_string(),
        "pro_plus" | "pro+" => "Cursor Pro+".to_string(),
        "ultra" => "Cursor Ultra".to_string(),
        _ if lower.starts_with("cursor ") => value.to_string(),
        _ => format!("Cursor {value}"),
    };
    Some(label)
}

pub(crate) fn sdk_client_version() -> String {
    std::env::var("CC_SWITCH_CURSOR_SDK_CLIENT_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| value.starts_with("sdk-") && value.len() > 4)
        .unwrap_or_else(|| DEFAULT_SDK_CLIENT_VERSION.to_string())
}
