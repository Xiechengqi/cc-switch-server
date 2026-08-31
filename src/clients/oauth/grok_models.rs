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
    pub source: &'static str,
    pub source_url: String,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
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
    source_url: String,
    etag: Option<String>,
    fetched_at: Instant,
    fetched_at_ms: i64,
}

fn cache() -> &'static Mutex<BTreeMap<GrokModelCatalogScope, CachedCatalog>> {
    static CACHE: OnceLock<Mutex<BTreeMap<GrokModelCatalogScope, CachedCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
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
    let models =
        parse_models(&raw).map_err(|message| GrokModelCatalogFailure::new(502, false, message))?;
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
                source_url: source_url.clone(),
                etag,
                fetched_at: Instant::now(),
                fetched_at_ms,
            },
        );
    }
    crate::metrics::record_grok_model_catalog("upstream");
    Ok(GrokModelCatalog {
        models,
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
        source,
        source_url: cached.source_url.clone(),
        stale,
        fetched_at_ms: Some(cached.fetched_at_ms),
    }
}

fn parse_models(raw: &Value) -> Result<Vec<String>, String> {
    let values = raw
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.get("models").and_then(Value::as_array))
        .ok_or_else(|| "Grok model catalog omitted the data/models array".to_string())?;
    let mut models = BTreeMap::new();
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
            models.insert(id, ());
        }
    }
    Ok(models.into_keys().collect())
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
                        r#"{"data":[{"id":"grok-live"}]}"#,
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
