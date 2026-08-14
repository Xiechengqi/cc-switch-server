use std::fmt;
use std::time::{Duration, SystemTime};

use chrono::DateTime;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER,
};
use reqwest::{Client, Method, StatusCode, Url};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::domain::providers::ollama_cloud::{
    OllamaCloudAccountView, OllamaCloudActivityPeriod, OllamaCloudActivityView,
    OllamaCloudErrorKind, OllamaCloudModelUsage, OllamaCloudUsageView, OllamaCloudUsageWindow,
    OllamaCloudUsageWindowKind, OLLAMA_CLOUD_MAX_MODELS,
};

const OLLAMA_CLOUD_ORIGIN: &str = "https://ollama.com";
const OLLAMA_ME_PATH: &str = "/api/me";
const OLLAMA_USAGE_PATH: &str = "/api/usage";
const OLLAMA_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OLLAMA_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone)]
pub struct OllamaCloudClient {
    http: Client,
    me_endpoint: Url,
    usage_endpoint: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaCloudFetchError {
    pub kind: OllamaCloudErrorKind,
    message: String,
    pub retry_after_ms: Option<u64>,
}

impl OllamaCloudFetchError {
    pub fn public_message(&self) -> &str {
        &self.message
    }

    pub fn permits_stale(&self) -> bool {
        matches!(
            self.kind,
            OllamaCloudErrorKind::RateLimited | OllamaCloudErrorKind::Transient
        )
    }

    fn authentication() -> Self {
        Self::new(
            OllamaCloudErrorKind::Authentication,
            "Ollama Cloud rejected this API key",
            None,
        )
    }

    fn rate_limited(retry_after_ms: Option<u64>) -> Self {
        Self::new(
            OllamaCloudErrorKind::RateLimited,
            "Ollama Cloud account information is rate limited",
            retry_after_ms,
        )
    }

    fn transient() -> Self {
        Self::new(
            OllamaCloudErrorKind::Transient,
            "Ollama Cloud account information is temporarily unavailable",
            None,
        )
    }

    fn invalid_response() -> Self {
        Self::new(
            OllamaCloudErrorKind::InvalidResponse,
            "Ollama Cloud returned an invalid account response",
            None,
        )
    }

    fn new(
        kind: OllamaCloudErrorKind,
        message: impl Into<String>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_ms,
        }
    }
}

impl fmt::Display for OllamaCloudFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OllamaCloudFetchError {}

#[derive(Debug)]
pub struct OllamaCloudFetchResult {
    pub account: Result<OllamaCloudAccountView, OllamaCloudFetchError>,
    pub usage: Result<OllamaCloudUsageView, OllamaCloudFetchError>,
}

impl OllamaCloudClient {
    pub fn official() -> anyhow::Result<Self> {
        Self::for_origin(OLLAMA_CLOUD_ORIGIN)
    }

