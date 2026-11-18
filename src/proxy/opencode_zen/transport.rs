//! Zen's verified Bun fetch transport. Stdio carries one bounded request and
//! incremental response; dropping any phase kills the private child process.
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::proxy::ProxyError;

const HELPER: &str = include_str!("fetch.mjs");
const MAX_HEADER: usize = 16 * 1024;
type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, ProxyError>> + Send>>;

pub(crate) struct ZenResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: BodyStream,
}

impl ZenResponse {
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }
    pub(crate) fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub(crate) fn bytes_stream(self) -> BodyStream {
        self.body
    }

    pub(crate) async fn read_limited(&mut self, limit: usize) -> Result<Bytes, ProxyError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.body.next().await {
            let chunk = chunk?;
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(ProxyError::bad_gateway(
                    "OpenCode Zen response exceeds body limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes.into())
    }

    #[cfg(test)]
    pub(crate) fn from_reqwest(response: reqwest::Response) -> Self {
        Self {
            status: response.status(),
            headers: response.headers().clone(),
            body: Box::pin(
                response
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(|_| transport_error())),
            ),
        }
    }
}

fn transport_error() -> ProxyError {
    ProxyError::bad_gateway("OpenCode Zen fetch transport failed")
}

#[derive(Deserialize)]
struct ResponseMetadata {
    status: u16,
    headers: std::collections::BTreeMap<String, String>,
}

