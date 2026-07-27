use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine;
use bytes::Bytes;
use futures_util::{stream, StreamExt, TryStreamExt};
use reqwest::Url;
use serde_json::Value;

use super::ProxyError;

pub(crate) const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const BATCH_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 3;
pub(crate) const MAX_IMAGES_PER_REQUEST: usize = 16;
pub(crate) const MAX_CONCURRENT_FETCHES: usize = 4;

#[derive(Debug)]
pub(crate) struct LoadedRemoteImage {
    pub(crate) data: Bytes,
    pub(crate) mime_type: String,
}

pub(crate) async fn fetch_remote_image(
    url: &str,
    max_bytes: usize,
) -> Result<LoadedRemoteImage, ProxyError> {
    tokio::time::timeout(FETCH_TIMEOUT, fetch_remote_image_inner(url, max_bytes))
        .await
        .map_err(|_| remote_image_error("image download timed out"))?
}

async fn fetch_remote_image_inner(
    url: &str,
    max_bytes: usize,
) -> Result<LoadedRemoteImage, ProxyError> {
    let mut current =
        Url::parse(url).map_err(|error| invalid_image(format!("image URL invalid: {error}")))?;

    for redirect_count in 0..=MAX_REDIRECTS {
        let pinned = validate_and_pin_url(&current).await?;
        let mut builder = crate::infra::http::direct_client_builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some((host, address)) = pinned {
            builder = builder.resolve(&host, address);
        }
        let client = builder
            .build()
            .map_err(|error| remote_image_error(format!("build image client failed: {error}")))?;
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|error| remote_image_error(format!("image download failed: {error}")))?;

        if response.status().is_redirection() {
            if redirect_count >= MAX_REDIRECTS {
                return Err(invalid_image("image redirect limit exceeded"));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| invalid_image("image redirect is missing Location header"))?;
            let next = current.join(location).map_err(|error| {
                invalid_image(format!("image redirect Location invalid: {error}"))
            })?;
            if current.scheme() == "https" && next.scheme() != "https" {
                return Err(invalid_image("image redirect cannot downgrade HTTPS"));
            }
            current = next;
            continue;
        }
        if !response.status().is_success() {
            return Err(remote_image_error(format!(
                "image download returned HTTP {}",
                response.status()
            )));
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes as u64 {
                return Err(image_too_large(max_bytes, length));
            }
        }
        let claimed_mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
        let mut stream = response.bytes_stream();
        let mut data = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|error| remote_image_error(format!("read image bytes failed: {error}")))?;
            let next_len = data.len().saturating_add(chunk.len());
            if next_len > max_bytes {
                return Err(image_too_large(max_bytes, next_len as u64));
            }
            data.extend_from_slice(&chunk);
        }
        let mime_type = validate_image_bytes(&data, claimed_mime.as_deref(), max_bytes)?;
        return Ok(LoadedRemoteImage {
            data: Bytes::from(data),
            mime_type,
        });
    }

    Err(invalid_image("image redirect limit exceeded"))
}

pub(crate) fn validate_image_bytes(
    data: &[u8],
    claimed_mime: Option<&str>,
    max_bytes: usize,
) -> Result<String, ProxyError> {
    if data.len() > max_bytes {
        return Err(image_too_large(max_bytes, data.len() as u64));
    }
    let detected = detect_image_mime(data)
        .ok_or_else(|| invalid_image("image bytes do not match a supported image signature"))?;
    if let Some(claimed) = claimed_mime
        .map(str::trim)
        .filter(|mime| !mime.is_empty() && !mime.eq_ignore_ascii_case("application/octet-stream"))
    {
        let normalized = normalize_mime(claimed);
        if !normalized.starts_with("image/") {
            return Err(invalid_image(format!(
                "image MIME must start with image/: {claimed}"
            )));
        }
        if normalized != detected {
            return Err(invalid_image(format!(
                "image MIME {claimed} does not match detected {detected}"
            )));
        }
    }
    Ok(detected.to_string())
}

