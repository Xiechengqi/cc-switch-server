//! Reviewed WorkBuddy CN account-maintenance protocol. Never used by inference.
use super::*;
use crate::domain::accounts::checkin::{unsupported_reason, CheckinStatus};

const STATUS_PATH: &str = "/v2/billing/meter/checkin-activity-status";
const OLD_STATUS_PATH: &str = "/v2/billing/meter/checkin-status";
const SUBMIT_PATH: &str = "/v2/billing/meter/daily-checkin";

#[derive(Debug, Clone)]
pub(crate) struct CheckinFailure {
    pub status: CheckinStatus,
    // Fixed diagnostic keys only: upstream bodies may contain account secrets.
    pub message: &'static str,
    pub retry_after_ms: Option<i64>,
}

impl CheckinFailure {
    pub(crate) fn new(status: CheckinStatus, message: &'static str) -> Self {
        Self {
            status,
            message,
            retry_after_ms: None,
        }
    }
}

fn uncertain(submit: bool, message: &'static str) -> CheckinFailure {
    CheckinFailure::new(
        if submit {
            CheckinStatus::Unknown
        } else {
            CheckinStatus::RetryableError
        },
        message,
    )
}

fn retry_after(value: Option<&str>, now: i64) -> Option<i64> {
    let value = value?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.min(i64::MAX as u64 / 1_000) as i64 * 1_000);
    }
    let time = httpdate::parse_http_date(value)
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(
        (time.as_millis().min(i64::MAX as u128) as i64)
            .saturating_sub(now)
            .max(0),
    )
}

fn classify(http: StatusCode, body: &[u8], submit: bool) -> Result<Value, CheckinFailure> {
    if matches!(http.as_u16(), 401 | 403) {
        return Err(CheckinFailure::new(
            CheckinStatus::AuthenticationRequired,
            "authentication_required",
        ));
    }
    if http == StatusCode::TOO_MANY_REQUESTS {
        return Err(CheckinFailure::new(
            CheckinStatus::RetryableError,
            "rate_limited",
        ));
    }
    if http.is_server_error() || http == StatusCode::REQUEST_TIMEOUT {
        return Err(uncertain(submit, "upstream_unavailable"));
    }
    let value: Value =
        serde_json::from_slice(body).map_err(|_| uncertain(submit, "invalid_response"))?;
    let code = value
        .get("code")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or_else(|| uncertain(submit, "missing_business_code"))?;
    // Explicit authentication signals only; activity expiry is not token expiry.
    let message = string_at(&value, &["/message", "/msg"])
        .unwrap_or_default()
        .to_lowercase();
    if matches!(code, 401 | 403)
        || message.contains("token expired")
        || message.contains("token已过期")
        || message.contains("token失效")
        || message.contains("请重新登录")
    {
        return Err(CheckinFailure::new(
            CheckinStatus::AuthenticationRequired,
            "authentication_required",
        ));
    }
    if (http.is_success() || http == StatusCode::BAD_REQUEST)
        && (message.contains("已签到")
            || message.contains("already checked in")
            || message.contains("already signed in"))
    {
        return Err(CheckinFailure::new(
            CheckinStatus::Already,
            "already_checked_in",
        ));
    }
    if message.contains("签到活动未开启")
        || message.contains("签到活动已过期")
        || message.contains("签到活动未开放")
        || message.contains("check-in activity is inactive")
        || message.contains("checkin activity is inactive")
    {
        return Err(CheckinFailure::new(
            CheckinStatus::Inactive,
            "activity_inactive",
        ));
    }
    if !http.is_success() || !matches!(code, 0 | 200) {
        return Err(CheckinFailure::new(
            CheckinStatus::Error,
            "upstream_rejected",
        ));
    }
    Ok(value.get("data").cloned().unwrap_or(Value::Null))
}

