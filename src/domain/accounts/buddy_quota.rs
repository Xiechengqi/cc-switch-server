//! Display-only Buddy billing evidence. Never use request credits as quota capacity.
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::store::{Account, AccountQuota, AccountQuotaTier};
use crate::domain::codebuddy::{CodeBuddyAccountProfile, CodeBuddySite};

/// Whitelisted account-wide aggregates; request/model details stay on the Server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuddyOfficialUsageSummary {
    pub status: String,
    pub source: String,
    pub scope: String,
    pub timezone: String,
    pub collected_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_today: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage7_days: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_this_month: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_count: Option<u64>,
}

pub fn buddy_official_usage_summary(account: &Account) -> Option<BuddyOfficialUsageSummary> {
    buddy_official_usage_summary_from_quota(account, account.quota.as_ref())
}

pub fn buddy_official_usage_summary_from_quota(
    account: &Account,
    quota: Option<&AccountQuota>,
) -> Option<BuddyOfficialUsageSummary> {
    if !account.provider_type.is_buddy_oauth() {
        return None;
    }
    let profile =
        CodeBuddyAccountProfile::parse_for(account.provider_type, account.profile.as_ref()).ok()?;
    let usage = quota?.extra_usage.as_ref()?.get("codeBuddyOfficialUsage")?;
    let collected_at = usage.get("collectedAt")?.as_i64().filter(|at| *at > 0)?;
    chrono::DateTime::from_timestamp_millis(collected_at)?;
    let mut summary = BuddyOfficialUsageSummary {
        status: "unavailable".into(),
        source: "buddy_official_request_usage".into(),
        scope: "account".into(),
        timezone: match profile.site {
            CodeBuddySite::Cn => "Asia/Shanghai",
            CodeBuddySite::Intl => "UTC",
        }
        .into(),
        collected_at,
        range_start: None,
        range_end: None,
        usage_today: None,
        usage7_days: None,
        usage_this_month: None,
        usage_total: None,
        request_count: None,
    };
    if usage.get("status").and_then(Value::as_str) != Some("complete") {
        return Some(summary);
    }
    let number = |key: &str| {
        usage
            .get(key)?
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
    };
    let complete = || {
        let start = usage.get("rangeStart")?.as_str()?;
        let end = usage.get("rangeEnd")?.as_str()?;
        if start.len() != 10 || end.len() != 10 {
            return None;
        }
        let first = NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()?;
        let last = NaiveDate::parse_from_str(end, "%Y-%m-%d").ok()?;
        if !(0..=30).contains(&(last - first).num_days()) {
            return None;
        }
        Some((
            start.to_string(),
            end.to_string(),
            number("usageToday")?,
            number("usage7Days")?,
            number("usageThisMonth")?,
            number("usageTotal")?,
            usage.get("requestCount")?.as_u64()?,
        ))
    };
    if let Some((start, end, today, week, month, total, count)) = complete() {
        summary.status = "complete".into();
        summary.range_start = Some(start);
        summary.range_end = Some(end);
        summary.usage_today = Some(today);
        summary.usage7_days = Some(week);
        summary.usage_this_month = Some(month);
        summary.usage_total = Some(total);
        summary.request_count = Some(count);
    }
    Some(summary)
}

/// Read deadlines from the same snapshot, including already-persisted accounts.
pub fn buddy_package_expires_at(account: &Account, tier: &AccountQuotaTier) -> Option<i64> {
    buddy_package_expires_at_from_quota(account, account.quota.as_ref(), tier)
}

pub fn buddy_package_expires_at_from_quota(
    account: &Account,
    quota: Option<&AccountQuota>,
    tier: &AccountQuotaTier,
) -> Option<i64> {
    if !account.provider_type.is_buddy_oauth() {
        return None;
    }
    let index = tier
        .name
        .strip_prefix("codebuddy_package_")?
        .parse::<usize>()
        .ok()?
        .checked_sub(1)?;
    let package = quota?
        .extra_usage
        .as_ref()?
        .pointer("/codeBuddyBilling/packages")?
        .as_array()?
        .get(index)?;
    let package_label = package
        .get("packageName")
        .and_then(Value::as_str)
        .or_else(|| package.get("subProductCode").and_then(Value::as_str));
    // A mismatched or stale package array cannot supply another grant's deadline.
    if tier.limit.is_none()
        || tier
            .label
            .as_deref()
            .is_none_or(|label| label.trim().is_empty())
        || package.get("capacitySize").and_then(Value::as_f64) != tier.limit
        || package_label != tier.label.as_deref()
    {
        return None;
    }
    let at = package.get("expiresAt")?.as_i64().filter(|at| *at > 0)?;
    chrono::DateTime::from_timestamp_millis(at)?;
    Some(at)
}

