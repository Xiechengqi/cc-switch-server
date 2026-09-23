use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;

use crate::domain::accounts::claude_subscription::is_claude_fable_5_model;
use crate::domain::accounts::store::{
    CLAUDE_FABLE_CAPACITY_POOL, CLAUDE_FIVE_HOUR_OBSERVATION_MAX_FUTURE_MS,
    CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
};
use crate::domain::providers::model::ProviderType;
use crate::state::ServerState;

use super::super::bounded_upstream_rate_limit_until;
use super::super::claude_quota_headers::{
    claude_shared_window_explicitly_healthy, header_lower, parse_anthropic_reset_header,
    parse_claude_quota_headers, parse_claude_utilization_header,
};
use super::super::provider_ops::ProviderExecution;
use super::super::router::ProxyRoute;

const DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS: i64 = 60_000;
const DEFAULT_SHARE_MODEL_COOLDOWN_MS: i64 = 5 * 60_000;

pub(crate) async fn record_quota_response_headers(
    state: &ServerState,
    execution: &ProviderExecution,
    route: ProxyRoute,
    status: StatusCode,
    headers: &HeaderMap,
    model: Option<&str>,
) {
    if route != ProxyRoute::ClaudeMessages
        || execution.stored.provider_type != ProviderType::ClaudeOAuth
    {
        return;
    }
    let Some((provider_type, account_id, auth_identity_generation)) =
        execution.managed_account_identity_target()
    else {
        crate::metrics::record_claude_quota_header_observation("unmanaged");
        return;
    };
    if provider_type != ProviderType::ClaudeOAuth {
        return;
    }
    let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let observations = parse_claude_quota_headers(
        headers,
        status,
        model.is_some_and(is_claude_fable_5_model),
        now_ms,
    );
    if observations.is_empty() {
        crate::metrics::record_claude_quota_header_observation("absent");
        return;
    }
    let commit = state
        .record_claude_quota_window_observations_if_current(
            account_id,
            provider_type,
            auth_identity_generation,
            observations,
            now_ms,
        )
        .await;
    crate::metrics::record_claude_quota_header_observation(commit.metric_label());
}

pub(crate) async fn handle_rate_limit(
    state: &ServerState,
    execution: &ProviderExecution,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    share_id: Option<&str>,
    model: Option<&str>,
) -> bool {
    if status != StatusCode::TOO_MANY_REQUESTS
        || execution.stored.provider_type != ProviderType::ClaudeOAuth
    {
        return false;
    }
    let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
    let decision = classify_rate_limit(
        headers,
        body,
        model.is_some_and(is_claude_fable_5_model),
        now,
    );
    crate::metrics::record_claude_rate_limit_scope(
        decision.scope.metric_label(),
        decision.reason,
        decision.evidence.metric_label(),
    );
    apply_rate_limit_decision(state, execution, decision, share_id, model, now).await;
    true
}

