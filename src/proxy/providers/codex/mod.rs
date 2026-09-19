use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use rand::RngCore;
use serde_json::Value;

use crate::domain::providers::model::ProviderType;
use crate::state::ServerState;

use super::super::openai_capacity_shed::openai_payload_values;
use super::super::provider_ops::ProviderExecution;
use super::super::response_semantics::SemanticFailure;
use super::super::{bounded_upstream_rate_limit_until, ProxyError};

pub(crate) const MAX_CAPACITY_RETRIES: u32 = 2;

const CAPACITY_RETRY_FIRST_MIN_DELAY_MS: u64 = 500;
const CAPACITY_RETRY_FIRST_MAX_DELAY_MS: u64 = 1_000;
const CAPACITY_RETRY_NEXT_MIN_DELAY_MS: u64 = 1_000;
const CAPACITY_RETRY_NEXT_MAX_DELAY_MS: u64 = 2_000;
const DEFAULT_ACCOUNT_RATE_LIMIT_COOLDOWN_MS: i64 = 60_000;
const DEFAULT_SHARE_MODEL_COOLDOWN_MS: i64 = 5 * 60_000;

pub(crate) async fn handle_rate_limit(
    state: &ServerState,
    execution: &ProviderExecution,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    share_id: Option<&str>,
    model: Option<&str>,
) -> bool {
    if execution.stored.provider_type != ProviderType::CodexOAuth
        || status != StatusCode::TOO_MANY_REQUESTS
    {
        return false;
    }
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    if !account_rate_limit_evidence(headers, body) {
        if let (Some(share_id), Some(model)) = (
            share_id.map(str::trim).filter(|value| !value.is_empty()),
            model.map(str::trim).filter(|value| !value.is_empty()),
        ) {
            state.mark_share_model_cooldown(
                share_id,
                &execution.plan.runtime_fingerprint,
                model,
                now.saturating_add(DEFAULT_SHARE_MODEL_COOLDOWN_MS),
                if model_capacity_error(body) {
                    "model_capacity"
                } else {
                    "rate_limited_model"
                },
                now,
            );
            return true;
        }
    }
    let Some((provider_type, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        return true;
    };
    let Some(until) = rate_limit_until(status, headers, body, now) else {
        return true;
    };
    state
        .mark_account_rate_limited_until_if_current(
            account_id,
            provider_type,
            auth_identity_generation,
            until,
            Some(format!(
                "upstream returned 429; account is rate limited until {until}"
            )),
        )
        .await;
    true
}

pub(crate) fn semantic_rate_limit_error(
    failure: &SemanticFailure,
    headers: &HeaderMap,
    body: &[u8],
) -> ProxyError {
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let until = rate_limit_until(StatusCode::TOO_MANY_REQUESTS, headers, body, now)
        .unwrap_or_else(|| now.saturating_add(DEFAULT_ACCOUNT_RATE_LIMIT_COOLDOWN_MS));
    let retry_after_seconds = u64::try_from(until.saturating_sub(now))
        .unwrap_or(u64::MAX)
        .saturating_add(999)
        / 1_000;
    let message = if failure.message.trim().is_empty() {
        "OpenAI Codex upstream is rate limited".to_string()
    } else {
        failure.message.trim().to_string()
    };
    ProxyError::rate_limited(message, retry_after_seconds.max(1))
}

pub(crate) fn capacity_shed_proxy_error(_failure: &SemanticFailure) -> ProxyError {
    ProxyError::upstream_capacity_shed(1)
}

pub(crate) fn capacity_retry_allowed(
    attempted: u32,
    retry_allowed: bool,
    remaining_ms: u128,
) -> bool {
    attempted < MAX_CAPACITY_RETRIES
        && retry_allowed
        && remaining_ms >= capacity_retry_delay_bounds(attempted).0 as u128
}

pub(crate) fn capacity_retry_delay_bounds(attempted: u32) -> (u64, u64) {
    if attempted == 0 {
        (
            CAPACITY_RETRY_FIRST_MIN_DELAY_MS,
            CAPACITY_RETRY_FIRST_MAX_DELAY_MS,
        )
    } else {
        (
            CAPACITY_RETRY_NEXT_MIN_DELAY_MS,
            CAPACITY_RETRY_NEXT_MAX_DELAY_MS,
        )
    }
}

pub(crate) fn capacity_retry_delay(remaining_ms: u128, attempted: u32) -> Duration {
    let (min_delay_ms, max_delay_ms) = capacity_retry_delay_bounds(attempted);
    if remaining_ms < min_delay_ms as u128 {
        return Duration::from_millis(remaining_ms as u64);
    }
    let max_delay = max_delay_ms.min(remaining_ms as u64);
    let delay_ms = if max_delay <= min_delay_ms {
        max_delay
    } else {
        let span = max_delay - min_delay_ms;
        min_delay_ms + (rand::rngs::OsRng.next_u64() % (span + 1))
    };
    Duration::from_millis(delay_ms)
}

pub(crate) fn rate_limit_until(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    now_ms: i64,
) -> Option<i64> {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    let until = rate_limit_reset_at_ms(body, now_ms)
        .or_else(|| exhausted_window_reset_at_ms(headers, now_ms))
        .or_else(|| super::super::grok::retry_after_until_ms(headers, now_ms))
        .unwrap_or_else(|| now_ms.saturating_add(DEFAULT_ACCOUNT_RATE_LIMIT_COOLDOWN_MS));
    Some(bounded_upstream_rate_limit_until(now_ms, until))
}

pub(crate) fn account_rate_limit_evidence(headers: &HeaderMap, body: &[u8]) -> bool {
    usage_limit_reached(body) || exhausted_window_reset_at_ms(headers, 0).is_some()
}

pub(crate) fn usage_limit_reached(body: &[u8]) -> bool {
    openai_payload_values(body).iter().any(|value| {
        [
            "/error/type",
            "/error/code",
            "/body/error/type",
            "/body/error/code",
            "/response/error/type",
            "/response/error/code",
            "/response/status_details/error/type",
            "/response/status_details/error/code",
        ]
        .into_iter()
        .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .any(|kind| kind.trim().eq_ignore_ascii_case("usage_limit_reached"))
    })
}

fn model_capacity_error(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("selected model is at capacity")
        || text.contains("model is at capacity. please try a different model")
}

pub(crate) fn exhausted_window_reset_at_ms(headers: &HeaderMap, now_ms: i64) -> Option<i64> {
    let mut exhausted = ["primary", "secondary"]
        .into_iter()
        .filter_map(|window| {
            let used = header_decimal(headers, &format!("x-codex-{window}-used-percent"))?;
            let window_minutes =
                header_decimal(headers, &format!("x-codex-{window}-window-minutes"))?;
            if used < 100.0 || window_minutes <= 0.0 {
                return None;
            }
            let reset_seconds =
                header_decimal(headers, &format!("x-codex-{window}-reset-after-seconds"))
                    .filter(|value| *value > 0.0);
            Some((window_minutes, reset_seconds))
        })
        .collect::<Vec<_>>();
    exhausted.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (window_minutes, reset_seconds) = exhausted.pop()?;
    let fallback_ms = if window_minutes >= 1_440.0 {
        7 * 24 * 60 * 60 * 1_000
    } else if window_minutes >= 60.0 {
        5 * 60 * 60 * 1_000
    } else {
        DEFAULT_ACCOUNT_RATE_LIMIT_COOLDOWN_MS
    };
    let reset_ms = reset_seconds
        .map(|seconds| (seconds * 1_000.0).min(i64::MAX as f64) as i64)
        .unwrap_or(fallback_ms);
    Some(bounded_upstream_rate_limit_until(
        now_ms,
        now_ms.saturating_add(reset_ms),
    ))
}

fn header_decimal(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

pub(crate) fn rate_limit_reset_at_ms(body: &[u8], now_ms: i64) -> Option<i64> {
    for value in openai_payload_values(body) {
        let seconds = [
            "/error/resets_in_seconds",
            "/body/error/resets_in_seconds",
            "/response/error/resets_in_seconds",
            "/response/status_details/error/resets_in_seconds",
        ]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_i64));
        if let Some(seconds) = seconds.filter(|seconds| *seconds > 0) {
            return Some(now_ms.saturating_add(seconds.saturating_mul(1_000)));
        }
        let reset_at = [
            "/error/resets_at",
            "/body/error/resets_at",
            "/response/error/resets_at",
            "/response/status_details/error/resets_at",
        ]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_i64))
        .map(|value| {
            if value < 10_000_000_000 {
                value.saturating_mul(1_000)
            } else {
                value
            }
        })
        .filter(|until| *until > now_ms);
        if reset_at.is_some() {
            return reset_at;
        }
    }
    None
}
