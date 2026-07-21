use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use super::types::AccountQuotaResponse;
use crate::domain::accounts::store::{Account, AccountQuota, AccountQuotaTier};
use crate::domain::accounts::subscription_expiry::{
    resolved_subscription_expiry, SubscriptionExpirySource,
};
use crate::domain::providers::model::ProviderType;

/// Convert stored account quota (0–1 utilization fractions) into the desktop
/// `SubscriptionQuota` shape expected by provider card footers (0–100).
#[allow(dead_code)]
pub(in crate::api) fn subscription_quota_from_account(account: &Account, tool: &str) -> Value {
    let credential_status = account_credential_status(account);
    if account.quota.is_none()
        && resolved_subscription_expiry(account)
            .expires_at_ms
            .is_none()
    {
        return subscription_quota_not_found(tool);
    }
    subscription_quota_from_parts(account, tool, credential_status, account.quota.as_ref())
}

pub(in crate::api) fn subscription_quota_from_response(
    account: &Account,
    response: &AccountQuotaResponse,
    tool: &str,
) -> Value {
    let credential_status = account_credential_status(account);
    let quota = response.quota.as_ref().or(account.quota.as_ref());
    if quota.is_none()
        && resolved_subscription_expiry(account)
            .expires_at_ms
            .is_none()
    {
        return subscription_quota_not_found(tool);
    }
    subscription_quota_from_parts(account, tool, credential_status, quota)
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
        ProviderType::GrokOAuth => "grok_oauth",
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
        "quotaStatus": Value::Null,
        "warningCodes": [],
        "warnings": [],
        "staleTierNames": [],
        "tiers": [],
        "extraUsage": Value::Null,
        "error": Value::Null,
        "queriedAt": Value::Null,
    })
}

