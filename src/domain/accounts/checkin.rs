//! Optional account maintenance; never an inference entitlement or usage debit.
use chrono::{TimeZone, Timelike};
use serde::{Deserialize, Serialize};

use super::store::Account;
use crate::domain::codebuddy::{buddy_account_id, CodeBuddyAccountProfile, CodeBuddySite};
use crate::domain::providers::model::ProviderType;

pub const CHECKIN_TIMEZONE: &str = "Asia/Shanghai";
pub const DEFAULT_CHECKIN_TIME: &str = "09:00";
pub const CHECKIN_DAILY_ATTEMPTS: u32 = 6;

pub fn unsupported_reason(account: &Account) -> Option<&'static str> {
    if account.provider_type != ProviderType::WorkBuddyOAuth {
        return Some("unsupported_provider");
    }
    let Ok(profile) =
        CodeBuddyAccountProfile::parse_for(account.provider_type, account.profile.as_ref())
    else {
        return Some("invalid_account");
    };
    if profile.site != CodeBuddySite::Cn {
        return Some("unsupported_site");
    }
    if !profile.enterprise_id.is_empty() {
        return Some("unsupported_account");
    }
    None
}

pub fn principal(account: &Account) -> Option<String> {
    unsupported_reason(account).is_none().then_some(())?;
    let profile =
        CodeBuddyAccountProfile::parse_for(account.provider_type, account.profile.as_ref()).ok()?;
    buddy_account_id(
        account.provider_type,
        profile.site,
        &profile.uid,
        &profile.enterprise_id,
    )
    .ok()
}

pub fn valid_time(value: &str) -> bool {
    value.len() == 5
        && value.as_bytes()[2] == b':'
        && value
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
        && value[..2].parse::<u32>().is_ok_and(|h| h < 24)
        && value[3..].parse::<u32>().is_ok_and(|m| m < 60)
}