pub fn buddy_package_label(account: &Account, tier: &AccountQuotaTier) -> Option<String> {
    if !account.provider_type.is_buddy_oauth() {
        return None;
    }
    tier.label
        .as_deref()
        .map(str::trim)
        .filter(|label| {
            !label.is_empty() && label.len() <= 256 && !label.chars().any(char::is_control)
        })
        .map(str::to_string)
        .or_else(|| {
            let index = tier
                .name
                .strip_prefix("codebuddy_package_")?
                .parse::<usize>()
                .ok()?;
            let product = if account.provider_type.as_str() == "workbuddy_oauth" {
                "WorkBuddy"
            } else {
                "CodeBuddy"
            };
            Some(format!("{product} package {index}"))
        })
}

pub fn buddy_quota_visual(account: &Account) -> Option<Value> {
    if !account.provider_type.is_buddy_oauth() {
        return None;
    }
    let quota = account.quota.as_ref()?;
    Some(json!({
        "success": quota.success,
        "plan": quota.credential_message,
        "queriedAt": account.quota_refreshed_at,
        "tiers": quota.tiers.iter().map(|tier| json!({
            "tier": tier, "expiresAt": buddy_package_expires_at(account, tier)
        })).collect::<Vec<_>>(),
        "providerUsage": buddy_official_usage_summary(account),
    }))
}

