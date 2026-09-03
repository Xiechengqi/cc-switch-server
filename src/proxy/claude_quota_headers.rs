use axum::http::{HeaderMap, StatusCode};

use crate::domain::accounts::store::{
    AccountQuotaWindowObservationDraft, ClaudeFableEntitlementEvidence, CLAUDE_FABLE_QUOTA_TIER,
    CLAUDE_FIVE_HOUR_OBSERVATION_MAX_FUTURE_MS, CLAUDE_FIVE_HOUR_QUOTA_TIER,
    CLAUDE_SEVEN_DAY_QUOTA_TIER, CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
};

const MAX_UTILIZATION_WITH_OVERSHOOT: f64 = 1.05;

#[derive(Debug, Clone, Copy)]
struct WindowHeaderSpec {
    tier_name: &'static str,
    header_window: &'static str,
    max_future_ms: i64,
}

const WINDOW_HEADERS: &[WindowHeaderSpec] = &[
    WindowHeaderSpec {
        tier_name: CLAUDE_FIVE_HOUR_QUOTA_TIER,
        header_window: "5h",
        max_future_ms: CLAUDE_FIVE_HOUR_OBSERVATION_MAX_FUTURE_MS,
    },
    WindowHeaderSpec {
        tier_name: CLAUDE_SEVEN_DAY_QUOTA_TIER,
        header_window: "7d",
        max_future_ms: CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
    },
    WindowHeaderSpec {
        tier_name: CLAUDE_FABLE_QUOTA_TIER,
        header_window: "7d_oi",
        max_future_ms: CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
    },
];

pub(crate) fn parse_claude_quota_headers(
    headers: &HeaderMap,
    status: StatusCode,
    fable_request: bool,
    now_ms: i64,
) -> Vec<AccountQuotaWindowObservationDraft> {
    let fable_entitlement_evidence = if status.is_success() && fable_request {
        Some(ClaudeFableEntitlementEvidence::SuccessfulFableRequest)
    } else if status == StatusCode::TOO_MANY_REQUESTS
        && claude_fable_only_rejected(headers, fable_request)
    {
        Some(ClaudeFableEntitlementEvidence::FableOnlyRateLimit)
    } else {
        None
    };

    WINDOW_HEADERS
        .iter()
        .filter_map(|spec| {
            let utilization_header = format!(
                "anthropic-ratelimit-unified-{}-utilization",
                spec.header_window
            );
            let reset_header = format!("anthropic-ratelimit-unified-{}-reset", spec.header_window);
            let status_header =
                format!("anthropic-ratelimit-unified-{}-status", spec.header_window);
            let utilization =
                parse_utilization_header(headers, &utilization_header).or_else(|| {
                    (header_lower(headers, &status_header).as_deref() == Some("rejected"))
                        .then_some(1.0)
                });
            let resets_at_ms =
                parse_anthropic_reset_header(headers, &reset_header, now_ms, spec.max_future_ms);
            if utilization.is_none() && resets_at_ms.is_none() {
                return None;
            }
            Some(AccountQuotaWindowObservationDraft {
                tier_name: spec.tier_name,
                utilization,
                resets_at_ms,
                observed_at_ms: now_ms,
                fable_entitlement_evidence: (spec.tier_name == CLAUDE_FABLE_QUOTA_TIER)
                    .then_some(fable_entitlement_evidence)
                    .flatten(),
            })
        })
        .collect()
}

pub(crate) fn claude_fable_only_rejected(headers: &HeaderMap, fable_request: bool) -> bool {
    fable_request
        && header_lower(headers, "anthropic-ratelimit-unified-7d_oi-status").as_deref()
            == Some("rejected")
        && claude_shared_window_allowed(
            header_lower(headers, "anthropic-ratelimit-unified-5h-status").as_deref(),
        )
        && claude_shared_window_allowed(
            header_lower(headers, "anthropic-ratelimit-unified-7d-status").as_deref(),
        )
}

pub(crate) fn header_lower(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

pub(crate) fn parse_anthropic_reset_header(
    headers: &HeaderMap,
    name: &str,
    now_ms: i64,
    max_future_ms: i64,
) -> Option<i64> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    parse_anthropic_reset_value(value, now_ms, max_future_ms)
}

fn parse_utilization_header(headers: &HeaderMap, name: &str) -> Option<f64> {
    let value = headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    (value.is_finite() && (0.0..=MAX_UTILIZATION_WITH_OVERSHOOT).contains(&value))
        .then(|| value.clamp(0.0, 1.0))
}

