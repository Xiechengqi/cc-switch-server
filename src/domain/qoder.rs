use aes::Aes128;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cbc::cipher::block_padding::Pkcs7;
#[cfg(test)]
use cbc::cipher::BlockDecryptMut;
use cbc::cipher::{BlockEncryptMut, KeyIvInit};
use rand::rngs::OsRng;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const STANDARD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const CUSTOM_ALPHABET: &[u8; 64] =
    b"_doRTgHZBKcGVjlvpC,@aFSx#DPuNJme&i*MzLOEn)sUrthbf%Y^w.(kIQyXqWA!";

pub const QODER_RSA_PUBLIC_KEY: &str = r#"-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDA8iMH5c02LilrsERw9t6Pv5Nc
4k6Pz1EaDicBMpdpxKduSZu5OANqUq8er4GM95omAGIOPOh+Nx0spthYA2BqGz+l
6HRkPJ7S236FZz73In/KVuLnwI8JJ2CbuJap8kvheCCZpmAWpb/cPx/3Vr/J6I17
XcW+ML9FoCI6AOvOzwIDAQAB
-----END PUBLIC KEY-----"#;

pub const QODER_GENERATION_PATH: &str =
    "/algo/api/v2/service/pro/sse/agent_chat_generation?FetchKeys=llm_model_result&AgentId=agent_common&Encode=1";
pub const QODER_GENERATION_SIGNATURE_PATH: &str = "/api/v2/service/pro/sse/agent_chat_generation";
pub const QODER_MODEL_LIST_PATH: &str = "/algo/api/v2/model/list";
pub const QODER_MODEL_LIST_SIGNATURE_PATH: &str = "/api/v2/model/list";
pub const QODER_QUOTA_PATH: &str = "/algo/api/v2/quota/usage";
pub const QODER_QUOTA_SIGNATURE_PATH: &str = "/api/v2/quota/usage";
pub const QODER_PAT_EXCHANGE_PATH: &str = "/api/v1/jobToken/exchange";

pub const QODER_REFRESH_MODE_COSY: &str = "cosy";
pub const QODER_REFRESH_MODE_QODER_CN20: &str = "qodercn20";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QoderCredentialRail {
    GlobalOauth,
    PatJobToken,
    CnOauth,
}

