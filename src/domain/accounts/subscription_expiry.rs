use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::accounts::store::Account;
use crate::domain::providers::model::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionExpiryCapability {
    Automatic,
    AutomaticOrManual,
    ManualRequired,
    ResearchPending,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionExpirySource {
    Automatic,
    RecurringRule,
    LegacyManual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionExpiryCadence {
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionExpiryRule {
    pub cadence: SubscriptionExpiryCadence,
    #[serde(default)]
    pub month: Option<u8>,
    pub day: u8,
    pub time_zone: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionExpiryRuleDraft {
    pub cadence: SubscriptionExpiryCadence,
    #[serde(default)]
    pub month: Option<u8>,
    pub day: u8,
    pub time_zone: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionExpiryRuleError {
    InvalidDay,
    MissingMonth,
    UnexpectedMonth,
    InvalidAnnualDate,
    InvalidTimeZone,
    InvalidUpdatedAt,
}

impl std::fmt::Display for SubscriptionExpiryRuleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDay => "subscription expiry day must be between 1 and 31",
            Self::MissingMonth => "yearly subscription expiry requires a month",
            Self::UnexpectedMonth => "monthly subscription expiry must not include a month",
            Self::InvalidAnnualDate => "yearly subscription expiry month and day are invalid",
            Self::InvalidTimeZone => "subscription expiry time zone must be a valid IANA time zone",
            Self::InvalidUpdatedAt => "subscription expiry update time is invalid",
        })
    }
}

impl std::error::Error for SubscriptionExpiryRuleError {}

impl SubscriptionExpiryRuleDraft {
    pub fn into_rule(
        self,
        updated_at_ms: i64,
    ) -> Result<SubscriptionExpiryRule, SubscriptionExpiryRuleError> {
        let rule = SubscriptionExpiryRule {
            cadence: self.cadence,
            month: self.month,
            day: self.day,
            time_zone: self.time_zone.trim().to_string(),
            updated_at_ms,
        };
        rule.validate()?;
        Ok(rule)
    }
}

impl SubscriptionExpiryRule {
    pub fn validate(&self) -> Result<(), SubscriptionExpiryRuleError> {
        if !(1..=31).contains(&self.day) {
            return Err(SubscriptionExpiryRuleError::InvalidDay);
        }
        match self.cadence {
            SubscriptionExpiryCadence::Monthly if self.month.is_some() => {
                return Err(SubscriptionExpiryRuleError::UnexpectedMonth);
            }
            SubscriptionExpiryCadence::Yearly => {
                let month = self
                    .month
                    .ok_or(SubscriptionExpiryRuleError::MissingMonth)?;
                if NaiveDate::from_ymd_opt(2000, u32::from(month), u32::from(self.day)).is_none() {
                    return Err(SubscriptionExpiryRuleError::InvalidAnnualDate);
                }
            }
            SubscriptionExpiryCadence::Monthly => {}
        }
        if self.time_zone.parse::<Tz>().is_err() {
            return Err(SubscriptionExpiryRuleError::InvalidTimeZone);
        }
        if self.updated_at_ms < 0 {
            return Err(SubscriptionExpiryRuleError::InvalidUpdatedAt);
        }
        Ok(())
    }
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
        ProviderType::GrokOAuth => SubscriptionExpiryCapability::AutomaticOrManual,
        ProviderType::CodexOAuth | ProviderType::OllamaCloud => {
            SubscriptionExpiryCapability::Automatic
        }
        ProviderType::GeminiCli
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth
        | ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth => SubscriptionExpiryCapability::ResearchPending,
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

pub fn supports_automatic_expiry(capability: SubscriptionExpiryCapability) -> bool {
    matches!(
        capability,
        SubscriptionExpiryCapability::Automatic | SubscriptionExpiryCapability::AutomaticOrManual
    )
}

pub fn supports_manual_expiry(capability: SubscriptionExpiryCapability) -> bool {
    matches!(
        capability,
        SubscriptionExpiryCapability::ManualRequired
            | SubscriptionExpiryCapability::AutomaticOrManual
    )
}

pub fn automatic_subscription_expires_at_ms(account: &Account) -> Option<i64> {
    if !supports_automatic_expiry(subscription_expiry_capability(account.provider_type)) {
        return None;
    }

    let extra_usage = account.quota.as_ref()?.extra_usage.as_ref()?;
    automatic_expiry_paths(account.provider_type)
        .iter()
        .find_map(|path| timestamp_at(extra_usage, path))
}

pub fn recurring_subscription_expires_at_ms(account: &Account, now_ms: i64) -> Option<i64> {
    account
        .manual_subscription_expiry_rule
        .as_ref()
        .and_then(|rule| next_subscription_expiry_occurrence_ms(rule, now_ms))
}

pub fn next_subscription_expiry_occurrence_ms(
    rule: &SubscriptionExpiryRule,
    now_ms: i64,
) -> Option<i64> {
    rule.validate().ok()?;
    let time_zone = rule.time_zone.parse::<Tz>().ok()?;
    let now = Utc.timestamp_millis_opt(now_ms).single()?;
    let local_now = now.with_timezone(&time_zone);
    let current_year = local_now.year();
    let current_month = local_now.month();

    let candidate = match rule.cadence {
        SubscriptionExpiryCadence::Monthly => {
            let current = clamped_date(current_year, current_month, rule.day)?;
            let current_expiry = local_day_end_ms(time_zone, current)?;
            if current_expiry >= now_ms {
                return Some(current_expiry);
            }
            let (year, month) = next_month(current_year, current_month)?;
            clamped_date(year, month, rule.day)?
        }
        SubscriptionExpiryCadence::Yearly => {
            let month = u32::from(rule.month?);
            let current = clamped_date(current_year, month, rule.day)?;
            let current_expiry = local_day_end_ms(time_zone, current)?;
            if current_expiry >= now_ms {
                return Some(current_expiry);
            }
            clamped_date(current_year.checked_add(1)?, month, rule.day)?
        }
    };
    local_day_end_ms(time_zone, candidate)
}

pub fn resolved_subscription_expiry_at(
    account: &Account,
    now_ms: i64,
) -> ResolvedSubscriptionExpiry {
    let capability = subscription_expiry_capability(account.provider_type);
    let manual = recurring_subscription_expires_at_ms(account, now_ms)
        .map(|expires_at_ms| (expires_at_ms, SubscriptionExpirySource::RecurringRule))
        .or_else(|| {
            account
                .manual_subscription_expires_at_ms
                .map(|expires_at_ms| (expires_at_ms, SubscriptionExpirySource::LegacyManual))
        });
    let (expires_at_ms, source) = match capability {
        SubscriptionExpiryCapability::Automatic => (
            automatic_subscription_expires_at_ms(account),
            Some(SubscriptionExpirySource::Automatic),
        ),
        SubscriptionExpiryCapability::AutomaticOrManual => {
            match automatic_subscription_expires_at_ms(account) {
                Some(expires_at_ms) => (
                    Some(expires_at_ms),
                    Some(SubscriptionExpirySource::Automatic),
                ),
                None => manual.map_or((None, None), |(expires_at_ms, source)| {
                    (Some(expires_at_ms), Some(source))
                }),
            }
        }
        SubscriptionExpiryCapability::ManualRequired => manual
            .map_or((None, None), |(expires_at_ms, source)| {
                (Some(expires_at_ms), Some(source))
            }),
        SubscriptionExpiryCapability::ResearchPending
        | SubscriptionExpiryCapability::NotApplicable => (None, None),
    };

    ResolvedSubscriptionExpiry {
        capability,
        expires_at_ms,
        source: expires_at_ms.and(source),
    }
}

pub fn resolved_subscription_expiry(account: &Account) -> ResolvedSubscriptionExpiry {
    resolved_subscription_expiry_at(
        account,
        crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
    )
}

fn clamped_date(year: i32, month: u32, day: u8) -> Option<NaiveDate> {
    let last_day = days_in_month(year, month)?;
    NaiveDate::from_ymd_opt(year, month, u32::from(day).min(last_day))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year.checked_add(1)?, 1)
    } else {
        (year, month.checked_add(1)?)
    };
    let first_of_next_month = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    Some((first_of_next_month - Duration::days(1)).day())
}

