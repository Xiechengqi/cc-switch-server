use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use rand::RngCore;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde_json::{json, Map, Value};

pub(crate) const CHATGPT_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
pub(crate) const CHATGPT_RESET_CREDIT_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const DETAILS_TIMEOUT: Duration = Duration::from_secs(5);
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResetCreditDetailsError {
    Timeout,
    RequestFailed,
    UpstreamHttp(u16),
    InvalidJson,
    MissingFields,
}

impl ResetCreditDetailsError {
    pub(crate) fn code(&self) -> String {
        match self {
            Self::Timeout => "timeout".to_string(),
            Self::RequestFailed => "request_failed".to_string(),
            Self::UpstreamHttp(status) => format!("upstream_http_{status}"),
            Self::InvalidJson => "invalid_json".to_string(),
            Self::MissingFields => "missing_fields".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedResetCreditDetails {
    pub(crate) explicit_count: Option<i64>,
    pub(crate) credit_list_present: bool,
    pub(crate) credits: Vec<Value>,
    can_derive_count: bool,
    details_complete: bool,
}

impl ParsedResetCreditDetails {
    fn has_signal(&self) -> bool {
        self.explicit_count.is_some() || self.credit_list_present
    }

    fn derived_available_count(&self) -> i64 {
        self.credits
            .iter()
            .filter(|credit| credit_status(credit) == "available")
            .count() as i64
    }
}

pub(crate) async fn fetch_reset_credit_details(
    http: &reqwest::Client,
    access_token: &str,
    workspace_id: Option<&str>,
    request_timeout: Duration,
) -> Result<ParsedResetCreditDetails, ResetCreditDetailsError> {
    fetch_reset_credit_details_from_url(
        http,
        CHATGPT_RESET_CREDITS_URL,
        access_token,
        workspace_id,
        request_timeout,
    )
    .await
}

async fn fetch_reset_credit_details_from_url(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    workspace_id: Option<&str>,
    request_timeout: Duration,
) -> Result<ParsedResetCreditDetails, ResetCreditDetailsError> {
    let timeout = request_timeout.min(DETAILS_TIMEOUT);
    let response = codex_authenticated_get(http, url, access_token, workspace_id, timeout)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                ResetCreditDetailsError::Timeout
            } else {
                ResetCreditDetailsError::RequestFailed
            }
        })?;
    if !response.status().is_success() {
        return Err(ResetCreditDetailsError::UpstreamHttp(
            response.status().as_u16(),
        ));
    }
    let body = response.bytes().await.map_err(|error| {
        if error.is_timeout() {
            ResetCreditDetailsError::Timeout
        } else {
            ResetCreditDetailsError::RequestFailed
        }
    })?;
    let value =
        serde_json::from_slice::<Value>(&body).map_err(|_| ResetCreditDetailsError::InvalidJson)?;
    let parsed = parse_reset_credit_details(&value)?;
    parsed
        .has_signal()
        .then_some(parsed)
        .ok_or(ResetCreditDetailsError::MissingFields)
}

pub(crate) fn codex_authenticated_get(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    workspace_id: Option<&str>,
    request_timeout: Duration,
) -> reqwest::RequestBuilder {
    let mut identity_headers = vec![("user-agent", crate::codex_identity::default_user_agent())];
    crate::codex_identity::finalize_headers(&mut identity_headers);
    let mut request = http
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json")
        .timeout(request_timeout);
    for (name, value) in identity_headers {
        request = request.header(name, value);
    }
    if let Some(workspace_id) = workspace_id {
        request = request.header("ChatGPT-Account-Id", workspace_id);
    }
    request
}

