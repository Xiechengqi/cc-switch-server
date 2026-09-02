use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const TRAE_OAUTH_ORIGIN: &str = "https://api.trae.com.cn";
pub const TRAE_AGENT_ORIGIN: &str = "https://trae-api-cn.mchost.guru";
pub const TRAE_BILLING_ORIGIN: &str = "https://api.trae.cn";
pub const TRAE_CONSOLE_ORIGIN: &str = "https://www.trae.cn";
pub const TRAE_EXCHANGE_TOKEN_PATH: &str = "/cloudide/api/v3/trae/oauth/ExchangeToken";
pub const TRAE_USER_INFO_PATH: &str = "/cloudide/api/v3/trae/GetUserInfo";
pub const TRAE_CHAT_PATH: &str = "/api/agent/v3/llm_utils_chat";
pub const TRAE_MODEL_DETAIL_PATH: &str = "/api/ide/v1/get_detail_param";
pub const TRAE_QUOTA_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
pub const TRAE_AUTHORIZATION_PATH: &str = "/authorization";
pub const TRAE_CLIENT_ID: &str = "en1oxy7wnw8j9n";
pub const TRAE_APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
pub const TRAE_IDE_VERSION: &str = "0.1.52";
pub const TRAE_IDE_VERSION_CODE: &str = "20260811";
pub const TRAE_PLUGIN_VERSION: &str = "2.3.62834";
pub const TRAE_DEVICE_BRAND: &str = "83DG";
pub const TRAE_OS_VERSION: &str = "Windows 11 Pro";
pub const TRAE_FUNCTION: &str = "solo_work_lite";
pub const TRAE_DEFAULT_MODEL: &str = "glm-5.2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraeAccountProfile {
    pub uid: String,
    #[serde(default)]
    pub enterprise_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub machine_id: String,
    pub device_id: String,
}

impl TraeAccountProfile {
    pub fn parse(value: Option<&Value>) -> Result<Self, String> {
        let value = value
            .and_then(Value::as_object)
            .ok_or_else(|| "Trae account profile must be an object".to_string())?;
        let profile = Self {
            uid: profile_string(value, &["uid", "userId", "user_id", "sub"]),
            enterprise_id: profile_string(
                value,
                &["enterpriseId", "enterprise_id", "tenantId", "tenant_id"],
            ),
            name: profile_string(value, &["name", "userName", "user_name", "nickname"]),
            email: profile_string(value, &["email"]),
            machine_id: profile_string(value, &["machineId", "machine_id"]),
            device_id: profile_string(value, &["deviceId", "device_id"]),
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("uid", self.uid.as_str()),
            ("machineId", self.machine_id.as_str()),
            ("deviceId", self.device_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("Trae account profile is missing {label}"));
            }
            if value.len() > 512 || value.chars().any(char::is_control) {
                return Err(format!("Trae account profile has an invalid {label}"));
            }
        }
        Ok(())
    }

    pub fn stable_identity_components(&self) -> [&str; 4] {
        [
            self.uid.trim(),
            self.enterprise_id.trim(),
            self.machine_id.trim(),
            self.device_id.trim(),
        ]
    }
}

pub fn random_trae_identity() -> (String, String) {
    (random_hex(16), random_hex(16))
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    hex::encode(value)
}

pub fn trae_account_id(uid: &str) -> Result<String, String> {
    let uid = uid.trim();
    if uid.is_empty() {
        return Err("Trae account id requires uid".to_string());
    }
    let mut digest = Sha256::new();
    digest.update(b"cc-switch/trae-cn-solo/account-id/v1");
    digest.update([0]);
    digest.update(uid.as_bytes());
    let encoded = hex::encode(digest.finalize());
    Ok(format!("trae-{}", &encoded[..12]))
}

pub fn contains_untrusted_host_field(value: Option<&Value>) -> bool {
    fn visit(value: &Value) -> bool {
        match value {
            Value::Object(object) => object.iter().any(|(key, value)| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "api_host" | "apihost" | "api_url" | "apiurl" | "base_url" | "baseurl"
                ) && value.as_str().is_some_and(|value| !value.trim().is_empty())
                    || visit(value)
            }),
            Value::Array(values) => values.iter().any(visit),
            _ => false,
        }
    }
    value.is_some_and(visit)
}

fn profile_string(value: &serde_json::Map<String, Value>, fields: &[&str]) -> String {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn imported_hosts_are_detected_recursively() {
        assert!(contains_untrusted_host_field(Some(&json!({
            "token": {"api_host": "https://attacker.invalid"}
        }))));
        assert!(!contains_untrusted_host_field(Some(&json!({
            "source": "fixture"
        }))));
    }

    #[test]
    fn identity_generation_inputs_include_device_ownership() {
        let profile = TraeAccountProfile::parse(Some(&json!({
            "uid": "u1",
            "machineId": "m1",
            "deviceId": "d1"
        })))
        .unwrap();
        assert_eq!(profile.stable_identity_components(), ["u1", "", "m1", "d1"]);
    }
}