    fn for_origin(origin: &str) -> anyhow::Result<Self> {
        let origin = Url::parse(origin)?;
        if origin.cannot_be_a_base()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            anyhow::bail!("Ollama Cloud origin must be an origin URL");
        }
        let http = crate::infra::http::outbound_client_builder()?
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(OLLAMA_REQUEST_TIMEOUT)
            .pool_max_idle_per_host(2)
            .tcp_keepalive(Duration::from_secs(60))
            .no_gzip()
            .build()?;
        Ok(Self {
            http,
            me_endpoint: exact_endpoint(&origin, OLLAMA_ME_PATH)?,
            usage_endpoint: exact_endpoint(&origin, OLLAMA_USAGE_PATH)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_origin(origin: &str) -> anyhow::Result<Self> {
        Self::for_origin(origin)
    }

    pub async fn fetch(&self, api_key: &str) -> OllamaCloudFetchResult {
        let authorization = authorization_value(api_key);
        let (account, usage) = match authorization {
            Ok(authorization) => tokio::join!(
                self.fetch_account(authorization.clone()),
                self.fetch_usage(authorization)
            ),
            Err(error) => (Err(error.clone()), Err(error)),
        };
        OllamaCloudFetchResult { account, usage }
    }

    async fn fetch_account(
        &self,
        authorization: HeaderValue,
    ) -> Result<OllamaCloudAccountView, OllamaCloudFetchError> {
        let body = self
            .execute_json(Method::POST, &self.me_endpoint, authorization, true)
            .await?;
        parse_account(&body)
    }

    async fn fetch_usage(
        &self,
        authorization: HeaderValue,
    ) -> Result<OllamaCloudUsageView, OllamaCloudFetchError> {
        let body = self
            .execute_json(Method::GET, &self.usage_endpoint, authorization, false)
            .await?;
        parse_usage(&body)
    }

    async fn execute_json(
        &self,
        method: Method,
        endpoint: &Url,
        authorization: HeaderValue,
        content_type_json: bool,
    ) -> Result<serde_json::Value, OllamaCloudFetchError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(AUTHORIZATION, authorization);
        if content_type_json {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        }
        let mut response = self
            .http
            .request(method, endpoint.clone())
            .headers(headers)
            .timeout(OLLAMA_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| OllamaCloudFetchError::transient())?;
        if response.url() != endpoint {
            return Err(OllamaCloudFetchError::invalid_response());
        }
        match response.status() {
            status if status.is_success() => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(OllamaCloudFetchError::authentication())
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(OllamaCloudFetchError::rate_limited(retry_after_ms(
                    response.headers(),
                )))
            }
            status if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT => {
                return Err(OllamaCloudFetchError::transient())
            }
            _ => return Err(OllamaCloudFetchError::invalid_response()),
        }
        let bytes = crate::infra::http::read_response_body_limited(
            &mut response,
            MAX_OLLAMA_RESPONSE_BYTES,
        )
        .await
        .map_err(|error| {
            if error.is_connect()
                || matches!(
                    &error,
                    crate::infra::http::BoundedResponseBodyError::Request(inner)
                        if inner.is_timeout()
                )
            {
                OllamaCloudFetchError::transient()
            } else {
                OllamaCloudFetchError::invalid_response()
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|_| OllamaCloudFetchError::invalid_response())
    }
}

fn exact_endpoint(origin: &Url, path: &str) -> anyhow::Result<Url> {
    let mut endpoint = origin.clone();
    endpoint.set_path(path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn authorization_value(api_key: &str) -> Result<HeaderValue, OllamaCloudFetchError> {
    let api_key = api_key.trim();
    if api_key.is_empty() || api_key.len() > 4_096 {
        return Err(OllamaCloudFetchError::authentication());
    }
    let bearer = Zeroizing::new(format!("Bearer {api_key}"));
    let mut value =
        HeaderValue::from_str(&bearer).map_err(|_| OllamaCloudFetchError::authentication())?;
    value.set_sensitive(true);
    Ok(value)
}

fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1_000).min(MAX_SAFE_INTEGER));
    }
    let target = httpdate::parse_http_date(value).ok()?;
    Some(
        target
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .as_millis()
            .min(MAX_SAFE_INTEGER as u128) as u64,
    )
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    #[serde(default, alias = "ID", alias = "id")]
    id: Option<serde_json::Value>,
    #[serde(default, alias = "Email", alias = "email")]
    email: Option<serde_json::Value>,
    #[serde(default, alias = "Name", alias = "name")]
    name: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "FirstName",
        alias = "firstName",
        alias = "first_name",
        alias = "firstname"
    )]
    first_name: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "LastName",
        alias = "lastName",
        alias = "last_name",
        alias = "lastname"
    )]
    last_name: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "AvatarURL",
        alias = "avatarURL",
        alias = "avatar_url",
        alias = "avatarurl"
    )]
    avatar_url: Option<serde_json::Value>,
    #[serde(default, alias = "Plan", alias = "plan")]
    plan: Option<serde_json::Value>,
    #[serde(
        default,
        alias = "CreatedAt",
        alias = "createdAt",
        alias = "created_at",
        alias = "createdat"
    )]
    created_at: Option<serde_json::Value>,
}