pub(crate) fn codex_authenticated_post(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    workspace_id: Option<&str>,
    body: Value,
    request_timeout: Duration,
) -> reqwest::RequestBuilder {
    let mut identity_headers = vec![("user-agent", crate::codex_identity::default_user_agent())];
    crate::codex_identity::finalize_headers(&mut identity_headers);
    let mut request = http
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json")
        .header("content-type", "application/json")
        .json(&body)
        .timeout(request_timeout);
    for (name, value) in identity_headers {
        request = request.header(name, value);
    }
    if let Some(workspace_id) = workspace_id {
        request = request.header("ChatGPT-Account-Id", workspace_id);
    }
    request
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BankedResetActionError {
    Timeout,
    RequestFailed,
    UpstreamHttp(u16, String),
    InvalidJson,
}

impl BankedResetActionError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Timeout => "upstream request timed out".to_string(),
            Self::RequestFailed => "upstream request failed".to_string(),
            Self::UpstreamHttp(status, body) => format!("upstream returned {status}: {body}"),
            Self::InvalidJson => "upstream returned invalid json".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConsumeResetCreditResult {
    pub(crate) code: Option<String>,
    pub(crate) credit_id: String,
    pub(crate) redeem_request_id: String,
    pub(crate) windows_reset: Option<i64>,
    pub(crate) available_count: Option<i64>,
    pub(crate) remaining_credits: Vec<Value>,
}

fn generate_redeem_request_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

async fn request_upstream_json(
    request: reqwest::RequestBuilder,
) -> Result<Value, BankedResetActionError> {
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            BankedResetActionError::Timeout
        } else {
            BankedResetActionError::RequestFailed
        }
    })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|error| {
        if error.is_timeout() {
            BankedResetActionError::Timeout
        } else {
            BankedResetActionError::RequestFailed
        }
    })?;
    if body.is_empty() {
        return if status.is_success() {
            Ok(Value::Object(Map::new()))
        } else {
            Err(BankedResetActionError::UpstreamHttp(
                status.as_u16(),
                status.canonical_reason().unwrap_or("error").to_string(),
            ))
        };
    }
    let value =
        serde_json::from_slice::<Value>(&body).map_err(|_| BankedResetActionError::InvalidJson)?;
    if status.is_success() {
        Ok(value)
    } else {
        let message = value
            .get("detail")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_string());
        Err(BankedResetActionError::UpstreamHttp(
            status.as_u16(),
            if message.is_empty() {
                status.canonical_reason().unwrap_or("error").to_string()
            } else {
                message
            },
        ))
    }
}

pub(crate) async fn consume_reset_credit(
    http: &reqwest::Client,
    access_token: &str,
    workspace_id: Option<&str>,
    credit_id: &str,
    request_timeout: Duration,
) -> Result<ConsumeResetCreditResult, BankedResetActionError> {
    let credit_id = credit_id.trim().to_string();
    let redeem_request_id = generate_redeem_request_id();
    let mut payload = Map::new();
    payload.insert(
        "redeem_request_id".to_string(),
        Value::String(redeem_request_id.clone()),
    );
    if !credit_id.is_empty() {
        payload.insert("credit_id".to_string(), Value::String(credit_id.clone()));
    }
    let timeout = request_timeout.min(ACTION_TIMEOUT);
    let request = codex_authenticated_post(
        http,
        CHATGPT_RESET_CREDIT_CONSUME_URL,
        access_token,
        workspace_id,
        Value::Object(payload),
        timeout,
    );
    let raw = request_upstream_json(request).await?;
    let available_count = raw
        .get("available_count")
        .or_else(|| raw.get("availableCount"))
        .and_then(nonnegative_integer);
    let remaining_credits = raw
        .get("credits")
        .or_else(|| raw.get("remainingCredits"))
        .or_else(|| raw.get("remaining_credits"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(ConsumeResetCreditResult {
        code: raw.get("code").and_then(Value::as_str).map(str::to_string),
        credit_id,
        redeem_request_id,
        windows_reset: raw
            .get("windows_reset")
            .or_else(|| raw.get("windowsReset"))
            .and_then(nonnegative_integer),
        available_count,
        remaining_credits,
    })
}

pub(crate) fn parse_usage_available_count(usage: &Value) -> Option<i64> {
    let summary = usage
        .get("rate_limit_reset_credits")
        .or_else(|| usage.get("rateLimitResetCredits"))?;
    parse_reset_credit_details(summary)
        .ok()
        .and_then(|parsed| parsed.explicit_count)
}

pub(crate) fn parse_reset_credit_details(
    value: &Value,
) -> Result<ParsedResetCreditDetails, ResetCreditDetailsError> {
    let envelope =
        locate_reset_credit_envelope(value, 0).ok_or(ResetCreditDetailsError::MissingFields)?;
    let items = envelope.items.map(Vec::as_slice).unwrap_or_default();
    let normalized = items
        .iter()
        .enumerate()
        .map(|(index, value)| normalize_credit(value, index))
        .collect::<Vec<_>>();
    let details_complete = envelope.list_present
        && normalized
            .iter()
            .all(|credit| credit.as_ref().is_some_and(|credit| credit.complete));
    let credits = normalized
        .into_iter()
        .flatten()
        .map(|credit| credit.value)
        .collect::<Vec<_>>();
    let can_derive_count = details_complete
        && credits
            .iter()
            .all(|credit| credit_status(credit) != "unknown");
    Ok(ParsedResetCreditDetails {
        explicit_count: envelope.explicit_count,
        credit_list_present: envelope.list_present,
        credits,
        can_derive_count,
        details_complete,
    })
}

#[derive(Debug)]
struct LocatedEnvelope<'a> {
    explicit_count: Option<i64>,
    list_present: bool,
    items: Option<&'a Vec<Value>>,
}

fn locate_reset_credit_envelope(value: &Value, depth: usize) -> Option<LocatedEnvelope<'_>> {
    if depth > 3 {
        return None;
    }
    if let Some(items) = value.as_array() {
        return Some(LocatedEnvelope {
            explicit_count: None,
            list_present: true,
            items: Some(items),
        });
    }
    let object = value.as_object()?;
    let explicit_count = ["available_count", "availableCount", "available"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(nonnegative_integer));

    for key in ["credits", "remainingCredits", "remaining_credits", "items"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            return Some(LocatedEnvelope {
                explicit_count,
                list_present: true,
                items: Some(items),
            });
        }
    }

    for key in ["rate_limit_reset_credits", "rateLimitResetCredits", "data"] {
        let Some(nested) = object.get(key) else {
            continue;
        };
        if nested.is_null() {
            continue;
        }
        if let Some(mut located) = locate_reset_credit_envelope(nested, depth + 1) {
            if explicit_count.is_some() {
                located.explicit_count = explicit_count;
            }
            return Some(located);
        }
    }

    (explicit_count.is_some()
        || ["credits", "remainingCredits", "remaining_credits", "items"]
            .iter()
            .any(|key| object.contains_key(*key)))
    .then_some(LocatedEnvelope {
        explicit_count,
        list_present: false,
        items: None,
    })
}

