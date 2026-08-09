use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::Serialize;
use serde_json::{json, Value};

const REFERRAL_BASE_URL: &str = "https://chatgpt.com/backend-api/referrals/invite";
const DEFAULT_PROGRAM_ID: &str = "codex_referral_consumer";
const DEFAULT_ENTRYPOINT: &str = "persistent";
const DEFAULT_TRACKING_PERIOD: &str = "past_90_days";
const DEFAULT_TRACKING_LIMIT: usize = 100;
const MAX_EMAILS: usize = 10;
const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const MAX_SESSION_CLIENTS: usize = 64;
const MAX_TEXT_FIELD_BYTES: usize = 4096;
const MAX_IDENTIFIER_FIELD_BYTES: usize = 512;
const MAX_EMAIL_FIELD_BYTES: usize = 254;
const MAX_URL_FIELD_BYTES: usize = 4096;
const MAX_TIMESTAMP_FIELD_BYTES: usize = 128;
const MAX_REFERRAL_GRANTS: usize = 32;
const MAX_ELIGIBILITY_RULES: usize = 32;
const MAX_TIME_FRAME_RULES: usize = 32;
const REFERRAL_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";
const CLOUDFLARE_CHALLENGE_MARKER: &str =
    "cloudflare managed challenge (html challenge page omitted)";
