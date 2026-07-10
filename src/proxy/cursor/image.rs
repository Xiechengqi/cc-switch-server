//! Image input loader for Cursor AgentService.

use axum::http::StatusCode;
use base64::Engine;
use bytes::Bytes;
use rand::RngCore;
use std::net::IpAddr;
use std::time::Duration;

use super::agent_proto::EncodedImage;
use crate::proxy::ProxyError;

pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 3;

/// Image references extracted from downstream request bodies.
#[derive(Debug, Clone)]
pub enum ImageRef {
    DataUri(String),
    HttpUrl(String),
    Inline { mime: String, data: Bytes },
}

pub async fn load_images(refs: Vec<ImageRef>) -> Result<Vec<EncodedImage>, ProxyError> {
    let mut out = Vec::with_capacity(refs.len());
    for reference in refs {
        match reference {
            ImageRef::DataUri(uri) => out.push(decode_data_uri(&uri)?),
            ImageRef::Inline { mime, data } => {
                check_mime(&mime)?;
                check_size(data.len())?;
                out.push(EncodedImage {
                    data,
                    mime_type: Some(mime),
                    width: None,
                    height: None,
                    uuid: random_uuid_like(),
                });
            }
            ImageRef::HttpUrl(url) => out.push(fetch_http(&url).await?),
        }
    }
    Ok(out)
}

fn decode_data_uri(uri: &str) -> Result<EncodedImage, ProxyError> {
    let body = uri
        .strip_prefix("data:")
        .ok_or_else(|| invalid_image("invalid data URI prefix"))?;
    let (header, payload) = body
        .split_once(',')
        .ok_or_else(|| invalid_image("data URI is missing comma separator"))?;
    let mime = header
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or("application/octet-stream");
    check_mime(mime)?;
    if !header.contains(";base64") {
        return Err(invalid_image(
            "Cursor image data URI must use base64 encoding",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|error| invalid_image(format!("image base64 decode failed: {error}")))?;
    check_size(bytes.len())?;
    Ok(EncodedImage {
        data: Bytes::from(bytes),
        mime_type: Some(mime.to_string()),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

async fn fetch_http(url: &str) -> Result<EncodedImage, ProxyError> {
    let mut current_url = reqwest::Url::parse(url)
        .map_err(|error| invalid_image(format!("image URL invalid: {error}")))?;

    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            cursor_forward_error(format!("build image fetch client failed: {error}"))
        })?;
    let mut response = None;
    for redirect_count in 0..=MAX_REDIRECTS {
        guard_http_url(&current_url).await?;
        let candidate = client
            .get(current_url.clone())
            .send()
            .await
            .map_err(|error| cursor_forward_error(format!("image download failed: {error}")))?;
        if candidate.status().is_redirection() {
            if redirect_count >= MAX_REDIRECTS {
                return Err(invalid_image("image redirect limit exceeded"));
            }
            let location = candidate
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| invalid_image("image redirect is missing Location header"))?;
            current_url = current_url.join(location).map_err(|error| {
                invalid_image(format!("image redirect Location invalid: {error}"))
            })?;
            continue;
        }
        response = Some(candidate);
        break;
    }
    let response = response.ok_or_else(|| invalid_image("image redirect limit exceeded"))?;
    if !response.status().is_success() {
        return Err(cursor_forward_error(format!(
            "image download returned HTTP {}",
            response.status()
        )));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    check_mime(&mime)?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| cursor_forward_error(format!("read image bytes failed: {error}")))?;
    check_size(bytes.len())?;
    Ok(EncodedImage {
        data: bytes,
        mime_type: Some(mime),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

async fn guard_http_url(parsed: &reqwest::Url) -> Result<(), ProxyError> {
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid_image(format!(
            "image URL scheme must be http/https: {}",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| invalid_image("image URL is missing host"))?
        .to_string();
    guard_host(&host)?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let resolved = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| invalid_image(format!("image host resolution failed ({host}): {error}")))?
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        return Err(invalid_image(format!(
            "image host resolved to no addresses: {host}"
        )));
    }
    for address in &resolved {
        guard_ip(&address.ip())?;
    }
    Ok(())
}

fn check_mime(mime: &str) -> Result<(), ProxyError> {
    if !mime.to_ascii_lowercase().starts_with("image/") {
        return Err(invalid_image(format!(
            "image MIME must start with image/: {mime}"
        )));
    }
    Ok(())
}

fn check_size(len: usize) -> Result<(), ProxyError> {
    if len > MAX_IMAGE_BYTES {
        return Err(invalid_image(format!(
            "image exceeds {MAX_IMAGE_BYTES} byte limit; got {len}"
        )));
    }
    Ok(())
}

fn guard_ip(ip: &IpAddr) -> Result<(), ProxyError> {
    let blocked = match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
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
    if [".internal", ".local", ".lan"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
    {
        return Err(invalid_image(format!("image host is blocked: {host}")));
    }
    Ok(())
}

fn invalid_image(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: message.into(),
    }
}

fn cursor_forward_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: message.into(),
    }
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_uri_decodes_base64_image() {
        let image = decode_data_uri("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(image.data, Bytes::from_static(b"hello"));
        assert_eq!(image.mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn rejects_non_base64_data_uri() {
        let error = decode_data_uri("data:image/png,hello").unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn blocks_private_ip() {
        let error = guard_ip(&"127.0.0.1".parse().unwrap()).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn blocks_internal_host_suffixes() {
        for host in ["service.internal", "printer.local.", "router.lan"] {
            let error = guard_host(host).unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
        }
        assert!(guard_host("images.example.com").is_ok());
    }
}