#[cfg(test)]
pub(crate) fn test_buddy_account(
    product: crate::domain::providers::model::ProviderType,
    site: CodeBuddySite,
) -> Account {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/buddy-quota-presentation-v1.json"
    ))
    .unwrap();
    let domain = match site {
        CodeBuddySite::Cn => "www.codebuddy.cn",
        CodeBuddySite::Intl if product.as_str() == "workbuddy_oauth" => "www.workbuddy.ai",
        CodeBuddySite::Intl => "www.codebuddy.ai",
    };
    let platform = crate::domain::codebuddy::buddy_product_policy(product, site)
        .unwrap()
        .auth_platform;
    let tiers = fixture["tiers"].as_array().unwrap().iter().enumerate().map(|(index, tier)| json!({
        "name": format!("codebuddy_package_{}", index + 1), "label": tier["label"],
        "utilization": tier["utilization"].as_f64().unwrap() / 100.0,
        "used": tier["used"], "limit": tier["limit"], "unit": "credits",
        "resetsAt": chrono::DateTime::parse_from_rfc3339(tier["resetsAt"].as_str().unwrap()).unwrap().timestamp_millis(),
        "source": tier["source"],
    })).collect::<Vec<_>>();
    let packages = fixture["tiers"].as_array().unwrap().iter().map(|tier| json!({
        "packageName": tier["label"], "capacitySize": tier["limit"],
        "expiresAt": chrono::DateTime::parse_from_rfc3339(tier["expiresAt"].as_str().unwrap()).unwrap().timestamp_millis(),
    })).collect::<Vec<_>>();
    serde_json::from_value(json!({
        "id": "buddy-fixture", "providerType": product.as_str(), "authIdentityGeneration":1,
        "accessToken":"fixture-access", "refreshToken":"fixture-refresh",
        "profile": {"site": if site == CodeBuddySite::Cn {"cn"} else {"intl"},
            "domain":domain,"uid":"fixture-uid","productPlatform":platform},
        "quotaRefreshedAt":fixture["queriedAt"],
        "quota":{"success":true,"credentialMessage":fixture["plan"],"tiers":tiers,
            "extraUsage":{"codeBuddyBilling":{"packages":packages},"codeBuddyOfficialUsage":fixture["providerUsage"]}}
    })).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::ProviderType;

    #[test]
    fn buddy_usage_projection_is_product_site_scoped_and_never_copies_private_details() {
        for product in [ProviderType::WorkBuddyOAuth, ProviderType::CodeBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                let mut account = test_buddy_account(product, site);
                let usage = &mut account
                    .quota
                    .as_mut()
                    .unwrap()
                    .extra_usage
                    .as_mut()
                    .unwrap()["codeBuddyOfficialUsage"];
                usage["requests"] =
                    json!([{"prompt":"private-prompt","requestId":"private-request"}]);
                usage["models"] = json!([{"accountId":"private-identity"}]);
                usage["error"] = json!("private-upstream-error");
                let summary = buddy_official_usage_summary(&account).unwrap();
                assert_eq!(
                    summary.timezone,
                    if site == CodeBuddySite::Cn {
                        "Asia/Shanghai"
                    } else {
                        "UTC"
                    }
                );
                assert_eq!(summary.usage_today, Some(0.45));
                assert_eq!(summary.request_count, Some(132));
                let wire = serde_json::to_string(&summary).unwrap();
                assert!(!wire.contains("private"));
                assert!(wire.contains("\"usage7Days\":0.45"));
            }
        }
        let mut other = test_buddy_account(ProviderType::WorkBuddyOAuth, CodeBuddySite::Cn);
        other.provider_type = ProviderType::CodexOAuth;
        assert!(buddy_official_usage_summary(&other).is_none());
    }

    #[test]
    fn buddy_usage_missing_or_invalid_evidence_is_unavailable_not_zero() {
        for (key, value) in [
            ("usageToday", json!(-1)),
            ("usage7Days", Value::Null),
            ("rangeStart", json!("2026-11-04")),
            ("rangeEnd", json!("2027-12-31")),
            ("requestCount", json!(-1)),
            ("status", json!("unavailable")),
        ] {
            let mut account = test_buddy_account(ProviderType::WorkBuddyOAuth, CodeBuddySite::Cn);
            account
                .quota
                .as_mut()
                .unwrap()
                .extra_usage
                .as_mut()
                .unwrap()["codeBuddyOfficialUsage"][key] = value;
            let summary = buddy_official_usage_summary(&account).unwrap();
            assert_eq!(summary.status, "unavailable", "{key}");
            assert!(summary.usage_today.is_none());
            assert!(summary.request_count.is_none());
        }
    }

    #[test]
    fn buddy_package_deadlines_recover_from_existing_snapshot_and_reject_mismatches() {
        let mut account = test_buddy_account(ProviderType::WorkBuddyOAuth, CodeBuddySite::Cn);
        let tier = account.quota.as_ref().unwrap().tiers[0].clone();
        assert_eq!(
            buddy_package_expires_at(&account, &tier),
            Some(1796203104000)
        );
        assert_eq!(
            buddy_package_label(&account, &tier).as_deref(),
            Some("CodeBuddy个人版拉新权益包")
        );
        account
            .quota
            .as_mut()
            .unwrap()
            .extra_usage
            .as_mut()
            .unwrap()["codeBuddyBilling"]["packages"][0]["capacitySize"] = json!(999);
        assert!(buddy_package_expires_at(&account, &tier).is_none());
        let no_label = AccountQuotaTier {
            label: None,
            ..tier
        };
        assert_eq!(
            buddy_package_label(&account, &no_label).as_deref(),
            Some("WorkBuddy package 1")
        );
        let mut account = test_buddy_account(ProviderType::WorkBuddyOAuth, CodeBuddySite::Cn);
        let package = &mut account
            .quota
            .as_mut()
            .unwrap()
            .extra_usage
            .as_mut()
            .unwrap()["codeBuddyBilling"]["packages"][0];
        package["subProductCode"] = package["packageName"].clone();
        package["packageName"] = Value::Null;
        let tier = &account.quota.as_ref().unwrap().tiers[0];
        assert_eq!(
            buddy_package_expires_at(&account, tier),
            Some(1796203104000)
        );
    }
}
