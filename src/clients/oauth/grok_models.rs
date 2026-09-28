use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

pub const GROK_MODELS_URL: &str = "https://cli-chat-proxy.grok.com/v1/models";
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;
const MAX_STALE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_MODELS_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GrokModelCatalogScope {
    pub app: String,
    pub provider_id: String,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
}

#[derive(Debug, Clone)]
pub struct GrokModelCatalog {
    pub models: Vec<String>,
    pub capabilities: BTreeMap<String, GrokModelCapability>,
    pub source: &'static str,
    pub source_url: String,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrokModelCapability {
    pub reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: Option<String>,
    pub supports_reasoning_effort: Option<bool>,
    pub context_window: Option<u64>,
    pub max_completion_tokens: Option<u64>,
    pub supports_backend_search: Option<bool>,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct GrokModelCatalogFailure {
    pub status_code: u16,
    pub retryable: bool,
    message: String,
}

impl GrokModelCatalogFailure {
    pub fn is_unauthorized(&self) -> bool {
        self.status_code == 401
    }

    fn new(status_code: u16, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            status_code,
            retryable,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct CachedCatalog {
    models: Vec<String>,
    capabilities: BTreeMap<String, GrokModelCapability>,
    source_url: String,
    etag: Option<String>,
    fetched_at: Instant,
    fresh_for: Duration,
    fetched_at_ms: i64,
}

fn cache() -> &'static Mutex<BTreeMap<GrokModelCatalogScope, CachedCatalog>> {
    static CACHE: OnceLock<Mutex<BTreeMap<GrokModelCatalogScope, CachedCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub async fn fresh_grok_model_capability(
    scope: &GrokModelCatalogScope,
    model: &str,
) -> Option<GrokModelCapability> {
    let entries = cache().lock().await;
    let cached = entries.get(scope)?;
    if cached.fetched_at.elapsed() >= cached.fresh_for {
        return None;
    }
    cached.capabilities.get(model).cloned()
}

pub async fn grok_model_catalog(
    http: &reqwest::Client,
    scope: &GrokModelCatalogScope,
    access_token: &str,
    timeout: Duration,
) -> Result<GrokModelCatalog, GrokModelCatalogFailure> {
    fetch_catalog(
        http,
        scope,
        access_token,
        GROK_MODELS_URL,
        cache_ttl(),
        timeout,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn grok_model_catalog_at_test_url(
    http: &reqwest::Client,
    scope: &GrokModelCatalogScope,
    access_token: &str,
    url: &str,
    timeout: Duration,
) -> Result<GrokModelCatalog, GrokModelCatalogFailure> {
    fetch_catalog(http, scope, access_token, url, cache_ttl(), timeout).await
}

async fn fetch_catalog(
    http: &reqwest::Client,
    scope: &GrokModelCatalogScope,
    access_token: &str,
    url: &str,
    ttl: Duration,
    timeout: Duration,
) -> Result<GrokModelCatalog, GrokModelCatalogFailure> {
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err(GrokModelCatalogFailure::new(
            401,
            false,
            "bound Grok account has no access token",
        ));
    }
    let previous = cache().lock().await.get(scope).cloned();
    if let Some(cached) = previous.as_ref() {
        if cached.fetched_at.elapsed() < ttl {
            crate::metrics::record_grok_model_catalog("cache_fresh");
            return Ok(catalog_from_cache(cached, "cache_fresh", false));
        }
    }

    let mut request = http
        .get(url)
        .timeout(if timeout.is_zero() {
            DEFAULT_REQUEST_TIMEOUT
        } else {
            timeout
        })
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("User-Agent", crate::domain::grok_cli::grok_cli_user_agent())
        .header(
            "x-xai-token-auth",
            crate::domain::grok_cli::GROK_CLI_TOKEN_AUTH,
        )
        .header(
            "x-grok-client-identifier",
            crate::domain::grok_cli::GROK_CLI_CLIENT_IDENTIFIER,
        )
        .header(
            "x-grok-client-version",
            crate::domain::grok_cli::grok_cli_version(),
        );
    if let Some(etag) = previous.as_ref().and_then(|cached| cached.etag.as_deref()) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, "fetch Grok model catalog failed");
            return stale_or_failure(
                previous.as_ref(),
                502,
                true,
                "Grok model catalog request failed",
            );
        }
    };
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        let Some(mut cached) = previous else {
            return Err(GrokModelCatalogFailure::new(
                502,
                false,
                "Grok model catalog returned 304 without a scoped cache entry",
            ));
        };
        cached.fetched_at = Instant::now();
        cached.fetched_at_ms = chrono::Utc::now().timestamp_millis();
        cached.fresh_for = ttl;
        let result = catalog_from_cache(&cached, "upstream_not_modified", false);
        cache().lock().await.insert(scope.clone(), cached);
        crate::metrics::record_grok_model_catalog("upstream_not_modified");
        return Ok(result);
    }
    if !status.is_success() {
        let status_code = status.as_u16();
        let retryable = status_code == 408 || status_code == 429 || status_code >= 500;
        tracing::warn!(status = %status, "fetch Grok model catalog failed");
        if retryable {
            return stale_or_failure(
                previous.as_ref(),
                status_code,
                true,
                format!("Grok model catalog returned HTTP {status_code}"),
            );
        }
        return Err(GrokModelCatalogFailure::new(
            status_code,
            false,
            format!("Grok model catalog returned HTTP {status_code}"),
        ));
    }

    let mut response = response;
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let body = match crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_MODELS_RESPONSE_BODY_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(crate::infra::http::BoundedResponseBodyError::Request(error)) => {
            tracing::warn!(error = %error, "read Grok model catalog failed");
            return stale_or_failure(
                previous.as_ref(),
                502,
                true,
                "Grok model catalog response read failed",
            );
        }
        Err(error @ crate::infra::http::BoundedResponseBodyError::TooLarge { .. }) => {
            return Err(GrokModelCatalogFailure::new(
                502,
                false,
                format!("Grok model catalog response was invalid: {error}"),
            ));
        }
    };
    let raw = serde_json::from_slice::<Value>(&body).map_err(|error| {
        GrokModelCatalogFailure::new(
            502,
            false,
            format!("Grok model catalog JSON was invalid: {error}"),
        )
    })?;
    let parsed =
        parse_catalog(&raw).map_err(|message| GrokModelCatalogFailure::new(502, false, message))?;
    let models = parsed.models;
    let capabilities = parsed.capabilities;
    let fetched_at_ms = chrono::Utc::now().timestamp_millis();
    let source_url = url.to_string();
    {
        let mut entries = cache().lock().await;
        entries.retain(|candidate, _| {
            candidate.app != scope.app
                || candidate.provider_id != scope.provider_id
                || candidate.account_id != scope.account_id
                || candidate == scope
        });
        entries.insert(
            scope.clone(),
            CachedCatalog {
                models: models.clone(),
                capabilities: capabilities.clone(),
                source_url: source_url.clone(),
                etag,
                fetched_at: Instant::now(),
                fresh_for: ttl,
                fetched_at_ms,
            },
        );
    }
    crate::metrics::record_grok_model_catalog("upstream");
    Ok(GrokModelCatalog {
        models,
        capabilities,
        source: "upstream",
        source_url,
        stale: false,
        fetched_at_ms: Some(fetched_at_ms),
    })
}

