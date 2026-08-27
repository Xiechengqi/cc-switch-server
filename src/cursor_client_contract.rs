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

pub(crate) fn sdk_client_version() -> String {
    std::env::var("CC_SWITCH_CURSOR_SDK_CLIENT_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| value.starts_with("sdk-") && value.len() > 4)
        .unwrap_or_else(|| DEFAULT_SDK_CLIENT_VERSION.to_string())
}
