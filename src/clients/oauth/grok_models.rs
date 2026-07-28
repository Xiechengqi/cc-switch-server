use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::domain::providers::model_routing::DEFAULT_GROK_MODEL;

pub const GROK_MODELS_URL: &str = "https://cli-chat-proxy.grok.com/v1/models";
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;
const MAX_MODELS_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct GrokModelCatalog {
    pub models: Vec<String>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct CachedCatalog {
    models: Vec<String>,
    etag: Option<String>,
    fetched_at: Instant,
    fetched_at_ms: i64,
}

fn cache() -> &'static Mutex<BTreeMap<String, CachedCatalog>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, CachedCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub async fn grok_model_catalog(
    http: &reqwest::Client,
    account_id: &str,
    access_token: &str,
) -> GrokModelCatalog {
    fetch_catalog(http, account_id, access_token, GROK_MODELS_URL, cache_ttl()).await
}

#[cfg(test)]
pub(crate) async fn grok_model_catalog_at_test_url(
    http: &reqwest::Client,
    account_id: &str,
    access_token: &str,
    url: &str,
) -> GrokModelCatalog {
    fetch_catalog(http, account_id, access_token, url, cache_ttl()).await
}

pub fn static_grok_model_catalog(reason: &'static str) -> GrokModelCatalog {
    fallback_catalog(reason)
}

async fn fetch_catalog(
    http: &reqwest::Client,
    account_id: &str,
    access_token: &str,
    url: &str,
    ttl: Duration,
) -> GrokModelCatalog {
    let cache_key = account_id.trim().to_string();
    let previous = cache().lock().await.get(&cache_key).cloned();
    if let Some(cached) = previous.as_ref() {
        if cached.fetched_at.elapsed() < ttl {
            crate::metrics::record_grok_model_catalog("cache_fresh");
            return catalog_from_cache(cached, "cache_fresh", false);
        }
    }

    let mut request = http
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .bearer_auth(access_token.trim())
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

    let response = request.send().await;
    match response {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
            if let Some(mut cached) = previous {
                cached.fetched_at = Instant::now();
                cached.fetched_at_ms = chrono::Utc::now().timestamp_millis();
                let result = catalog_from_cache(&cached, "upstream_not_modified", false);
                cache().lock().await.insert(cache_key, cached);
                crate::metrics::record_grok_model_catalog("upstream_not_modified");
                return result;
            }
            fallback_catalog("not_modified_without_cache")
        }
        Ok(mut response) if response.status().is_success() => {
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            match crate::infra::http::read_response_body_limited(
                &mut response,
                MAX_MODELS_RESPONSE_BODY_BYTES,
            )
            .await
            {
                Ok(body) => match serde_json::from_slice::<Value>(&body) {
                    Ok(raw) => {
                        let models = parse_models(&raw);
                        if models.is_empty() {
                            return stale_or_fallback(previous.as_ref(), "empty_catalog");
                        }
                        let fetched_at_ms = chrono::Utc::now().timestamp_millis();
                        cache().lock().await.insert(
                            cache_key,
                            CachedCatalog {
                                models: models.clone(),
                                etag,
                                fetched_at: Instant::now(),
                                fetched_at_ms,
                            },
                        );
                        crate::metrics::record_grok_model_catalog("upstream");
                        GrokModelCatalog {
                            models,
                            source: "upstream",
                            stale: false,
                            fetched_at_ms: Some(fetched_at_ms),
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "parse Grok model catalog failed");
                        stale_or_fallback(previous.as_ref(), "parse_error")
                    }
                },
                Err(error) => {
                    tracing::warn!(error = %error, "read Grok model catalog failed");
                    stale_or_fallback(previous.as_ref(), "body_read_error")
                }
            }
        }
        Ok(response) => {
            tracing::warn!(status = %response.status(), "fetch Grok model catalog failed");
            stale_or_fallback(previous.as_ref(), "upstream_status")
        }
        Err(error) => {
            tracing::warn!(error = %error, "fetch Grok model catalog failed");
            stale_or_fallback(previous.as_ref(), "network_error")
        }
    }
}

fn stale_or_fallback(cached: Option<&CachedCatalog>, reason: &'static str) -> GrokModelCatalog {
    if let Some(cached) = cached {
        crate::metrics::record_grok_model_catalog("last_known_good");
        tracing::warn!(reason, "using last-known-good Grok model catalog");
        catalog_from_cache(cached, "last_known_good", true)
    } else {
        fallback_catalog(reason)
    }
}

fn fallback_catalog(reason: &'static str) -> GrokModelCatalog {
    crate::metrics::record_grok_model_catalog("static_fallback");
    tracing::warn!(reason, "using static fallback Grok model catalog");
    GrokModelCatalog {
        models: vec![DEFAULT_GROK_MODEL.to_string()],
        source: "static_fallback",
        stale: true,
        fetched_at_ms: None,
    }
}

fn catalog_from_cache(
    cached: &CachedCatalog,
    source: &'static str,
    stale: bool,
) -> GrokModelCatalog {
    GrokModelCatalog {
        models: cached.models.clone(),
        source,
        stale,
        fetched_at_ms: Some(cached.fetched_at_ms),
    }
}

fn parse_models(raw: &Value) -> Vec<String> {
    let values = raw
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.get("models").and_then(Value::as_array));
    let mut models = BTreeMap::new();
    for value in values.into_iter().flatten() {
        let id = value
            .as_str()
            .or_else(|| value.get("id").and_then(Value::as_str))
            .or_else(|| value.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            models.insert(id.trim_start_matches("models/").to_string(), ());
        }
    }
    models.into_keys().collect()
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

    #[test]
    fn parses_openai_and_named_model_shapes() {
        assert_eq!(
            parse_models(&serde_json::json!({
                "data": [{"id": "grok-b"}, {"name": "models/grok-a"}, "grok-b"]
            })),
            vec!["grok-a", "grok-b"]
        );
    }

    #[tokio::test]
    async fn etag_304_and_last_known_good_are_preserved() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for attempt in 0..3 {
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
        let account_id = format!("catalog-test-{}", address.port());

        let first = fetch_catalog(&client, &account_id, "access-token", &url, Duration::ZERO).await;
        assert_eq!(first.source, "upstream");
        assert_eq!(first.models, vec!["grok-live"]);
        let second =
            fetch_catalog(&client, &account_id, "access-token", &url, Duration::ZERO).await;
        assert_eq!(second.source, "upstream_not_modified");
        let third = fetch_catalog(&client, &account_id, "access-token", &url, Duration::ZERO).await;
        assert_eq!(third.source, "last_known_good");
        assert!(third.stale);
        assert_eq!(third.models, vec!["grok-live"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_catalog_fails_closed_to_the_static_fallback() {
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
        let catalog = fetch_catalog(
            &reqwest::Client::new(),
            &format!("oversized-catalog-{}", address.port()),
            "access-token",
            &format!("http://{address}/v1/models"),
            Duration::ZERO,
        )
        .await;
        assert_eq!(catalog.source, "static_fallback");
        assert!(catalog.stale);
        server.await.unwrap();
    }
}