fn parse_account(
    body: &serde_json::Value,
) -> Result<OllamaCloudAccountView, OllamaCloudFetchError> {
    let raw: RawAccount = serde_json::from_value(body.clone())
        .map_err(|_| OllamaCloudFetchError::invalid_response())?;
    let id = optional_json_string(raw.id, 256)?;
    let email = optional_json_string(raw.email, 320)?;
    let name = optional_json_string(raw.name, 256)?;
    let first_name = optional_json_string(raw.first_name, 256)?;
    let last_name = optional_json_string(raw.last_name, 256)?;
    let avatar_url = optional_json_string(raw.avatar_url, 2_048)?;
    let plan = optional_json_string(raw.plan, 128)?;
    let created_at_ms = optional_json_timestamp(raw.created_at);
    if id.is_none()
        && email.is_none()
        && name.is_none()
        && first_name.is_none()
        && last_name.is_none()
        && plan.is_none()
    {
        return Err(OllamaCloudFetchError::invalid_response());
    }
    Ok(OllamaCloudAccountView {
        id,
        email,
        name,
        first_name,
        last_name,
        avatar_url,
        plan,
        created_at_ms,
    })
}

#[derive(Debug, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    activity: Option<RawActivity>,
    #[serde(default)]
    limits: RawLimits,
}

#[derive(Debug, Default, Deserialize)]
struct RawLimits {
    #[serde(default)]
    session: Option<RawLimit>,
    #[serde(default)]
    weekly: Option<RawLimit>,
}

#[derive(Debug, Deserialize)]
struct RawLimit {
    usage: f64,
    #[serde(default)]
    models: Vec<RawModelUsage>,
}

#[derive(Debug, Deserialize)]
struct RawModelUsage {
    name: String,
    #[serde(alias = "requestCount")]
    request_count: u64,
}

#[derive(Debug, Deserialize)]
struct RawActivity {
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    period: Option<RawPeriod>,
    #[serde(default)]
    models: Vec<RawModelUsage>,
}

#[derive(Debug, Deserialize)]
struct RawPeriod {
    #[serde(rename = "type", alias = "kind")]
    kind: String,
    #[serde(default, alias = "startingAt")]
    starting_at: Option<String>,
    #[serde(default, alias = "endingAt")]
    ending_at: Option<String>,
}

fn parse_usage(body: &serde_json::Value) -> Result<OllamaCloudUsageView, OllamaCloudFetchError> {
    let raw: RawUsage = serde_json::from_value(body.clone())
        .map_err(|_| OllamaCloudFetchError::invalid_response())?;
    let mut limits = Vec::with_capacity(2);
    for (kind, limit) in [
        (OllamaCloudUsageWindowKind::Session, raw.limits.session),
        (OllamaCloudUsageWindowKind::Weekly, raw.limits.weekly),
    ] {
        let Some(limit) = limit else {
            continue;
        };
        if !limit.usage.is_finite() || !(0.0..=1.0).contains(&limit.usage) {
            return Err(OllamaCloudFetchError::invalid_response());
        }
        let (models, models_truncated) = normalize_models(limit.models)?;
        limits.push(OllamaCloudUsageWindow {
            kind,
            utilization: limit.usage * 100.0,
            models,
            models_truncated,
        });
    }
    let activity = raw.activity.map(normalize_activity).transpose()?;
    if limits.is_empty() && activity.is_none() {
        return Err(OllamaCloudFetchError::invalid_response());
    }
    Ok(OllamaCloudUsageView { limits, activity })
}

fn normalize_activity(raw: RawActivity) -> Result<OllamaCloudActivityView, OllamaCloudFetchError> {
    let cost = raw
        .cost
        .map(|value| {
            let value = value.trim();
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'+'))
            {
                return Err(OllamaCloudFetchError::invalid_response());
            }
            Ok(value.to_string())
        })
        .transpose()?;
    let period = raw
        .period
        .map(|period| {
            Ok(OllamaCloudActivityPeriod {
                kind: required_string(Some(period.kind), 64)?,
                starting_at_ms: period
                    .starting_at
                    .map(|value| parse_timestamp(&value))
                    .transpose()?,
                ending_at_ms: period
                    .ending_at
                    .map(|value| parse_timestamp(&value))
                    .transpose()?,
            })
        })
        .transpose()?;
    let (models, models_truncated) = normalize_models(raw.models)?;
    Ok(OllamaCloudActivityView {
        cost,
        period,
        models,
        models_truncated,
    })
}