pub fn local_day(now_ms: i64) -> String {
    chrono_tz::Asia::Shanghai
        .timestamp_millis_opt(now_ms)
        .single()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

pub fn scheduled_at(now_ms: i64, time: &str, account_id: &str) -> Option<i64> {
    if !valid_time(time) {
        return None;
    }
    let now = chrono_tz::Asia::Shanghai
        .timestamp_millis_opt(now_ms)
        .single()?;
    // Explicit stable FNV, independent of process seeds and Rust DefaultHasher.
    let jitter = account_id.bytes().fold(2_166_136_261_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    }) % 60;
    now.with_hour(time[..2].parse().ok()?)?
        .with_minute(time[3..].parse().ok()?)?
        .with_second(jitter)?
        .with_nanosecond(0)
        .map(|date| date.timestamp_millis())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckinStatus {
    Success,
    Already,
    Inactive,
    AuthenticationRequired,
    RetryableError,
    Error,
    Unknown,
}

impl CheckinStatus {
    pub fn confirmed(self) -> bool {
        matches!(self, Self::Success | Self::Already)
    }
    pub fn retryable(self) -> bool {
        matches!(self, Self::Unknown | Self::RetryableError)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinOutcome {
    pub status: CheckinStatus,
    pub at_ms: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountCheckin {
    pub principal: String,
    pub enabled: bool,
    pub time: String,
    pub revision: u64,
    pub attempt_day: String,
    pub attempts: u32,
    pub next_retry_at: Option<i64>,
    pub pending: bool,
    pub confirmed_day: Option<String>,
    pub last_result: Option<CheckinOutcome>,
    pub quota_pending: bool,
    pub quota_retry_at: Option<i64>,
    pub quota_attempts: u32,
    pub history: Vec<CheckinOutcome>,
}

impl Default for AccountCheckin {
    fn default() -> Self {
        Self {
            principal: String::new(),
            enabled: false,
            time: DEFAULT_CHECKIN_TIME.into(),
            revision: 0,
            attempt_day: String::new(),
            attempts: 0,
            next_retry_at: None,
            pending: false,
            confirmed_day: None,
            last_result: None,
            quota_pending: false,
            quota_retry_at: None,
            quota_attempts: 0,
            history: Vec::new(),
        }
    }
}

impl AccountCheckin {
    pub(super) fn authentication_recovered(&mut self) {
        if !self.pending
            && self
                .last_result
                .as_ref()
                .is_some_and(|r| r.status == CheckinStatus::AuthenticationRequired)
        {
            // Keep today's attempt budget and historical result. Fresh native
            // credentials allow the scheduler to retry the status query.
            self.next_retry_at = Some(0);
        }
    }

    pub fn matches(&self, account: &Account) -> bool {
        principal(account).as_deref() == Some(&self.principal)
    }

    pub fn confirmed_today(&self, now: i64) -> bool {
        self.confirmed_day.as_deref() == Some(local_day(now).as_str())
    }

    pub fn due(&self, account: &Account, now: i64) -> bool {
        // Turning auto off stops new submissions, but an already-dispatched
        // manual/automatic intent may still be reconciled read-only.
        if (!self.enabled && !self.pending)
            || !self.matches(account)
            || account.needs_relogin
            || self.confirmed_today(now)
        {
            return false;
        }
        if self.attempt_day == local_day(now) {
            if self.attempts >= CHECKIN_DAILY_ATTEMPTS {
                return false;
            }
            // Persisted pending work is recovered by querying status after its lease.
            if self.pending || self.last_result.is_some() {
                return self.next_retry_at.is_some_and(|due| due <= now);
            }
        }
        scheduled_at(now, &self.time, &account.id).is_some_and(|due| due <= now)
    }

    pub fn begin(&mut self, now: i64) {
        let day = local_day(now);
        if self.attempt_day != day {
            self.attempt_day = day;
            self.attempts = 0;
        }
        self.attempts = self.attempts.saturating_add(1);
        self.pending = true;
        self.next_retry_at = Some(now.saturating_add(5 * 60_000));
    }

    pub fn finish(&mut self, result: CheckinOutcome, retry_after_ms: Option<i64>) {
        self.pending = result.status == CheckinStatus::Unknown;
        self.next_retry_at = if result.status.retryable() && self.attempts < CHECKIN_DAILY_ATTEMPTS
        {
            let delay = match self.attempts {
                0 | 1 => 5 * 60_000,
                2 => 15 * 60_000,
                _ => 60 * 60_000,
            };
            Some(
                result
                    .at_ms
                    .saturating_add(delay.max(retry_after_ms.unwrap_or(0))),
            )
        } else {
            None
        };
        if result.status.confirmed() {
            self.confirmed_day = Some(local_day(result.at_ms));
            self.quota_pending = true;
            self.quota_retry_at = Some(result.at_ms);
            self.quota_attempts = 0;
        }
        self.history
            .retain(|row| row.at_ms >= result.at_ms.saturating_sub(30 * 86_400_000));
        self.history.push(result.clone());
        if self.history.len() > 100 {
            self.history.drain(..self.history.len() - 100);
        }
        self.last_result = Some(result);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinView {
    pub supported: bool,
    pub reason: Option<&'static str>,
    pub enabled: bool,
    pub time: String,
    pub timezone: &'static str,
    pub revision: u64,
    pub today_checked_in: bool,
    pub pending: bool,
    pub last_result: Option<CheckinOutcome>,
    pub next_retry_at: Option<i64>,
    pub quota_pending: bool,
}

impl CheckinView {
    pub fn new(account: &Account, record: Option<&AccountCheckin>, now: i64) -> Self {
        let default = AccountCheckin::default();
        let record = record.filter(|r| r.matches(account)).unwrap_or(&default);
        let reason = unsupported_reason(account);
        Self {
            supported: reason.is_none(),
            reason: reason.or(account.needs_relogin.then_some("needs_relogin")),
            enabled: record.enabled,
            time: record.time.clone(),
            timezone: CHECKIN_TIMEZONE,
            revision: record.revision,
            today_checked_in: record.confirmed_today(now),
            pending: record.pending,
            last_result: record.last_result.clone(),
            next_retry_at: record.next_retry_at,
            quota_pending: record.quota_pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn account() -> Account {
        serde_json::from_value(json!({"id":"workbuddy-cn", "providerType":"workbuddy_oauth", "accessToken":"fixture", "scopes":[],
            "profile":{"site":"cn", "uid":"uid-cn", "domain":"www.codebuddy.cn", "enterpriseId":"", "productPlatform":"workbuddy"}})).unwrap()
    }

    #[test]
    fn checkin_product_site_and_personal_scope_are_exact() {
        let mut a = account();
        assert_eq!(unsupported_reason(&a), None);
        a.provider_type = ProviderType::CodeBuddyOAuth;
        assert_eq!(unsupported_reason(&a), Some("unsupported_provider"));
        a = account();
        a.profile.as_mut().unwrap()["site"] = json!("intl");
        a.profile.as_mut().unwrap()["domain"] = json!("www.workbuddy.ai");
        a.profile.as_mut().unwrap()["productPlatform"] = json!("workbuddy-ai");
        assert_eq!(unsupported_reason(&a), Some("unsupported_site"));
    }

    #[test]
    fn checkin_day_schedule_and_restart_use_shanghai() {
        let a = account();
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-04T01:02:00Z")
            .unwrap()
            .timestamp_millis();
        let mut r = AccountCheckin {
            principal: principal(&a).unwrap(),
            enabled: true,
            ..Default::default()
        };
        assert!(r.due(&a, now));
        assert!(!r.due(&a, now - 5 * 60_000));
        assert_eq!(local_day(now - 10 * 60 * 60_000), "2026-11-03");
        r.begin(now);
        let restored: AccountCheckin =
            serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert!(!restored.due(&a, now));
        assert!(restored.due(&a, now + 5 * 60_000));
        r.finish(
            CheckinOutcome {
                status: CheckinStatus::Already,
                at_ms: now,
                message: "already".into(),
            },
            None,
        );
        assert!(!r.due(&a, now + 60 * 60_000));
        assert!(r.due(&a, now + 86_400_000));
        r.enabled = false;
        assert!(!r.due(&a, now + 86_400_000));
    }

    #[test]
    fn checkin_retries_are_bounded_and_terminal_states_are_quiet() {
        let a = account();
        let now = 1_794_000_000_000;
        let mut r = AccountCheckin {
            principal: principal(&a).unwrap(),
            enabled: true,
            ..Default::default()
        };
        for n in 1..=CHECKIN_DAILY_ATTEMPTS {
            r.begin(now);
            r.finish(
                CheckinOutcome {
                    status: CheckinStatus::Unknown,
                    at_ms: now,
                    message: "unknown".into(),
                },
                Some(7_200_000),
            );
            assert_eq!(
                r.next_retry_at,
                (n < CHECKIN_DAILY_ATTEMPTS).then_some(now + 7_200_000)
            );
        }
        assert!(!r.due(&a, now + 7_200_001));
        for status in [
            CheckinStatus::Inactive,
            CheckinStatus::Error,
            CheckinStatus::AuthenticationRequired,
        ] {
            r.attempts = 1;
            r.finish(
                CheckinOutcome {
                    status,
                    at_ms: now,
                    message: "terminal".into(),
                },
                None,
            );
            assert_eq!(r.next_retry_at, None);
        }
        for bad in ["9:00", "24:00", "12:60", "12:0x", "你好"] {
            assert!(!valid_time(bad));
        }
        assert!(valid_time("09:00"));
    }
}
