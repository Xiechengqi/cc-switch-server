use std::time::Duration;

use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use bytes::Bytes;

use super::ProxyError;

pub(crate) struct OutboundPostRequest<'a> {
    pub url: &'a str,
    pub body: Bytes,
    pub client_headers: &'a HeaderMap,
    pub target_headers: &'a [(String, String)],
    pub default_accept: &'a str,
    pub default_content_type: &'a str,
    pub timeout: Duration,
    pub stream_requested: bool,
}

pub(crate) fn build_post_request(
    client: &reqwest::Client,
    request: OutboundPostRequest<'_>,
) -> Result<reqwest::RequestBuilder, ProxyError> {
    let headers = assemble_headers(
        request.client_headers,
        request.target_headers,
        request.default_accept,
        request.default_content_type,
    )?;
    let mut builder = client.post(request.url).headers(headers).body(request.body);
    if !request.stream_requested {
        builder = builder.timeout(request.timeout);
    }
    Ok(builder)
}

pub(crate) fn assemble_headers(
    client_headers: &HeaderMap,
    target_headers: &[(String, String)],
    default_accept: &str,
    default_content_type: &str,
) -> Result<HeaderMap, ProxyError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        client_headers.get(ACCEPT).cloned().unwrap_or(
            HeaderValue::from_str(default_accept)
                .map_err(|_| ProxyError::bad_request("invalid default outbound Accept header"))?,
        ),
    );
    headers.insert(
        CONTENT_TYPE,
        client_headers.get(CONTENT_TYPE).cloned().unwrap_or(
            HeaderValue::from_str(default_content_type).map_err(|_| {
                ProxyError::bad_request("invalid default outbound Content-Type header")
            })?,
        ),
    );

    insert_target_headers(&mut headers, target_headers)?;
    Ok(headers)
}

pub(crate) fn insert_target_headers(
    headers: &mut HeaderMap,
    target_headers: &[(String, String)],
) -> Result<(), ProxyError> {
    for (name, value) in target_headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            ProxyError::bad_request(format!("invalid outbound header name: {name}"))
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            ProxyError::bad_request(format!("invalid outbound header value for {name}"))
        })?;
        headers.insert(name, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_headers_replace_single_value_defaults() {
        let mut client = HeaderMap::new();
        client.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/client+json"),
        );
        let target = vec![
            ("content-type".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/signed+json".to_string(),
            ),
        ];

        let headers = assemble_headers(&client, &target, "*/*", "application/json").unwrap();

        assert_eq!(
            headers.get_all(CONTENT_TYPE).iter().count(),
            1,
            "single-value headers must never be appended"
        );
        assert_eq!(
            headers.get(CONTENT_TYPE).unwrap(),
            "application/signed+json"
        );
    }
}
