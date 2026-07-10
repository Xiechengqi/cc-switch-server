use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use super::types::AccountQuotaResponse;
use crate::domain::accounts::store::{Account, AccountQuota, AccountQuotaTier};
use crate::domain::providers::model::ProviderType;

/// Convert stored account quota (0–1 utilization fractions) into the desktop
/// `SubscriptionQuota` shape expected by provider card footers (0–100).
#[allow(dead_code)]
pub(in crate::api) fn subscription_quota_from_account(account: &Account, tool: &str) -> Value {
    let credential_status = account_credential_status(account);
    let Some(quota) = account.quota.as_ref() else {
        return subscription_quota_not_found(tool);
    };
    subscription_quota_from_parts(
        tool,
        credential_status,
        quota,
        account.quota_refreshed_at,
        account.last_refresh_error.as_deref(),
        account.subscription_level.as_deref(),
    )
}

pub(in crate::api) fn subscription_quota_from_response(
    account: &Account,
    response: &AccountQuotaResponse,
    tool: &str,
) -> Value {
    let credential_status = account_credential_status(account);
    let quota = response.quota.as_ref().or(account.quota.as_ref());
    let Some(quota) = quota else {
        return subscription_quota_not_found(tool);
    };
    subscription_quota_from_parts(
        tool,
        credential_status,
        quota,
        account.quota_refreshed_at,
        account.last_refresh_error.as_deref(),
        account.subscription_level.as_deref(),
    )
}

pub(in crate::api) fn cached_oauth_quota_from_response(
    auth_provider: &str,
    account: &Account,
    response: &AccountQuotaResponse,
    provider_id: Option<&str>,
    app_type: Option<&str>,
    source: &str,
) -> Value {
    let tool = auth_provider;
    let quota = subscription_quota_from_response(account, response, tool);
    json!({
        "authProvider": auth_provider,
        "accountId": account.id,
        "providerId": provider_id,
        "providerName": Value::Null,
        "appType": app_type,
        "quota": quota,
        "refreshedAt": account
            .quota_refreshed_at
            .filter(|value| *value > 0),
        "nextRefreshAt": response.next_refresh_at,
        "source": source,
    })
}

pub(in crate::api) fn subscription_tool_provider_type(tool: &str) -> Option<ProviderType> {
    match tool.trim().to_ascii_lowercase().as_str() {
        "claude" => Some(ProviderType::ClaudeOAuth),
        "codex" => Some(ProviderType::CodexOAuth),
        "gemini" => Some(ProviderType::GeminiCli),
        _ => None,
    }
}

#[allow(dead_code)]
pub(in crate::api) fn subscription_tool_label(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::ClaudeOAuth => "claude",
        ProviderType::CodexOAuth => "codex",
        ProviderType::GeminiCli => "gemini",
        ProviderType::GitHubCopilot => "github_copilot",
        ProviderType::AntigravityOAuth => "antigravity_oauth",
        ProviderType::CursorOAuth => "cursor_oauth",
        ProviderType::KiroOAuth => "kiro_oauth",
        _ => "unknown",
    }
}

pub(in crate::api) fn subscription_quota_not_found(tool: &str) -> Value {
    json!({
        "tool": tool,
        "credentialStatus": "not_found",
        "credentialMessage": Value::Null,
        "subscription": Value::Null,
        "success": false,
        "tiers": [],
        "extraUsage": Value::Null,
        "error": Value::Null,
        "queriedAt": Value::Null,
    })
}

fn subscription_quota_from_parts(
    tool: &str,
    credential_status: &str,
    quota: &AccountQuota,
    queried_at: Option<i64>,
    last_refresh_error: Option<&str>,
    subscription_level: Option<&str>,
) -> Value {
    if credential_status == "not_found" {
        return subscription_quota_not_found(tool);
    }

    let queried_at = quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("queriedAt").and_then(Value::as_i64))
        .or(queried_at)
        .filter(|value| *value > 0);

    let subscription = quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("subscription"))
        .cloned()
        .filter(|value| !value.is_null());

    let credential_message = quota
        .credential_message
        .clone()
        .or_else(|| subscription_level.map(str::to_string));

    let success = quota.success && credential_status == "valid";
    let queried_at = queried_at.or_else(|| {
        if success || !quota.tiers.is_empty() {
            Some(crate::infra::time::now_ms() as i64)
        } else {
            None
        }
    });
    let error = if success {
        Value::Null
    } else {
        last_refresh_error
            .map(|message| Value::String(message.to_string()))
            .or_else(|| {
                quota
                    .credential_message
                    .as_ref()
                    .map(|message| Value::String(message.clone()))
            })
            .unwrap_or(Value::Null)
    };

    json!({
        "tool": tool,
        "credentialStatus": credential_status,
        "credentialMessage": credential_message,
        "subscription": subscription,
        "success": success,
        "tiers": quota
            .tiers
            .iter()
            .map(subscription_tier_from_account_tier)
            .collect::<Vec<_>>(),
        "extraUsage": extra_usage_for_ui(quota.extra_usage.as_ref()),
        "error": error,
        "queriedAt": queried_at,
    })
}