async fn apply_rate_limit_decision(
    state: &ServerState,
    execution: &ProviderExecution,
    decision: RateLimitDecision,
    share_id: Option<&str>,
    model: Option<&str>,
    now: i64,
) {
    let identity = execution.managed_account_identity_target();
    match decision.scope {
        RateLimitScope::RequestEntitlement => {}
        RateLimitScope::FablePool => {
            let Some(until) = decision.until else {
                return;
            };
            if let Some((provider_type, account_id, auth_identity_generation)) = identity {
                state
                    .mark_account_capacity_pool_limit_if_current(
                        account_id,
                        provider_type,
                        auth_identity_generation,
                        CLAUDE_FABLE_CAPACITY_POOL,
                        until,
                        now,
                        "Anthropic Fable 7d_oi capacity pool is exhausted",
                        Some(1.0),
                        "anthropic_ratelimit_7d_oi",
                    )
                    .await;
            }
            if let (Some(share_id), Some(model)) = (
                share_id.map(str::trim).filter(|value| !value.is_empty()),
                model.map(str::trim).filter(|value| !value.is_empty()),
            ) {
                state.mark_share_model_cooldown(
                    share_id,
                    &execution.plan.runtime_fingerprint,
                    model,
                    until,
                    decision.reason,
                    now,
                );
            }
        }
        RateLimitScope::AccountSharedWindow => {
            let Some(until) = decision.until else {
                return;
            };
            if let Some((provider_type, account_id, auth_identity_generation)) = identity {
                state
                    .mark_account_rate_limited_until_if_current(
                        account_id,
                        provider_type,
                        auth_identity_generation,
                        until,
                        Some(format!(
                            "Anthropic shared subscription window is rate limited until {until}"
                        )),
                    )
                    .await;
            }
        }
        RateLimitScope::ExactModel => {
            let Some(until) = decision.until else {
                return;
            };
            if let (Some(share_id), Some(model)) = (
                share_id.map(str::trim).filter(|value| !value.is_empty()),
                model.map(str::trim).filter(|value| !value.is_empty()),
            ) {
                state.mark_share_model_cooldown(
                    share_id,
                    &execution.plan.runtime_fingerprint,
                    model,
                    until,
                    decision.reason,
                    now,
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitScope {
    AccountSharedWindow,
    FablePool,
    ExactModel,
    RequestEntitlement,
}

impl RateLimitScope {
    fn metric_label(self) -> &'static str {
        match self {
            Self::AccountSharedWindow => "account_shared_window",
            Self::FablePool => "fable_pool",
            Self::ExactModel => "share_model",
            Self::RequestEntitlement => "request_entitlement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitEvidence {
    Complete,
    Partial,
    Missing,
    Conflicting,
}

impl RateLimitEvidence {
    fn metric_label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Missing => "missing",
            Self::Conflicting => "conflicting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateLimitDecision {
    pub(crate) scope: RateLimitScope,
    pub(crate) reason: &'static str,
    pub(crate) evidence: RateLimitEvidence,
    pub(crate) until: Option<i64>,
}

pub(crate) fn classify_rate_limit(
    headers: &HeaderMap,
    body: &[u8],
    fable_request: bool,
    now: i64,
) -> RateLimitDecision {
    if fast_credit_refusal(body) {
        return RateLimitDecision {
            scope: RateLimitScope::RequestEntitlement,
            reason: "fast_credit_entitlement",
            evidence: RateLimitEvidence::Complete,
            until: None,
        };
    }
    let unified = header_lower(headers, "anthropic-ratelimit-unified-status");
    let status_5h = header_lower(headers, "anthropic-ratelimit-unified-5h-status");
    let status_7d = header_lower(headers, "anthropic-ratelimit-unified-7d-status");
    let status_7d_oi = header_lower(headers, "anthropic-ratelimit-unified-7d_oi-status");
    let overage_status = header_lower(headers, "anthropic-ratelimit-unified-overage-status");
    let overage_disabled_reason = header_lower(
        headers,
        "anthropic-ratelimit-unified-overage-disabled-reason",
    );
    let representative_claim =
        header_lower(headers, "anthropic-ratelimit-unified-representative-claim");
    if overage_disabled_reason.is_some() || organization_entitlement_refusal(body) {
        return RateLimitDecision {
            scope: RateLimitScope::RequestEntitlement,
            reason: "organization_or_overage_entitlement",
            evidence: RateLimitEvidence::Complete,
            until: None,
        };
    }
    let utilization_5h =
        parse_claude_utilization_header(headers, "anthropic-ratelimit-unified-5h-utilization");
    let utilization_7d =
        parse_claude_utilization_header(headers, "anthropic-ratelimit-unified-7d-utilization");
    let rejected_5h = status_5h.as_deref() == Some("rejected");
    let rejected_7d = status_7d.as_deref() == Some("rejected");
    let overage_rejected = status_7d_oi.as_deref() == Some("rejected")
        || overage_status.as_deref() == Some("rejected")
        || representative_claim
            .as_deref()
            .is_some_and(|claim| claim.contains("overage"));
    let conflict_5h = window_evidence_conflicts(status_5h.as_deref(), utilization_5h);
    let conflict_7d = window_evidence_conflicts(status_7d.as_deref(), utilization_7d);
    let retry_after = super::super::grok::retry_after_until_ms(headers, now);
    let overage_or_fable_only_rejection =
        overage_rejected && !rejected_5h && !rejected_7d && !conflict_5h && !conflict_7d;
    let exact_until = || {
        bounded_upstream_rate_limit_until(
            now,
            (!overage_or_fable_only_rejection)
                .then_some(retry_after)
                .flatten()
                .unwrap_or_else(|| now.saturating_add(DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS)),
        )
    };

    if conflict_5h || conflict_7d {
        return RateLimitDecision {
            scope: RateLimitScope::ExactModel,
            reason: "anthropic_conflicting_window_evidence",
            evidence: RateLimitEvidence::Conflicting,
            until: Some(exact_until()),
        };
    }

    if rejected_5h || rejected_7d {
        let reset_5h = rejected_5h
            .then(|| {
                parse_anthropic_reset_header(
                    headers,
                    "anthropic-ratelimit-unified-5h-reset",
                    now,
                    CLAUDE_FIVE_HOUR_OBSERVATION_MAX_FUTURE_MS,
                )
            })
            .flatten();
        let reset_7d = rejected_7d
            .then(|| {
                parse_anthropic_reset_header(
                    headers,
                    "anthropic-ratelimit-unified-7d-reset",
                    now,
                    CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
                )
            })
            .flatten();
        let resets_complete =
            (!rejected_5h || reset_5h.is_some()) && (!rejected_7d || reset_7d.is_some());
        if resets_complete {
            let shared_reset = [reset_5h, reset_7d]
                .into_iter()
                .flatten()
                .max()
                .expect("a rejected shared window has a reset");
            let until = retry_after.map_or(shared_reset, |retry| retry.max(shared_reset));
            return RateLimitDecision {
                scope: RateLimitScope::AccountSharedWindow,
                reason: match (rejected_5h, rejected_7d) {
                    (true, true) => "anthropic_shared_5h_7d_rejected",
                    (true, false) => "anthropic_shared_5h_rejected",
                    (false, true) => "anthropic_shared_7d_rejected",
                    (false, false) => unreachable!(),
                },
                evidence: RateLimitEvidence::Complete,
                until: Some(bounded_upstream_rate_limit_until(now, until)),
            };
        }
        return RateLimitDecision {
            scope: RateLimitScope::ExactModel,
            reason: "anthropic_shared_window_reset_missing",
            evidence: RateLimitEvidence::Partial,
            until: Some(exact_until()),
        };
    }

    let shared_5h_healthy =
        claude_shared_window_explicitly_healthy(status_5h.as_deref(), utilization_5h);
    let shared_7d_healthy =
        claude_shared_window_explicitly_healthy(status_7d.as_deref(), utilization_7d);
    if shared_5h_healthy && shared_7d_healthy {
        if fable_request && status_7d_oi.as_deref() == Some("rejected") {
            let until = parse_anthropic_reset_header(
                headers,
                "anthropic-ratelimit-unified-7d_oi-reset",
                now,
                CLAUDE_WEEKLY_OBSERVATION_MAX_FUTURE_MS,
            )
            .unwrap_or_else(|| now.saturating_add(DEFAULT_SHARE_MODEL_COOLDOWN_MS));
            return RateLimitDecision {
                scope: RateLimitScope::FablePool,
                reason: "anthropic_fable_7d_oi",
                evidence: RateLimitEvidence::Complete,
                until: Some(bounded_upstream_rate_limit_until(now, until)),
            };
        }
        return RateLimitDecision {
            scope: RateLimitScope::ExactModel,
            reason: if overage_rejected {
                "anthropic_overage_model_scope"
            } else if unified.as_deref() == Some("rejected") {
                "anthropic_unified_rejected_shared_healthy"
            } else {
                "anthropic_model_rate_limit"
            },
            evidence: RateLimitEvidence::Complete,
            until: Some(exact_until()),
        };
    }

    let has_partial_evidence = unified.is_some()
        || status_5h.is_some()
        || status_7d.is_some()
        || status_7d_oi.is_some()
        || utilization_5h.is_some()
        || utilization_7d.is_some()
        || overage_status.is_some()
        || representative_claim.is_some();
    RateLimitDecision {
        scope: RateLimitScope::ExactModel,
        reason: if overage_rejected {
            "anthropic_overage_evidence_incomplete"
        } else if retry_after.is_some() {
            "anthropic_model_rate_limit"
        } else {
            "anthropic_unknown_429"
        },
        evidence: if has_partial_evidence {
            RateLimitEvidence::Partial
        } else {
            RateLimitEvidence::Missing
        },
        until: Some(exact_until()),
    }
}

fn window_evidence_conflicts(status: Option<&str>, utilization: Option<f64>) -> bool {
    matches!(status, Some("rejected")) && utilization.is_some_and(|value| value < 1.0)
        || matches!(status, Some("allowed" | "allowed_warning"))
            && utilization.is_some_and(|value| value >= 1.0)
}

fn organization_entitlement_refusal(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let is_entitlement = [
        value.pointer("/error/code").and_then(Value::as_str),
        value.pointer("/error/type").and_then(Value::as_str),
        value.pointer("/error/reason").and_then(Value::as_str),
        value.pointer("/error/message").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::to_ascii_lowercase)
    .any(|value| {
        value.contains("org_spend_cap_reached")
            || value.contains("organization_spend_cap_reached")
            || value.contains("overage_disabled")
    });
    is_entitlement
}

fn fast_credit_refusal(body: &[u8]) -> bool {
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
        .to_ascii_lowercase();
    message.contains("fast request rejected")
        || (message.contains("fast")
            && (message.contains("usage credits") || message.contains("credits are required")))
}

pub(crate) fn transport_replay_safe(
    route: ProxyRoute,
    reason: &str,
    attempt: u32,
    retry_allowed: bool,
) -> bool {
    retry_allowed
        && (route == ProxyRoute::ClaudeCountTokens
            || (route == ProxyRoute::ClaudeMessages && reason == "connect_error" && attempt == 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(values: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn fable_only_rejection_ignores_generic_retry_after() {
        let now = 1_790_121_600_000;
        let decision = classify_rate_limit(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-status", "allowed"),
                ("anthropic-ratelimit-unified-7d-status", "allowed"),
                ("anthropic-ratelimit-unified-7d_oi-status", "rejected"),
                ("retry-after", "3600"),
            ]),
            b"{}",
            true,
            now,
        );
        assert_eq!(decision.scope, RateLimitScope::FablePool);
        assert_eq!(decision.until, Some(now + DEFAULT_SHARE_MODEL_COOLDOWN_MS));
    }

    #[test]
    fn overage_only_rejection_ignores_generic_retry_after() {
        let now = 1_790_121_600_000;
        let decision = classify_rate_limit(
            &headers(&[
                ("anthropic-ratelimit-unified-5h-status", "allowed"),
                ("anthropic-ratelimit-unified-7d-status", "allowed"),
                ("anthropic-ratelimit-unified-overage-status", "rejected"),
                ("retry-after", "3600"),
            ]),
            b"{}",
            false,
            now,
        );
        assert_eq!(decision.scope, RateLimitScope::ExactModel);
        assert_eq!(decision.reason, "anthropic_overage_model_scope");
        assert_eq!(
            decision.until,
            Some(now + DEFAULT_UPSTREAM_RATE_LIMIT_COOLDOWN_MS)
        );
    }

    #[test]
    fn ordinary_model_rejection_still_honors_retry_after() {
        let now = 1_790_121_600_000;
        let decision = classify_rate_limit(&headers(&[("retry-after", "120")]), b"{}", false, now);
        assert_eq!(decision.scope, RateLimitScope::ExactModel);
        assert_eq!(decision.until, Some(now + 120_000));
    }
}