const NON_JSON_RESPONSE_MARKER: &str = "upstream non-json response body omitted";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralGrant {
    pub(crate) recipient: Option<String>,
    pub(crate) grant_type: Option<String>,
    pub(crate) amount: Option<f64>,
    pub(crate) reward_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralTimeFrameRule {
    pub(crate) invites_sent: Option<i64>,
    pub(crate) invites_total: Option<i64>,
    pub(crate) time_frame: Option<String>,
    pub(crate) rule_type: Option<String>,
    pub(crate) capacity_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralEligibility {
    pub(crate) ok: bool,
    pub(crate) status_code: u16,
    pub(crate) request_id: Option<String>,
    pub(crate) should_show: bool,
    pub(crate) ineligible_reason: Option<String>,
    pub(crate) ineligible_reason_code: Option<String>,
    pub(crate) program_id: String,
    pub(crate) entrypoint: String,
    pub(crate) offer_id: Option<String>,
    pub(crate) grants: Vec<ReferralGrant>,
    pub(crate) remaining_send_capacity: Option<i64>,
    pub(crate) remaining_reward_capacity: Option<i64>,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) rules: Vec<String>,
    pub(crate) time_frame_rules: Vec<ReferralTimeFrameRule>,
    pub(crate) requires_explicit_confirmation: bool,
    pub(crate) upstream_message: Option<String>,
    pub(crate) challenged: bool,
    pub(crate) diagnostic: Option<String>,
}

impl ReferralEligibility {
    pub(crate) fn unauthorized(&self) -> bool {
        self.status_code == 401
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralInviteItem {
    pub(crate) referral_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) invite_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralSendResult {
    pub(crate) ok: bool,
    pub(crate) status_code: u16,
    pub(crate) request_id: Option<String>,
    pub(crate) program_id: String,
    pub(crate) entrypoint: String,
    pub(crate) emails: Vec<String>,
    pub(crate) invites: Vec<ReferralInviteItem>,
    pub(crate) upstream_message: Option<String>,
    pub(crate) failed_emails: Vec<String>,
    pub(crate) challenged: bool,
    pub(crate) diagnostic: Option<String>,
}

impl ReferralSendResult {
    pub(crate) fn unauthorized(&self) -> bool {
        self.status_code == 401
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralTrackingItem {
    pub(crate) referral_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) can_resend: bool,
    pub(crate) invite_url: Option<String>,
    pub(crate) resend_available_at: Option<String>,
    pub(crate) grants: Vec<ReferralGrant>,
    pub(crate) created_at: Option<String>,
    pub(crate) expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferralTracking {
    pub(crate) ok: bool,
    pub(crate) status_code: u16,
    pub(crate) request_id: Option<String>,
    pub(crate) items: Vec<ReferralTrackingItem>,
    pub(crate) cursor: Option<String>,
    pub(crate) upstream_message: Option<String>,
    pub(crate) challenged: bool,
    pub(crate) diagnostic: Option<String>,
}

impl ReferralTracking {
    pub(crate) fn unauthorized(&self) -> bool {
        self.status_code == 401
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReferralError {
    InvalidInput(String),
    ClientUnavailable,
    Timeout,
    RequestFailed,
    ResponseTooLarge,
}

impl ReferralError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::InvalidInput(message) => message.clone(),
            Self::ClientUnavailable => "referral HTTP client is unavailable".to_string(),
            Self::Timeout => "referral upstream request timed out".to_string(),
            Self::RequestFailed => "referral upstream request failed".to_string(),
            Self::ResponseTooLarge => "referral upstream response body is too large".to_string(),
        }
    }
}

#[derive(Clone)]
struct SessionClient {
    client: reqwest::Client,
    touched: u64,
}

#[derive(Default)]
struct SessionClientPool {
    clients: HashMap<String, SessionClient>,
    clock: u64,
}

static SESSION_CLIENTS: OnceLock<Mutex<SessionClientPool>> = OnceLock::new();

fn referral_client(session_key: &str) -> Result<reqwest::Client, ReferralError> {
    let pool = SESSION_CLIENTS.get_or_init(|| Mutex::new(SessionClientPool::default()));
    let mut pool = pool.lock().map_err(|_| ReferralError::ClientUnavailable)?;
    pool.clock = pool.clock.saturating_add(1);
    let touched = pool.clock;
    if let Some(entry) = pool.clients.get_mut(session_key) {
        entry.touched = touched;
        return Ok(entry.client.clone());
    }
    let client = crate::infra::http::outbound_client_builder()
        .map_err(|_| ReferralError::ClientUnavailable)?
        .cookie_store(true)
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(2)
        .tcp_keepalive(Duration::from_secs(60))
        .no_gzip()
        .build()
        .map_err(|_| ReferralError::ClientUnavailable)?;
    if pool.clients.len() >= MAX_SESSION_CLIENTS {
        if let Some(oldest) = pool
            .clients
            .iter()
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(key, _)| key.clone())
        {
            pool.clients.remove(&oldest);
        }
    }
    pool.clients.insert(
        session_key.to_string(),
        SessionClient {
            client: client.clone(),
            touched,
        },
    );
    Ok(client)
}

fn referral_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    access_token: &str,
    workspace_id: &str,
    timeout: Duration,
) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json")
        .header("ChatGPT-Account-Id", workspace_id)
        .header("Oai-Language", "zh-CN")
        .header("Originator", "Codex Desktop")
        .header("User-Agent", REFERRAL_USER_AGENT)
        .timeout(timeout)
}

async fn execute_referral_request(
    request: reqwest::RequestBuilder,
) -> Result<(u16, Option<String>, Vec<u8>), ReferralError> {
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            ReferralError::Timeout
        } else {
            ReferralError::RequestFailed
        }
    })?;
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get("x-oai-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| bounded_string(value, MAX_IDENTIFIER_FIELD_BYTES));
    let body =
        crate::infra::http::read_response_body_limited(&mut response, MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|error| match error {
                crate::infra::http::BoundedResponseBodyError::Request(error)
                    if error.is_timeout() =>
                {
                    ReferralError::Timeout
                }
                crate::infra::http::BoundedResponseBodyError::Request(_) => {
                    ReferralError::RequestFailed
                }
                crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                    ReferralError::ResponseTooLarge
                }
            })?;
    Ok((status, request_id, body.to_vec()))
}

fn is_cloudflare_challenge(status: u16, body: &[u8]) -> bool {
    if !matches!(status, 403 | 429 | 503) {
        return false;
    }
    let trimmed = body
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|index| &body[index..])
        .unwrap_or_default();
    if !trimmed.starts_with(b"<") {
        return false;
    }
    [
        b"cf_chl_opt".as_slice(),
        b"challenge-platform".as_slice(),
        b"Enable JavaScript and cookies to continue".as_slice(),
    ]
    .iter()
    .any(|marker| body.windows(marker.len()).any(|window| window == *marker))
}