pub(crate) fn decode_image_data_uri(
    uri: &str,
    max_bytes: usize,
) -> Result<LoadedRemoteImage, ProxyError> {
    let (header, payload) = uri
        .split_once(',')
        .ok_or_else(|| invalid_image("image data URI is missing a payload"))?;
    let prefix = header
        .as_bytes()
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case(b"data:"))
        .ok_or_else(|| invalid_image("image data URI must start with data:"))?;
    debug_assert_eq!(prefix.len(), 5);
    let metadata = &header[5..];
    let mut fields = metadata.split(';');
    let claimed_mime = fields
        .next()
        .map(str::trim)
        .filter(|mime| !mime.is_empty() && mime.to_ascii_lowercase().starts_with("image/"))
        .ok_or_else(|| invalid_image("image data URI must declare an image MIME"))?;
    if !fields.any(|field| field.trim().eq_ignore_ascii_case("base64")) {
        return Err(invalid_image("image data URI must use base64 encoding"));
    }

    let payload = payload.trim();
    let max_encoded_bytes = (max_bytes.saturating_add(2) / 3).saturating_mul(4);
    if payload.len() > max_encoded_bytes {
        return Err(image_too_large(max_bytes, payload.len() as u64));
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| invalid_image(format!("image base64 decode failed: {error}")))?;
    let mime_type = validate_image_bytes(&data, Some(claimed_mime), max_bytes)?;
    Ok(LoadedRemoteImage {
        data: Bytes::from(data),
        mime_type,
    })
}

pub(crate) async fn inline_codex_remote_images(body: &Bytes) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| invalid_image(format!("invalid Codex image request JSON: {error}")))?;
    let image_count = codex_image_part_count(&value);
    if image_count > MAX_IMAGES_PER_REQUEST {
        return Err(invalid_image(format!(
            "Codex request exceeds {MAX_IMAGES_PER_REQUEST} image limit"
        )));
    }
    let references = codex_remote_image_references(&value)?;
    if references.is_empty() {
        return Ok(body.clone());
    }

    let loaded_images =
        tokio::time::timeout(
            BATCH_FETCH_TIMEOUT,
            stream::iter(references.into_iter().map(
                |(input_index, content_index, url)| async move {
                    let loaded = fetch_remote_image(&url, MAX_IMAGE_BYTES).await?;
                    Ok::<_, ProxyError>((input_index, content_index, loaded))
                },
            ))
            .buffered(MAX_CONCURRENT_FETCHES)
            .try_collect::<Vec<_>>(),
        )
        .await
        .map_err(|_| remote_image_error("remote image batch timed out"))??;

    for (input_index, content_index, loaded) in loaded_images {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&loaded.data);
        let data_uri = format!("data:{};base64,{encoded}", loaded.mime_type);
        let Some(part) = value
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .and_then(|input| input.get_mut(input_index))
            .and_then(|item| item.get_mut("content"))
            .and_then(Value::as_array_mut)
            .and_then(|content| content.get_mut(content_index))
        else {
            return Err(invalid_image("Codex image content changed while loading"));
        };
        match part.get_mut("image_url") {
            Some(Value::Object(image_url)) => {
                image_url.insert("url".to_string(), Value::String(data_uri));
            }
            Some(image_url) => *image_url = Value::String(data_uri),
            None => {
                part["image_url"] = Value::String(data_uri);
            }
        }
    }

    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| invalid_image(format!("encode Codex image request failed: {error}")))
}

fn codex_image_part_count(value: &Value) -> usize {
    value
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("input_image" | "image_url")
            )
        })
        .count()
}

fn codex_remote_image_references(value: &Value) -> Result<Vec<(usize, usize, String)>, ProxyError> {
    let mut references = Vec::new();
    let Some(input) = value.get("input").and_then(Value::as_array) else {
        return Ok(references);
    };
    for (input_index, item) in input.iter().enumerate() {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (content_index, part) in content.iter().enumerate() {
            if !matches!(
                part.get("type").and_then(Value::as_str),
                Some("input_image" | "image_url")
            ) {
                continue;
            }
            let Some(url) = codex_image_url(part).map(str::trim) else {
                continue;
            };
            if is_data_image_uri(url) {
                decode_image_data_uri(url, MAX_IMAGE_BYTES)?;
                continue;
            }
            require_http_image_url(url)?;
            references.push((input_index, content_index, url.to_string()));
        }
    }
    Ok(references)
}

