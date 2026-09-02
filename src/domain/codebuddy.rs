use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::Account;

pub const CODEBUDDY_CLIENT_VERSION: &str = "2.142.0";
pub const CODEBUDDY_PLATFORM: &str = "CLI";
pub const CODEBUDDY_AUTH_STATE_PATH: &str = "/v2/plugin/auth/state?platform=CLI";
pub const CODEBUDDY_AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token";
pub const CODEBUDDY_LOGIN_ACCOUNT_PATH: &str = "/v2/plugin/login/account";
pub const CODEBUDDY_ACCOUNTS_PATH: &str = "/v2/plugin/accounts";
pub const CODEBUDDY_REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";
pub const CODEBUDDY_CHAT_PATH: &str = "/v2/chat/completions";
pub const CODEBUDDY_CONFIG_PATH: &str = "/v3/config";
pub const CODEBUDDY_RESOURCE_PATH: &str = "/v2/billing/meter/get-user-resource";
pub const CODEBUDDY_USAGE_PATH: &str = "/billing/meter/get-user-request-usage";
pub const CODEBUDDY_SESSION_REFRESH_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;
const CODEBUDDY_SESSION_REFRESH_JITTER_MS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeBuddySite {
    Intl,
    Cn,
}

impl CodeBuddySite {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "intl" | "global" | "international" => Ok(Self::Intl),
            "cn" | "china" | "internal" => Ok(Self::Cn),
            "ioa" | "cloudhosted" | "selfhosted" => Err(format!(
                "CodeBuddy site {value:?} is an enterprise deployment and is not supported"
            )),
            value => Err(format!("unsupported CodeBuddy site {value:?}")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intl => "intl",
            Self::Cn => "cn",
        }
    }

    pub const fn profile(self) -> CodeBuddySiteProfile {
        match self {
            Self::Intl => CodeBuddySiteProfile {
                site: self,
                endpoint: "https://www.codebuddy.ai",
                billing_endpoint: "https://www.codebuddy.ai",
                domain: "www.codebuddy.ai",
                browser_origin: "https://www.codebuddy.ai",
            },
            Self::Cn => CodeBuddySiteProfile {
                site: self,
                endpoint: "https://copilot.tencent.com",
                billing_endpoint: "https://www.codebuddy.cn",
                domain: "copilot.tencent.com",
                browser_origin: "https://www.codebuddy.cn",
            },
        }
    }

    /// Browser URLs are returned by the authenticated API and opened by an
    /// administrator. Keep that redirect surface on the reviewed public brand
    /// hosts; identity-provider redirects happen after the browser opens the
    /// trusted landing page and do not need to be accepted here.
    pub fn allows_browser_auth_host(self, host: &str) -> bool {
        let host = normalize_host(host);
        match self {
            Self::Intl => matches!(
                host.as_str(),
                "codebuddy.ai" | "www.codebuddy.ai" | "workbuddy.ai" | "www.workbuddy.ai"
            ),
            Self::Cn => matches!(
                host.as_str(),
                "copilot.tencent.com"
                    | "codebuddy.cn"
                    | "www.codebuddy.cn"
                    | "workbuddy.cn"
                    | "www.workbuddy.cn"
            ),
        }
    }

    /// Token responses carry a domain identity. It may use either reviewed
    /// brand for a site, but it must never cross the Intl/CN boundary.
    pub fn allows_token_domain(self, domain: &str) -> bool {
        self.canonical_token_domain(domain).is_some()
    }

    pub fn canonical_token_domain(self, domain: &str) -> Option<String> {
        domain_host(domain).filter(|host| self.allows_browser_auth_host(host))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeBuddySiteProfile {
    pub site: CodeBuddySite,
    pub endpoint: &'static str,
    pub billing_endpoint: &'static str,
    pub domain: &'static str,
    pub browser_origin: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBuddyAccountProfile {
    pub site: CodeBuddySite,
    #[serde(default)]
    pub domain: String,
    pub uid: String,
    #[serde(default)]
    pub enterprise_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub account_type: String,
    #[serde(default = "default_client_version")]
    pub client_version: String,
    #[serde(default = "default_platform")]
    pub product_platform: String,
}

fn default_client_version() -> String {
    CODEBUDDY_CLIENT_VERSION.to_string()
}

fn default_platform() -> String {
    CODEBUDDY_PLATFORM.to_string()
}

impl CodeBuddyAccountProfile {
    pub fn parse(value: Option<&Value>) -> Result<Self, String> {
        let value = value
            .and_then(Value::as_object)
            .ok_or_else(|| "CodeBuddy account profile must be an object".to_string())?;
        let site = CodeBuddySite::parse(
            value
                .get("site")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )?;
        let domain = profile_string(value, &["domain"]);
        if domain.is_empty() {
            return Err(
                "CodeBuddy account profile is missing its bound domain; re-login is required"
                    .to_string(),
            );
        }
        let profile = Self {
            site,
            domain: site.canonical_token_domain(&domain).ok_or_else(|| {
                format!(
                    "CodeBuddy account domain is outside the bound {} site",
                    site.as_str()
                )
            })?,
            uid: profile_string(value, &["uid", "userId", "user_id", "sub"]),
            enterprise_id: profile_string(
                value,
                &["enterpriseId", "enterprise_id", "enterpriseID"],
            ),
            name: profile_string(value, &["name", "userName", "user_name"]),
            email: profile_string(value, &["email"]),
            nickname: profile_string(value, &["nickname", "nickName"]),
            account_type: profile_string(value, &["accountType", "account_type", "type"]),
            client_version: profile_string(value, &["clientVersion", "client_version"]),
            product_platform: profile_string(
                value,
                &["productPlatform", "product_platform", "platform"],
            ),
        };
        let profile = Self {
            client_version: if profile.client_version.is_empty() {
                default_client_version()
            } else {
                profile.client_version
            },
            product_platform: if profile.product_platform.is_empty() {
                default_platform()
            } else {
                profile.product_platform
            },
            ..profile
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.uid.trim().is_empty() {
            return Err("CodeBuddy account profile is missing uid".to_string());
        }
        if self.site.canonical_token_domain(&self.domain).as_deref() != Some(self.domain.as_str()) {
            return Err(format!(
                "CodeBuddy account domain is outside the bound {} site",
                self.site.as_str()
            ));
        }
        if self.client_version.trim() != CODEBUDDY_CLIENT_VERSION {
            return Err(format!(
                "CodeBuddy clientVersion must be the reviewed {CODEBUDDY_CLIENT_VERSION} identity"
            ));
        }
        if self.product_platform.trim() != CODEBUDDY_PLATFORM {
            return Err("CodeBuddy productPlatform must be CLI".to_string());
        }
        Ok(())
    }

    pub fn stable_identity_components(&self) -> [&str; 4] {
        [
            self.site.as_str(),
            self.domain.trim(),
            self.uid.trim(),
            self.enterprise_id.trim(),
        ]
    }
}

pub fn codebuddy_account_id(
    site: CodeBuddySite,
    domain: &str,
    uid: &str,
    enterprise_id: &str,
) -> Result<String, String> {
    let domain = site.canonical_token_domain(domain).ok_or_else(|| {
        format!(
            "CodeBuddy account id domain is outside the bound {} site",
            site.as_str()
        )
    })?;
    let uid = uid.trim();
    if uid.is_empty() {
        return Err("CodeBuddy account id requires uid".to_string());
    }
    let mut digest = Sha256::new();
    digest.update(b"cc-switch/codebuddy/account-id/v1");
    digest.update([0]);
    digest.update(site.as_str().as_bytes());
    digest.update([0]);
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(uid.as_bytes());
    digest.update([0]);
    digest.update(enterprise_id.trim().as_bytes());
    let encoded = hex::encode(digest.finalize());
    Ok(format!("codebuddy-{}", &encoded[..12]))
}

pub fn codebuddy_access_token_subject(access_token: &str) -> Option<String> {
    let payload = access_token.trim().split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// CodeBuddy access tokens currently live for roughly one year while the
/// refresh session can be reclaimed after only a few idle days. A bounded,
/// deterministic per-account jitter prevents all locally configured accounts
/// from refreshing on the same scan without introducing account selection.
pub fn codebuddy_session_refresh_due(account: &Account, now_ms: i64) -> bool {
    if account.provider_type != crate::domain::providers::model::ProviderType::CodeBuddyOAuth
        || account.needs_relogin
        || account
            .refresh_token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty())
    {
        return false;
    }
    let last_refresh_ms = account.raw.as_ref().and_then(|raw| {
        raw.pointer("/codeBuddyRefreshReceipt/receivedAtMs")
            .and_then(json_i64)
            .or_else(|| raw.get("observedAtMs").and_then(json_i64))
    });
    let Some(last_refresh_ms) = last_refresh_ms else {
        // Imported/legacy credentials have no trustworthy session heartbeat.
        return true;
    };
    let mut digest = Sha256::new();
    digest.update(b"cc-switch/codebuddy/session-refresh-jitter/v1\0");
    digest.update(account.id.as_bytes());
    let bytes = digest.finalize();
    let bucket = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64;
    let span = CODEBUDDY_SESSION_REFRESH_JITTER_MS
        .saturating_mul(2)
        .saturating_add(1);
    let jitter_ms = bucket.rem_euclid(span) - CODEBUDDY_SESSION_REFRESH_JITTER_MS;
    now_ms
        >= last_refresh_ms
            .saturating_add(CODEBUDDY_SESSION_REFRESH_INTERVAL_MS)
            .saturating_add(jitter_ms)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn domain_host(domain: &str) -> Option<String> {
    let domain = domain.trim();
    if domain.is_empty() {
        return None;
    }
    if domain.contains("://") {
        return url::Url::parse(domain).ok()?.host_str().map(normalize_host);
    }
    let without_port = domain
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
        .map(|(host, _)| host)
        .or_else(|| domain.split_once(':').map(|(host, _)| host))
        .unwrap_or(domain);
    let host = normalize_host(without_port);
    (!host.is_empty()).then_some(host)
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
    fn account_identity_is_site_and_enterprise_scoped() {
        let intl =
            codebuddy_account_id(CodeBuddySite::Intl, "www.codebuddy.ai", "uid-1", "").unwrap();
        let intl_alias =
            codebuddy_account_id(CodeBuddySite::Intl, "www.workbuddy.ai", "uid-1", "").unwrap();
        let cn =
            codebuddy_account_id(CodeBuddySite::Cn, "copilot.tencent.com", "uid-1", "").unwrap();
        let enterprise =
            codebuddy_account_id(CodeBuddySite::Intl, "www.codebuddy.ai", "uid-1", "ent-1")
                .unwrap();
        assert_ne!(intl, cn);
        assert_ne!(intl, intl_alias);
        assert_ne!(intl, enterprise);
    }

    #[test]
    fn profile_rejects_unreviewed_sites_and_client_identity() {
        assert!(CodeBuddyAccountProfile::parse(Some(&json!({
            "site": "intl",
            "uid": "uid-1"
        })))
        .is_err());
        assert!(CodeBuddyAccountProfile::parse(Some(&json!({
            "site": "selfhosted",
            "domain": "www.codebuddy.ai",
            "uid": "uid-1"
        })))
        .is_err());
        assert!(CodeBuddyAccountProfile::parse(Some(&json!({
            "site": "intl",
            "domain": "www.codebuddy.ai",
            "uid": "uid-1",
            "clientVersion": "0.0.0"
        })))
        .is_err());
    }

    #[test]
    fn access_token_subject_is_extracted_without_treating_it_as_verified_jwt() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"uid-1"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            codebuddy_access_token_subject(&token).as_deref(),
            Some("uid-1")
        );
        assert!(codebuddy_access_token_subject("opaque-token").is_none());
    }

    #[test]
    fn site_host_allowlists_do_not_cross_regions_or_accept_suffix_confusion() {
        assert!(CodeBuddySite::Intl.allows_browser_auth_host("WWW.CodeBuddy.AI."));
        assert!(CodeBuddySite::Intl.allows_token_domain("https://www.workbuddy.ai/path"));
        assert!(!CodeBuddySite::Intl.allows_browser_auth_host("www.codebuddy.ai.attacker.test"));
        assert!(!CodeBuddySite::Intl.allows_token_domain("www.codebuddy.cn"));
        assert!(CodeBuddySite::Cn.allows_token_domain("copilot.tencent.com"));
        assert!(!CodeBuddySite::Cn.allows_token_domain("www.codebuddy.ai"));
        assert_eq!(
            CodeBuddySite::Intl
                .canonical_token_domain("https://WWW.CodeBuddy.AI/path")
                .as_deref(),
            Some("www.codebuddy.ai")
        );
        assert_eq!(
            CodeBuddySite::Intl.profile().billing_endpoint,
            "https://www.codebuddy.ai"
        );
        assert_eq!(
            CodeBuddySite::Cn.profile().billing_endpoint,
            "https://www.codebuddy.cn"
        );
        assert_ne!(
            CodeBuddySite::Cn.profile().billing_endpoint,
            CodeBuddySite::Cn.profile().endpoint
        );
    }

    #[test]
    fn session_refresh_is_due_independently_of_one_year_access_expiry() {
        let mut account: crate::domain::accounts::store::Account = serde_json::from_value(json!({
            "id": "codebuddy-session",
            "providerType": "codebuddy_oauth"
        }))
        .unwrap();
        account.refresh_token = Some("refresh-token".to_string());
        account.expires_at = Some(4_102_444_800_000);
        account.raw = Some(json!({"observedAtMs": 1_000_000}));
        assert!(!codebuddy_session_refresh_due(
            &account,
            1_000_000 + CODEBUDDY_SESSION_REFRESH_INTERVAL_MS
                - CODEBUDDY_SESSION_REFRESH_JITTER_MS
                - 1
        ));
        assert!(codebuddy_session_refresh_due(
            &account,
            1_000_000
                + CODEBUDDY_SESSION_REFRESH_INTERVAL_MS
                + CODEBUDDY_SESSION_REFRESH_JITTER_MS
                + 1
        ));
        account.raw = None;
        assert!(codebuddy_session_refresh_due(&account, 1_000_000));
        account.needs_relogin = true;
        assert!(!codebuddy_session_refresh_due(&account, i64::MAX));
    }
}