fn subscription_quota_from_parts(
    account: &Account,
    tool: &str,
    credential_status: &str,
    quota: Option<&AccountQuota>,
) -> Value {
    if credential_status == "not_found" {
        return subscription_quota_not_found(tool);
    }

    let queried_at = quota
        .and_then(|quota| quota.extra_usage.as_ref())
        .as_ref()
        .and_then(|extra| extra.get("queriedAt").and_then(Value::as_i64))
        .or(account.quota_refreshed_at)
        .filter(|value| *value > 0);

    let subscription = subscription_for_ui(account, quota);
    let quota_metadata = quota_metadata_for_ui(quota.and_then(|quota| quota.extra_usage.as_ref()));

    let credential_message = quota
        .and_then(|quota| quota.credential_message.clone())
        .or_else(|| account.subscription_level.clone());

    let success = credential_status == "valid" && quota.is_some_and(|quota| quota.success);
    let queried_at = queried_at.or_else(|| {
        if success || quota.is_some_and(|quota| !quota.tiers.is_empty()) {
            Some(crate::infra::time::now_ms() as i64)
        } else {
            None
        }
    });
    let error = if success {
        Value::Null
    } else {
        account
            .last_refresh_error
            .as_deref()
            .map(|message| Value::String(message.to_string()))
            .or_else(|| {
                quota
                    .and_then(|quota| quota.credential_message.as_ref())
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
        "quotaStatus": quota_metadata.get("quotaStatus").cloned().unwrap_or(Value::Null),
        "warningCodes": quota_metadata.get("warningCodes").cloned().unwrap_or_else(|| json!([])),
        "warnings": quota_metadata.get("warnings").cloned().unwrap_or_else(|| json!([])),
        "staleTierNames": quota_metadata.get("staleTierNames").cloned().unwrap_or_else(|| json!([])),
        "tiers": quota
            .map(|quota| quota.tiers.as_slice())
            .unwrap_or_default()
            .iter()
            .map(subscription_tier_from_account_tier)
            .collect::<Vec<_>>(),
        "extraUsage": extra_usage_for_ui(quota.and_then(|quota| quota.extra_usage.as_ref())),
        "error": error,
        "queriedAt": queried_at,
    })
}

fn subscription_for_ui(account: &Account, quota: Option<&AccountQuota>) -> Option<Value> {
    let existing = quota
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(|extra| extra.get("subscription"))
        .cloned()
        .filter(|value| !value.is_null());
    let resolved = resolved_subscription_expiry(account);
    let Some(expires_at_ms) = resolved.expires_at_ms else {
        return existing;
    };
    let expires_at = Utc
        .timestamp_millis_opt(expires_at_ms)
        .single()
        .map(|value| value.to_rfc3339())?;
    let mut subscription = existing
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = subscription
        .as_object_mut()
        .expect("subscription was normalized to an object");
    object.insert("expiresAt".to_string(), Value::String(expires_at));
    object.insert(
        "expiryCapability".to_string(),
        serde_json::to_value(resolved.capability).unwrap_or(Value::Null),
    );
    if matches!(
        resolved.source,
        Some(SubscriptionExpirySource::RecurringRule | SubscriptionExpirySource::LegacyManual)
    ) {
        object.insert(
            "expiresSource".to_string(),
            Value::String(
                match resolved.source {
                    Some(SubscriptionExpirySource::RecurringRule) => "recurring_rule",
                    _ => "manual",
                }
                .to_string(),
            ),
        );
        object.insert(
            "expiresKind".to_string(),
            Value::String(
                match resolved.source {
                    Some(SubscriptionExpirySource::RecurringRule) => "recurring_billing_period",
                    _ => "billing_period",
                }
                .to_string(),
            ),
        );
        object.insert(
            "expiryAvailability".to_string(),
            Value::String("available".to_string()),
        );
    }
    if !object.contains_key("planLabel") {
        if let Some(plan_label) = account.subscription_level.as_ref() {
            object.insert("planLabel".to_string(), Value::String(plan_label.clone()));
        }
    }
    Some(subscription)
}

fn subscription_tier_from_account_tier(tier: &AccountQuotaTier) -> Value {
    json!({
        "name": tier.name,
        "label": tier.label,
        "utilization": utilization_for_ui(tier.utilization),
        "resetsAt": resets_at_for_ui(tier.resets_at),
        "used": tier.used,
        "limit": tier.limit,
        "unit": tier.unit,
    })
}

fn quota_metadata_for_ui(extra_usage: Option<&Value>) -> Value {
    let Some(extra_usage) = extra_usage else {
        return json!({});
    };
    let mut metadata = serde_json::Map::new();
    for key in ["quotaStatus", "warningCodes", "warnings", "staleTierNames"] {
        if let Some(value) = extra_usage.get(key) {
            metadata.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(metadata)
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
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("user@example.com".to_string()),
            access_token: Some("token".to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: Default::default(),
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
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
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

    #[test]
    fn grok_quota_exposes_plan_and_usage_without_using_token_expiry() {
        let mut account = sample_account(AccountQuota {
            success: true,
            credential_message: Some("SuperGrok".to_string()),
            tiers: vec![AccountQuotaTier {
                name: "grok_credits".to_string(),
                label: Some("Credits".to_string()),
                utilization: Some(0.75),
                used: Some(75.0),
                limit: Some(100.0),
                unit: Some("credits".to_string()),
                resets_at: None,
            }],
            extra_usage: Some(json!({
                "queriedAt": 1_700_000_000_000i64,
                "quotaStatus": "valid_numeric",
                "warningCodes": [],
                "warnings": [],
                "staleTierNames": [],
                "subscription": {
                    "planType": "SuperGrok",
                    "planLabel": "SuperGrok",
                    "expiresAt": Value::Null,
                    "expiryCapability": "automatic",
                    "expiryAvailability": "upstream_not_provided"
                }
            })),
        });
        account.provider_type = ProviderType::GrokOAuth;
        account.subscription_level = Some("SuperGrok".to_string());
        account.expires_at = Some(1_786_924_800_000);

        let quota = subscription_quota_from_account(&account, "grok_oauth");

        assert_eq!(quota["credentialMessage"], "SuperGrok");
        assert_eq!(quota["tiers"][0]["utilization"], 75.0);
        assert_eq!(quota["tiers"][0]["used"], 75.0);
        assert_eq!(quota["tiers"][0]["label"], "Credits");
        assert_eq!(quota["quotaStatus"], "valid_numeric");
        assert_eq!(quota["subscription"]["planLabel"], "SuperGrok");
        assert_eq!(
            quota["subscription"]["expiryAvailability"],
            "upstream_not_provided"
        );
        assert!(quota["subscription"]["expiresAt"].is_null());
    }

    #[test]
    fn codex_quota_preserves_expiry_probe_state_for_the_ui() {
        let mut account = sample_account(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Pro 20x".to_string()),
            tiers: Vec::new(),
            extra_usage: Some(json!({
                "queriedAt": 1_700_000_000_000i64,
                "subscription": {
                    "planType": "pro",
                    "planLabel": "ChatGPT Pro 20x",
                    "expiresAt": Value::Null,
                    "expiryCapability": "automatic",
                    "expiryAvailability": "workspace_unverified",
                    "expiryStale": false
                },
                "warningCodes": ["codex_subscription_workspace_unverified"]
            })),
        });
        account.provider_type = ProviderType::CodexOAuth;
        account.subscription_level = Some("ChatGPT Pro 20x".to_string());

        let quota = subscription_quota_from_account(&account, "codex_oauth");

        assert_eq!(
            quota["subscription"]["expiryAvailability"],
            "workspace_unverified"
        );
        assert_eq!(
            quota["warningCodes"],
            json!(["codex_subscription_workspace_unverified"])
        );
        assert!(quota["subscription"]["expiresAt"].is_null());
    }

    #[test]
    fn manual_subscription_expiry_is_synthesized_without_mutating_quota_storage() {
        let mut account = sample_account(AccountQuota::default());
        account.provider_type = ProviderType::ClaudeOAuth;
        account.subscription_level = Some("Claude Pro".to_string());
        account.quota = None;
        account.quota_refreshed_at = None;
        account.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        account.manual_subscription_expiry_updated_at_ms = Some(1_784_000_000_000);
        account.last_refresh_error = Some("upstream quota unavailable".to_string());

        let quota = subscription_quota_from_account(&account, "claude_oauth");

        assert_eq!(quota["credentialStatus"], "valid");
        assert_eq!(quota["success"], false);
        assert_eq!(quota["credentialMessage"], "Claude Pro");
        assert_eq!(
            quota["subscription"]["expiresAt"],
            "2026-08-17T00:00:00+00:00"
        );
        assert_eq!(quota["subscription"]["expiresSource"], "manual");
        assert_eq!(quota["subscription"]["expiresKind"], "billing_period");
        assert_eq!(quota["error"], "upstream quota unavailable");
        assert!(quota["queriedAt"].is_null());
        assert!(account.quota.is_none());
    }

    #[test]
    fn recurring_subscription_expiry_is_synthesized_for_quota_ui() {
        use crate::domain::accounts::subscription_expiry::{
            SubscriptionExpiryCadence, SubscriptionExpiryRuleDraft,
        };

        let mut account = sample_account(AccountQuota::default());
        account.provider_type = ProviderType::ClaudeOAuth;
        account.quota = None;
        account.manual_subscription_expiry_rule = Some(
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Monthly,
                month: None,
                day: 10,
                time_zone: "UTC".to_string(),
            }
            .into_rule(1_784_000_000_000)
            .unwrap(),
        );

        let quota = subscription_quota_from_account(&account, "claude_oauth");

        assert!(quota["subscription"]["expiresAt"].is_string());
        assert_eq!(quota["subscription"]["expiresSource"], "recurring_rule");
        assert_eq!(
            quota["subscription"]["expiresKind"],
            "recurring_billing_period"
        );
        assert_eq!(quota["subscription"]["expiryCapability"], "manual_required");
    }

    #[test]
    fn grok_manual_expiry_replaces_missing_upstream_expiry_for_ui() {
        let mut account = sample_account(AccountQuota {
            success: true,
            credential_message: Some("XPremium".to_string()),
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "XPremium",
                    "planLabel": "XPremium",
                    "expiresAt": Value::Null,
                    "expiryCapability": "automatic_or_manual",
                    "expiryAvailability": "upstream_not_provided"
                }
            })),
            ..AccountQuota::default()
        });
        account.provider_type = ProviderType::GrokOAuth;
        account.subscription_level = Some("XPremium".to_string());
        account.manual_subscription_expires_at_ms = Some(1_786_924_800_000);

        let quota = subscription_quota_from_account(&account, "grok_oauth");

        assert_eq!(
            quota["subscription"]["expiresAt"],
            "2026-08-17T00:00:00+00:00"
        );
        assert_eq!(quota["subscription"]["expiresSource"], "manual");
        assert_eq!(quota["subscription"]["expiresKind"], "billing_period");
        assert_eq!(
            quota["subscription"]["expiryCapability"],
            "automatic_or_manual"
        );
        assert_eq!(quota["subscription"]["expiryAvailability"], "available");
        assert_eq!(
            account
                .quota
                .as_ref()
                .and_then(|quota| quota.extra_usage.as_ref())
                .and_then(|extra| extra.pointer("/subscription/expiresAt")),
            Some(&Value::Null)
        );
    }
}
