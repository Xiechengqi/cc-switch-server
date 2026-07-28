use chrono::{Datelike, TimeZone, Utc, Weekday};

use crate::domain::sharing::router_contract::{ShareTokenPeriod, ShareUserPolicy};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenPeriodWindow {
    pub starts_at_ms: Option<i64>,
    pub ends_at_ms: Option<i64>,
}

impl ShareTokenPeriod {
    pub fn requires_anchor(self) -> bool {
        matches!(self, Self::SevenDays | Self::ThirtyDays)
    }

    fn duration_ms(self) -> Option<i64> {
        match self {
            Self::SevenDays => Some(7 * DAY_MS),
            Self::ThirtyDays => Some(30 * DAY_MS),
            _ => None,
        }
    }
}

pub fn validate_user_policy(policy: &ShareUserPolicy, now_ms: i64) -> Result<(), String> {
    match (
        policy.token_period.requires_anchor(),
        policy.token_period_anchor_at_ms,
    ) {
        (true, None) => Err("tokenPeriodAnchorAtMs is required for fixed token periods".into()),
        (false, Some(_)) => {
            Err("tokenPeriodAnchorAtMs is only allowed for sevenDays or thirtyDays".into())
        }
        (true, Some(anchor)) if anchor < 0 => {
            Err("tokenPeriodAnchorAtMs must be a non-negative UTC timestamp".into())
        }
        (true, Some(anchor)) if anchor % MINUTE_MS != 0 => {
            Err("tokenPeriodAnchorAtMs must use minute precision".into())
        }
        (true, Some(anchor)) if anchor > now_ms => {
            Err("tokenPeriodAnchorAtMs cannot be in the future".into())
        }
        _ => Ok(()),
    }
}

pub fn token_period_window(
    policy: &ShareUserPolicy,
    now_ms: i64,
) -> Result<TokenPeriodWindow, String> {
    let calendar_start = |start_ms: i64, end_ms: i64| TokenPeriodWindow {
        starts_at_ms: Some(start_ms),
        ends_at_ms: Some(end_ms),
    };
    let now = Utc
        .timestamp_millis_opt(now_ms)
        .single()
        .ok_or_else(|| "current time is outside the supported UTC range".to_string())?;

    Ok(match policy.token_period {
        ShareTokenPeriod::Lifetime => TokenPeriodWindow {
            starts_at_ms: None,
            ends_at_ms: None,
        },
        ShareTokenPeriod::Day => {
            let start = now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .expect("UTC midnight is valid")
                .and_utc()
                .timestamp_millis();
            calendar_start(start, start + DAY_MS)
        }
        ShareTokenPeriod::Week => {
            let days = match now.weekday() {
                Weekday::Mon => 0,
                Weekday::Tue => 1,
                Weekday::Wed => 2,
                Weekday::Thu => 3,
                Weekday::Fri => 4,
                Weekday::Sat => 5,
                Weekday::Sun => 6,
            };
            let start = (now.date_naive() - chrono::Duration::days(days))
                .and_hms_opt(0, 0, 0)
                .expect("UTC week boundary is valid")
                .and_utc()
                .timestamp_millis();
            calendar_start(start, start + 7 * DAY_MS)
        }
        ShareTokenPeriod::CalendarMonth => {
            let start = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .expect("UTC month boundary is valid")
                .and_hms_opt(0, 0, 0)
                .expect("UTC month midnight is valid")
                .and_utc()
                .timestamp_millis();
            let (year, month) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            let end = chrono::NaiveDate::from_ymd_opt(year, month, 1)
                .expect("next UTC month boundary is valid")
                .and_hms_opt(0, 0, 0)
                .expect("UTC month midnight is valid")
                .and_utc()
                .timestamp_millis();
            calendar_start(start, end)
        }
        period @ (ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays) => {
            let anchor = policy
                .token_period_anchor_at_ms
                .ok_or_else(|| "tokenPeriodAnchorAtMs is required".to_string())?;
            let duration = period.duration_ms().expect("fixed period has duration");
            let index = (i128::from(now_ms) - i128::from(anchor)).div_euclid(i128::from(duration));
            let start = i128::from(anchor)
                .checked_add(index.checked_mul(i128::from(duration)).ok_or_else(|| {
                    "fixed token period window multiplication overflow".to_string()
                })?)
                .ok_or_else(|| "fixed token period window start overflow".to_string())?;
            let end = start
                .checked_add(i128::from(duration))
                .ok_or_else(|| "fixed token period window end overflow".to_string())?;
            calendar_start(
                i64::try_from(start)
                    .map_err(|_| "fixed token period window start is out of range".to_string())?,
                i64::try_from(end)
                    .map_err(|_| "fixed token period window end is out of range".to_string())?,
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn calendar_windows_use_utc_boundaries() {
        let now = at(2026, 7, 24, 15, 30);
        let policy = |token_period| ShareUserPolicy {
            token_period,
            ..ShareUserPolicy::default()
        };
        assert_eq!(
            token_period_window(&policy(ShareTokenPeriod::Day), now).unwrap(),
            TokenPeriodWindow {
                starts_at_ms: Some(at(2026, 7, 24, 0, 0)),
                ends_at_ms: Some(at(2026, 7, 25, 0, 0)),
            }
        );
        assert_eq!(
            token_period_window(&policy(ShareTokenPeriod::Week), now).unwrap(),
            TokenPeriodWindow {
                starts_at_ms: Some(at(2026, 7, 20, 0, 0)),
                ends_at_ms: Some(at(2026, 7, 27, 0, 0)),
            }
        );
        assert_eq!(
            token_period_window(&policy(ShareTokenPeriod::CalendarMonth), now).unwrap(),
            TokenPeriodWindow {
                starts_at_ms: Some(at(2026, 7, 1, 0, 0)),
                ends_at_ms: Some(at(2026, 8, 1, 0, 0)),
            }
        );
    }

    #[test]
    fn fixed_windows_are_anchored_and_half_open() {
        let anchor = at(2026, 7, 1, 12, 15);
        let policy = ShareUserPolicy {
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(anchor),
            ..ShareUserPolicy::default()
        };
        assert_eq!(
            token_period_window(&policy, at(2026, 7, 15, 12, 14)).unwrap(),
            TokenPeriodWindow {
                starts_at_ms: Some(at(2026, 7, 8, 12, 15)),
                ends_at_ms: Some(at(2026, 7, 15, 12, 15)),
            }
        );
        assert_eq!(
            token_period_window(&policy, at(2026, 7, 15, 12, 15)).unwrap(),
            TokenPeriodWindow {
                starts_at_ms: Some(at(2026, 7, 15, 12, 15)),
                ends_at_ms: Some(at(2026, 7, 22, 12, 15)),
            }
        );
    }

    #[test]
    fn anchors_are_required_exclusive_and_minute_aligned() {
        let now = at(2026, 7, 24, 15, 30);
        let missing = ShareUserPolicy {
            token_period: ShareTokenPeriod::ThirtyDays,
            ..ShareUserPolicy::default()
        };
        assert!(validate_user_policy(&missing, now).is_err());
        let stray = ShareUserPolicy {
            token_period: ShareTokenPeriod::Day,
            token_period_anchor_at_ms: Some(now),
            ..ShareUserPolicy::default()
        };
        assert!(validate_user_policy(&stray, now).is_err());
        let seconds = ShareUserPolicy {
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(now - 1),
            ..ShareUserPolicy::default()
        };
        assert!(validate_user_policy(&seconds, now).is_err());
    }
}
