use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use serde_json::{json, Value};
use std::time::Duration;

use crate::cursor_client_contract::cursor_membership_label;
use crate::domain::accounts::store::{AccountQuota, AccountQuotaTier};

const CURSOR_DASHBOARD_ORIGIN: &str = "https://api2.cursor.sh";
const MAX_CURSOR_DASHBOARD_BODY_BYTES: usize = 1024 * 1024;
const MAX_CURSOR_DASHBOARD_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorDashboardError {
    pub status_code: Option<u16>,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub message: String,
}

impl std::fmt::Display for CursorDashboardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CursorDashboardError {}

#[derive(Debug, Clone, Default)]
pub struct CursorDashboardSnapshot {
    pub stripe_profile: Option<Value>,
    pub plan_info: Option<Value>,
    pub current_period_usage: Option<Value>,
    pub errors: Vec<CursorDashboardError>,
}

impl CursorDashboardSnapshot {
    pub fn has_data(&self) -> bool {
        self.stripe_profile.is_some()
            || self.plan_info.is_some()
            || self.current_period_usage.is_some()
    }

    pub fn subscription_level(&self) -> Option<String> {
        let stripe = self.stripe_profile.as_ref();
        let team = stripe
            .and_then(|value| value.get("isTeamMember"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let raw = if team {
            first_string(
                stripe,
                &[
                    "/teamMembershipType",
                    "/membershipType",
                    "/individualMembershipType",
                ],
            )
        } else {
            first_string(
                stripe,
                &[
                    "/individualMembershipType",
                    "/membershipType",
                    "/teamMembershipType",
                ],
            )
        }
        .or_else(|| {
            first_string(
                self.plan_info.as_ref(),
                &["/planInfo/planName", "/planName"],
            )
        })
        .or_else(|| {
            first_string(
                self.current_period_usage.as_ref(),
                &["/membershipType", "/planName"],
            )
        });
        raw.as_deref().and_then(cursor_membership_label)
    }

    pub fn safe_profile(&self) -> Value {
        json!({
            "source": "cursor_dashboard_api",
            "subscriptionLevel": self.subscription_level(),
            "stripeProfile": self.stripe_profile.as_ref().map(safe_stripe_profile),
            "planInfo": self.plan_info.as_ref().map(safe_plan_info),
            "currentPeriodUsage": self.current_period_usage.as_ref().map(safe_period_usage),
        })
    }

    pub fn account_quota(&self, now_ms: i64) -> Option<AccountQuota> {
        let subscription = self.subscription_level();
        let usage = self.current_period_usage.as_ref();
        if usage.is_none() && subscription.is_none() {
            return None;
        }
        let tiers = usage
            .and_then(|usage| usage.get("planUsage").or_else(|| usage.get("plan_usage")))
            .map(|plan_usage| {
                let limit = number_at(plan_usage, &["limit"]).unwrap_or(0.0);
                let used =
                    number_at(plan_usage, &["used", "totalSpend", "total_spend"]).or_else(|| {
                        number_at(plan_usage, &["remaining"])
                            .map(|remaining| (limit - remaining).max(0.0))
                    });
                let utilization =
                    number_at(plan_usage, &["totalPercentUsed", "total_percent_used"])
                        .map(|value| (value / 100.0).clamp(0.0, 1.0))
                        .or_else(|| {
                            (limit > 0.0)
                                .then(|| used.map(|used| (used / limit).clamp(0.0, 1.0)))
                                .flatten()
                        });
                AccountQuotaTier {
                    name: if limit > 0.0 {
                        "cursor_credits".to_string()
                    } else {
                        "cursor_included_usage".to_string()
                    },
                    label: None,
                    utilization,
                    used: (limit > 0.0)
                        .then(|| used.map(|value| value / 100.0))
                        .flatten(),
                    limit: (limit > 0.0).then_some(limit / 100.0),
                    unit: (limit > 0.0).then(|| "USD".to_string()),
                    resets_at: usage.and_then(cursor_billing_cycle_end_ms),
                    ..Default::default()
                }
            })
            .into_iter()
            .collect();
        Some(AccountQuota {
            success: true,
            credential_message: subscription,
            tiers,
            extra_usage: Some(json!({
                "source": "cursor_dashboard_api",
                "queriedAt": now_ms,
                "dashboard": self.safe_profile(),
                "partial": self.current_period_usage.is_none() || !self.errors.is_empty(),
            })),
        })
    }
}

fn number_at(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
    })
}

