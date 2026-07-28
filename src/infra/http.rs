use anyhow::Context;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::error::{Error as TungsteniteError, UrlError};
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

const PROVISION_IP_FAMILY_ENV: &str = "CC_SWITCH_PROVISION_IP_FAMILY";
pub const OUTBOUND_PROXY_ENV: &str = "CC_SWITCH_OUTBOUND_PROXY";
const MAX_PROXY_RESPONSE_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BoundedResponseBodyError {
    #[error("read upstream response body: {0}")]
    Request(#[from] reqwest::Error),
    #[error("upstream response body exceeds the {limit} byte limit")]
    TooLarge { limit: usize },
}

impl BoundedResponseBodyError {
    pub(crate) fn is_connect(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_connect())
    }
}

pub(crate) async fn read_response_body_limited(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<Bytes, BoundedResponseBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(BoundedResponseBodyError::TooLarge { limit });
    }

    let mut body = Vec::with_capacity(limit.min(16 * 1024));
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(BoundedResponseBodyError::TooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboundProxyConfig {
    endpoint: Url,
    username: Option<String>,
    password: Option<String>,
}

impl OutboundProxyConfig {
    fn connect_authority(&self) -> anyhow::Result<String> {
        url_authority(&self.endpoint).context("outbound proxy URL has no authority")
    }

    fn authorization_header(&self) -> Option<String> {
        let username = self.username.as_deref()?;
        let password = self.password.as_deref().unwrap_or_default();
        Some(format!(
            "Basic {}",
            STANDARD.encode(format!("{username}:{password}"))
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum ProvisionIpFamily {
    V4,
    V6,
}

#[derive(Debug)]
struct ProvisionIpFamilyResolver {
    family: ProvisionIpFamily,
}

impl Resolve for ProvisionIpFamilyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let family = self.family;
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let selected = addresses
                .filter(|address| match family {
                    ProvisionIpFamily::V4 => address.is_ipv4(),
                    ProvisionIpFamily::V6 => address.is_ipv6(),
                })
                .collect::<Vec<SocketAddr>>();
            if selected.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!("{host} has no address in the provisioning IP family"),
                )
                .into());
            }
            Ok(Box::new(selected.into_iter()) as Addrs)
        })
    }
}

fn provision_ip_family() -> Option<ProvisionIpFamily> {
    match std::env::var(PROVISION_IP_FAMILY_ENV).ok().as_deref() {
        Some("4") => Some(ProvisionIpFamily::V4),
        Some("6") => Some(ProvisionIpFamily::V6),
        _ => None,
    }
}

pub fn direct_client_builder() -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(same_origin_redirect_policy());
    if let Some(family) = provision_ip_family() {
        builder = builder.dns_resolver(Arc::new(ProvisionIpFamilyResolver { family }));
    }
    builder
}

/// Builds the shared server outbound client. Proxy environment inherited by
/// reqwest is disabled; only the explicitly validated server setting applies.
pub fn outbound_client_builder() -> anyhow::Result<reqwest::ClientBuilder> {
    let mut builder = direct_client_builder();
    if let Some(proxy) = outbound_proxy_config()? {
        let mut reqwest_proxy = reqwest::Proxy::all(proxy.endpoint.as_str())
            .context("configure explicit outbound HTTP proxy")?;
        if let Some(username) = proxy.username.as_deref() {
            reqwest_proxy =
                reqwest_proxy.basic_auth(username, proxy.password.as_deref().unwrap_or_default());
        }
        builder = builder.proxy(reqwest_proxy);
    }
    Ok(builder)
}