fn nonnegative_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().filter(|value| *value >= 0),
        Value::String(value) => value.trim().parse::<i64>().ok().filter(|value| *value >= 0),
        _ => None,
    }
}

struct NormalizedCredit {
    value: Value,
    complete: bool,
}

fn normalize_credit(value: &Value, index: usize) -> Option<NormalizedCredit> {
    let object = value.as_object()?;
    let id = string_field(object, &["id", "credit_id", "creditId"])
        .unwrap_or_else(|| format!("credit-{}", index + 1));
    let raw_reset_type = string_field(object, &["reset_type", "resetType", "type"])
        .map(|value| value.to_ascii_lowercase());
    if raw_reset_type
        .as_deref()
        .is_some_and(|reset_type| !matches!(reset_type, "codex_rate_limits" | "unknown"))
    {
        return None;
    }
    let reset_type = raw_reset_type.as_deref().unwrap_or("unknown");
    let status = match string_field(object, &["status", "state"])
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "available" => "available",
        "redeeming" => "redeeming",
        "redeemed" | "used" | "consumed" | "expired" => "redeemed",
        _ => "unknown",
    };
    let (granted_at, granted_at_valid) =
        normalize_optional_timestamp_field(object, &["granted_at", "grantedAt", "created_at"]);
    let (expires_at, expires_at_valid) = normalize_optional_timestamp_field(
        object,
        &["expires_at", "expiresAt", "expire_at", "expireAt"],
    );
    let title = string_field(object, &["title"]);
    let description = string_field(object, &["description"]);
    Some(NormalizedCredit {
        value: json!({
            "id": id,
            "resetType": reset_type,
            "status": status,
            "grantedAt": granted_at,
            "expiresAt": expires_at,
            "title": title,
            "description": description,
        }),
        // Unknown statuses cannot safely participate in an available-count
        // derivation, and malformed timestamps must not replace the last
        // complete per-credit deadline snapshot.
        complete: status != "unknown" && granted_at_valid && expires_at_valid,
    })
}

