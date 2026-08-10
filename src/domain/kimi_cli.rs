use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
pub const KIMI_CLI_VERSION: &str = "1.37.0";
pub const KIMI_USER_AGENT: &str = "KimiCLI/1.37.0";
pub const KIMI_OAUTH_HOST: &str = "https://auth.kimi.com";
pub const KIMI_TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
pub const KIMI_DEVICE_AUTHORIZATION_URL: &str =
    "https://auth.kimi.com/api/oauth/device_authorization";
pub const KIMI_API_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub const KIMI_DEFAULT_MODEL: &str = "kimi-for-coding";

const KIMI_DEVICE_PROFILE_KEY: &str = "kimiDevice";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiDeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    pub device_model: String,
    pub os_version: String,
}

impl KimiDeviceIdentity {
    pub fn random() -> Self {
        let mut bytes = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self::with_device_id(hex::encode(bytes))
    }

    pub fn stable_for_account(account_id: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-server:kimi-device:v1:");
        digest.update(account_id.trim().as_bytes());
        let digest = digest.finalize();
        Self::with_device_id(hex::encode(&digest[..16]))
    }

    fn with_device_id(device_id: String) -> Self {
        let device_name = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        Self {
            device_id,
            device_name: ascii_header_value(&device_name, "unknown"),
            device_model: ascii_header_value(
                &format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                "unknown",
            ),
            os_version: ascii_header_value(std::env::consts::ARCH, "unknown"),
        }
    }

    pub fn headers(&self) -> Vec<(String, String)> {
        vec![
            ("X-Msh-Platform".to_string(), "kimi_cli".to_string()),
            ("X-Msh-Version".to_string(), KIMI_CLI_VERSION.to_string()),
            ("X-Msh-Device-Name".to_string(), self.device_name.clone()),
            ("X-Msh-Device-Model".to_string(), self.device_model.clone()),
            ("X-Msh-Os-Version".to_string(), self.os_version.clone()),
            ("X-Msh-Device-Id".to_string(), self.device_id.clone()),
            ("User-Agent".to_string(), KIMI_USER_AGENT.to_string()),
        ]
    }
}

pub fn extract_user_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    ["/user_id", "/userId"]
        .into_iter()
        .find_map(|pointer| claims.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn account_record_id(user_id_or_seed: &str) -> String {
    let digest = Sha256::digest(user_id_or_seed.trim().as_bytes());
    format!("kimi-code-{}", hex::encode(&digest[..16]))
}

pub fn device_identity_from_profile(profile: Option<&Value>) -> Option<KimiDeviceIdentity> {
    let profile = profile?;
    let candidate = profile
        .get(KIMI_DEVICE_PROFILE_KEY)
        .filter(|value| value.is_object())
        .unwrap_or(profile);
    let identity = KimiDeviceIdentity {
        device_id: profile_string(candidate, &["deviceId", "device_id"])?,
        device_name: profile_string(candidate, &["deviceName", "device_name"])
            .unwrap_or_else(|| "unknown".to_string()),
        device_model: profile_string(candidate, &["deviceModel", "device_model"])
            .unwrap_or_else(|| "unknown".to_string()),
        os_version: profile_string(candidate, &["osVersion", "os_version"])
            .unwrap_or_else(|| "unknown".to_string()),
    };
    valid_device_id(&identity.device_id).then_some(identity)
}

pub fn user_id_from_profile(profile: Option<&Value>) -> Option<String> {
    let profile = profile?;
    ["/userId", "/user_id", "/accountId", "/account_id"]
        .into_iter()
        .find_map(|pointer| profile.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn enrich_profile(
    profile: &mut Option<Value>,
    user_id: Option<&str>,
    identity: &KimiDeviceIdentity,
) {
    let value = profile
        .take()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.insert("providerType".to_string(), json!("kimi_code"));
    if let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) {
        object.insert("userId".to_string(), Value::String(user_id.to_string()));
        object.insert("accountId".to_string(), Value::String(user_id.to_string()));
    }
    object.insert(
        KIMI_DEVICE_PROFILE_KEY.to_string(),
        serde_json::to_value(identity).unwrap_or(Value::Null),
    );
    *profile = Some(Value::Object(object));
}

fn profile_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| ascii_header_value(value, "unknown"))
}

fn valid_device_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn ascii_header_value(value: &str, fallback: &str) -> String {
    let value = value
        .chars()
        .filter(|character| character.is_ascii() && !character.is_ascii_control())
        .collect::<String>();
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.chars().take(128).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_kimi_user_id_and_derives_stable_account_identity() {
        let token = "e30.eyJ1c2VyX2lkIjoidXNlci0xIn0.signature";
        assert_eq!(extract_user_id(token).as_deref(), Some("user-1"));
        assert_eq!(account_record_id("user-1"), account_record_id("user-1"));
    }

    #[test]
    fn profile_round_trip_preserves_account_scoped_device() {
        let identity = KimiDeviceIdentity::stable_for_account("account-1");
        let mut profile = Some(json!({"plan": "coding"}));
        enrich_profile(&mut profile, Some("user-1"), &identity);
        assert_eq!(
            device_identity_from_profile(profile.as_ref()),
            Some(identity)
        );
        assert_eq!(
            user_id_from_profile(profile.as_ref()).as_deref(),
            Some("user-1")
        );
    }
}