fn codex_image_url(part: &Value) -> Option<&str> {
    part.get("image_url").and_then(|value| {
        value
            .as_str()
            .or_else(|| value.get("url").and_then(Value::as_str))
    })
}

pub(crate) fn is_data_image_uri(value: &str) -> bool {
    value
        .trim()
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"data:"))
}

pub(crate) fn is_http_image_url(value: &str) -> bool {
    Url::parse(value.trim())
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn require_http_image_url(value: &str) -> Result<(), ProxyError> {
    let url =
        Url::parse(value).map_err(|error| invalid_image(format!("image URL invalid: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid_image(format!(
            "image URL scheme must be http/https: {}",
            url.scheme()
        )));
    }
    Ok(())
}

async fn validate_and_pin_url(url: &Url) -> Result<Option<(String, SocketAddr)>, ProxyError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid_image(format!(
            "image URL scheme must be http/https: {}",
            url.scheme()
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_image("image URL must not contain credentials"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid_image("image URL is missing host"))?;
    guard_host(host)?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| invalid_image("image URL has no usable port"))?;

    if let Ok(ip) = host.parse::<IpAddr>() {
        guard_ip(&ip)?;
        return Ok(None);
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| invalid_image(format!("image host resolution failed ({host}): {error}")))?
        .collect::<Vec<_>>();
    validate_resolved_addresses(host, &addresses)?;
    Ok(Some((host.to_string(), addresses[0])))
}

fn validate_resolved_addresses(host: &str, addresses: &[SocketAddr]) -> Result<(), ProxyError> {
    if addresses.is_empty() {
        return Err(invalid_image(format!(
            "image host resolved to no addresses: {host}"
        )));
    }
    for address in addresses {
        guard_ip(&address.ip())?;
    }
    Ok(())
}

fn detect_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"BM") {
        Some("image/bmp")
    } else if data.len() >= 12
        && &data[4..8] == b"ftyp"
        && matches!(&data[8..12], b"avif" | b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
}

fn normalize_mime(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "image/jpg" | "image/pjpeg" => "image/jpeg".to_string(),
        other => other.to_string(),
    }
}

fn guard_ip(ip: &IpAddr) -> Result<(), ProxyError> {
    let blocked = match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || octets[0] == 0
                || (octets[0] == 100 && (octets[1] & 0xc0) == 0x40)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                || octets[0] >= 240
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4() {
                return guard_ip(&IpAddr::V4(v4));
            }
            let segments = v6.segments();
            let is_global_unicast = (segments[0] & 0xe000) == 0x2000;
            let is_ietf_special = segments[0] == 0x2001 && (segments[1] & 0xfe00) == 0;
            let is_documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0);
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || !is_global_unicast
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xffc0) == 0xfec0
                || is_ietf_special
                || is_documentation
                || segments[0] == 0x2002
        }
    };
    if blocked {
        return Err(invalid_image(format!(
            "image host resolved to blocked IP: {ip}"
        )));
    }
    Ok(())
}

fn guard_host(host: &str) -> Result<(), ProxyError> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost"
        || [".internal", ".local", ".localhost", ".lan"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
    {
        return Err(invalid_image(format!("image host is blocked: {host}")));
    }
    Ok(())
}

fn image_too_large(max_bytes: usize, actual: u64) -> ProxyError {
    invalid_image(format!(
        "image exceeds {max_bytes} byte limit; got {actual}"
    ))
}

fn invalid_image(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: message.into(),
    }
}

