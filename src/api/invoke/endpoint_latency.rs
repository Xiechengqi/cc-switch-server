use std::time::{Duration, Instant};

use futures_util::future::join_all;
use reqwest::{Client, Url};
use serde::Serialize;

use crate::api::ApiError;

const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 15;
const MAX_ENDPOINTS: usize = 16;
const MAX_ENDPOINT_URL_LENGTH: usize = 2_048;

#[derive(Debug, Clone, Serialize)]
pub(super) struct EndpointLatency {
    url: String,
    latency: Option<u64>,
    status: Option<u16>,
    error: Option<String>,
}

pub(super) async fn test_api_endpoints(
    http_client: &Client,
    urls: Vec<String>,
    timeout_secs: Option<u64>,
) -> Result<Vec<EndpointLatency>, ApiError> {
    if urls.len() > MAX_ENDPOINTS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_ENDPOINTS} API endpoints can be tested at once"
        )));
    }

    let timeout = Duration::from_secs(sanitize_timeout(timeout_secs));
    let probes = urls
        .into_iter()
        .map(|url| probe_endpoint(http_client.clone(), url, timeout));
    Ok(join_all(probes).await)
}

async fn probe_endpoint(
    http_client: Client,
    raw_url: String,
    timeout: Duration,
) -> EndpointLatency {
    let url = raw_url.trim().to_string();
    let parsed_url = match parse_endpoint_url(&url) {
        Ok(parsed_url) => parsed_url,
        Err(error) => {
            return EndpointLatency {
                url,
                latency: None,
                status: None,
                error: Some(error),
            };
        }
    };

    let started = Instant::now();
    match http_client
        .head(parsed_url)
        .timeout(timeout)
        .header("accept", "*/*")
        .header("accept-encoding", "identity")
        .send()
        .await
    {
        Ok(response) => EndpointLatency {
            url,
            latency: Some(elapsed_millis(started)),
            status: Some(response.status().as_u16()),
            error: None,
        },
        Err(error) => EndpointLatency {
            url,
            latency: None,
            status: error.status().map(|status| status.as_u16()),
            error: Some(if error.is_timeout() {
                "request timed out".to_string()
            } else if error.is_connect() {
                "connection failed".to_string()
            } else {
                error.to_string()
            }),
        },
    }
}

fn parse_endpoint_url(value: &str) -> Result<Url, String> {
    if value.is_empty() {
        return Err("URL is required".to_string());
    }
    if value.len() > MAX_ENDPOINT_URL_LENGTH {
        return Err(format!(
            "URL exceeds the {MAX_ENDPOINT_URL_LENGTH} character limit"
        ));
    }

    let parsed = Url::parse(value).map_err(|_| "URL is invalid".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only HTTP and HTTPS URLs are supported".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL-embedded credentials are not supported".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("URL host is required".to_string());
    }
    Ok(parsed)
}

fn sanitize_timeout(timeout_secs: Option<u64>) -> u64 {
    timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use axum::{http::StatusCode, routing::head, Router};

    use super::*;

    #[test]
    fn timeout_is_bounded() {
        assert_eq!(sanitize_timeout(None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(sanitize_timeout(Some(0)), MIN_TIMEOUT_SECS);
        assert_eq!(sanitize_timeout(Some(999)), MAX_TIMEOUT_SECS);
    }

    #[test]
    fn endpoint_validation_rejects_unsupported_or_credentialed_urls() {
        assert!(parse_endpoint_url("ftp://api.example.com").is_err());
        assert!(parse_endpoint_url("https://user:secret@api.example.com").is_err());
        assert!(parse_endpoint_url("https://api.example.com/v1").is_ok());
    }

    #[tokio::test]
    async fn endpoint_probe_returns_server_side_latency_and_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/probe", head(|| async { StatusCode::NO_CONTENT })),
            )
            .await
            .unwrap();
        });

        let url = format!("http://{address}/probe");
        let result = test_api_endpoints(&Client::new(), vec![url.clone()], Some(2))
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, url);
        assert_eq!(result[0].status, Some(StatusCode::NO_CONTENT.as_u16()));
        assert!(result[0].latency.is_some());
        assert!(result[0].error.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn invalid_endpoint_is_reported_without_failing_the_batch() {
        let result =
            test_api_endpoints(&Client::new(), vec!["file:///etc/passwd".to_string()], None)
                .await
                .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result[0].latency.is_none());
        assert!(result[0].status.is_none());
        assert!(result[0].error.is_some());
    }
}