async fn request(
    http: &reqwest::Client,
    account: &Account,
    path: &str,
    submit: bool,
) -> Result<(StatusCode, Vec<u8>, Option<i64>), CheckinFailure> {
    if unsupported_reason(account).is_some() {
        return Err(CheckinFailure::new(
            CheckinStatus::Error,
            "unsupported_account",
        ));
    }
    let profile =
        CodeBuddyAccountProfile::parse_for(account.provider_type, account.profile.as_ref())
            .map_err(|_| CheckinFailure::new(CheckinStatus::Error, "invalid_account"))?;
    let token = profile_access_token(account).map_err(|_| {
        CheckinFailure::new(
            CheckinStatus::AuthenticationRequired,
            "authentication_required",
        )
    })?;
    // Check-in has its own reviewed origin; never use an account-supplied URL.
    #[allow(unused_mut)]
    let mut origin = Url::parse("https://www.codebuddy.cn").expect("constant origin");
    #[cfg(test)]
    if let Some(url) = account
        .raw
        .as_ref()
        .and_then(|r| r.get("testWorkBuddyCheckinBaseUrl"))
        .and_then(Value::as_str)
    {
        let test_url = Url::parse(url)
            .map_err(|_| CheckinFailure::new(CheckinStatus::Error, "invalid_test_origin"))?;
        if test_url.scheme() != "http"
            || !test_url
                .host_str()
                .is_some_and(|h| h == "127.0.0.1" || h == "[::1]")
        {
            return Err(CheckinFailure::new(
                CheckinStatus::Error,
                "invalid_test_origin",
            ));
        }
        origin = test_url;
    }
    let identity = buddy_product_policy(account.provider_type, profile.site)
        .map_err(|_| CheckinFailure::new(CheckinStatus::Error, "unsupported_account"))?
        .client_identity;
    let builder = http
        .post(origin.join(path).expect("constant path"))
        .header("Origin", "https://www.codebuddy.cn")
        .header("Referer", "https://www.codebuddy.cn/")
        .json(&json!({}));
    let mut response = authenticated_request(
        identity,
        builder,
        token,
        &profile.uid,
        None,
        &profile.domain,
    )
    .timeout(REQUEST_TIMEOUT)
    .send()
    .await
    .map_err(|_| uncertain(submit, "request_failed"))?;
    let status = response.status();
    let delay = retry_after(
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok()),
        crate::infra::time::now_ms() as i64,
    );
    let body =
        crate::infra::http::read_response_body_limited(&mut response, MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|_| {
                let mut error = uncertain(submit, "response_unavailable");
                error.retry_after_ms = delay;
                error
            })?;
    Ok((status, body.to_vec(), delay))
}

pub(crate) async fn query_checkin_status(
    http: &reqwest::Client,
    account: &Account,
) -> Result<bool, CheckinFailure> {
    let mut response = request(http, account, STATUS_PATH, false).await?;
    // Only an HTTP route miss justifies falling back to the older route.
    if response.0 == StatusCode::NOT_FOUND {
        response = request(http, account, OLD_STATUS_PATH, false).await?;
    }
    let data = classify(response.0, &response.1, false).map_err(|mut e| {
        e.retry_after_ms = response.2;
        e
    })?;
    let a = data.get("today_checked_in");
    let b = data.get("todayCheckedIn");
    if a.is_some_and(|v| !v.is_boolean())
        || b.is_some_and(|v| !v.is_boolean())
        || a.zip(b).is_some_and(|(a, b)| a != b)
    {
        return Err(uncertain(false, "invalid_today_status"));
    }
    a.or(b)
        .and_then(Value::as_bool)
        .ok_or_else(|| uncertain(false, "missing_today_status"))
}

pub(crate) async fn submit_checkin(
    http: &reqwest::Client,
    account: &Account,
) -> Result<CheckinStatus, CheckinFailure> {
    let (status, body, delay) = request(http, account, SUBMIT_PATH, true).await?;
    classify(status, &body, true)
        .map(|_| CheckinStatus::Success)
        .map_err(|mut e| {
            e.retry_after_ms = delay;
            e
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkin_strict_envelope_and_business_results() {
        assert_eq!(
            classify(StatusCode::OK, br#"{}"#, true).unwrap_err().status,
            CheckinStatus::Unknown
        );
        assert_eq!(
            classify(
                StatusCode::BAD_REQUEST,
                "{\"code\":400,\"message\":\"今天已签到，请明天再来\"}".as_bytes(),
                true
            )
            .unwrap_err()
            .status,
            CheckinStatus::Already
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                "{\"code\":400,\"message\":\"签到活动已过期\"}".as_bytes(),
                true
            )
            .unwrap_err()
            .status,
            CheckinStatus::Inactive
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                br#"{"code":401,"message":"already checked in"}"#,
                true
            )
            .unwrap_err()
            .status,
            CheckinStatus::AuthenticationRequired
        );
        assert_eq!(
            classify(StatusCode::BAD_GATEWAY, br#"{"code":0}"#, true)
                .unwrap_err()
                .status,
            CheckinStatus::Unknown
        );
        assert_eq!(
            classify(StatusCode::TOO_MANY_REQUESTS, b"", true)
                .unwrap_err()
                .status,
            CheckinStatus::RetryableError
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                br#"{"code":400,"message":"repeat payment"}"#,
                true
            )
            .unwrap_err()
            .status,
            CheckinStatus::Error
        );
    }

    #[test]
    fn checkin_retry_after_seconds_and_http_date() {
        assert_eq!(retry_after(Some("120"), 0), Some(120_000));
        assert_eq!(
            retry_after(Some("Thu, 01 Jan 1970 00:02:00 GMT"), 1_000),
            Some(119_000)
        );
        assert_eq!(retry_after(Some("nonsense"), 0), None);
    }
}