fn subscription_tier_from_account_tier(tier: &AccountQuotaTier) -> Value {
    json!({
        "name": tier.name,
        "utilization": utilization_for_ui(tier.utilization),
        "resetsAt": resets_at_for_ui(tier.resets_at),
        "used": tier.used,
        "limit": tier.limit,
        "unit": tier.unit,
    })
}

fn extra_usage_for_ui(extra_usage: Option<&Value>) -> Value {
    let Some(extra_usage) = extra_usage else {
        return Value::Null;
    };
    if let Some(extra) = extra_usage
        .get("extraUsage")
        .or_else(|| extra_usage.get("extra_usage"))
    {
        return extra.clone();
    }
    Value::Null
}

fn account_credential_status(account: &Account) -> &'static str {
    if !account_has_credentials(account) {
        return "not_found";
    }
    let now_ms = crate::infra::time::now_ms() as i64;
    if account
        .expires_at
        .is_some_and(|expires_at| expires_at > 0 && expires_at <= now_ms)
    {
        return "expired";
    }
    "valid"
}

fn account_has_credentials(account: &Account) -> bool {
    account
        .access_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || account
            .refresh_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

pub(in crate::api) fn utilization_for_ui(value: Option<f64>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if !value.is_finite() {
        return 0.0;
    }
    if value <= 1.0 {
        (value * 100.0).clamp(0.0, 100.0)
    } else {
        value.clamp(0.0, 100.0)
    }
}

pub(in crate::api) fn resets_at_for_ui(ms: Option<i64>) -> Value {
    match ms {
        Some(ms) if ms > 0 => Utc
            .timestamp_millis_opt(ms)
            .single()
            .map(|value| Value::String(value.to_rfc3339()))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::ProviderType;

    fn sample_account(quota: AccountQuota) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type: ProviderType::CodexOAuth,
            email: Some("user@example.com".to_string()),
            access_token: Some("token".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: None,
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: Some(quota),
            quota_refreshed_at: Some(1_700_000_000_000),
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        }
    }

    #[test]
    fn utilization_for_ui_scales_fractions_to_percent() {
        assert_eq!(utilization_for_ui(Some(0.42)), 42.0);
        assert_eq!(utilization_for_ui(Some(42.0)), 42.0);
        assert_eq!(utilization_for_ui(Some(1.0)), 100.0);
    }

    #[test]
    fn subscription_quota_maps_codex_tiers_for_provider_cards() {
        let account = sample_account(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Plus".to_string()),
            tiers: vec![
                AccountQuotaTier {
                    name: "five_hour".to_string(),
                    utilization: Some(0.42),
                    resets_at: Some(1_700_000_000_000),
                    ..Default::default()
                },
                AccountQuotaTier {
                    name: "seven_day".to_string(),
                    utilization: Some(0.18),
                    resets_at: Some(1_700_086_400_000),
                    ..Default::default()
                },
            ],
            extra_usage: Some(json!({
                "queriedAt": 1_700_000_000_000i64,
                "subscription": {
                    "planType": "plus",
                    "planLabel": "ChatGPT Plus",
                    "expiresAt": "2026-08-01T00:00:00Z"
                }
            })),
        });

        let quota = subscription_quota_from_account(&account, "codex_oauth");
        assert_eq!(quota["success"], true);
        assert_eq!(quota["credentialStatus"], "valid");
        assert_eq!(quota["tiers"][0]["utilization"], 42.0);
        assert_eq!(quota["tiers"][1]["utilization"], 18.0);
        assert!(quota["tiers"][0]["resetsAt"].is_string());
        assert_eq!(
            quota["subscription"]["planLabel"],
            Value::String("ChatGPT Plus".to_string())
        );
    }

    #[test]
    fn cached_oauth_quota_wraps_subscription_quota_shape() {
        let account = sample_account(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Plus".to_string()),
            tiers: vec![AccountQuotaTier {
                name: "five_hour".to_string(),
                utilization: Some(0.25),
                ..Default::default()
            }],
            extra_usage: None,
        });
        let response = AccountQuotaResponse {
            ok: true,
            quota: account.quota.clone(),
            account: Some(account.clone()),
            refreshed: false,
            message: None,
            next_refresh_at: Some(1_700_000_300_000),
        };

        let cached = cached_oauth_quota_from_response(
            "codex_oauth",
            &account,
            &response,
            None,
            Some("codex"),
            "server",
        );
        assert_eq!(cached["authProvider"], "codex_oauth");
        assert_eq!(cached["quota"]["tiers"][0]["utilization"], 25.0);
        assert_eq!(cached["nextRefreshAt"], json!(1_700_000_300_000i64));
    }
}