fn bounded_string(value: &str, max_bytes: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || max_bytes == 0 {
        return None;
    }
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (end > 0).then(|| value[..end].to_string())
}

fn bounded_text(value: Option<&Value>, max_bytes: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(|value| bounded_string(value, max_bytes))
}

fn text(value: Option<&Value>) -> Option<String> {
    bounded_text(value, MAX_TEXT_FIELD_BYTES)
}

fn identifier_text(value: Option<&Value>) -> Option<String> {
    bounded_text(value, MAX_IDENTIFIER_FIELD_BYTES)
}

fn email_text(value: Option<&Value>) -> Option<String> {
    bounded_text(value, MAX_EMAIL_FIELD_BYTES)
}

fn url_text(value: Option<&Value>) -> Option<String> {
    bounded_text(value, MAX_URL_FIELD_BYTES)
}

fn timestamp_text(value: Option<&Value>) -> Option<String> {
    bounded_text(value, MAX_TIMESTAMP_FIELD_BYTES)
}

fn integer(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
    })
}

fn parse_grants(value: Option<&Value>) -> Vec<ReferralGrant> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .take(MAX_REFERRAL_GRANTS)
        .map(|grant| ReferralGrant {
            recipient: identifier_text(grant.get("recipient")),
            grant_type: identifier_text(grant.get("grant_type").or_else(|| grant.get("grantType"))),
            amount: grant.get("amount").and_then(Value::as_f64),
            reward_id: identifier_text(grant.get("reward_id").or_else(|| grant.get("rewardId"))),
        })
        .collect()
}

fn parse_failure(value: &Value) -> (Option<String>, Vec<String>) {
    let fallback_message = text(value.get("message"));
    let Some(detail) = value.get("detail") else {
        return (fallback_message, Vec::new());
    };
    if let Some(message) = text(Some(detail)) {
        return (Some(message), Vec::new());
    }
    let Some(detail) = detail.as_object() else {
        return (fallback_message, Vec::new());
    };
    let failed = detail
        .get("failed_emails")
        .or_else(|| detail.get("failedEmails"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| email_text(Some(value)))
        .take(MAX_EMAILS)
        .collect();
    (text(detail.get("message")).or(fallback_message), failed)
}

fn diagnostic_for_body(body: &[u8]) -> Option<String> {
    body.iter()
        .any(|byte| !byte.is_ascii_whitespace())
        .then(|| NON_JSON_RESPONSE_MARKER.to_string())
}

fn parse_invite_items(value: &Value, max_items: usize) -> Vec<ReferralInviteItem> {
    value
        .get("invites")
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .take(max_items.min(MAX_EMAILS))
        .map(|item| ReferralInviteItem {
            referral_id: identifier_text(
                item.get("referral_id").or_else(|| item.get("referralId")),
            ),
            email: email_text(item.get("email")),
            invite_url: url_text(item.get("invite_url").or_else(|| item.get("inviteUrl"))),
        })
        .collect()
}

pub(crate) fn normalize_referral_emails(emails: &[String]) -> Result<Vec<String>, ReferralError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for email in emails {
        let email = email.trim();
        if email.is_empty() {
            continue;
        }
        let key = email.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        if email.len() > 254
            || email.chars().any(char::is_whitespace)
            || email.matches('@').count() != 1
            || email.starts_with('@')
            || email.ends_with('@')
            || !email.split_once('@').is_some_and(|(_, domain)| {
                domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
            })
        {
            return Err(ReferralError::InvalidInput(format!(
                "invalid referral email: {email}"
            )));
        }
        normalized.push(email.to_string());
    }
    if normalized.is_empty() {
        return Err(ReferralError::InvalidInput(
            "at least one referral email is required".to_string(),
        ));
    }
    if normalized.len() > MAX_EMAILS {
        return Err(ReferralError::InvalidInput(format!(
            "referral email count exceeds {MAX_EMAILS}"
        )));
    }
    Ok(normalized)
}