pub(crate) async fn post(
    executable: &Path,
    url: &str,
    body: &[u8],
    session: &str,
    stream: bool,
    total_deadline: tokio::time::Instant,
    header_deadline: tokio::time::Instant,
) -> Result<ZenResponse, ProxyError> {
    post_inner(
        executable,
        url,
        body,
        session,
        stream,
        total_deadline,
        header_deadline,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn post_inner(
    executable: &Path,
    url: &str,
    body: &[u8],
    session: &str,
    stream: bool,
    total_deadline: tokio::time::Instant,
    header_deadline: tokio::time::Instant,
    loopback_test: bool,
) -> Result<ZenResponse, ProxyError> {
    let timeout_ms = total_deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .as_millis()
        .min(86_400_000) as u64;
    if timeout_ms == 0 {
        return Err(deadline_error());
    }
    if body.len() > 128 * 1024 * 1024 {
        return Err(ProxyError::bad_request(
            "OpenCode Zen fetch body exceeds transport limit",
        ));
    }
    let headers: std::collections::BTreeMap<_, _> = super::gateway_headers(session, stream)
        .into_iter()
        .collect();
    let family_proxy = if !loopback_test && crate::infra::http::zen_ip_family_selected() {
        Some(FamilyProxy::start(total_deadline).await?)
    } else {
        None
    };
    let fetch_proxy = if let Some(proxy) = &family_proxy {
        Some(proxy.url.clone())
    } else {
        crate::infra::http::outbound_proxy_url().map_err(|_| transport_error())?
    };
    let metadata = serde_json::to_vec(&serde_json::json!({
        "url": url, "headers": headers, "bodyLength": body.len(), "timeoutMs": timeout_ms,
        "proxy": fetch_proxy,
    }))
    .map_err(|_| transport_error())?;
    if metadata.len() > MAX_HEADER {
        return Err(transport_error());
    }
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("BUN_BE_BUN", "1")
        .arg("--eval")
        .arg(HELPER);
    for name in ["SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_EXTRA_CA_CERTS"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if loopback_test {
        command.arg("--").arg("--loopback-test");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| {
            ProxyError::bad_gateway("OpenCode Zen verified fetch runtime could not start")
        })?;
    let mut stdin = child.stdin.take().ok_or_else(transport_error)?;
    let mut stdout = child.stdout.take().ok_or_else(transport_error)?;
    let read = async {
        stdin
            .write_u32(metadata.len() as u32)
            .await
            .map_err(|_| transport_error())?;
        stdin
            .write_all(&metadata)
            .await
            .map_err(|_| transport_error())?;
        stdin.write_all(body).await.map_err(|_| transport_error())?;
        stdin.shutdown().await.map_err(|_| transport_error())?;
        drop(stdin);
        let length = stdout.read_u32().await.map_err(|_| transport_error())? as usize;
        if !(2..=MAX_HEADER).contains(&length) {
            return Err(transport_error());
        }
        let mut encoded = vec![0; length];
        stdout
            .read_exact(&mut encoded)
            .await
            .map_err(|_| transport_error())?;
        let value: ResponseMetadata =
            serde_json::from_slice(&encoded).map_err(|_| transport_error())?;
        let status = StatusCode::from_u16(value.status).map_err(|_| transport_error())?;
        if !status.is_success() && !status.is_client_error() && !status.is_server_error() {
            return Err(transport_error());
        }
        let mut headers = HeaderMap::new();
        for (key, value) in value.headers {
            let name = HeaderName::try_from(key).map_err(|_| transport_error())?;
            let value = HeaderValue::try_from(value).map_err(|_| transport_error())?;
            headers.insert(name, value);
        }
        Ok::<_, ProxyError>((status, headers))
    };
    let (status, headers) = tokio::time::timeout_at(header_deadline, read)
        .await
        .map_err(|_| deadline_error())??;
    let body = async_stream::stream! {
        // Capture ownership before the first poll, including unread responses.
        let mut child = child;
        let _family_proxy = family_proxy;
        let mut buffer = vec![0; 32 * 1024];
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => {
                    match child.wait().await {
                        Ok(status) if status.success() => {}
                        _ => yield Err(transport_error()),
                    }
                    break;
                }
                Ok(n) => yield Ok(Bytes::copy_from_slice(&buffer[..n])),
                Err(_) => { yield Err(transport_error()); break; }
            }
        }
    };
    Ok(ZenResponse {
        status,
        headers,
        body: Box::pin(body),
    })
}

fn deadline_error() -> ProxyError {
    ProxyError {
        status: StatusCode::GATEWAY_TIMEOUT,
        message: "OpenCode Zen fetch deadline exceeded".into(),
    }
}

struct FamilyProxy {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl FamilyProxy {
    async fn start(deadline: tokio::time::Instant) -> Result<Self, ProxyError> {
        use base64::Engine;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| transport_error())?;
        let address = listener.local_addr().map_err(|_| transport_error())?;
        let password = hex::encode(rand::random::<[u8; 16]>());
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("zen:{password}"))
        );
        let task = tokio::spawn(async move {
            let work = async {
                let (mut incoming, _) = listener.accept().await?;
                let mut header = Vec::new();
                loop {
                    let byte = incoming.read_u8().await?;
                    header.push(byte);
                    if header.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    anyhow::ensure!(header.len() < MAX_HEADER, "CONNECT header too large");
                }
                let header = std::str::from_utf8(&header)?;
                anyhow::ensure!(
                    header.starts_with("CONNECT opencode.ai:443 HTTP/1.1\r\n")
                        && header.lines().any(|line| line
                            .split_once(':')
                            .is_some_and(|(key, value)| key
                                .eq_ignore_ascii_case("proxy-authorization")
                                && value.trim() == authorization)),
                    "invalid CONNECT request"
                );
                let mut outgoing = crate::infra::http::connect_zen_tcp("opencode.ai", 443).await?;
                incoming
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await?;
                Ok::<(), anyhow::Error>(())
            };
            let _ = tokio::time::timeout_at(deadline, work).await;
        });
        Ok(Self {
            url: format!("http://zen:{password}@{address}"),
            task,
        })
    }
}
impl Drop for FamilyProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    fn bun() -> std::path::PathBuf {
        std::env::var_os("CC_SWITCH_ZEN_TEST_BUN")
            .map(std::path::PathBuf::from)
            .expect("set CC_SWITCH_ZEN_TEST_BUN to the pinned Bun executable")
    }

    struct Disconnected(Arc<AtomicBool>);
    impl Drop for Disconnected {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    #[ignore = "requires pinned Bun; executed explicitly in CI"]
    async fn zen_bun_incremental_stream_and_unread_drop_close_upstream() {
        for unread in [false, true] {
            let disconnected = Arc::new(AtomicBool::new(false));
            let flag = disconnected.clone();
            let release = Arc::new(tokio::sync::Notify::new());
            let notify = release.clone();
            let app = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let flag = flag.clone(); let notify = notify.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer public");
                    assert_eq!(body, "{\"model\":\"test-free\"}");
                    axum::response::Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(async_stream::stream! {
                        let _disconnected = Disconnected(flag);
                        yield Ok::<_, std::io::Error>(Bytes::from_static(b"data: first\n\n"));
                        notify.notified().await;
                        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                    })).unwrap()
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!(
                "http://{}/inference/openai/v1/chat/completions",
                listener.local_addr().unwrap()
            );
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let response = post_inner(
                &bun(),
                &url,
                br#"{"model":"test-free"}"#,
                "ses_test",
                true,
                deadline,
                deadline,
                true,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            if unread {
                drop(response);
            } else {
                let mut stream = response.bytes_stream();
                let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(first, "data: first\n\n");
                release.notify_one();
                let mut tail = Vec::new();
                while let Some(chunk) = stream.next().await {
                    tail.extend_from_slice(&chunk.unwrap());
                }
                assert_eq!(tail, b"data: [DONE]\n\n");
            }
            tokio::time::timeout(Duration::from_secs(2), async {
                while !disconnected.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("dropping response must close upstream even before the first body poll");
            task.abort();
        }
    }

    #[tokio::test]
    #[ignore = "requires pinned Bun; executed explicitly in CI"]
    async fn zen_bun_error_metadata_limits_and_header_deadline() {
        let app = axum::Router::new().fallback(axum::routing::post(|body: Bytes| async move {
            if body == "wait" {
                std::future::pending::<()>().await;
            }
            axum::response::Response::builder()
                .status(429)
                .header("retry-after", "43200")
                .header("x-request-id", "safe-request")
                .header("content-type", "application/json")
                .body(axum::body::Body::from("typed-error"))
                .unwrap()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/inference/openai/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut response = post_inner(
            &bun(),
            &url,
            b"error",
            "ses_test",
            false,
            deadline,
            deadline,
            true,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "43200");
        assert_eq!(response.read_limited(11).await.unwrap(), "typed-error");
        let mut response = post_inner(
            &bun(),
            &url,
            b"error",
            "ses_test",
            false,
            deadline,
            deadline,
            true,
        )
        .await
        .unwrap();
        assert!(response.read_limited(3).await.is_err());
        let short = tokio::time::Instant::now() + Duration::from_millis(150);
        let result = post_inner(
            &bun(),
            &url,
            b"wait",
            "ses_test",
            true,
            deadline,
            short,
            true,
        )
        .await;
        assert_eq!(result.err().unwrap().status, StatusCode::GATEWAY_TIMEOUT);
        let rejected = post_inner(
            &bun(),
            "http://127.0.0.1:1/not-reviewed",
            b"",
            "ses_test",
            false,
            deadline,
            deadline,
            true,
        )
        .await;
        assert!(rejected.is_err());
        task.abort();
    }
}