fn value_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn normalize_optional_timestamp_field(
    object: &Map<String, Value>,
    keys: &[&str],
) -> (Option<String>, bool) {
    let Some(value) = value_field(object, keys) else {
        return (None, true);
    };
    if value.is_null() {
        return (None, true);
    }
    let normalized = normalize_timestamp(value);
    let valid = normalized.is_some();
    (normalized, valid)
}

fn normalize_timestamp(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => number.as_i64().and_then(timestamp_number_to_rfc3339),
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            if let Ok(number) = value.parse::<i64>() {
                return timestamp_number_to_rfc3339(number);
            }
            DateTime::parse_from_rfc3339(value).ok().map(|timestamp| {
                timestamp
                    .with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true)
            })
        }
        _ => None,
    }
}

fn timestamp_number_to_rfc3339(value: i64) -> Option<String> {
    let millis = if value.unsigned_abs() < 10_000_000_000 {
        value.checked_mul(1000)?
    } else {
        value
    };
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(crate) fn merge_reset_credit_snapshot(
    usage_available_count: Option<i64>,
    details: Result<ParsedResetCreditDetails, ResetCreditDetailsError>,
    previous: Option<&Value>,
    workspace_id: Option<&str>,
    now_ms: i64,
) -> Value {
    let previous = previous.and_then(|value| previous_details(value, workspace_id));
    let mut details_error = None;
    let mut details_available = false;
    let mut details_stale = false;
    let mut details_source = "unavailable";
    let mut details_fetched_at = None;
    let mut credits = Vec::new();

    let (available_count, count_source) = match details {
        Ok(parsed) => {
            let count = parsed
                .explicit_count
                .map(|count| (Some(count), "details"))
                .or_else(|| {
                    parsed
                        .can_derive_count
                        .then(|| (Some(parsed.derived_available_count()), "details_derived"))
                })
                .unwrap_or_else(|| {
                    usage_available_count
                        .map(|count| (Some(count), "usage"))
                        .unwrap_or((None, "unknown"))
                });
            if parsed.credit_list_present && parsed.details_complete {
                credits = parsed.credits;
                details_available = true;
                details_source = "details";
                details_fetched_at = Some(now_ms);
            } else {
                details_error = Some(
                    if parsed.credit_list_present {
                        "partial_or_unknown_items"
                    } else {
                        "missing_credit_list"
                    }
                    .to_string(),
                );
                if let Some(previous) = previous.as_ref() {
                    credits = previous.credits.clone();
                    details_fetched_at = previous.details_fetched_at;
                    details_stale = previous.had_details;
                    details_source = if previous.had_details {
                        "cache"
                    } else {
                        "unavailable"
                    };
                }
            }
            count
        }
        Err(error) => {
            details_error = Some(error.code());
            if let Some(previous) = previous.as_ref() {
                credits = previous.credits.clone();
                details_fetched_at = previous.details_fetched_at;
                details_stale = previous.had_details;
                details_source = if previous.had_details {
                    "cache"
                } else {
                    "unavailable"
                };
            }
            usage_available_count
                .map(|count| (Some(count), "usage"))
                .unwrap_or((None, "unknown"))
        }
    };

    if available_count == Some(0) && !details_available {
        credits.retain(|credit| credit_status(credit) != "available");
    }

    let count_fetched_at = available_count.map(|_| now_ms);
    let next_expires_at = next_available_expiry(&credits);
    // Reaching this merge means the Codex account supports reset-credit probing.
    // Missing detail fields are represented as unknown, not as a disabled feature.
    let enabled = true;
    let source = if details_available {
        "upstream"
    } else if count_source == "usage" {
        "usage"
    } else if details_stale {
        "cache"
    } else {
        "unknown"
    };
    json!({
        "enabled": enabled,
        "workspaceId": workspace_id,
        "availableCount": available_count,
        "credits": credits,
        "nextExpiresAt": next_expires_at,
        "countSource": count_source,
        "detailsSource": details_source,
        "countFetchedAt": count_fetched_at,
        "detailsFetchedAt": details_fetched_at,
        "detailsAvailable": details_available,
        "detailsStale": details_stale,
        "detailsError": details_error,
        "source": source,
        "queriedAt": now_ms,
    })
}

#[derive(Debug)]
struct PreviousDetails {
    credits: Vec<Value>,
    details_fetched_at: Option<i64>,
    had_details: bool,
}

fn previous_details(value: &Value, workspace_id: Option<&str>) -> Option<PreviousDetails> {
    let previous_workspace = value
        .get("workspaceId")
        .or_else(|| value.get("workspace_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if previous_workspace != workspace_id {
        return None;
    }
    let parsed = parse_reset_credit_details(value).ok()?;
    if !parsed.details_complete {
        return None;
    }
    let details_fetched_at = value
        .get("detailsFetchedAt")
        .or_else(|| value.get("details_fetched_at"))
        .and_then(metadata_timestamp_to_millis);
    let had_details = parsed.credit_list_present
        && (details_fetched_at.is_some()
            || value
                .get("detailsAvailable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || value
                .get("detailsStale")
                .and_then(Value::as_bool)
                .unwrap_or(false));
    Some(PreviousDetails {
        credits: parsed.credits,
        details_fetched_at,
        had_details,
    })
}

fn timestamp_to_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            let value = number.as_i64()?;
            if value.unsigned_abs() < 10_000_000_000 {
                value.checked_mul(1000)
            } else {
                Some(value)
            }
        }
        Value::String(value) => {
            if let Ok(number) = value.trim().parse::<i64>() {
                return if number.unsigned_abs() < 10_000_000_000 {
                    number.checked_mul(1000)
                } else {
                    Some(number)
                };
            }
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|timestamp| timestamp.timestamp_millis())
        }
        _ => None,
    }
}