impl QoderCredentialRail {
    pub fn parse(value: &str, site: QoderSite) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" if site == QoderSite::Global => Ok(Self::GlobalOauth),
            "" if site == QoderSite::Cn => Ok(Self::CnOauth),
            "global_oauth" | "global-oauth" | "oauth" if site == QoderSite::Global => {
                Ok(Self::GlobalOauth)
            }
            "pat_job_token" | "pat-job-token" | "pat" if site == QoderSite::Global => {
                Ok(Self::PatJobToken)
            }
            "cn_oauth" | "cn-oauth" | "qodercn20" if site == QoderSite::Cn => Ok(Self::CnOauth),
            value => Err(format!(
                "unsupported Qoder credentialRail {value:?} for {} site",
                site.as_str()
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GlobalOauth => "global_oauth",
            Self::PatJobToken => "pat_job_token",
            Self::CnOauth => "cn_oauth",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderAccountProfile {
    pub site: QoderSite,
    pub credential_rail: QoderCredentialRail,
    pub refresh_mode: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub aid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub organization_id: String,
    #[serde(default)]
    pub organization_name: String,
    #[serde(default)]
    pub user_type: String,
    pub machine_id: String,
    #[serde(default)]
    pub machine_type: String,
}

impl QoderAccountProfile {
    pub fn parse(value: Option<&Value>) -> Result<Self, String> {
        let value = value
            .and_then(Value::as_object)
            .ok_or_else(|| "Qoder account profile must be an object".to_string())?;
        let site = QoderSite::parse(
            value
                .get("site")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let refresh_mode = parse_refresh_mode(
            value
                .get("refreshMode")
                .or_else(|| value.get("refresh_mode"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            site,
        )?;
        let credential_rail = QoderCredentialRail::parse(
            value
                .get("credentialRail")
                .or_else(|| value.get("credential_rail"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            site,
        )?;
        let profile = Self {
            site,
            credential_rail,
            refresh_mode: refresh_mode.to_string(),
            uid: profile_string(value, &["uid", "userId", "user_id"]),
            aid: profile_string(value, &["aid", "accountId", "account_id"]),
            name: profile_string(value, &["name", "userName", "user_name"]),
            email: profile_string(value, &["email"]),
            organization_id: profile_string(value, &["organizationId", "organization_id", "orgId"]),
            organization_name: profile_string(
                value,
                &["organizationName", "organization_name", "orgName"],
            ),
            user_type: profile_string(value, &["userType", "user_type"]),
            machine_id: profile_string(value, &["machineId", "machine_id"]),
            machine_type: profile_string(value, &["machineType", "machine_type"]),
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.uid.trim().is_empty() {
            return Err("Qoder account profile is missing uid".to_string());
        }
        if self.machine_id.trim().is_empty() {
            return Err("Qoder account profile is missing machineId".to_string());
        }
        if self.site == QoderSite::Cn && self.refresh_mode != QODER_REFRESH_MODE_QODER_CN20 {
            return Err("Qoder CN accounts require qodercn20 refreshMode".to_string());
        }
        if self.site == QoderSite::Global && self.refresh_mode != QODER_REFRESH_MODE_COSY {
            return Err("Qoder global accounts require cosy refreshMode".to_string());
        }
        match (self.site, self.credential_rail) {
            (
                QoderSite::Global,
                QoderCredentialRail::GlobalOauth | QoderCredentialRail::PatJobToken,
            )
            | (QoderSite::Cn, QoderCredentialRail::CnOauth) => {}
            _ => {
                return Err(format!(
                    "Qoder credential rail {} is not available for {} site",
                    self.credential_rail.as_str(),
                    self.site.as_str()
                ))
            }
        }
        Ok(())
    }

    pub fn identity(&self, access_token: &str, refresh_token: &str) -> QoderIdentity {
        QoderIdentity {
            name: self.name.clone(),
            aid: self.aid.clone(),
            uid: self.uid.clone(),
            organization_id: self.organization_id.clone(),
            organization_name: self.organization_name.clone(),
            user_type: if self.user_type.trim().is_empty() {
                "personal_standard".to_string()
            } else {
                self.user_type.clone()
            },
            security_oauth_token: access_token.trim().to_string(),
            refresh_token: refresh_token.trim().to_string(),
        }
    }

    pub fn machine(&self, machine_token: &str) -> QoderMachineIdentity {
        QoderMachineIdentity {
            machine_id: self.machine_id.clone(),
            machine_token: machine_token.trim().to_string(),
            machine_type: self.machine_type.clone(),
        }
    }

    pub fn stable_identity_components(&self) -> [&str; 6] {
        [
            self.site.as_str(),
            self.credential_rail.as_str(),
            self.uid.trim(),
            self.aid.trim(),
            self.organization_id.trim(),
            self.machine_id.trim(),
        ]
    }
}

pub fn qoder_account_id(
    site: QoderSite,
    rail: QoderCredentialRail,
    uid: &str,
) -> Result<String, String> {
    let uid = uid.trim();
    if uid.is_empty() {
        return Err("Qoder account id requires uid".to_string());
    }
    let mut digest = Sha256::new();
    digest.update(site.as_str().as_bytes());
    digest.update([0]);
    digest.update(rail.as_str().as_bytes());
    digest.update([0]);
    digest.update(uid.as_bytes());
    let suffix = hex::encode(digest.finalize());
    Ok(format!("qoder-{}-{}", site.as_str(), &suffix[..16]))
}

pub fn random_qoder_token(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes.max(1)];
    OsRng.fill_bytes(&mut value);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value)
}

pub fn random_qoder_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes.max(1)];
    OsRng.fill_bytes(&mut value);
    hex::encode(value)
}

pub fn random_qoder_uuid() -> String {
    let mut value = [0_u8; 16];
    OsRng.fill_bytes(&mut value);
    value[6] = (value[6] & 0x0f) | 0x40;
    value[8] = (value[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes(value[0..4].try_into().expect("four bytes")),
        u16::from_be_bytes(value[4..6].try_into().expect("two bytes")),
        u16::from_be_bytes(value[6..8].try_into().expect("two bytes")),
        u16::from_be_bytes(value[8..10].try_into().expect("two bytes")),
        u64::from_be_bytes([
            0, 0, value[10], value[11], value[12], value[13], value[14], value[15]
        ])
    )
}

pub fn qoder_machine_os() -> String {
    let arch = std::env::consts::ARCH;
    format!("{arch}_{}", std::env::consts::OS)
}

pub fn random_qoder_machine(site: QoderSite) -> QoderMachineIdentity {
    match site {
        QoderSite::Global => QoderMachineIdentity {
            machine_id: random_qoder_hex(18),
            machine_token: random_qoder_token(38),
            machine_type: random_qoder_hex(9),
        },
        QoderSite::Cn => QoderMachineIdentity {
            machine_id: random_qoder_uuid(),
            machine_token: String::new(),
            machine_type: String::new(),
        },
    }
}

pub fn machine_token_from_raw(value: Option<&Value>) -> Option<String> {
    let value = value?;
    [
        "/qoderSecrets/machineToken",
        "/qoderSecrets/machine_token",
        "/machineToken",
        "/machine_token",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_string)
}

pub fn parse_refresh_mode(value: &str, site: QoderSite) -> Result<&'static str, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" if site == QoderSite::Global => Ok(QODER_REFRESH_MODE_COSY),
        "" if site == QoderSite::Cn => Ok(QODER_REFRESH_MODE_QODER_CN20),
        QODER_REFRESH_MODE_COSY if site == QoderSite::Global => Ok(QODER_REFRESH_MODE_COSY),
        QODER_REFRESH_MODE_QODER_CN20 if site == QoderSite::Cn => Ok(QODER_REFRESH_MODE_QODER_CN20),
        value => Err(format!(
            "unsupported Qoder refreshMode {value:?} for {} site",
            site.as_str()
        )),
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QoderSite {
    Global,
    Cn,
}

impl QoderSite {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "global" => Ok(Self::Global),
            "cn" => Ok(Self::Cn),
            value => Err(format!("unsupported Qoder site {value:?}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Cn => "cn",
        }
    }

    pub fn profile(self) -> QoderSiteProfile {
        match self {
            Self::Global => QoderSiteProfile {
                site: self,
                device_authorization_url: "https://qoder.com/device/selectAccounts",
                openapi_base_url: "https://openapi.qoder.sh",
                center_base_url: Some("https://center.qoder.sh"),
                gateway_base_url: "https://api1.qoder.sh",
                job_gateway_base_url: "https://api2.qoder.sh",
                client_version: "1.21.2",
                oauth_client_id: "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb",
                client_type: "5",
                data_policy: "disagree",
            },
            Self::Cn => QoderSiteProfile {
                site: self,
                device_authorization_url: "https://qoder.com.cn/device/selectAccounts",
                openapi_base_url: "https://openapi.qoder.com.cn",
                center_base_url: None,
                gateway_base_url: "https://gateway.qoder.com.cn",
                job_gateway_base_url: "https://gateway.qoder.com.cn",
                client_version: "1.10.0",
                oauth_client_id: "f5a7f67c-11a8-491e-8b8e-a07f2d0df4b7",
                client_type: "0",
                data_policy: "DISAGREE",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QoderSiteProfile {
    pub site: QoderSite,
    pub device_authorization_url: &'static str,
    pub openapi_base_url: &'static str,
    pub center_base_url: Option<&'static str>,
    pub gateway_base_url: &'static str,
    pub job_gateway_base_url: &'static str,
    pub client_version: &'static str,
    pub oauth_client_id: &'static str,
    pub client_type: &'static str,
    pub data_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderIdentity {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aid: String,
    pub uid: String,
    #[serde(default)]
    pub organization_id: String,
    #[serde(default)]
    pub organization_name: String,
    #[serde(default)]
    pub user_type: String,
    pub security_oauth_token: String,
    #[serde(default)]
    pub refresh_token: String,
}

impl QoderIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.uid.trim().is_empty() {
            return Err("Qoder identity is missing uid".to_string());
        }
        if self.security_oauth_token.trim().is_empty() {
            return Err("Qoder identity is missing security OAuth token".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderMachineIdentity {
    pub machine_id: String,
    #[serde(default)]
    pub machine_token: String,
    #[serde(default)]
    pub machine_type: String,
}

impl QoderMachineIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.machine_id.trim().is_empty() {
            return Err("Qoder identity is missing machine id".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct QoderCosySession {
    pub site: QoderSite,
    pub client_version: String,
    pub identity: QoderIdentity,
    pub machine: QoderMachineIdentity,
    pub cosy_key: String,
    pub encrypted_info: String,
}

impl QoderCosySession {
    pub fn new(
        site: QoderSite,
        identity: QoderIdentity,
        machine: QoderMachineIdentity,
    ) -> Result<Self, String> {
        let mut key = [0_u8; 16];
        OsRng.fill_bytes(&mut key);
        Self::new_with_key(site, identity, machine, &key)
    }

    pub fn new_with_key(
        site: QoderSite,
        identity: QoderIdentity,
        machine: QoderMachineIdentity,
        key: &[u8],
    ) -> Result<Self, String> {
        identity.validate()?;
        machine.validate()?;
        if key.len() != 16 {
            return Err("Qoder COSY AES key must be exactly 16 bytes".to_string());
        }
        let public_key = RsaPublicKey::from_public_key_pem(QODER_RSA_PUBLIC_KEY)
            .map_err(|error| format!("parse Qoder COSY public key: {error}"))?;
        let encrypted_key = public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, key)
            .map_err(|error| format!("encrypt Qoder COSY key: {error}"))?;
        let identity_json = serde_json::to_vec(&identity)
            .map_err(|error| format!("encode Qoder COSY identity: {error}"))?;
        let encrypted_info = aes_encrypt(&identity_json, key)?;
        Ok(Self {
            site,
            client_version: site.profile().client_version.to_string(),
            identity,
            machine,
            cosy_key: STANDARD.encode(encrypted_key),
            encrypted_info: STANDARD.encode(encrypted_info),
        })
    }

    pub fn signed_headers(
        &self,
        encoded_body: &[u8],
        signature_path: &str,
        unix_seconds: i64,
        request_id: &str,
        machine_os: &str,
        client_ip: &str,
    ) -> Result<Vec<(String, String)>, String> {
        if signature_path.trim().is_empty() || !signature_path.starts_with('/') {
            return Err("Qoder signature path must be an absolute path".to_string());
        }
        if request_id.trim().is_empty() {
            return Err("Qoder request id is required".to_string());
        }
        let payload = json!({
            "cosyVersion": self.client_version,
            "ideVersion": "",
            "info": self.encrypted_info,
            "requestId": request_id,
            "version": "v1"
        });
        let payload = serde_json::to_vec(&payload)
            .map_err(|error| format!("encode Qoder COSY payload: {error}"))?;
        let payload_b64 = STANDARD.encode(payload);
        let body = std::str::from_utf8(encoded_body)
            .map_err(|_| "Qoder encoded body is not ASCII".to_string())?;
        let date = unix_seconds.to_string();
        let signature =
            sign_qoder_request(&payload_b64, &self.cosy_key, &date, body, signature_path);
        let profile = self.site.profile();
        let machine_token = if self.site == QoderSite::Cn {
            ""
        } else if self.machine.machine_token.trim().is_empty() {
            self.machine.machine_id.as_str()
        } else {
            self.machine.machine_token.as_str()
        };
        let machine_type = if self.site == QoderSite::Cn {
            ""
        } else if self.machine.machine_type.trim().is_empty() {
            "5"
        } else {
            self.machine.machine_type.as_str()
        };
        let effective_client_ip = if self.site == QoderSite::Cn {
            client_ip
        } else {
            self.machine.machine_id.as_str()
        };
        let organization_tags = if self.site == QoderSite::Cn {
            ""
        } else {
            "Normal"
        };
        let mut headers = vec![
            (
                "authorization".to_string(),
                compose_bearer(&payload_b64, &signature),
            ),
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
            ("accept-encoding".to_string(), "identity".to_string()),
            ("cache-control".to_string(), "no-cache".to_string()),
            ("cosy-key".to_string(), self.cosy_key.clone()),
            ("cosy-user".to_string(), self.identity.uid.clone()),
            ("cosy-date".to_string(), date),
            ("cosy-version".to_string(), self.client_version.clone()),
            (
                "cosy-machineid".to_string(),
                self.machine.machine_id.clone(),
            ),
            ("cosy-machinetoken".to_string(), machine_token.to_string()),
            ("cosy-machinetype".to_string(), machine_type.to_string()),
            ("cosy-machineos".to_string(), machine_os.to_string()),
            (
                "cosy-clienttype".to_string(),
                profile.client_type.to_string(),
            ),
            ("cosy-clientip".to_string(), effective_client_ip.to_string()),
            ("cosy-bodyhash".to_string(), md5_hex(encoded_body)),
            (
                "cosy-bodylength".to_string(),
                encoded_body.len().to_string(),
            ),
            ("cosy-sigpath".to_string(), signature_path.to_string()),
            (
                "cosy-data-policy".to_string(),
                profile.data_policy.to_string(),
            ),
            (
                "cosy-organization-id".to_string(),
                self.identity.organization_id.clone(),
            ),
            (
                "cosy-organization-tags".to_string(),
                organization_tags.to_string(),
            ),
            ("login-version".to_string(), "v2".to_string()),
            ("x-request-id".to_string(), request_id.to_string()),
        ];
        if self.site == QoderSite::Global {
            headers.extend([
                ("cosy-scene".to_string(), "assistant".to_string()),
                ("cosy-business-product".to_string(), "cli".to_string()),
                ("cosy-business-type".to_string(), "agent".to_string()),
            ]);
        } else {
            headers.push(("cosy-machinecode".to_string(), String::new()));
        }
        Ok(headers)
    }
}

pub fn qoder_encode(plaintext: &[u8]) -> String {
    let standard = STANDARD.encode(plaintext);
    let pivot = standard.len() / 3;
    let rearranged = format!(
        "{}{}{}",
        &standard[standard.len() - pivot..],
        &standard[pivot..standard.len() - pivot],
        &standard[..pivot]
    );
    rearranged
        .bytes()
        .map(|byte| match byte {
            b'=' => '$',
            byte => STANDARD_ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .map(|index| CUSTOM_ALPHABET[index] as char)
                .unwrap_or(byte as char),
        })
        .collect()
}

pub fn qoder_decode(encoded: &str) -> Result<Vec<u8>, String> {
    let mapped = encoded
        .bytes()
        .map(|byte| match byte {
            b'$' => '=',
            byte => CUSTOM_ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .map(|index| STANDARD_ALPHABET[index] as char)
                .unwrap_or(byte as char),
        })
        .collect::<String>();
    let pivot = mapped.len() / 3;
    let standard = format!(
        "{}{}{}",
        &mapped[mapped.len() - pivot..],
        &mapped[pivot..mapped.len() - pivot],
        &mapped[..pivot]
    );
    let without_padding = standard.replace('=', "");
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(without_padding)
        .map_err(|error| format!("decode Qoder body: {error}"))
}

pub fn sign_qoder_request(
    payload_b64: &str,
    cosy_key: &str,
    cosy_date: &str,
    body: &str,
    signature_path: &str,
) -> String {
    md5_hex(format!("{payload_b64}\n{cosy_key}\n{cosy_date}\n{body}\n{signature_path}").as_bytes())
}

pub fn compose_bearer(payload_b64: &str, signature: &str) -> String {
    format!("Bearer COSY.{payload_b64}.{signature}")
}

pub fn md5_hex(value: &[u8]) -> String {
    format!("{:x}", md5::compute(value))
}

fn aes_encrypt(plaintext: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    Ok(cbc::Encryptor::<Aes128>::new_from_slices(key, key)
        .map_err(|error| format!("initialize Qoder AES: {error}"))?
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext))
}

#[cfg(test)]
fn aes_decrypt(ciphertext: &[u8], key: &[u8]) -> Result<Vec<u8>, String> {
    cbc::Decryptor::<Aes128>::new_from_slices(key, key)
        .map_err(|error| format!("initialize Qoder AES: {error}"))?
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|error| format!("decrypt Qoder AES: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qoder_sites_are_strict_and_keep_independent_protocol_profiles() {
        assert_eq!(QoderSite::parse(""), Ok(QoderSite::Global));
        assert_eq!(QoderSite::parse("CN"), Ok(QoderSite::Cn));
        assert!(QoderSite::parse("enterprise").is_err());
        let global = QoderSite::Global.profile();
        let cn = QoderSite::Cn.profile();
        assert_ne!(global.gateway_base_url, cn.gateway_base_url);
        assert_ne!(global.oauth_client_id, cn.oauth_client_id);
        assert_eq!(global.client_type, "5");
        assert_eq!(cn.client_type, "0");
    }

    #[test]
    fn qoder_account_profile_freezes_site_identity_and_machine() {
        let profile = QoderAccountProfile::parse(Some(&json!({
            "site": "cn",
            "refreshMode": "qodercn20",
            "uid": "user-a",
            "aid": "account-a",
            "organizationId": "org-a",
            "machineId": "machine-a",
            "machineType": "type-a"
        })))
        .unwrap();
        assert_eq!(profile.site, QoderSite::Cn);
        assert_eq!(profile.stable_identity_components()[2], "user-a");
        assert_eq!(
            profile.machine("machine-token").machine_token,
            "machine-token"
        );
        assert!(QoderAccountProfile::parse(Some(&json!({
            "site": "cn",
            "refreshMode": "cosy",
            "uid": "user-a",
            "machineId": "machine-a"
        })))
        .is_err());
        assert_eq!(
            machine_token_from_raw(Some(&json!({
                "qoderSecrets": {"machineToken": " secret-machine-token "}
            })))
            .as_deref(),
            Some("secret-machine-token")
        );
    }

    #[test]
    fn qoder_waf_encoding_round_trips_reference_vectors() {
        for body in [b"".as_slice(), b"hello", br#"{"model":"qmodel"}"#] {
            let encoded = qoder_encode(body);
            assert_eq!(qoder_decode(&encoded).unwrap(), body);
            assert!(encoded.is_ascii());
        }
        assert_eq!(qoder_encode(b"hello"), "q$FruHPH");
    }

    #[test]
    fn qoder_aes_and_signature_match_fixed_contract() {
        let key = b"0123456789abcdef";
        let plaintext = br#"{"uid":"user-a"}"#;
        let encrypted = aes_encrypt(plaintext, key).unwrap();
        assert_eq!(aes_decrypt(&encrypted, key).unwrap(), plaintext);
        assert_eq!(
            sign_qoder_request("payload", "cosy-key", "1700000000", "encoded", "/api/chat"),
            "8062969eb6d0ae858a16b978a79485d2"
        );
        assert_eq!(
            compose_bearer("payload", "signature"),
            "Bearer COSY.payload.signature"
        );
    }

    #[test]
    fn signed_headers_bind_exact_encoded_body_site_and_identity() {
        let session = QoderCosySession::new_with_key(
            QoderSite::Global,
            QoderIdentity {
                name: "User".to_string(),
                aid: String::new(),
                uid: "uid-a".to_string(),
                organization_id: "org-a".to_string(),
                organization_name: String::new(),
                user_type: "personal_standard".to_string(),
                security_oauth_token: "secret-token".to_string(),
                refresh_token: String::new(),
            },
            QoderMachineIdentity {
                machine_id: "machine-a".to_string(),
                machine_token: "machine-token-a".to_string(),
                machine_type: "5".to_string(),
            },
            b"0123456789abcdef",
        )
        .unwrap();
        let body = qoder_encode(br#"{"messages":[]}"#);
        let headers = session
            .signed_headers(
                body.as_bytes(),
                QODER_GENERATION_SIGNATURE_PATH,
                1_700_000_000,
                "request-a",
                "x86_64_linux",
                "127.0.0.1",
            )
            .unwrap();
        let value = |name: &str| {
            headers
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.as_str())
                .unwrap()
        };
        assert!(value("authorization").starts_with("Bearer COSY."));
        assert_eq!(value("cosy-user"), "uid-a");
        assert_eq!(value("cosy-machineid"), "machine-a");
        assert_eq!(value("cosy-bodyhash"), md5_hex(body.as_bytes()));
        assert_eq!(value("cosy-bodylength"), body.len().to_string());
        assert_eq!(value("cosy-sigpath"), QODER_GENERATION_SIGNATURE_PATH);
        assert!(!headers.iter().any(|(_, value)| value == "secret-token"));
    }
}