fn next_month(year: i32, month: u32) -> Option<(i32, u32)> {
    if month == 12 {
        Some((year.checked_add(1)?, 1))
    } else {
        Some((year, month.checked_add(1)?))
    }
}

fn local_day_end_ms(time_zone: Tz, date: NaiveDate) -> Option<i64> {
    let next_date = date.succ_opt()?;
    let next_midnight = next_date.and_hms_opt(0, 0, 0)?;
    for minute in 0..=(48 * 60) {
        let candidate = next_midnight.checked_add_signed(Duration::minutes(minute))?;
        match time_zone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return value.timestamp_millis().checked_sub(1),
            LocalResult::Ambiguous(first, second) => {
                return first
                    .timestamp_millis()
                    .min(second.timestamp_millis())
                    .checked_sub(1);
            }
            LocalResult::None => {}
        }
    }
    None
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
        ProviderType::GrokOAuth => &[
            "/subscription/expiresAt",
            "/subscription/expires_at",
            "/subscriptionExpiresAt",
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
                ProviderType::GrokOAuth => SubscriptionExpiryCapability::AutomaticOrManual,
                ProviderType::CodexOAuth | ProviderType::OllamaCloud => {
                    SubscriptionExpiryCapability::Automatic
                }
                ProviderType::GeminiCli
                | ProviderType::AntigravityOAuth
                | ProviderType::AgyOAuth
                | ProviderType::GitHubCopilot
                | ProviderType::KiroOAuth
                | ProviderType::CursorOAuth => SubscriptionExpiryCapability::ResearchPending,
                _ => SubscriptionExpiryCapability::NotApplicable,
            };
            assert_eq!(subscription_expiry_capability(provider_type), expected);
        }
    }

    #[test]
    fn automatic_resolver_reads_only_trusted_subscription_paths() {
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

        let grok = account(
            ProviderType::GrokOAuth,
            Some(json!({
                "subscription": {"expiresAt": "2026-08-19T00:00:00Z"},
                "billingPeriodEnd": "2030-01-01T00:00:00Z"
            })),
        );
        assert_eq!(
            automatic_subscription_expires_at_ms(&grok),
            Some(1_787_097_600_000)
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
                source: Some(SubscriptionExpirySource::LegacyManual),
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
    fn grok_uses_manual_expiry_only_when_trusted_automatic_expiry_is_absent() {
        let mut grok = account(ProviderType::GrokOAuth, None);
        grok.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        assert_eq!(
            resolved_subscription_expiry(&grok),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::AutomaticOrManual,
                expires_at_ms: Some(1_786_924_800_000),
                source: Some(SubscriptionExpirySource::LegacyManual),
            }
        );

        grok.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {"expiresAt": "2026-08-19T00:00:00Z"}
            })),
            ..AccountQuota::default()
        });
        assert_eq!(
            resolved_subscription_expiry(&grok),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::AutomaticOrManual,
                expires_at_ms: Some(1_787_097_600_000),
                source: Some(SubscriptionExpirySource::Automatic),
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

    fn timestamp(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    }

    fn rule(
        cadence: SubscriptionExpiryCadence,
        month: Option<u8>,
        day: u8,
        time_zone: &str,
    ) -> SubscriptionExpiryRule {
        SubscriptionExpiryRuleDraft {
            cadence,
            month,
            day,
            time_zone: time_zone.to_string(),
        }
        .into_rule(1_000)
        .unwrap()
    }

    #[test]
    fn monthly_rule_uses_current_or_next_occurrence_at_local_day_end() {
        let monthly = rule(
            SubscriptionExpiryCadence::Monthly,
            None,
            10,
            "Asia/Shanghai",
        );

        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2026-07-05T00:00:00Z")),
            Some(timestamp("2026-07-10T15:59:59.999Z"))
        );
        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2026-07-20T00:00:00Z")),
            Some(timestamp("2026-08-10T15:59:59.999Z"))
        );
        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2026-07-10T08:00:00Z")),
            Some(timestamp("2026-07-10T15:59:59.999Z"))
        );
    }

    #[test]
    fn monthly_rule_clamps_short_months_without_changing_the_anchor_day() {
        let monthly = rule(SubscriptionExpiryCadence::Monthly, None, 31, "UTC");

        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2027-02-10T00:00:00Z")),
            Some(timestamp("2027-02-28T23:59:59.999Z"))
        );
        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2027-03-01T00:00:00Z")),
            Some(timestamp("2027-03-31T23:59:59.999Z"))
        );
    }

    #[test]
    fn yearly_february_29_rule_clamps_only_non_leap_years() {
        let yearly = rule(SubscriptionExpiryCadence::Yearly, Some(2), 29, "UTC");

        assert_eq!(
            next_subscription_expiry_occurrence_ms(&yearly, timestamp("2027-01-01T00:00:00Z")),
            Some(timestamp("2027-02-28T23:59:59.999Z"))
        );
        assert_eq!(
            next_subscription_expiry_occurrence_ms(&yearly, timestamp("2028-01-01T00:00:00Z")),
            Some(timestamp("2028-02-29T23:59:59.999Z"))
        );
    }

    #[test]
    fn rule_keeps_iana_timezone_across_daylight_saving_time() {
        let monthly = rule(
            SubscriptionExpiryCadence::Monthly,
            None,
            10,
            "America/New_York",
        );

        assert_eq!(
            next_subscription_expiry_occurrence_ms(&monthly, timestamp("2026-03-01T00:00:00Z")),
            Some(timestamp("2026-03-11T03:59:59.999Z"))
        );
    }

    #[test]
    fn recurring_rule_precedes_legacy_manual_and_remains_grok_fallback() {
        let mut grok = account(ProviderType::GrokOAuth, None);
        grok.manual_subscription_expiry_rule =
            Some(rule(SubscriptionExpiryCadence::Monthly, None, 10, "UTC"));
        grok.manual_subscription_expires_at_ms = Some(timestamp("2030-01-01T00:00:00Z"));
        let now = timestamp("2026-07-20T00:00:00Z");

        assert_eq!(
            resolved_subscription_expiry_at(&grok, now),
            ResolvedSubscriptionExpiry {
                capability: SubscriptionExpiryCapability::AutomaticOrManual,
                expires_at_ms: Some(timestamp("2026-08-10T23:59:59.999Z")),
                source: Some(SubscriptionExpirySource::RecurringRule),
            }
        );

        grok.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {"expiresAt": "2026-08-19T00:00:00Z"}
            })),
            ..AccountQuota::default()
        });
        assert_eq!(
            resolved_subscription_expiry_at(&grok, now).source,
            Some(SubscriptionExpirySource::Automatic)
        );
    }

    #[test]
    fn rule_validation_rejects_ambiguous_or_impossible_shapes() {
        for draft in [
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Monthly,
                month: Some(1),
                day: 10,
                time_zone: "UTC".to_string(),
            },
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Yearly,
                month: None,
                day: 10,
                time_zone: "UTC".to_string(),
            },
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Yearly,
                month: Some(4),
                day: 31,
                time_zone: "UTC".to_string(),
            },
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Monthly,
                month: None,
                day: 10,
                time_zone: "not/a-zone".to_string(),
            },
        ] {
            assert!(draft.into_rule(1_000).is_err());
        }
    }
}