pub(crate) fn outbound_proxy_config() -> anyhow::Result<Option<OutboundProxyConfig>> {
    let Some(value) = std::env::var(OUTBOUND_PROXY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    parse_outbound_proxy_config(&value).map(Some)
}

fn parse_outbound_proxy_config(value: &str) -> anyhow::Result<OutboundProxyConfig> {
    let mut endpoint = Url::parse(value).context("parse CC_SWITCH_OUTBOUND_PROXY")?;
    if endpoint.scheme() != "http" {
        anyhow::bail!("CC_SWITCH_OUTBOUND_PROXY must use http:// for shared HTTP CONNECT support");
    }
    if endpoint.host_str().is_none() {
        anyhow::bail!("CC_SWITCH_OUTBOUND_PROXY must include a host");
    }
    if endpoint.path() != "/" || endpoint.query().is_some() || endpoint.fragment().is_some() {
        anyhow::bail!("CC_SWITCH_OUTBOUND_PROXY must not include a path, query, or fragment");
    }
    let username = (!endpoint.username().is_empty()).then(|| endpoint.username().to_string());
    let password = endpoint.password().map(str::to_string);
    if username.is_none() && password.is_some() {
        anyhow::bail!("CC_SWITCH_OUTBOUND_PROXY must not include a password without a username");
    }
    endpoint
        .set_username("")
        .map_err(|_| anyhow::anyhow!("clear outbound proxy username"))?;
    endpoint
        .set_password(None)
        .map_err(|_| anyhow::anyhow!("clear outbound proxy password"))?;
    Ok(OutboundProxyConfig {
        endpoint,
        username,
        password,
    })
}

pub(crate) async fn connect_websocket(
    request: Request,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), TungsteniteError> {
    let Some(proxy) = outbound_proxy_config().map_err(invalid_proxy_websocket_error)? else {
        return tokio_tungstenite::connect_async(request).await;
    };
    connect_websocket_via_http_proxy(request, &proxy).await
}

async fn connect_websocket_via_http_proxy(
    request: Request,
    proxy: &OutboundProxyConfig,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), TungsteniteError> {
    let target = Url::parse(&request.uri().to_string())
        .map_err(|_| TungsteniteError::Url(UrlError::UnsupportedUrlScheme))?;
    if !matches!(target.scheme(), "ws" | "wss") {
        return Err(TungsteniteError::Url(UrlError::UnsupportedUrlScheme));
    }
    let target_authority =
        url_authority(&target).ok_or(TungsteniteError::Url(UrlError::NoHostName))?;
    let proxy_authority = proxy
        .connect_authority()
        .map_err(invalid_proxy_websocket_error)?;
    let mut stream = TcpStream::connect(proxy_authority)
        .await
        .map_err(TungsteniteError::Io)?;
    let mut connect_request = format!(
        "CONNECT {target_authority} HTTP/1.1\r\nHost: {target_authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(authorization) = proxy.authorization_header() {
        connect_request.push_str("Proxy-Authorization: ");
        connect_request.push_str(&authorization);
        connect_request.push_str("\r\n");
    }
    connect_request.push_str("\r\n");
    stream
        .write_all(connect_request.as_bytes())
        .await
        .map_err(TungsteniteError::Io)?;

    let status = read_connect_response_status(&mut stream).await?;
    if !(200..300).contains(&status) {
        let status = http::StatusCode::from_u16(status).map_err(|_| {
            TungsteniteError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "outbound proxy CONNECT status is invalid",
            ))
        })?;
        let response = http::Response::builder()
            .status(status)
            .body(Some(Vec::new()))
            .expect("validated CONNECT status builds an HTTP response");
        return Err(TungsteniteError::Http(response));
    }

    tokio_tungstenite::client_async_tls_with_config(request, stream, None, None).await
}

async fn read_connect_response_status(stream: &mut TcpStream) -> Result<u16, TungsteniteError> {
    let mut response = Vec::with_capacity(1024);
    let header_end = loop {
        if let Some(index) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = index + 4;
            if header_end > MAX_PROXY_RESPONSE_HEADER_BYTES {
                return Err(proxy_response_header_too_large(header_end));
            }
            break header_end;
        }
        if response.len() >= MAX_PROXY_RESPONSE_HEADER_BYTES {
            return Err(proxy_response_header_too_large(response.len()));
        }
        let mut chunk = [0u8; 1024];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(TungsteniteError::Io)?;
        if read == 0 {
            return Err(TungsteniteError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "outbound proxy closed before CONNECT response",
            )));
        }
        response.extend_from_slice(&chunk[..read]);
    };
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut headers);
    match parsed.parse(&response[..header_end]) {
        Ok(httparse::Status::Complete(_)) => parsed.code.ok_or_else(|| {
            TungsteniteError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "outbound proxy CONNECT response has no status",
            ))
        }),
        Ok(httparse::Status::Partial) | Err(_) => Err(TungsteniteError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "outbound proxy CONNECT response is invalid",
        ))),
    }
}

fn proxy_response_header_too_large(size: usize) -> TungsteniteError {
    TungsteniteError::Capacity(
        tokio_tungstenite::tungstenite::error::CapacityError::MessageTooLong {
            size,
            max_size: MAX_PROXY_RESPONSE_HEADER_BYTES,
        },
    )
}

fn invalid_proxy_websocket_error(error: impl std::fmt::Display) -> TungsteniteError {
    TungsteniteError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        error.to_string(),
    ))
}

fn url_authority(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Some(format!("{host}:{}", url.port_or_known_default()?))
}