fn cursor_billing_cycle_end_ms(value: &Value) -> Option<i64> {
    let value = value
        .get("billingCycleEnd")
        .or_else(|| value.get("billing_cycle_end"))?;
    if let Some(number) = value.as_i64() {
        return Some(if number.abs() < 10_000_000_000 {
            number.saturating_mul(1_000)
        } else {
            number
        });
    }
    let value = value.as_str()?.trim();
    value
        .parse::<i64>()
        .ok()
        .map(|number| {
            if number.abs() < 10_000_000_000 {
                number.saturating_mul(1_000)
            } else {
                number
            }
        })
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|value| value.timestamp_millis())
        })
}

pub async fn fetch_cursor_dashboard_snapshot(
    client: &reqwest::Client,
    access_token: &str,
    timeout: Duration,
) -> CursorDashboardSnapshot {
    fetch_cursor_dashboard_snapshot_at_origin(
        client,
        access_token,
        CURSOR_DASHBOARD_ORIGIN,
        timeout,
    )
    .await
}

pub(crate) async fn fetch_cursor_dashboard_snapshot_at_origin(
    client: &reqwest::Client,
    access_token: &str,
    origin: &str,
    timeout: Duration,
) -> CursorDashboardSnapshot {
    let stripe = dashboard_json(
        client,
        access_token,
        origin,
        reqwest::Method::GET,
        "/auth/full_stripe_profile",
        timeout,
    );
    let plan = dashboard_json(
        client,
        access_token,
        origin,
        reqwest::Method::POST,
        "/aiserver.v1.DashboardService/GetPlanInfo",
        timeout,
    );
    let usage = dashboard_json(
        client,
        access_token,
        origin,
        reqwest::Method::POST,
        "/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
        timeout,
    );
    let (stripe, plan, usage) = tokio::join!(stripe, plan, usage);
    let mut snapshot = CursorDashboardSnapshot::default();
    match stripe {
        Ok(value) => snapshot.stripe_profile = Some(value),
        Err(error) => snapshot.errors.push(error),
    }
    match plan {
        Ok(value) => snapshot.plan_info = Some(value),
        Err(error) => snapshot.errors.push(error),
    }
    match usage {
        Ok(value) => snapshot.current_period_usage = Some(value),
        Err(error) => snapshot.errors.push(error),
    }
    snapshot
}

async fn dashboard_json(
    client: &reqwest::Client,
    access_token: &str,
    origin: &str,
    method: reqwest::Method,
    path: &str,
    timeout: Duration,
) -> Result<Value, CursorDashboardError> {
    let mut request = client
        .request(
            method.clone(),
            format!("{}{path}", origin.trim_end_matches('/')),
        )
        .timeout(timeout)
        .header(AUTHORIZATION, format!("Bearer {}", access_token.trim()))
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json");
    if method == reqwest::Method::POST {
        request = request.body("{}");
    }
    let response = request.send().await.map_err(|error| CursorDashboardError {
        status_code: None,
        retryable: true,
        retry_after_ms: None,
        message: format!("Cursor dashboard request failed: {error}"),
    })?;
    let status = response.status();
    let retry_after_ms = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after_ms);
    let body = read_limited(response).await?;
    if !status.is_success() {
        let detail = safe_error_detail(&body).map(|detail| {
            crate::logging::redact_sensitive_text_with_values(&detail, [access_token])
        });
        return Err(CursorDashboardError {
            status_code: Some(status.as_u16()),
            retryable: status.as_u16() == 429 || status.is_server_error(),
            retry_after_ms,
            message: match detail {
                Some(detail) => format!(
                    "Cursor dashboard returned HTTP {}: {detail}",
                    status.as_u16()
                ),
                None => format!("Cursor dashboard returned HTTP {}", status.as_u16()),
            },
        });
    }
    serde_json::from_slice(&body).map_err(|error| CursorDashboardError {
        status_code: Some(status.as_u16()),
        retryable: false,
        retry_after_ms: None,
        message: format!("Cursor dashboard returned invalid JSON: {error}"),
    })
}