fn normalize_models(
    raw: Vec<RawModelUsage>,
) -> Result<(Vec<OllamaCloudModelUsage>, bool), OllamaCloudFetchError> {
    let models_truncated = raw.len() > OLLAMA_CLOUD_MAX_MODELS;
    let models = raw
        .into_iter()
        .take(OLLAMA_CLOUD_MAX_MODELS)
        .map(|item| {
            if item.request_count > MAX_SAFE_INTEGER {
                return Err(OllamaCloudFetchError::invalid_response());
            }
            Ok(OllamaCloudModelUsage {
                name: required_string(Some(item.name), 256)?,
                request_count: item.request_count,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((models, models_truncated))
}

fn required_string(value: Option<String>, max_len: usize) -> Result<String, OllamaCloudFetchError> {
    optional_string(value, max_len)?.ok_or_else(OllamaCloudFetchError::invalid_response)
}

fn optional_string(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, OllamaCloudFetchError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_len || value.chars().any(char::is_control) {
        return Err(OllamaCloudFetchError::invalid_response());
    }
    Ok(Some(value.to_string()))
}

fn optional_json_string(
    value: Option<serde_json::Value>,
    max_len: usize,
) -> Result<Option<String>, OllamaCloudFetchError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => optional_string(Some(value), max_len),
        Some(_) => Ok(None),
    }
}

fn optional_json_timestamp(value: Option<serde_json::Value>) -> Option<i64> {
    match value {
        Some(serde_json::Value::String(value)) => parse_timestamp(&value).ok(),
        _ => None,
    }
}

fn parse_timestamp(value: &str) -> Result<i64, OllamaCloudFetchError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|value| value.timestamp_millis())
        .map_err(|_| OllamaCloudFetchError::invalid_response())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::response::Redirect;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;

    struct RecordedRequest {
        method: String,
        authorization: String,
        content_type: String,
        content_length: String,
    }

    #[derive(Clone, Default)]
    struct RequestLog(Arc<Mutex<Vec<RecordedRequest>>>);

    async fn me(
        State(log): State<RequestLog>,
        headers: AxumHeaderMap,
    ) -> (StatusCode, Json<serde_json::Value>) {
        log.0.lock().await.push(RecordedRequest {
            method: "POST".to_string(),
            authorization: headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            content_type: headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            content_length: headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        });
        (
            StatusCode::CREATED,
            Json(json!({
                "ID": "account-1",
                "CreatedAt": "2026-06-21T08:39:00.256543Z",
                "Email": "owner@example.com",
                "Name": "owner",
                "Plan": "free"
            })),
        )
    }

    async fn usage(
        State(log): State<RequestLog>,
        headers: AxumHeaderMap,
    ) -> Json<serde_json::Value> {
        log.0.lock().await.push(RecordedRequest {
            method: "GET".to_string(),
            authorization: headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            content_type: String::new(),
            content_length: String::new(),
        });
        Json(json!({
            "activity": {
                "cost": "0.00000",
                "period": {
                    "type": "last_4_weeks",
                    "starting_at": "2026-07-20T00:00:00Z",
                    "ending_at": "2026-08-14T09:47:44.37299543Z"
                },
                "models": []
            },
            "limits": {
                "session": {"usage": 0, "models": [{"name": "gpt-oss:120b", "request_count": 1}]},
                "weekly": {"usage": 0.25, "models": [{"name": "gpt-oss:120b", "request_count": 6}]}
            }
        }))
    }

    async fn test_server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (format!("http://{addr}"), server)
    }

    #[tokio::test]
    async fn fetch_uses_exact_methods_paths_and_bearer_header() {
        let log = RequestLog::default();
        let router = Router::new()
            .route(OLLAMA_ME_PATH, post(me))
            .route(OLLAMA_USAGE_PATH, get(usage))
            .with_state(log.clone());
        let (origin, server) = test_server(router).await;
        let result = OllamaCloudClient::for_test_origin(&origin)
            .unwrap()
            .fetch("free-test-key")
            .await;
        assert_eq!(result.account.unwrap().plan.as_deref(), Some("free"));
        let usage = result.usage.unwrap();
        assert_eq!(usage.limits[0].utilization, 0.0);
        assert_eq!(usage.limits[1].utilization, 25.0);
        assert_eq!(usage.limits[0].models[0].name, "gpt-oss:120b");
        let requests = log.0.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().any(|item| item.method == "POST"
            && item.authorization == "Bearer free-test-key"
            && item.content_type == "application/json"
            && item.content_length == "0"));
        assert!(requests
            .iter()
            .any(|item| item.method == "GET" && item.authorization == "Bearer free-test-key"));
        server.abort();
    }

    #[test]
    fn account_parser_accepts_case_variants_and_missing_optional_free_fields() {
        let upper = parse_account(&json!({
            "ID": "one",
            "CreatedAt": "2026-06-21T08:39:00.256543Z",
            "Email": "a@example.com",
            "Name": "owner",
            "Bio": "",
            "AvatarURL": "/public/avatar.png",
            "FirstName": "",
            "LastName": "",
            "Links": [],
            "Plan": "free"
        }))
        .unwrap();
        assert_eq!(upper.id.as_deref(), Some("one"));
        assert_eq!(upper.email.as_deref(), Some("a@example.com"));
        assert_eq!(upper.plan.as_deref(), Some("free"));
        let lower = parse_account(&json!({
            "id": "two",
            "createdat": "2026-08-14T00:00:00Z",
            "firstname": "First",
            "lastname": "Last",
            "avatarurl": "/public/avatar.png",
            "plan": "free"
        }))
        .unwrap();
        assert_eq!(lower.id.as_deref(), Some("two"));
        assert_eq!(lower.first_name.as_deref(), Some("First"));
        assert_eq!(lower.plan.as_deref(), Some("free"));

        let drifted_optional_fields = parse_account(&json!({
            "id": {"future": "shape"},
            "created_at": 1786700000,
            "plan": "free"
        }))
        .unwrap();
        assert!(drifted_optional_fields.id.is_none());
        assert!(drifted_optional_fields.created_at_ms.is_none());
        assert_eq!(drifted_optional_fields.plan.as_deref(), Some("free"));

        assert_eq!(
            parse_account(&json!({"unrecognized": true}))
                .unwrap_err()
                .kind,
            OllamaCloudErrorKind::InvalidResponse
        );
    }

    #[test]
    fn usage_parser_accepts_zero_fraction_and_one_but_rejects_invalid_ratios() {
        for (ratio, expected) in [(0.0, 0.0), (0.125, 12.5), (1.0, 100.0)] {
            let parsed = parse_usage(&json!({
                "limits": {"session": {"usage": ratio, "models": []}}
            }))
            .unwrap();
            assert_eq!(parsed.limits[0].utilization, expected);
        }
        for ratio in [-0.01, 1.01] {
            let error = parse_usage(&json!({
                "limits": {"weekly": {"usage": ratio, "models": []}}
            }))
            .unwrap_err();
            assert_eq!(error.kind, OllamaCloudErrorKind::InvalidResponse);
        }
    }

    #[test]
    fn only_retryable_failures_permit_stale_data_and_retry_after_is_js_safe() {
        assert!(OllamaCloudFetchError::transient().permits_stale());
        assert!(OllamaCloudFetchError::rate_limited(None).permits_stale());
        assert!(!OllamaCloudFetchError::authentication().permits_stale());
        assert!(!OllamaCloudFetchError::invalid_response().permits_stale());

        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("18446744073709551615"),
        );
        assert_eq!(retry_after_ms(&headers), Some(MAX_SAFE_INTEGER));
    }

    #[test]
    fn model_lists_are_bounded_without_rewriting_cloud_aliases() {
        let models = (0..OLLAMA_CLOUD_MAX_MODELS + 5)
            .map(|index| json!({"name": format!("gpt-oss:{index}-cloud"), "request_count": index}))
            .collect::<Vec<_>>();
        let parsed = parse_usage(&json!({
            "limits": {"weekly": {"usage": 0, "models": models}}
        }))
        .unwrap();
        assert_eq!(parsed.limits[0].models.len(), OLLAMA_CLOUD_MAX_MODELS);
        assert!(parsed.limits[0].models_truncated);
        assert_eq!(parsed.limits[0].models[0].name, "gpt-oss:0-cloud");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let router = Router::new()
            .route(
                OLLAMA_ME_PATH,
                post(|| async { Redirect::temporary("/elsewhere") }),
            )
            .route(
                OLLAMA_USAGE_PATH,
                get(|| async { Redirect::temporary("/elsewhere") }),
            )
            .route(
                "/elsewhere",
                get(|| async { Json(json!({"ID": "leaked"})) }),
            );
        let (origin, server) = test_server(router).await;
        let result = OllamaCloudClient::for_test_origin(&origin)
            .unwrap()
            .fetch("free-test-key")
            .await;
        assert_eq!(
            result.account.unwrap_err().kind,
            OllamaCloudErrorKind::InvalidResponse
        );
        assert_eq!(
            result.usage.unwrap_err().kind,
            OllamaCloudErrorKind::InvalidResponse
        );
        server.abort();
    }

    #[tokio::test]
    async fn status_failures_are_classified_without_exposing_response_bodies() {
        let router = Router::new()
            .route(
                OLLAMA_ME_PATH,
                post(|| async { (StatusCode::UNAUTHORIZED, "secret upstream detail") }),
            )
            .route(
                OLLAMA_USAGE_PATH,
                get(|| async {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [(RETRY_AFTER, "12")],
                        "another secret detail",
                    )
                }),
            );
        let (origin, server) = test_server(router).await;
        let result = OllamaCloudClient::for_test_origin(&origin)
            .unwrap()
            .fetch("free-test-key")
            .await;
        let account = result.account.unwrap_err();
        assert_eq!(account.kind, OllamaCloudErrorKind::Authentication);
        assert!(!account.to_string().contains("secret"));
        let usage = result.usage.unwrap_err();
        assert_eq!(usage.kind, OllamaCloudErrorKind::RateLimited);
        assert_eq!(usage.retry_after_ms, Some(12_000));
        assert!(!usage.to_string().contains("secret"));
        server.abort();
    }

    #[tokio::test]
    async fn malformed_and_oversized_responses_are_invalid() {
        let oversized = "x".repeat(MAX_OLLAMA_RESPONSE_BYTES + 1);
        let router = Router::new()
            .route(OLLAMA_ME_PATH, post(|| async { "not json" }))
            .route(
                OLLAMA_USAGE_PATH,
                get(move || {
                    let oversized = oversized.clone();
                    async move { oversized }
                }),
            );
        let (origin, server) = test_server(router).await;
        let result = OllamaCloudClient::for_test_origin(&origin)
            .unwrap()
            .fetch("free-test-key")
            .await;
        assert_eq!(
            result.account.unwrap_err().kind,
            OllamaCloudErrorKind::InvalidResponse
        );
        assert_eq!(
            result.usage.unwrap_err().kind,
            OllamaCloudErrorKind::InvalidResponse
        );
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires OLLAMA_API_KEY and live Ollama Cloud access"]
    async fn ollama_cloud_live_account_usage_from_env() {
        let api_key = Zeroizing::new(
            std::env::var("OLLAMA_API_KEY")
                .expect("OLLAMA_API_KEY must be set for the ignored live smoke"),
        );
        assert!(
            !api_key.trim().is_empty(),
            "OLLAMA_API_KEY must not be empty"
        );

        let result = OllamaCloudClient::official()
            .expect("build official Ollama Cloud client")
            .fetch(&api_key)
            .await;
        let account = result.account.expect("fetch live Ollama account");
        let usage = result.usage.expect("fetch live Ollama usage");
        assert!(
            account.id.is_some()
                || account.email.is_some()
                || account.name.is_some()
                || account.plan.is_some()
        );
        assert!(usage
            .limits
            .iter()
            .any(|window| window.kind == OllamaCloudUsageWindowKind::Session));
        assert!(usage
            .limits
            .iter()
            .any(|window| window.kind == OllamaCloudUsageWindowKind::Weekly));
    }
}
