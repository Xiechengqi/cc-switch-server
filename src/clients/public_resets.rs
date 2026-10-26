//! Unauthenticated, fixed-origin public announcements. Returned URLs and free
//! text are data only and are never fetched, interpreted, or executed.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const ORIGIN: &str = "https://codex-resets.com";
pub const POLL_MS: i64 = 60_000;
pub const HISTORY_MS: i64 = 300_000;
pub const MAX_PAGES: usize = 10;
pub const MAX_BODY: usize = 1024 * 1024;
pub const MAX_BACKOFF_MS: i64 = 3_600_000;
pub const OVERLAP_MS: i64 = 2 * 86_400_000;
pub const REQUEST_TIMEOUT_SECONDS: u64 = 10;

pub fn backoff_ms(failures: u16, jitter: u16) -> i64 {
    let delay = POLL_MS
        .saturating_mul(1i64 << failures.min(6))
        .min(MAX_BACKOFF_MS);
    (delay + i64::from(jitter) % (delay / 10).max(1)).min(MAX_BACKOFF_MS)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicReset {
    pub id: String,
    pub reset_type: String,
    pub announced_at: String,
    pub text: String,
    pub source: serde_json::Value,
}

impl PublicReset {
    pub fn effective_at_ms(&self, now: i64) -> anyhow::Result<i64> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 64
                && self
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
            "invalid public event id"
        );
        ensure!(
            matches!(self.reset_type.as_str(), "regular" | "banked"),
            "unknown public reset type"
        );
        ensure!(self.text.len() <= 64 * 1024, "public event text too large");
        let source = self.source.as_object().context("invalid public source")?;
        ensure!(
            source
                .keys()
                .all(|key| matches!(key.as_str(), "type" | "url" | "author")),
            "unknown public source field"
        );
        match source.get("type").and_then(serde_json::Value::as_str) {
            Some("observed") => {
                ensure!(!source.contains_key("author"), "unexpected observed author")
            }
            Some("x_post") => {
                ensure!(
                    source.get("author").and_then(serde_json::Value::as_str) == Some("thsottiaux"),
                    "untrusted public author"
                );
                ensure!(
                    source
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .is_some(),
                    "missing post url"
                );
            }
            _ => anyhow::bail!("unknown public source type"),
        }
        if let Some(url) = source.get("url") {
            let url = url.as_str().context("invalid public url")?;
            ensure!(
                url.len() <= 2048 && reqwest::Url::parse(url).is_ok(),
                "invalid public url"
            );
        }
        let time = chrono::DateTime::parse_from_rfc3339(&self.announced_at)?.timestamp_millis();
        ensure!(
            time >= 0 && time <= now,
            "public event is not an executed record"
        );
        Ok(time)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetPage {
    pub data: Vec<PublicReset>,
    pub pagination: Pagination,
    pub meta: PageMeta,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    pub has_more: bool,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageMeta {
    pub api_version: String,
    pub generated_at: String,
}

#[derive(Debug)]
pub enum Fetch {
    NotModified,
    Limited(i64),
    InvalidCursor,
    Page {
        page: ResetPage,
        etag: Option<String>,
        cache_evidence: String,
    },
}

pub enum StatusFetch {
    NotModified,
    Limited(i64),
    Observed {
        latest: String,
        etag: Option<String>,
        cache_evidence: String,
    },
}

pub async fn status(http: &reqwest::Client, etag: Option<&str>) -> anyhow::Result<StatusFetch> {
    status_at(http, ORIGIN, etag).await
}

async fn status_at(
    http: &reqwest::Client,
    origin: &str,
    etag: Option<&str>,
) -> anyhow::Result<StatusFetch> {
    let mut request = http
        .get(format!("{origin}/api/v1/status"))
        .header(reqwest::header::CACHE_CONTROL, "no-cache");
    if let Some(etag) = etag.filter(|etag| etag.len() <= 1024) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await?;
    if response.status().as_u16() == 304 {
        return Ok(StatusFetch::NotModified);
    }
    if response.status().as_u16() == 429 {
        return Ok(StatusFetch::Limited(retry_delay(response.headers())));
    }
    ensure!(
        response.status().is_success(),
        "public status http {}",
        response.status()
    );
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 1024)
        .map(str::to_string);
    let cache_evidence = cache_evidence(response.headers());
    let bytes = crate::infra::http::read_response_body_limited(&mut response, MAX_BODY).await?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
    let data = raw.get("data").unwrap_or(&raw);
    let latest = data
        .get("latest_reset")
        .context("public status has no latest_reset")?;
    ensure!(
        latest.is_null() || latest.is_object(),
        "invalid public latest_reset"
    );
    // Status is a change detector only. It never creates an event and cannot
    // bypass the strict, durable history parser, even with an unknown shape.
    Ok(StatusFetch::Observed {
        latest: serde_json::to_string(latest)?,
        etag,
        cache_evidence,
    })
}

fn cache_evidence(headers: &reqwest::header::HeaderMap) -> String {
    format!(
        "date={};age={}",
        headers
            .get(reqwest::header::DATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown"),
        headers
            .get(reqwest::header::AGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
    )
}

fn retry_delay(headers: &reqwest::header::HeaderMap) -> i64 {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok());
    let seconds = value
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| {
            value
                .and_then(|v| chrono::DateTime::parse_from_rfc2822(v).ok())
                .map(|date| {
                    date.timestamp()
                        .saturating_sub(chrono::Utc::now().timestamp())
                })
        })
        .unwrap_or(60);
    seconds.clamp(1, 3600) * 1000
}

pub fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor.len() <= 1024
        && cursor
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

pub fn client() -> anyhow::Result<reqwest::Client> {
    Ok(crate::infra::http::direct_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .user_agent("cc-switch-server/accountResetV1")
        .build()?)
}

pub async fn history(
    http: &reqwest::Client,
    from_ms: i64,
    cursor: Option<&str>,
    etag: Option<&str>,
) -> anyhow::Result<Fetch> {
    history_at(http, ORIGIN, from_ms, cursor, etag).await
}

#[cfg(test)]
pub(crate) async fn history_from_test_origin(
    http: &reqwest::Client,
    origin: &str,
    from_ms: i64,
    cursor: Option<&str>,
    etag: Option<&str>,
) -> anyhow::Result<Fetch> {
    history_at(http, origin, from_ms, cursor, etag).await
}

async fn history_at(
    http: &reqwest::Client,
    origin: &str,
    from_ms: i64,
    cursor: Option<&str>,
    etag: Option<&str>,
) -> anyhow::Result<Fetch> {
    if let Some(cursor) = cursor {
        ensure!(valid_cursor(cursor), "invalid public pagination cursor");
    }
    let from = chrono::DateTime::from_timestamp_millis(from_ms)
        .context("invalid history start")?
        .to_rfc3339();
    let mut request = http
        .get(format!("{origin}/api/v1/resets"))
        .query(&[("limit", "100"), ("order", "asc"), ("from", from.as_str())])
        .header(reqwest::header::CACHE_CONTROL, "no-cache");
    if let Some(cursor) = cursor {
        request = request.query(&[("cursor", cursor)]);
    }
    if let Some(etag) = etag.filter(|etag| etag.len() <= 1024) {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await?;
    match response.status().as_u16() {
        304 => return Ok(Fetch::NotModified),
        400 | 410 if cursor.is_some() => return Ok(Fetch::InvalidCursor),
        429 => {
            return Ok(Fetch::Limited(retry_delay(response.headers())));
        }
        _ => ensure!(
            response.status().is_success(),
            "public reset http {}",
            response.status()
        ),
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 1024)
        .map(str::to_string);
    let cache_evidence = cache_evidence(response.headers());
    let bytes = crate::infra::http::read_response_body_limited(&mut response, MAX_BODY).await?;
    let page: ResetPage = serde_json::from_slice(&bytes)?;
    ensure!(
        page.data.len() <= 100 && page.meta.api_version == "v1",
        "unsupported public history schema"
    );
    chrono::DateTime::parse_from_rfc3339(&page.meta.generated_at)?;
    ensure!(
        page.pagination.has_more == page.pagination.next_cursor.is_some(),
        "inconsistent public pagination"
    );
    if let Some(next) = &page.pagination.next_cursor {
        ensure!(
            valid_cursor(next) && Some(next.as_str()) != cursor,
            "public pagination did not advance"
        );
    }
    Ok(Fetch::Page {
        page,
        etag,
        cache_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event() -> PublicReset {
        serde_json::from_value(serde_json::json!({"id":"r_1", "reset_type":"regular", "announced_at":"2026-10-01T00:00:00Z", "text":"ignored text", "source":{"type":"observed"}})).unwrap()
    }
    #[test]
    fn strict_executed_schema_does_not_interpret_text_urls_or_forecasts() {
        let mut reset = event();
        assert!(reset.effective_at_ms(1_800_000_000_000).is_ok());
        reset.source =
            serde_json::json!({"type":"x_post","author":"unknown","url":"https://example.com"});
        assert!(reset.effective_at_ms(1_800_000_000_000).is_err());
        reset = event();
        reset.reset_type = "scheduled".into();
        assert!(reset.effective_at_ms(1_800_000_000_000).is_err());
        reset = event();
        assert!(reset.effective_at_ms(0).is_err());
        assert!(!valid_cursor("https://evil.test"));
        assert!(!valid_cursor(&"x".repeat(1025)));
    }

    async fn mock(
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    ) -> (
        String,
        tokio::task::JoinHandle<()>,
        std::sync::Arc<std::sync::Mutex<Vec<axum::http::HeaderMap>>>,
    ) {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = requests.clone();
        let app = axum::Router::new().fallback(axum::routing::any(
            move |request: axum::extract::Request| {
                let body = body.clone();
                let headers = headers.clone();
                let capture = capture.clone();
                async move {
                    assert!(!request.uri().query().unwrap_or_default().contains("random"));
                    capture.lock().unwrap().push(request.headers().clone());
                    let mut response = axum::http::Response::builder().status(status);
                    for (key, value) in headers {
                        response = response.header(key, value);
                    }
                    response.body(axum::body::Body::from(body)).unwrap()
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (origin, handle, requests)
    }

    fn page(next: Option<&str>) -> String {
        serde_json::json!({"data":[event()],"pagination":{"has_more":next.is_some(),"next_cursor":next},
            "meta":{"api_version":"v1","generated_at":"2026-10-01T00:00:00Z"}}).to_string()
    }

    #[tokio::test]
    async fn public_quota_history_etag_pagination_and_status_are_independent() {
        let http = client().unwrap();
        let (origin, handle, requests) = mock(
            200,
            vec![("etag", "\"page1\""), ("age", "42")],
            page(Some("next_1")),
        )
        .await;
        let fetched = history_at(&http, &origin, 0, None, Some("\"prior\""))
            .await
            .unwrap();
        let Fetch::Page {
            page,
            etag,
            cache_evidence,
        } = fetched
        else {
            panic!("expected page");
        };
        assert_eq!(page.pagination.next_cursor.as_deref(), Some("next_1"));
        assert_eq!(etag.as_deref(), Some("\"page1\""));
        assert!(cache_evidence.contains("age=42"));
        let headers = requests.lock().unwrap()[0].clone();
        assert_eq!(headers["if-none-match"], "\"prior\"");
        assert!(!headers.contains_key("authorization") && !headers.contains_key("cookie"));
        handle.abort();
        let (origin, handle, _) = mock(304, vec![], String::new()).await;
        assert!(matches!(
            history_at(&http, &origin, 0, None, None).await.unwrap(),
            Fetch::NotModified
        ));
        assert!(matches!(
            status_at(&http, &origin, None).await.unwrap(),
            StatusFetch::NotModified
        ));
        handle.abort();
        let (origin, handle, _) = mock(200, vec![], serde_json::json!({"data":{"latest_reset":{"id":"changed","untrusted":"ignored"},"watch":{"future":true}}}).to_string()).await;
        assert!(matches!(
            status_at(&http, &origin, None).await.unwrap(),
            StatusFetch::Observed { .. }
        ));
        handle.abort();
    }

    #[tokio::test]
    async fn public_quota_limits_invalid_cursor_loops_redirect_and_body_size_fail_closed() {
        let http = client().unwrap();
        let (origin, handle, _) = mock(429, vec![("retry-after", "7200")], String::new()).await;
        assert!(matches!(
            history_at(&http, &origin, 0, None, None).await.unwrap(),
            Fetch::Limited(3_600_000)
        ));
        assert!(matches!(
            status_at(&http, &origin, None).await.unwrap(),
            StatusFetch::Limited(3_600_000)
        ));
        handle.abort();
        let (origin, handle, _) = mock(410, vec![], String::new()).await;
        assert!(matches!(
            history_at(&http, &origin, 0, Some("expired"), None)
                .await
                .unwrap(),
            Fetch::InvalidCursor
        ));
        handle.abort();
        let (origin, handle, _) = mock(200, vec![], page(Some("same"))).await;
        assert!(history_at(&http, &origin, 0, Some("same"), None)
            .await
            .is_err());
        handle.abort();
        let (origin, handle, _) = mock(
            302,
            vec![("location", "http://127.0.0.1:1/private")],
            String::new(),
        )
        .await;
        assert!(history_at(&http, &origin, 0, None, None).await.is_err());
        handle.abort();
        let (origin, handle, _) = mock(200, vec![], "x".repeat(MAX_BODY + 1)).await;
        assert!(history_at(&http, &origin, 0, None, None).await.is_err());
        handle.abort();
    }
}