fn remote_image_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_magic_bytes_and_claimed_mime() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(
            validate_image_bytes(png, Some("image/png"), MAX_IMAGE_BYTES).unwrap(),
            "image/png"
        );
        assert!(validate_image_bytes(png, Some("image/jpeg"), MAX_IMAGE_BYTES).is_err());
        assert!(validate_image_bytes(b"not an image", Some("image/png"), MAX_IMAGE_BYTES).is_err());
    }

    #[test]
    fn validates_data_uri_size_mime_and_signature_before_forwarding() {
        let image =
            decode_image_data_uri("data:image/png;base64,iVBORw0KGgo=", MAX_IMAGE_BYTES).unwrap();
        assert_eq!(image.data, Bytes::from_static(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(image.mime_type, "image/png");

        assert!(
            decode_image_data_uri("data:image/jpeg;base64,iVBORw0KGgo=", MAX_IMAGE_BYTES,).is_err()
        );
        assert!(decode_image_data_uri("data:text/plain;base64,aGVsbG8=", MAX_IMAGE_BYTES).is_err());

        let oversized = format!("data:image/png;base64,{}", "A".repeat(17));
        let error = decode_image_data_uri(&oversized, 8).unwrap_err();
        assert!(error.message.contains("byte limit"));
    }

    #[test]
    fn enforces_stream_size_limit_at_validation_boundary() {
        let mut data = vec![0u8; 9];
        data[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let error = validate_image_bytes(&data, Some("image/png"), 8).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_private_mapped_and_mixed_dns_answers() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "fec0::1",
            "2001::1",
            "2002:7f00:1::1",
            "3fff::1",
        ] {
            assert!(guard_ip(&ip.parse().unwrap()).is_err());
        }
        for ip in ["93.184.216.34", "2606:4700:4700::1111"] {
            guard_ip(&ip.parse().unwrap()).unwrap();
        }
        let addresses = [
            "93.184.216.34:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ];
        assert!(validate_resolved_addresses("images.example", &addresses).is_err());
    }

    #[test]
    fn extracts_http_codex_image_references_with_url_parser() {
        let value = serde_json::json!({
            "input": [{
                "type": "message",
                "content": [
                    {"type": "input_image", "image_url": "https://example.com/a.png"},
                    {"type": "input_image", "image_url": "HTTPS://example.com/b.png"},
                    {"type": "input_image", "image_url": "DATA:image/png;base64,iVBORw0KGgo="}
                ]
            }]
        });
        let references = codex_remote_image_references(&value).unwrap();
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].2, "https://example.com/a.png");
        assert_eq!(references[1].2, "HTTPS://example.com/b.png");
    }

    #[test]
    fn rejects_non_http_codex_image_references() {
        let value = serde_json::json!({
            "input": [{
                "type": "message",
                "content": [
                    {"type": "input_image", "image_url": "file:///etc/passwd"}
                ]
            }]
        });
        let error = codex_remote_image_references(&value).unwrap_err();
        assert!(error.message.contains("http/https"));
    }

    #[tokio::test]
    async fn rejects_codex_remote_image_count_before_fetching() {
        let content = (0..=MAX_IMAGES_PER_REQUEST)
            .map(|index| {
                serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("https://example.com/{index}.png")
                })
            })
            .collect::<Vec<_>>();
        let body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "input": [{"type": "message", "content": content}]
            }))
            .unwrap(),
        );

        let error = inline_codex_remote_images(&body).await.unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image limit"));
    }

    #[tokio::test]
    async fn rejects_codex_data_image_count_without_fetching() {
        let content = (0..=MAX_IMAGES_PER_REQUEST)
            .map(|_| {
                serde_json::json!({
                    "type": "input_image",
                    "image_url": "data:image/png;base64,iVBORw0KGgo="
                })
            })
            .collect::<Vec<_>>();
        let body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "input": [{"type": "message", "content": content}]
            }))
            .unwrap(),
        );

        let error = inline_codex_remote_images(&body).await.unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image limit"));
    }

    #[tokio::test]
    async fn rejects_codex_data_image_with_spoofed_mime() {
        let body = Bytes::from_static(
            br#"{"input":[{"type":"message","content":[{"type":"input_image","image_url":"data:image/jpeg;base64,iVBORw0KGgo="}]}]}"#,
        );

        let error = inline_codex_remote_images(&body).await.unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("does not match"));
    }
}