async fn read_limited(response: reqwest::Response) -> Result<Vec<u8>, CursorDashboardError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CURSOR_DASHBOARD_BODY_BYTES as u64)
    {
        return Err(body_too_large());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CursorDashboardError {
            status_code: None,
            retryable: true,
            retry_after_ms: None,
            message: format!("Cursor dashboard response read failed: {error}"),
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_CURSOR_DASHBOARD_BODY_BYTES {
            return Err(body_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn body_too_large() -> CursorDashboardError {
    CursorDashboardError {
        status_code: Some(502),
        retryable: false,
        retry_after_ms: None,
        message: "Cursor dashboard response exceeded the 1 MiB limit".to_string(),
    }
}

fn parse_retry_after_ms(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| seconds.saturating_mul(1000))
}

fn safe_error_detail(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    first_string(
        Some(&value),
        &["/error/message", "/message", "/error", "/code"],
    )
    .map(|value| {
        value
            .chars()
            .take(MAX_CURSOR_DASHBOARD_ERROR_CHARS)
            .collect()
    })
}

fn first_string(value: Option<&Value>, pointers: &[&str]) -> Option<String> {
    let value = value?;
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn safe_stripe_profile(value: &Value) -> Value {
    json!({
        "membershipType": value.get("membershipType"),
        "isTeamMember": value.get("isTeamMember"),
        "teamMembershipType": value.get("teamMembershipType"),
        "individualMembershipType": value.get("individualMembershipType"),
    })
}

fn safe_plan_info(value: &Value) -> Value {
    let plan = value.get("planInfo").unwrap_or(value);
    json!({
        "planName": plan.get("planName"),
        "includedAmountCents": plan.get("includedAmountCents"),
        "price": plan.get("price"),
        "billingCycleEnd": plan.get("billingCycleEnd"),
    })
}

fn safe_period_usage(value: &Value) -> Value {
    json!({
        "billingCycleStart": value.get("billingCycleStart"),
        "billingCycleEnd": value.get("billingCycleEnd"),
        "planUsage": value.get("planUsage"),
        "spendLimitUsage": value.get("spendLimitUsage"),
        "displayThreshold": value.get("displayThreshold"),
        "displayMessage": value.get("displayMessage"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        routing::{get, post},
        Json, Router,
    };

    #[test]
    fn subscription_prefers_team_membership_for_team_accounts() {
        let snapshot = CursorDashboardSnapshot {
            stripe_profile: Some(json!({
                "isTeamMember": true,
                "membershipType": "pro",
                "teamMembershipType": "enterprise"
            })),
            plan_info: Some(json!({"planInfo":{"planName":"fallback"}})),
            ..Default::default()
        };
        assert_eq!(
            snapshot.subscription_level().as_deref(),
            Some("Cursor enterprise")
        );
    }

    #[test]
    fn quota_converts_cursor_cent_values_and_period_end() {
        let snapshot = CursorDashboardSnapshot {
            stripe_profile: Some(json!({"membershipType":"pro_plus"})),
            current_period_usage: Some(json!({
                "billingCycleEnd": 1_800_000_000,
                "planUsage": {"limit": 2000, "totalSpend": 550, "totalPercentUsed": 27.5}
            })),
            ..Default::default()
        };
        let quota = snapshot.account_quota(1_000).unwrap();
        assert_eq!(quota.credential_message.as_deref(), Some("Cursor Pro+"));
        assert_eq!(quota.tiers[0].used, Some(5.5));
        assert_eq!(quota.tiers[0].limit, Some(20.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.275));
        assert_eq!(quota.tiers[0].resets_at, Some(1_800_000_000_000));
    }

    #[tokio::test]
    async fn fetches_partial_dashboard_snapshot_without_discarding_success() {
        let app = Router::new()
            .route(
                "/auth/full_stripe_profile",
                get(|| async { Json(json!({"membershipType":"pro_plus"})) }),
            )
            .route(
                "/aiserver.v1.DashboardService/GetPlanInfo",
                post(|| async { Json(json!({"planInfo":{"planName":"Pro+"}})) }),
            )
            .route(
                "/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
                post(|| async { axum::http::StatusCode::BAD_GATEWAY }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let snapshot = fetch_cursor_dashboard_snapshot_at_origin(
            &reqwest::Client::new(),
            "secret-token",
            &origin,
            Duration::from_secs(2),
        )
        .await;
        assert!(snapshot.has_data());
        assert_eq!(
            snapshot.subscription_level().as_deref(),
            Some("Cursor Pro+")
        );
        assert_eq!(snapshot.errors.len(), 1);
        assert!(snapshot.current_period_usage.is_none());
        server.abort();
    }
}