fn metadata_timestamp_to_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok().or_else(|| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|timestamp| timestamp.timestamp_millis())
        }),
        _ => None,
    }
}

fn credit_status(credit: &Value) -> &str {
    credit
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn next_available_expiry(credits: &[Value]) -> Option<String> {
    credits
        .iter()
        .filter(|credit| credit_status(credit) == "available")
        .filter_map(|credit| {
            let value = credit.get("expiresAt")?;
            let millis = timestamp_to_millis(value)?;
            let text = normalize_timestamp(value)?;
            Some((millis, text))
        })
        .min_by_key(|(millis, _)| *millis)
        .map(|(_, text)| text)
}

pub(crate) fn normalize_imported_snapshot(source: &Value) -> Value {
    let parsed = parse_reset_credit_details(source).unwrap_or(ParsedResetCreditDetails {
        explicit_count: None,
        credit_list_present: false,
        credits: Vec::new(),
        can_derive_count: false,
        details_complete: false,
    });
    let available_count = parsed.explicit_count.or_else(|| {
        parsed
            .can_derive_count
            .then(|| parsed.derived_available_count())
    });
    let fetched_at = source
        .get("detailsFetchedAt")
        .or_else(|| source.get("queriedAt"))
        .and_then(metadata_timestamp_to_millis);
    let workspace_id = source
        .get("workspaceId")
        .or_else(|| source.get("workspace_id"))
        .and_then(Value::as_str);
    let next_expires_at = next_available_expiry(&parsed.credits);
    let details_error = (parsed.credit_list_present && !parsed.can_derive_count)
        .then_some("partial_or_unknown_items");
    json!({
        "enabled": available_count.is_some() || parsed.credit_list_present,
        "workspaceId": workspace_id,
        "availableCount": available_count,
        "credits": parsed.credits,
        "nextExpiresAt": next_expires_at,
        "countSource": if parsed.explicit_count.is_some() { "imported_snapshot" } else if parsed.can_derive_count { "imported_derived" } else { "unknown" },
        "detailsSource": "imported_snapshot",
        "countFetchedAt": fetched_at,
        "detailsFetchedAt": fetched_at,
        "detailsAvailable": false,
        "detailsStale": parsed.credit_list_present,
        "detailsError": details_error,
        "source": "imported_snapshot",
        "queriedAt": fetched_at,
    })
}

pub(crate) fn empty_snapshot(workspace_id: Option<&str>) -> Value {
    json!({
        "enabled": false,
        "workspaceId": workspace_id,
        "availableCount": Value::Null,
        "credits": [],
        "nextExpiresAt": Value::Null,
        "countSource": "unknown",
        "detailsSource": "unavailable",
        "countFetchedAt": Value::Null,
        "detailsFetchedAt": Value::Null,
        "detailsAvailable": false,
        "detailsStale": false,
        "detailsError": Value::Null,
        "source": "unknown",
        "queriedAt": Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_one_http_response(
        status: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = socket.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/reset-credits"), task)
    }

    #[test]
    fn authenticated_requests_pair_identity_and_scope_workspace() {
        let request = codex_authenticated_get(
            &reqwest::Client::new(),
            "https://example.test/wham/usage",
            "secret-token",
            Some("workspace-a"),
            Duration::from_secs(10),
        )
        .build()
        .unwrap();
        let user_agent = request.headers()["user-agent"].to_str().unwrap();
        let originator = request.headers()["originator"].to_str().unwrap();

        assert_eq!(user_agent.split('/').next(), Some(originator));
        assert!(request.headers().contains_key("version"));
        assert_eq!(request.headers()["chatgpt-account-id"], "workspace-a");
    }

    #[tokio::test]
    async fn details_fetch_uses_scoped_identity_headers_and_parses_success() {
        let (url, request_task) = serve_one_http_response(
            "200 OK",
            r#"{"available_count":1,"credits":[{"id":"credit-1","reset_type":"codex_rate_limits","status":"available","granted_at":"2026-07-01T00:00:00Z","expires_at":"2026-08-01T00:00:00Z"}]}"#,
        )
        .await;
        let details = fetch_reset_credit_details_from_url(
            &reqwest::Client::new(),
            &url,
            "test-token",
            Some("workspace-a"),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        let request = request_task.await.unwrap().to_ascii_lowercase();

        assert_eq!(details.explicit_count, Some(1));
        assert_eq!(details.credits.len(), 1);
        assert!(request.starts_with("get /reset-credits http/1.1\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
        assert!(request.contains("chatgpt-account-id: workspace-a\r\n"));
        assert!(request.contains("originator: "));
        assert!(request.contains("version: "));
    }

    #[tokio::test]
    async fn details_fetch_reduces_upstream_errors_to_stable_codes() {
        let (url, request_task) = serve_one_http_response(
            "404 Not Found",
            r#"{"detail":"secret reflected body must not escape"}"#,
        )
        .await;
        let error = fetch_reset_credit_details_from_url(
            &reqwest::Client::new(),
            &url,
            "test-token",
            Some("workspace-a"),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        let _ = request_task.await.unwrap();

        assert_eq!(error, ResetCreditDetailsError::UpstreamHttp(404));
        assert_eq!(error.code(), "upstream_http_404");
        assert!(!error.code().contains("secret"));
    }

    #[test]
    fn parses_official_and_compatibility_shapes_without_losing_explicit_count() {
        let official = parse_reset_credit_details(&json!({
            "available_count": 5,
            "credits": [
                {
                    "id": "credit-1",
                    "reset_type": "codex_rate_limits",
                    "status": "available",
                    "granted_at": "2026-07-01T00:00:00Z",
                    "expires_at": "2026-08-01T00:00:00Z",
                    "title": "Full reset",
                    "description": "Ready"
                },
                {
                    "id": "credit-2",
                    "reset_type": "codex_rate_limits",
                    "status": "redeemed",
                    "granted_at": 1_783_209_600,
                    "expires_at": 1_785_888_000_000_i64
                }
            ]
        }))
        .unwrap();
        assert_eq!(official.explicit_count, Some(5));
        assert!(official.credit_list_present);
        assert_eq!(official.credits.len(), 2);
        assert_eq!(official.credits[0]["status"], "available");
        assert_eq!(official.credits[0]["grantedAt"], "2026-07-01T00:00:00.000Z");

        let compatibility = parse_reset_credit_details(&json!({
            "data": {
                "availableCount": "2",
                "items": [{
                    "creditId": "credit-camel",
                    "resetType": "codex_rate_limits",
                    "status": "redeeming",
                    "grantedAt": "1783209600",
                    "expiresAt": "1785888000000"
                }]
            }
        }))
        .unwrap();
        assert_eq!(compatibility.explicit_count, Some(2));
        assert_eq!(compatibility.credits[0]["status"], "redeeming");

        let array = parse_reset_credit_details(&json!([{
            "id": "array-credit",
            "status": "available"
        }]))
        .unwrap();
        assert!(array.credit_list_present);
        assert_eq!(array.explicit_count, None);

        let legacy = parse_reset_credit_details(&json!({
            "remaining_credits": [{"id": "legacy-credit", "status": "available"}]
        }))
        .unwrap();
        assert!(legacy.credit_list_present);
        assert_eq!(legacy.derived_available_count(), 1);
    }

    #[test]
    fn count_parser_preserves_zero_unknown_and_rejects_invalid_values() {
        assert_eq!(
            parse_usage_available_count(&json!({
                "rate_limit_reset_credits": {"available_count": 0}
            })),
            Some(0)
        );
        assert_eq!(parse_usage_available_count(&json!({})), None);
        for value in [json!(-1), json!(1.5), json!("-1"), json!("1.5")] {
            assert_eq!(
                parse_usage_available_count(&json!({
                    "rate_limit_reset_credits": {"available_count": value}
                })),
                None
            );
        }
        let empty = parse_reset_credit_details(&json!({"credits": []})).unwrap();
        assert_eq!(empty.derived_available_count(), 0);
        assert!(empty.credit_list_present);
        assert!(parse_reset_credit_details(&json!({})).is_err());

        let mixed_types = parse_reset_credit_details(&json!({
            "credits": [
                {"id": "codex", "reset_type": "codex_rate_limits", "status": "available"},
                {"id": "other", "reset_type": "another_product", "status": "available"},
                {"id": "legacy", "status": "unknown_new_status"}
            ]
        }))
        .unwrap();
        assert_eq!(mixed_types.credits.len(), 2);
        assert_eq!(mixed_types.derived_available_count(), 1);
        assert!(!mixed_types.can_derive_count);
        assert_eq!(mixed_types.credits[1]["resetType"], "unknown");
        assert_eq!(mixed_types.credits[1]["status"], "unknown");
    }

    #[test]
    fn malformed_or_unknown_items_do_not_turn_schema_drift_into_zero() {
        for payload in [
            json!({"credits": [null]}),
            json!({"credits": [{"id": "new", "status": "new_status"}]}),
        ] {
            let parsed = parse_reset_credit_details(&payload).unwrap();
            assert!(!parsed.can_derive_count);

            let fallback = merge_reset_credit_snapshot(
                Some(7),
                Ok(parsed.clone()),
                None,
                Some("workspace-a"),
                1_000,
            );
            assert_eq!(fallback["availableCount"], 7);
            assert_eq!(fallback["countSource"], "usage");
            assert_eq!(fallback["detailsAvailable"], false);
            assert_eq!(fallback["detailsStale"], false);
            assert!(fallback["credits"].as_array().unwrap().is_empty());
            assert_eq!(fallback["detailsError"], "partial_or_unknown_items");

            let unknown =
                merge_reset_credit_snapshot(None, Ok(parsed), None, Some("workspace-a"), 2_000);
            assert!(unknown["availableCount"].is_null());
            assert_eq!(unknown["countSource"], "unknown");
        }
    }

    #[test]
    fn merge_uses_documented_count_precedence_and_keeps_truncated_details() {
        let details = parse_reset_credit_details(&json!({
            "available_count": 5,
            "credits": [
                {"id": "one", "status": "available", "expires_at": "2026-08-02T00:00:00Z"},
                {"id": "two", "status": "available", "expires_at": "2026-08-01T00:00:00Z"}
            ]
        }))
        .unwrap();
        let merged =
            merge_reset_credit_snapshot(Some(9), Ok(details), None, Some("workspace-a"), 1000);
        assert_eq!(merged["availableCount"], 5);
        assert_eq!(merged["countSource"], "details");
        assert_eq!(merged["credits"].as_array().unwrap().len(), 2);
        assert_eq!(merged["nextExpiresAt"], "2026-08-01T00:00:00.000Z");

        let derived = merge_reset_credit_snapshot(
            Some(9),
            Ok(parse_reset_credit_details(&json!({"credits": []})).unwrap()),
            None,
            Some("workspace-a"),
            2000,
        );
        assert_eq!(derived["availableCount"], 0);
        assert_eq!(derived["countSource"], "details_derived");

        let unknown = merge_reset_credit_snapshot(
            None,
            Err(ResetCreditDetailsError::MissingFields),
            None,
            Some("workspace-a"),
            3000,
        );
        assert!(unknown["availableCount"].is_null());
        assert_eq!(unknown["countSource"], "unknown");
        assert_eq!(unknown["enabled"], true);
    }

    #[test]
    fn detail_failure_keeps_only_same_workspace_cache_and_zero_clears_available_rows() {
        let previous = merge_reset_credit_snapshot(
            Some(2),
            Ok(parse_reset_credit_details(&json!({
                "available_count": 2,
                "credits": [
                    {"id": "available", "status": "available", "expires_at": "2026-08-01T00:00:00Z"},
                    {"id": "redeemed", "status": "redeemed", "expires_at": "2026-07-01T00:00:00Z"}
                ]
            }))
            .unwrap()),
            None,
            Some("workspace-a"),
            1000,
        );

        let stale = merge_reset_credit_snapshot(
            Some(2),
            Err(ResetCreditDetailsError::UpstreamHttp(404)),
            Some(&previous),
            Some("workspace-a"),
            2000,
        );
        assert_eq!(stale["availableCount"], 2);
        assert_eq!(stale["detailsStale"], true);
        assert_eq!(stale["detailsAvailable"], false);
        assert_eq!(stale["detailsError"], "upstream_http_404");
        assert_eq!(stale["detailsFetchedAt"], 1000);
        assert_eq!(stale["credits"].as_array().unwrap().len(), 2);

        let cleared = merge_reset_credit_snapshot(
            Some(0),
            Err(ResetCreditDetailsError::Timeout),
            Some(&previous),
            Some("workspace-a"),
            3000,
        );
        assert_eq!(cleared["availableCount"], 0);
        assert_eq!(cleared["credits"].as_array().unwrap().len(), 1);
        assert_eq!(cleared["credits"][0]["status"], "redeemed");

        let other_workspace = merge_reset_credit_snapshot(
            Some(1),
            Err(ResetCreditDetailsError::Timeout),
            Some(&previous),
            Some("workspace-b"),
            4000,
        );
        assert!(other_workspace["credits"].as_array().unwrap().is_empty());
        assert_eq!(other_workspace["detailsStale"], false);
    }

    #[test]
    fn incomplete_details_preserve_last_complete_deadlines_as_stale() {
        let previous = merge_reset_credit_snapshot(
            Some(1),
            Ok(parse_reset_credit_details(&json!({
                "available_count": 1,
                "credits": [{
                    "id": "last-good",
                    "status": "available",
                    "granted_at": "2026-07-01T00:00:00Z",
                    "expires_at": "2026-08-01T00:00:00Z"
                }]
            }))
            .unwrap()),
            None,
            Some("workspace-a"),
            1_000,
        );

        for partial in [
            json!({"credits": [null]}),
            json!({"credits": [{"id": "unknown", "status": "new_status"}]}),
            json!({"credits": [{
                "id": "bad-time",
                "status": "available",
                "expires_at": "not-a-timestamp"
            }]}),
        ] {
            let merged = merge_reset_credit_snapshot(
                Some(1),
                Ok(parse_reset_credit_details(&partial).unwrap()),
                Some(&previous),
                Some("workspace-a"),
                2_000,
            );
            assert_eq!(merged["availableCount"], 1);
            assert_eq!(merged["countSource"], "usage");
            assert_eq!(merged["detailsAvailable"], false);
            assert_eq!(merged["detailsStale"], true);
            assert_eq!(merged["detailsSource"], "cache");
            assert_eq!(merged["detailsFetchedAt"], 1_000);
            assert_eq!(merged["credits"][0]["id"], "last-good");
            assert_eq!(
                merged["credits"][0]["expiresAt"],
                "2026-08-01T00:00:00.000Z"
            );
            assert_eq!(merged["detailsError"], "partial_or_unknown_items");
        }
    }

    #[test]
    fn snapshot_reads_do_not_forge_new_fetch_timestamps() {
        let imported = normalize_imported_snapshot(&json!({
            "availableCount": 1,
            "queriedAt": 1234,
            "credits": [{"id": "credit", "status": "available"}]
        }));
        assert_eq!(imported["countFetchedAt"], 1234);
        assert_eq!(imported["detailsFetchedAt"], 1234);
        assert_eq!(imported["queriedAt"], 1234);
    }
}