fn stale_or_failure(
    cached: Option<&CachedCatalog>,
    status_code: u16,
    retryable: bool,
    message: impl Into<String>,
) -> Result<GrokModelCatalog, GrokModelCatalogFailure> {
    if retryable {
        if let Some(cached) = cached.filter(|cached| cached.fetched_at.elapsed() <= MAX_STALE_AGE) {
            crate::metrics::record_grok_model_catalog("last_known_good");
            return Ok(catalog_from_cache(cached, "last_known_good", true));
        }
    }
    Err(GrokModelCatalogFailure::new(
        status_code,
        retryable,
        message,
    ))
}

fn catalog_from_cache(
    cached: &CachedCatalog,
    source: &'static str,
    stale: bool,
) -> GrokModelCatalog {
    GrokModelCatalog {
        models: cached.models.clone(),
        capabilities: cached.capabilities.clone(),
        source,
        source_url: cached.source_url.clone(),
        stale,
        fetched_at_ms: Some(cached.fetched_at_ms),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCatalog {
    models: Vec<String>,
    capabilities: BTreeMap<String, GrokModelCapability>,
}

fn parse_catalog(raw: &Value) -> Result<ParsedCatalog, String> {
    let values = raw
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.get("models").and_then(Value::as_array))
        .ok_or_else(|| "Grok model catalog omitted the data/models array".to_string())?;
    let mut capabilities = BTreeMap::new();
    for value in values {
        if value.get("hidden").and_then(Value::as_bool) == Some(true)
            || value.pointer("/_meta/hidden").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let id = value
            .as_str()
            .or_else(|| value.get("id").and_then(Value::as_str))
            .or_else(|| value.get("model").and_then(Value::as_str))
            .or_else(|| value.get("modelId").and_then(Value::as_str))
            .or_else(|| value.get("name").and_then(Value::as_str))
            .or_else(|| value.pointer("/_meta/model").and_then(Value::as_str))
            .or_else(|| value.pointer("/_meta/modelId").and_then(Value::as_str))
            .and_then(normalize_model_id);
        if let Some(id) = id {
            capabilities
                .entry(id)
                .or_insert_with(|| parse_model_capability(value));
        }
    }
    Ok(ParsedCatalog {
        models: capabilities.keys().cloned().collect(),
        capabilities,
    })
}

#[cfg(test)]
fn parse_models(raw: &Value) -> Result<Vec<String>, String> {
    parse_catalog(raw).map(|catalog| catalog.models)
}

fn parse_model_capability(value: &Value) -> GrokModelCapability {
    let Some(object) = value.as_object() else {
        return GrokModelCapability::default();
    };
    let reasoning_entries = object
        .get("reasoning_efforts")
        .or_else(|| object.get("reasoningEfforts"))
        .and_then(Value::as_array);
    let mut reasoning_efforts = Vec::new();
    let mut menu_default = None;
    if let Some(entries) = reasoning_entries {
        for entry in entries {
            let (candidate, is_default) = match entry {
                Value::String(value) => (Some(value.as_str()), false),
                Value::Object(object) => (
                    object.get("value").and_then(Value::as_str),
                    object.get("default").and_then(Value::as_bool) == Some(true),
                ),
                _ => (None, false),
            };
            let Some(effort) = candidate.and_then(normalize_reasoning_effort) else {
                continue;
            };
            if !reasoning_efforts.contains(&effort) {
                reasoning_efforts.push(effort.clone());
            }
            if is_default && menu_default.is_none() {
                menu_default = Some(effort);
            }
        }
    }
    let default_reasoning_effort = menu_default
        .or_else(|| {
            object
                .get("reasoning_effort")
                .or_else(|| object.get("reasoningEffort"))
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .filter(|effort| reasoning_efforts.contains(effort));

    GrokModelCapability {
        reasoning_efforts,
        default_reasoning_effort,
        supports_reasoning_effort: optional_bool(
            object,
            "supports_reasoning_effort",
            "supportsReasoningEffort",
        ),
        context_window: optional_u64(object, "context_window", "contextWindow"),
        max_completion_tokens: optional_u64(object, "max_completion_tokens", "maxCompletionTokens"),
        supports_backend_search: optional_bool(
            object,
            "supports_backend_search",
            "supportsBackendSearch",
        ),
    }
}

fn optional_bool(
    object: &serde_json::Map<String, Value>,
    snake_case: &str,
    camel_case: &str,
) -> Option<bool> {
    object
        .get(snake_case)
        .or_else(|| object.get(camel_case))
        .and_then(Value::as_bool)
}

fn optional_u64(
    object: &serde_json::Map<String, Value>,
    snake_case: &str,
    camel_case: &str,
) -> Option<u64> {
    object
        .get(snake_case)
        .or_else(|| object.get(camel_case))
        .and_then(Value::as_u64)
}

fn normalize_reasoning_effort(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    )
    .then_some(value)
}

fn normalize_model_id(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix("models/").unwrap_or(value.trim());
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    {
        return None;
    }
    Some(value.to_string())
}

fn cache_ttl() -> Duration {
    std::env::var("CC_SWITCH_GROK_MODELS_TTL_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(1, MAX_CACHE_TTL_SECONDS)))
        .unwrap_or(DEFAULT_CACHE_TTL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn scope(name: impl Into<String>, generation: u64) -> GrokModelCatalogScope {
        let name = name.into();
        GrokModelCatalogScope {
            app: "codex".to_string(),
            provider_id: format!("provider-{name}"),
            provider_revision: 1,
            runtime_fingerprint: format!("runtime-{name}"),
            account_id: format!("account-{name}"),
            auth_identity_generation: generation,
            token_refresh_generation: 0,
        }
    }

    #[test]
    fn parses_known_model_shapes_hides_entries_and_preserves_identifier_priority() {
        assert_eq!(
            parse_models(&serde_json::json!({
                "data": [
                    {"id": "grok-b", "model": "ignored-model", "name": "ignored-name"},
                    {"name": "models/grok-a"},
                    {"model": "grok-c"},
                    {"modelId": "grok-d"},
                    {"_meta": {"model": "grok-e"}},
                    {"_meta": {"modelId": "models/grok-f"}},
                    {"id": "hidden-top", "hidden": true},
                    {"id": "hidden-meta", "_meta": {"hidden": true}},
                    "grok-b",
                    {"id": "bad model"}
                ]
            }))
            .unwrap(),
            vec!["grok-a", "grok-b", "grok-c", "grok-d", "grok-e", "grok-f"]
        );
        assert!(parse_models(&serde_json::json!({"models": []}))
            .unwrap()
            .is_empty());
        assert!(parse_models(&serde_json::json!({"object": "list"})).is_err());
    }

    #[test]
    fn parses_scoped_capability_menu_without_losing_order_or_explicit_zeroes() {
        let parsed = parse_catalog(&serde_json::json!({
            "data": [
                {
                    "id": "grok-4.7",
                    "reasoning_efforts": [
                        {"value": "XHIGH", "default": false},
                        "minimal",
                        {"value": "high", "default": true},
                        {"value": "xhigh", "default": true},
                        {"value": "future"},
                        {"label": "missing value"}
                    ],
                    "reasoning_effort": "low",
                    "supports_reasoning_effort": false,
                    "context_window": 0,
                    "max_completion_tokens": 0,
                    "supports_backend_search": false
                },
                {
                    "id": "grok-missing",
                    "reasoningEfforts": ["low", {"value": "max"}],
                    "reasoningEffort": "unknown",
                    "contextWindow": 500000,
                    "maxCompletionTokens": 1000000,
                    "supportsBackendSearch": true
                }
            ]
        }))
        .unwrap();

        let explicit = &parsed.capabilities["grok-4.7"];
        assert_eq!(explicit.reasoning_efforts, ["xhigh", "minimal", "high"]);
        assert_eq!(explicit.default_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(explicit.supports_reasoning_effort, Some(false));
        assert_eq!(explicit.context_window, Some(0));
        assert_eq!(explicit.max_completion_tokens, Some(0));
        assert_eq!(explicit.supports_backend_search, Some(false));

        let missing = &parsed.capabilities["grok-missing"];
        assert_eq!(missing.reasoning_efforts, ["low", "max"]);
        assert_eq!(missing.default_reasoning_effort, None);
        assert_eq!(missing.supports_reasoning_effort, None);
        assert_eq!(missing.context_window, Some(500000));
        assert_eq!(missing.max_completion_tokens, Some(1000000));
        assert_eq!(missing.supports_backend_search, Some(true));
    }

    #[test]
    fn catalog_default_must_be_a_member_of_the_filtered_menu() {
        let parsed = parse_catalog(&serde_json::json!({
            "models": [
                {
                    "id": "grok-menu",
                    "reasoning_efforts": ["low", "high"],
                    "reasoning_effort": "max"
                }
            ]
        }))
        .unwrap();
        assert_eq!(
            parsed.capabilities["grok-menu"].default_reasoning_effort,
            None
        );
    }

    #[tokio::test]
    async fn etag_304_and_same_scope_last_known_good_are_preserved() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for attempt in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let request_lowercase = request.to_ascii_lowercase();
                assert!(request_lowercase.contains("authorization: bearer access-token\r\n"));
                match attempt {
                    0 => {
                        let body = r#"{"data":[{"id":"grok-live"}]}"#;
                        stream
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                )
                                .as_bytes(),
                            )
                            .await
                            .unwrap();
                    }
                    1 => {
                        assert!(request_lowercase.contains("if-none-match: \"v1\"\r\n"));
                        stream
                            .write_all(
                                b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await
                            .unwrap();
                    }
                    _ => {
                        stream
                            .write_all(
                                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await
                            .unwrap();
                    }
                }
            }
        });
        let url = format!("http://{address}/v1/models");
        let client = reqwest::Client::new();
        let scope = scope(address.port().to_string(), 1);

        let first = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(first.source, "upstream");
        assert_eq!(first.models, vec!["grok-live"]);
        let second = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(second.source, "upstream_not_modified");
        let third = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(third.source, "last_known_good");
        assert!(third.stale);
        assert!(fresh_grok_model_capability(&scope, "grok-live")
            .await
            .is_none());

        let mut next_generation = scope.clone();
        next_generation.auth_identity_generation += 1;
        let error = fetch_catalog(
            &client,
            &next_generation,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 503);

        let mut next_token_generation = scope.clone();
        next_token_generation.token_refresh_generation += 1;
        let error = fetch_catalog(
            &client,
            &next_token_generation,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 503);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn authorization_empty_and_malformed_results_are_authoritative_and_fail_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for attempt in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let (status, content_type, body) = match attempt {
                    0 => (
                        "200 OK",
                        "application/json",
                        r#"{"data":[{"id":"grok-live","reasoning_efforts":["max"]}]}"#,
                    ),
                    1 => (
                        "401 Unauthorized",
                        "application/json",
                        r#"{"error":"expired"}"#,
                    ),
                    2 => ("200 OK", "application/json", r#"{"data":[]}"#),
                    _ => ("200 OK", "application/json", "{not-json"),
                };
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let url = format!("http://{address}/v1/models");
        let client = reqwest::Client::new();
        let scope = scope(format!("authoritative-{}", address.port()), 1);

        let first = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(first.models, ["grok-live"]);
        assert_eq!(first.capabilities["grok-live"].reasoning_efforts, ["max"]);

        let unauthorized = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(unauthorized.status_code, 401);
        assert!(!unauthorized.retryable);

        let empty = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert!(empty.models.is_empty());
        assert!(empty.capabilities.is_empty());
        assert_eq!(empty.source, "upstream");
        assert!(!empty.stale);

        let malformed = fetch_catalog(
            &client,
            &scope,
            "access-token",
            &url,
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(malformed.status_code, 502);
        assert!(!malformed.retryable);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fresh_capability_lookup_is_exact_across_every_scope_dimension() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let body = r#"{"data":[{"id":"grok-4.7","reasoning_efforts":["minimal","max"]}]}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let exact = scope(format!("scope-{}", address.port()), 7);
        fetch_catalog(
            &reqwest::Client::new(),
            &exact,
            "access-token",
            &format!("http://{address}/v1/models"),
            Duration::from_secs(60),
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(
            fresh_grok_model_capability(&exact, "grok-4.7")
                .await
                .unwrap()
                .reasoning_efforts,
            ["minimal", "max"]
        );

        for drift in 0..7 {
            let mut changed = exact.clone();
            match drift {
                0 => changed.app.push_str("-changed"),
                1 => changed.provider_id.push_str("-changed"),
                2 => changed.provider_revision += 1,
                3 => changed.runtime_fingerprint.push_str("-changed"),
                4 => changed.account_id.push_str("-changed"),
                5 => changed.auth_identity_generation += 1,
                6 => changed.token_refresh_generation += 1,
                _ => unreachable!(),
            }
            assert!(fresh_grok_model_capability(&changed, "grok-4.7")
                .await
                .is_none());
        }
        assert!(fresh_grok_model_capability(&exact, "grok-unlisted")
            .await
            .is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_catalog_fails_closed_without_static_fallback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_MODELS_RESPONSE_BODY_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let error = fetch_catalog(
            &reqwest::Client::new(),
            &scope(format!("oversized-{}", address.port()), 1),
            "access-token",
            &format!("http://{address}/v1/models"),
            Duration::ZERO,
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 502);
        assert!(!error.retryable);
        server.await.unwrap();
    }
}