pub(crate) async fn query_eligibility(
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    request_timeout: Duration,
) -> Result<ReferralEligibility, ReferralError> {
    query_eligibility_from_url(
        REFERRAL_BASE_URL,
        session_key,
        access_token,
        workspace_id,
        request_timeout,
    )
    .await
}

async fn query_eligibility_from_url(
    base_url: &str,
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    request_timeout: Duration,
) -> Result<ReferralEligibility, ReferralError> {
    let client = referral_client(session_key)?;
    let url = format!("{}/eligibility", base_url.trim_end_matches('/'));
    let request = referral_request(
        &client,
        reqwest::Method::GET,
        &url,
        access_token,
        workspace_id,
        request_timeout.min(Duration::from_secs(30)),
    )
    .query(&[
        ("program_id", DEFAULT_PROGRAM_ID),
        ("entrypoint", DEFAULT_ENTRYPOINT),
    ]);
    let (status, request_id, body) = execute_referral_request(request).await?;
    let challenged = is_cloudflare_challenge(status, &body);
    let mut result = ReferralEligibility {
        ok: (200..300).contains(&status),
        status_code: status,
        request_id,
        should_show: false,
        ineligible_reason: None,
        ineligible_reason_code: None,
        program_id: DEFAULT_PROGRAM_ID.to_string(),
        entrypoint: DEFAULT_ENTRYPOINT.to_string(),
        offer_id: None,
        grants: Vec::new(),
        remaining_send_capacity: None,
        remaining_reward_capacity: None,
        title: None,
        description: None,
        rules: Vec::new(),
        time_frame_rules: Vec::new(),
        requires_explicit_confirmation: false,
        upstream_message: None,
        challenged,
        diagnostic: challenged.then(|| CLOUDFLARE_CHALLENGE_MARKER.to_string()),
    };
    if challenged {
        return Ok(result);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        result.diagnostic = diagnostic_for_body(&body);
        return Ok(result);
    };
    if !result.ok {
        result.upstream_message = parse_failure(&value).0;
    }
    result.should_show = value
        .get("should_show")
        .or_else(|| value.get("shouldShow"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    result.ineligible_reason = text(
        value
            .get("ineligible_reason")
            .or_else(|| value.get("ineligibleReason")),
    );
    result.ineligible_reason_code = text(
        value
            .get("ineligible_reason_code")
            .or_else(|| value.get("ineligibleReasonCode")),
    );
    result.program_id = identifier_text(value.get("program_id").or_else(|| value.get("programId")))
        .unwrap_or(result.program_id);
    result.entrypoint = identifier_text(value.get("entrypoint")).unwrap_or(result.entrypoint);
    result.offer_id = identifier_text(value.get("offer_id").or_else(|| value.get("offerId")));
    result.grants = parse_grants(value.get("grants"));
    result.remaining_send_capacity = integer(
        value
            .get("remaining_send_capacity")
            .or_else(|| value.get("remainingSendCapacity")),
    );
    result.remaining_reward_capacity = integer(
        value
            .get("remaining_reward_capacity")
            .or_else(|| value.get("remainingRewardCapacity")),
    );
    result.title = text(value.get("title"));
    result.description = text(value.get("description"));
    result.rules = value
        .get("rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| text(Some(item)))
        .take(MAX_ELIGIBILITY_RULES)
        .collect();
    result.time_frame_rules = value
        .get("time_frame_rules")
        .or_else(|| value.get("timeFrameRules"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .take(MAX_TIME_FRAME_RULES)
        .map(|rule| ReferralTimeFrameRule {
            invites_sent: integer(rule.get("invites_sent").or_else(|| rule.get("invitesSent"))),
            invites_total: integer(
                rule.get("invites_total")
                    .or_else(|| rule.get("invitesTotal")),
            ),
            time_frame: identifier_text(rule.get("time_frame").or_else(|| rule.get("timeFrame"))),
            rule_type: identifier_text(rule.get("type")),
            capacity_type: identifier_text(
                rule.get("capacity_type")
                    .or_else(|| rule.get("capacityType")),
            ),
        })
        .collect();
    result.requires_explicit_confirmation = value
        .get("requires_explicit_confirmation")
        .or_else(|| value.get("requiresExplicitConfirmation"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(result)
}

pub(crate) async fn send_invites(
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    emails: &[String],
    request_timeout: Duration,
) -> Result<ReferralSendResult, ReferralError> {
    send_invites_to_url(
        REFERRAL_BASE_URL,
        session_key,
        access_token,
        workspace_id,
        emails,
        request_timeout,
    )
    .await
}

async fn send_invites_to_url(
    base_url: &str,
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    emails: &[String],
    request_timeout: Duration,
) -> Result<ReferralSendResult, ReferralError> {
    let emails = normalize_referral_emails(emails)?;
    let client = referral_client(session_key)?;
    let request = referral_request(
        &client,
        reqwest::Method::POST,
        base_url,
        access_token,
        workspace_id,
        request_timeout.min(Duration::from_secs(45)),
    )
    .header(CONTENT_TYPE, "application/json")
    .json(&json!({
        "program_id": DEFAULT_PROGRAM_ID,
        "entrypoint": DEFAULT_ENTRYPOINT,
        "emails": emails,
    }));
    let (status, request_id, body) = execute_referral_request(request).await?;
    let challenged = is_cloudflare_challenge(status, &body);
    let mut result = ReferralSendResult {
        ok: (200..300).contains(&status),
        status_code: status,
        request_id,
        program_id: DEFAULT_PROGRAM_ID.to_string(),
        entrypoint: DEFAULT_ENTRYPOINT.to_string(),
        emails,
        invites: Vec::new(),
        upstream_message: None,
        failed_emails: Vec::new(),
        challenged,
        diagnostic: challenged.then(|| CLOUDFLARE_CHALLENGE_MARKER.to_string()),
    };
    if challenged {
        return Ok(result);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        result.diagnostic = diagnostic_for_body(&body);
        return Ok(result);
    };
    if !result.ok {
        (result.upstream_message, result.failed_emails) = parse_failure(&value);
    }
    result.invites = parse_invite_items(&value, result.emails.len());
    Ok(result)
}

pub(crate) async fn query_tracking(
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    limit: usize,
    request_timeout: Duration,
) -> Result<ReferralTracking, ReferralError> {
    query_tracking_from_url(
        REFERRAL_BASE_URL,
        session_key,
        access_token,
        workspace_id,
        limit,
        request_timeout,
    )
    .await
}

async fn query_tracking_from_url(
    base_url: &str,
    session_key: &str,
    access_token: &str,
    workspace_id: &str,
    limit: usize,
    request_timeout: Duration,
) -> Result<ReferralTracking, ReferralError> {
    let limit = if limit == 0 || limit > DEFAULT_TRACKING_LIMIT {
        DEFAULT_TRACKING_LIMIT
    } else {
        limit
    };
    let client = referral_client(session_key)?;
    let url = format!("{}/tracking", base_url.trim_end_matches('/'));
    let request = referral_request(
        &client,
        reqwest::Method::GET,
        &url,
        access_token,
        workspace_id,
        request_timeout.min(Duration::from_secs(30)),
    )
    .query(&[
        ("program_id", DEFAULT_PROGRAM_ID.to_string()),
        ("period", DEFAULT_TRACKING_PERIOD.to_string()),
        ("limit", limit.to_string()),
    ]);
    let (status, request_id, body) = execute_referral_request(request).await?;
    let challenged = is_cloudflare_challenge(status, &body);
    let mut result = ReferralTracking {
        ok: (200..300).contains(&status),
        status_code: status,
        request_id,
        items: Vec::new(),
        cursor: None,
        upstream_message: None,
        challenged,
        diagnostic: challenged.then(|| CLOUDFLARE_CHALLENGE_MARKER.to_string()),
    };
    if challenged {
        return Ok(result);
    }
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        result.diagnostic = diagnostic_for_body(&body);
        return Ok(result);
    };
    if !result.ok {
        result.upstream_message = parse_failure(&value).0;
    }
    result.items = value
        .get("items")
        .or_else(|| value.get("invites"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .take(limit)
        .map(|item| ReferralTrackingItem {
            referral_id: identifier_text(
                item.get("referral_id").or_else(|| item.get("referralId")),
            ),
            email: email_text(item.get("email")),
            status: identifier_text(item.get("status")),
            can_resend: item
                .get("can_resend")
                .or_else(|| item.get("canResend"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            invite_url: url_text(item.get("invite_url").or_else(|| item.get("inviteUrl"))),
            resend_available_at: timestamp_text(
                item.get("resend_available_at")
                    .or_else(|| item.get("resendAvailableAt")),
            ),
            grants: parse_grants(item.get("grants")),
            created_at: timestamp_text(item.get("created_at").or_else(|| item.get("createdAt"))),
            expires_at: timestamp_text(item.get("expires_at").or_else(|| item.get("expiresAt"))),
        })
        .collect();
    result.cursor = text(value.get("cursor"));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::Response;
    use axum::routing::{get, post};
    use axum::Router;
    use tokio::net::TcpListener;

    use super::*;

    async fn test_server(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/backend-api/referrals/invite")
    }

    #[test]
    fn normalizes_and_validates_emails() {
        let emails = normalize_referral_emails(&[
            " A@example.com ".to_string(),
            "a@example.com".to_string(),
            "b@example.org".to_string(),
        ])
        .unwrap();
        assert_eq!(emails, vec!["A@example.com", "b@example.org"]);
        assert!(normalize_referral_emails(&["invalid".to_string()]).is_err());
    }

    #[test]
    fn bounds_upstream_text_collections_and_failure_fallbacks() {
        let oversized = Value::String("é".repeat(MAX_TEXT_FIELD_BYTES));
        let bounded = text(Some(&oversized)).unwrap();
        assert!(bounded.len() <= MAX_TEXT_FIELD_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));

        let grants = Value::Array(
            (0..MAX_REFERRAL_GRANTS + 5)
                .map(|index| json!({"reward_id": format!("reward-{index}")}))
                .collect(),
        );
        assert_eq!(parse_grants(Some(&grants)).len(), MAX_REFERRAL_GRANTS);

        let invite_payload = json!({
            "invites": (0..MAX_EMAILS + 5)
                .map(|index| json!({"email": format!("user-{index}@example.com")}))
                .collect::<Vec<_>>()
        });
        assert_eq!(parse_invite_items(&invite_payload, 2).len(), 2);

        let fallback = parse_failure(&json!({"detail": null, "message": "fallback"}));
        assert_eq!(fallback.0.as_deref(), Some("fallback"));

        let failure = parse_failure(&json!({
            "message": "fallback",
            "detail": {
                "failed_emails": (0..MAX_EMAILS + 5)
                    .map(|index| format!("user-{index}@example.com"))
                    .collect::<Vec<_>>()
            }
        }));
        assert_eq!(failure.0.as_deref(), Some("fallback"));
        assert_eq!(failure.1.len(), MAX_EMAILS);
    }

    #[tokio::test]
    async fn eligibility_parses_dynamic_grants_and_capacities() {
        let router = Router::new().route(
            "/backend-api/referrals/invite/eligibility",
            get(|request: Request| async move {
                assert_eq!(
                    request.headers().get("chatgpt-account-id"),
                    Some(&HeaderValue::from_static("workspace-1"))
                );
                Response::builder()
                    .status(StatusCode::OK)
                    .header("x-oai-request-id", "req-elig")
                    .body(Body::from(
                        r#"{"should_show":true,"offer_id":"credits_1000","grants":[{"recipient":"referrer","grant_type":"personal_credits","amount":1000}],"remaining_send_capacity":9,"remaining_reward_capacity":2,"time_frame_rules":[{"invites_sent":1,"invites_total":3,"capacity_type":"reward"}]}"#,
                    ))
                    .unwrap()
            }),
        );
        let base = test_server(router).await;
        let result = query_eligibility_from_url(
            &base,
            "test-eligibility",
            "access-token",
            "workspace-1",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(result.ok && result.should_show);
        assert_eq!(result.offer_id.as_deref(), Some("credits_1000"));
        assert_eq!(result.grants[0].amount, Some(1000.0));
        assert_eq!(result.remaining_send_capacity, Some(9));
        assert_eq!(result.remaining_reward_capacity, Some(2));
        assert_eq!(result.request_id.as_deref(), Some("req-elig"));
    }

    #[tokio::test]
    async fn eligibility_bounds_upstream_copy_and_rule_collections() {
        let body = serde_json::to_vec(&json!({
            "should_show": true,
            "title": "é".repeat(MAX_TEXT_FIELD_BYTES),
            "rules": (0..MAX_ELIGIBILITY_RULES + 5)
                .map(|index| format!("rule-{index}"))
                .collect::<Vec<_>>(),
            "time_frame_rules": (0..MAX_TIME_FRAME_RULES + 5)
                .map(|index| json!({"type": format!("rule-{index}")}))
                .collect::<Vec<_>>()
        }))
        .unwrap();
        let router = Router::new().route(
            "/backend-api/referrals/invite/eligibility",
            get(move || {
                let body = body.clone();
                async move { Body::from(body) }
            }),
        );
        let base = test_server(router).await;
        let result = query_eligibility_from_url(
            &base,
            "test-eligibility-bounds",
            "access-token",
            "workspace-1",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(result.title.unwrap().len() <= MAX_TEXT_FIELD_BYTES);
        assert_eq!(result.rules.len(), MAX_ELIGIBILITY_RULES);
        assert_eq!(result.time_frame_rules.len(), MAX_TIME_FRAME_RULES);
    }

    #[tokio::test]
    async fn send_parses_recipient_rejection_without_leaking_html() {
        let router = Router::new().route(
            "/backend-api/referrals/invite",
            post(|| async {
                (
                    StatusCode::FORBIDDEN,
                    r#"{"detail":{"message":"already invited","failed_emails":["a@example.com"]}}"#,
                )
            }),
        );
        let base = test_server(router).await;
        let result = send_invites_to_url(
            &base,
            "test-send",
            "access-token",
            "workspace-1",
            &["a@example.com".to_string()],
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(!result.ok && !result.challenged);
        assert_eq!(result.upstream_message.as_deref(), Some("already invited"));
        assert_eq!(result.failed_emails, vec!["a@example.com"]);
    }

    #[tokio::test]
    async fn tracking_returns_no_more_than_the_requested_limit() {
        let body = serde_json::to_vec(&json!({
            "items": (0..5)
                .map(|index| json!({"referral_id": format!("referral-{index}")}))
                .collect::<Vec<_>>()
        }))
        .unwrap();
        let router = Router::new().route(
            "/backend-api/referrals/invite/tracking",
            get(move |request: Request| {
                let body = body.clone();
                async move {
                    assert!(request
                        .uri()
                        .query()
                        .is_some_and(|query| query.contains("limit=2")));
                    Body::from(body)
                }
            }),
        );
        let base = test_server(router).await;
        let result = query_tracking_from_url(
            &base,
            "test-tracking-limit",
            "access-token",
            "workspace-1",
            2,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(result.items.len(), 2);
    }

    #[test]
    fn detects_cloudflare_challenge_but_not_business_json() {
        let body = b"<html><script>window._cf_chl_opt={};</script></html>";
        assert!(is_cloudflare_challenge(403, body));
        assert!(!is_cloudflare_challenge(
            403,
            br#"{"detail":"not eligible"}"#
        ));
        assert!(!is_cloudflare_challenge(200, body));
    }

    #[test]
    fn non_json_diagnostic_never_exposes_upstream_body() {
        let diagnostic = diagnostic_for_body(b"<html>secret upstream response</html>").unwrap();
        assert_eq!(diagnostic, NON_JSON_RESPONSE_MARKER);
        assert!(!diagnostic.contains("secret"));
        assert_eq!(diagnostic_for_body(b" \n\t"), None);
    }
}
