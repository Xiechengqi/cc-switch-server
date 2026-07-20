use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::accounts::store::Account;
use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionExpiryCapability {
    Automatic,
    ManualRequired,
    ResearchPending,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionExpirySource {
    Automatic,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSubscriptionExpiry {
    pub capability: SubscriptionExpiryCapability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SubscriptionExpirySource>,
}

pub fn subscription_expiry_capability(provider_type: ProviderType) -> SubscriptionExpiryCapability {
    match provider_type {
        ProviderType::ClaudeOAuth => SubscriptionExpiryCapability::ManualRequired,
        ProviderType::CodexOAuth | ProviderType::OllamaCloud => {
            SubscriptionExpiryCapability::Automatic
        }
        ProviderType::GeminiCli
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth
        | ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth
        | ProviderType::GrokOAuth => SubscriptionExpiryCapability::ResearchPending,
        ProviderType::Claude
        | ProviderType::ClaudeAuth
        | ProviderType::Codex
        | ProviderType::Gemini
        | ProviderType::OpenRouter
        | ProviderType::DeepSeekAccount
        | ProviderType::CursorApiKey
        | ProviderType::AwsBedrock
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => SubscriptionExpiryCapability::NotApplicable,
    }
}

pub fn automatic_subscription_expires_at_ms(account: &Account) -> Option<i64> {
    if subscription_expiry_capability(account.provider_type)
        != SubscriptionExpiryCapability::Automatic
    {
        return None;
    }

    let extra_usage = account.quota.as_ref()?.extra_usage.as_ref()?;
    automatic_expiry_paths(account.provider_type)
        .iter()
        .find_map(|path| timestamp_at(extra_usage, path))
}

pub fn resolved_subscription_expiry(account: &Account) -> ResolvedSubscriptionExpiry {
    let capability = subscription_expiry_capability(account.provider_type);
    let (expires_at_ms, source) = match capability {
        SubscriptionExpiryCapability::Automatic => (
            automatic_subscription_expires_at_ms(account),
            Some(SubscriptionExpirySource::Automatic),
        ),
        SubscriptionExpiryCapability::ManualRequired => (
            account.manual_subscription_expires_at_ms,
            Some(SubscriptionExpirySource::Manual),
        ),
        SubscriptionExpiryCapability::ResearchPending
        | SubscriptionExpiryCapability::NotApplicable => (None, None),
    };

    ResolvedSubscriptionExpiry {
        capability,
        expires_at_ms,
        source: expires_at_ms.and(source),
    }
}

fn automatic_expiry_paths(provider_type: ProviderType) -> &'static [&'static str] {
    match provider_type {
        ProviderType::CodexOAuth => &[
            "/subscription/expiresAt",
            "/subscription/expires_at",
            "/subscriptionExpiresAt",
        ],
        ProviderType::OllamaCloud => &[
            "/subscriptionPeriodEnd",
            "/raw/SubscriptionPeriodEnd/Time",
            "/raw/subscriptionPeriodEnd/time",
        ],
        _ => &[],
    }
}

fn timestamp_at(value: &Value, pointer: &str) -> Option<i64> {
    match value.pointer(pointer)? {
        Value::String(value) => parse_timestamp(value),
        Value::Number(value) => value.as_i64().and_then(normalize_unix_timestamp),
        _ => None,
    }
}

fn parse_timestamp(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
        .or_else(|| value.parse::<i64>().ok().and_then(normalize_unix_timestamp))
}

fn normalize_unix_timestamp(value: i64) -> Option<i64> {
    if value <= 0 {
        return None;
    }
    if value < 100_000_000_000 {
        value.checked_mul(1_000)
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{AccountQuota, AccountStore, UpsertAccountInput};

    const ALL_PROVIDER_TYPES: [ProviderType; 20] = [
        ProviderType::Claude,
        ProviderType::ClaudeAuth,
        ProviderType::ClaudeOAuth,
        ProviderType::Codex,
        ProviderType::CodexOAuth,
        ProviderType::Gemini,
        ProviderType::GeminiCli,
        ProviderType::OpenRouter,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::CursorOAuth,
        ProviderType::CursorApiKey,
        ProviderType::AntigravityOAuth,
        ProviderType::AgyOAuth,
        ProviderType::OllamaCloud,
        ProviderType::AwsBedrock,
        ProviderType::Nvidia,
        ProviderType::DeepSeekApi,
        ProviderType::GrokOAuth,
    ];

    fn account(provider_type: ProviderType, extra_usage: Option<Value>) -> Account {
        let mut store = AccountStore::default();
        store.upsert(UpsertAccountInput {
            id: Some(format!("account-{}", provider_type.as_str())),
            provider_type,
            email: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: extra_usage.map(|extra_usage| AccountQuota {
                success: true,
                credential_message: None,
                tiers: Vec::new(),
                extra_usage: Some(extra_usage),
            }),
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        })
    }

    #[test]
    fn capability_matrix_covers_every_provider_type() {
        assert_eq!(ALL_PROVIDER_TYPES.len(), 20);
        for provider_type in ALL_PROVIDER_TYPES {
            let expected = match provider_type {
                ProviderType::ClaudeOAuth => SubscriptionExpiryCapability::ManualRequired,
                ProviderType::CodexOAuth | ProviderType::OllamaCloud => {
                    SubscriptionExpiryCapability::Automatic
                }
                ProviderType::GeminiCli
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::GitHubCopilot
                | ProviderType::KiroOAuth
                | ProviderType::CursorOAuth
                | ProviderType::GrokOAuth => SubscriptionExpiryCapability::ResearchPending,
                _ => SubscriptionExpiryCapability::NotApplicable,
            };
            assert_eq!(subscription_expiry_capability(provider_type), expected);
        }
    }

    #[test]
    fn automatic_resolver_reads_only_trusted_codex_and_ollama_paths() {
        let codex = account(
            ProviderType::CodexOAuth,
            Some(json!({
                "subscription": {"expiresAt": "2026-08-17T00:00:00Z"},
                "tokenExpiresAt": "2030-01-01T00:00:00Z"
            })),
        );
        assert_eq!(
            automatic_subscription_expires_at_ms(&codex),
            Some(1_786_924_800_000)
        );

        let ollama = account(
            ProviderType::OllamaCloud,
            Some(json!({
                "raw": {"SubscriptionPeriodEnd": {"Time": "2026-09-01T00:00:00Z"}}
            })),
        );
        assert_eq!(
            automatic_subscription_expires_at_ms(&ollama),
            Some(1_788_220_800_000)
        );
    }

    #[test]
    fn manual_and_research_capabilities_do_not_trust_quota_or_token_expiry() {
        let mut claude = account(
            ProviderType::ClaudeOAuth,
            Some(json!({"subscription": {"expiresAt": "2030-01-01T00:00:00Z"}})),
        );
        claude.expires_at = Some(1_999_999_999_999);
        claude.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        assert_eq!(
            resolved_subscription_expiry(&claude),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::ManualRequired,
                expires_at_ms: Some(1_786_924_800_000),
                source: Some(SubscriptionExpirySource::Manual),
            }
        );

        let mut cursor = account(
            ProviderType::CursorOAuth,
            Some(json!({
                "subscriptionPeriodEnd": "2030-01-01T00:00:00Z",
                "resetAt": "2030-02-01T00:00:00Z"
            })),
        );
        cursor.expires_at = Some(1_999_999_999_999);
        cursor.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        assert_eq!(
            resolved_subscription_expiry(&cursor),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::ResearchPending,
                expires_at_ms: None,
                source: None,
            }
        );
    }

    #[test]
    fn automatic_capability_does_not_fall_back_to_manual_value() {
        let mut codex = account(ProviderType::CodexOAuth, None);
        codex.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        codex.expires_at = Some(1_999_999_999_999);
        assert_eq!(
            resolved_subscription_expiry(&codex),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::Automatic,
                expires_at_ms: None,
                source: None,
            }
        );
    }

    #[test]
    fn codex_reconciled_snapshot_without_expiry_clears_share_expiry() {
        let codex = account(
            ProviderType::CodexOAuth,
            Some(json!({
                "subscription": {
                    "planType": "plus",
                    "planLabel": "ChatGPT Plus",
                    "expiresAt": null,
                    "expiresSource": null,
                    "expiresKind": null
                },
                "subscriptionEvidence": {
                    "usagePlanType": "plus",
                    "usageAllowed": true,
                    "discardedReasons": ["accounts_check_plan_mismatch"]
                }
            })),
        );

        assert_eq!(
            resolved_subscription_expiry(&codex),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::Automatic,
                expires_at_ms: None,
                source: None,
            }
        );
    }
}