fn parse_anthropic_reset_value(value: &str, now_ms: i64, max_future_ms: i64) -> Option<i64> {
    if value.is_empty() || max_future_ms <= 0 {
        return None;
    }
    let parsed = if let Ok(number) = value.parse::<f64>() {
        if !number.is_finite() || number <= 0.0 {
            return None;
        }
        Some(if number < 10_000_000.0 {
            now_ms.saturating_add((number * 1_000.0).min(i64::MAX as f64) as i64)
        } else if number > 10_000_000_000.0 {
            number.min(i64::MAX as f64) as i64
        } else {
            (number * 1_000.0).min(i64::MAX as f64) as i64
        })
    } else {
        chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.timestamp_millis())
            .or_else(|| {
                httpdate::parse_http_date(value).ok().and_then(|value| {
                    value
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                })
            })
    }?;
    (parsed > now_ms && parsed <= now_ms.saturating_add(max_future_ms)).then_some(parsed)
}

fn claude_shared_window_allowed(status: Option<&str>) -> bool {
    matches!(status, Some("allowed" | "allowed_warning"))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(values: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn parses_all_windows_and_preserves_zero_utilization() {
        let now = 1_700_000_000_000;
        let parsed = parse_claude_quota_headers(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-utilization", "0"),
                ("anthropic-ratelimit-unified-5h-reset", "3600"),
                ("anthropic-ratelimit-unified-7d-utilization", "0.24"),
                ("anthropic-ratelimit-unified-7d-reset", "1700500000"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "0.41"),
                ("anthropic-ratelimit-unified-7d_oi-reset", "1700500000000"),
            ]),
            StatusCode::OK,
            true,
            now,
        );
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].utilization, Some(0.0));
        assert_eq!(parsed[1].utilization, Some(0.24));
        assert_eq!(parsed[2].utilization, Some(0.41));
        assert_eq!(
            parsed[2].fable_entitlement_evidence,
            Some(ClaudeFableEntitlementEvidence::SuccessfulFableRequest)
        );
    }

    #[test]
    fn clamps_small_overshoot_and_rejects_invalid_utilization() {
        let now = 1_700_000_000_000;
        let parsed = parse_claude_quota_headers(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-utilization", "1.02"),
                ("anthropic-ratelimit-unified-7d-utilization", "1.5"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "NaN"),
            ]),
            StatusCode::OK,
            false,
            now,
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tier_name, CLAUDE_FIVE_HOUR_QUOTA_TIER);
        assert_eq!(parsed[0].utilization, Some(1.0));

        for invalid in ["-0.1", "inf", "-inf"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                "anthropic-ratelimit-unified-5h-utilization",
                HeaderValue::from_str(invalid).unwrap(),
            );
            assert!(parse_claude_quota_headers(&headers, StatusCode::OK, false, now).is_empty());
        }
    }

    #[test]
    fn rejected_window_without_utilization_is_full_and_fable_429_is_direct_evidence() {
        let now = 1_700_000_000_000;
        let parsed = parse_claude_quota_headers(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-status", "allowed"),
                ("anthropic-ratelimit-unified-7d-status", "allowed_warning"),
                ("anthropic-ratelimit-unified-7d_oi-status", "rejected"),
                ("anthropic-ratelimit-unified-7d_oi-reset", "3600"),
            ]),
            StatusCode::TOO_MANY_REQUESTS,
            true,
            now,
        );
        let fable = parsed
            .iter()
            .find(|item| item.tier_name == CLAUDE_FABLE_QUOTA_TIER)
            .unwrap();
        assert_eq!(fable.utilization, Some(1.0));
        assert_eq!(
            fable.fable_entitlement_evidence,
            Some(ClaudeFableEntitlementEvidence::FableOnlyRateLimit)
        );
    }

    #[test]
    fn reset_parser_accepts_supported_formats_and_rejects_bad_horizons() {
        let now = 1_700_000_000_000;
        for value in [
            "3600",
            "1700003600",
            "1700003600000",
            "2023-11-14T23:13:20Z",
            "Tue, 14 Nov 2023 23:13:20 GMT",
        ] {
            assert_eq!(
                parse_anthropic_reset_value(value, now, 8 * 24 * 60 * 60 * 1000),
                Some(1_700_003_600_000),
                "{value}"
            );
        }
        for value in ["", "0", "-1", "NaN", "not-a-date", "1701000000"] {
            assert_eq!(
                parse_anthropic_reset_value(value, now, 8 * 24 * 60 * 60 * 1000),
                None,
                "{value}"
            );
        }
    }
}