pub fn self_update_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(self_update_redirect_policy())
}

fn same_origin(previous: &reqwest::Url, target: &reqwest::Url) -> bool {
    previous.scheme() == target.scheme()
        && previous.host_str() == target.host_str()
        && previous.port_or_known_default() == target.port_or_known_default()
}

fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.follow();
        };
        if same_origin(origin, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn self_update_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.follow();
        };
        let target = attempt.url();
        if same_origin(origin, target) {
            return attempt.follow();
        }
        if attempt.previous().iter().all(is_github_release_url) && is_github_release_url(target) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn is_github_release_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    matches!(
        host,
        "github.com" | "api.github.com" | "githubusercontent.com"
    ) || host.ends_with(".githubusercontent.com")
}

pub fn direct_client() -> anyhow::Result<reqwest::Client> {
    direct_client_builder()
        .build()
        .context("build direct HTTP client")
}

#[cfg(test)]
mod tests {
    use reqwest::Url;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    #[test]
    fn direct_client_ignores_proxy_environment() {
        super::direct_client().unwrap();
    }

    #[test]
    fn explicit_outbound_proxy_is_sanitized_and_shared() {
        let proxy =
            super::parse_outbound_proxy_config("http://alice:secret@127.0.0.1:8080").unwrap();
        assert_eq!(proxy.endpoint.as_str(), "http://127.0.0.1:8080/");
        assert_eq!(proxy.connect_authority().unwrap(), "127.0.0.1:8080");
        assert_eq!(
            proxy.authorization_header().as_deref(),
            Some("Basic YWxpY2U6c2VjcmV0")
        );
    }

    #[test]
    fn explicit_outbound_proxy_rejects_non_connect_compatible_urls() {
        for value in [
            "https://proxy.example:8443",
            "socks5://proxy.example:1080",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080/?token=secret",
            "http://:secret@proxy.example:8080",
        ] {
            assert!(
                super::parse_outbound_proxy_config(value).is_err(),
                "{value}"
            );
        }
    }

    #[tokio::test]
    async fn connect_response_rejects_a_header_terminator_beyond_the_limit() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut response = b"HTTP/1.1 200 Connection Established\r\nX-Fill: ".to_vec();
            response.resize(super::MAX_PROXY_RESPONSE_HEADER_BYTES + 8, b'x');
            response.extend_from_slice(b"\r\n\r\n");
            stream.write_all(&response).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let error = super::read_connect_response_status(&mut stream)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            tokio_tungstenite::tungstenite::Error::Capacity(_)
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_connect_uses_authenticated_http_tunnel() {
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target_listener.local_addr().unwrap();
        let target = tokio::spawn(async move {
            let (stream, _) = target_listener.accept().await.unwrap();
            let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            websocket.close(None).await.unwrap();
        });

        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_address = proxy_listener.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            let (mut downstream, _) = proxy_listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let read = downstream.read(&mut chunk).await.unwrap();
                assert!(read > 0, "proxy client closed before CONNECT request");
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with(&format!("CONNECT {target_address} HTTP/1.1\r\n")));
            assert!(request.contains("Proxy-Authorization: Basic YWxpY2U6c2VjcmV0\r\n"));

            let mut upstream = tokio::net::TcpStream::connect(target_address)
                .await
                .unwrap();
            downstream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            tokio::io::copy_bidirectional(&mut downstream, &mut upstream)
                .await
                .unwrap();
        });

        let proxy_config =
            super::parse_outbound_proxy_config(&format!("http://alice:secret@{proxy_address}"))
                .unwrap();
        let request = format!("ws://{target_address}/v1/responses")
            .into_client_request()
            .unwrap();
        let (mut websocket, response) =
            super::connect_websocket_via_http_proxy(request, &proxy_config)
                .await
                .unwrap();
        assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
        websocket.close(None).await.unwrap();
        drop(websocket);

        tokio::time::timeout(std::time::Duration::from_secs(2), target)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), proxy)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn self_update_redirects_are_limited_to_github_https_hosts() {
        for value in [
            "https://github.com/example/project/releases/download/latest/asset",
            "https://api.github.com/repos/example/project/releases/tags/latest",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2",
        ] {
            assert!(super::is_github_release_url(&Url::parse(value).unwrap()));
        }
        for value in [
            "http://github.com/example/project/releases/download/latest/asset",
            "https://github.com:8443/example/project/releases/download/latest/asset",
            "https://github.com.example.invalid/asset",
            "https://example.invalid/asset",
        ] {
            assert!(!super::is_github_release_url(&Url::parse(value).unwrap()));
        }
    }
}
