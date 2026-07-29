//! Bidirectional HTTP/2 client for Cursor's `agent.v1.AgentService/Run`.
//!
//! Unlike the existing `protocol::send_cursor_request` path — which sends
//! one fixed-size protobuf and reads the response stream — `AgentService/Run`
//! is **client-streaming + server-streaming**. After the initial RunRequest
//! frame we may still need to write additional Connect-RPC frames (e.g.
//! `ExecClientMessage.RequestContextResult`, `McpResult`, `KvClient` blob
//! replies) while continuing to read server frames on the same h2 stream.
//!
//! This module sets up a hyper-1.x h2 client with a `StreamBody` whose stream
//! is fed by an mpsc channel, so the caller can `send_frame()` at any point
//! before closing the request side. The response body is parsed incrementally
//! through `ConnectFrameParser`.

use super::agent_proto::{
    is_end_stream, parse_trailers, ConnectFrame, ConnectFrameParser, ProtoError,
};
use crate::proxy::ProxyError;
use async_stream::stream;
use axum::http::StatusCode;
use bytes::Bytes;
use futures_util::Stream;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use http_body::Frame;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

pub const DEFAULT_AGENTSERVICE_BASE_URL: &str = "https://agentn.global.api5.cursor.sh";
const AGENT_PATH: &str = "/agent.v1.AgentService/Run";

/// Internal body stream item shape: data Frames carried by an mpsc channel.
type BodyStream = Pin<Box<dyn Stream<Item = Result<Frame<Bytes>, std::io::Error>> + Send>>;

/// One opened HTTP/2 client-streaming request to Cursor's AgentService.
///
/// Holds a writer side (mpsc sender → request body) and a reader side
/// (`hyper::body::Incoming` → `ConnectFrameParser`). Drop closes the request
/// body channel, which signals end-of-client-stream to hyper.
pub struct CursorH2Stream {
    writer: Option<UnboundedSender<Bytes>>,
    response: hyper::Response<Incoming>,
    parser: ConnectFrameParser,
    trailers: Option<HeaderMap>,
    pending: std::collections::VecDeque<ConnectFrame>,
    closed: bool,
    received_any_frame: bool,
    timeouts: CursorH2Timeouts,
    connect_grpc_status: Option<u32>,
    connect_grpc_message: Option<String>,
}

const ERROR_BODY_READ_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
pub struct CursorH2Timeouts {
    pub request: Duration,
    pub first_frame: Option<Duration>,
    pub inter_frame: Option<Duration>,
}

impl Default for CursorH2Timeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(300),
            first_frame: Some(Duration::from_secs(120)),
            inter_frame: Some(Duration::from_secs(300)),
        }
    }
}

impl CursorH2Stream {
    /// Open a fresh h2 stream to Cursor's AgentService endpoint,
    /// write the first Connect-RPC frame containing the encoded RunRequest,
    /// and return the live stream handle. Additional frames can be written
    /// via [`send_frame`].
    pub async fn open(
        base_url: &str,
        headers: Vec<(String, String)>,
        first_frame: Bytes,
        timeouts: CursorH2Timeouts,
    ) -> Result<Self, ProxyError> {
        let url = agentservice_url(base_url);
        let uri = url
            .parse::<http::Uri>()
            .map_err(|e| cursor_forward_error(format!("解析 Cursor URL 失败: {e}")))?;

        // ALPN-negotiated h2 via hyper-rustls. The legacy ALB on cursor's
        // edge rejects h2-prior-knowledge with 464, so we advertise h2 via
        // ALPN and refuse HTTP/1.1 downgrades.
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http2()
            .build();
        let mut builder = Client::builder(TokioExecutor::new());
        builder.http2_only(true);
        builder.http2_adaptive_window(true);

        let (tx, rx) = unbounded_channel::<Bytes>();
        let initial = first_frame;
        // Convert the mpsc receiver to a stream of body Frames. The initial
        // frame is enqueued before we await — guarantees the first byte hits
        // the wire as soon as hyper opens the stream.
        let _ = tx.send(initial);
        let body_stream: BodyStream = Box::pin(stream! {
            let mut rx = rx;
            while let Some(chunk) = rx.recv().await {
                yield Ok::<_, std::io::Error>(Frame::data(chunk));
            }
        });
        let body = BodyExt::boxed_unsync(StreamBody::new(body_stream));

        // Build the request with cursor-agent's actual headers (caller passes
        // identity headers — auth, machine id, checksum, content-type).
        let mut req = http::Request::builder()
            .method(http::Method::POST)
            .uri(uri)
            .body(body)
            .map_err(|e| cursor_forward_error(format!("创建 Cursor Agent 请求失败: {e}")))?;
        for (k, v) in &headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| cursor_forward_error(format!("Cursor 请求头名称无效: {e}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| cursor_forward_error(format!("Cursor 请求头值无效: {e}")))?;
            req.headers_mut().insert(name, value);
        }

        let client: Client<_, http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>> =
            builder.build(https);

        let response = tokio::time::timeout(timeouts.request, client.request(req))
            .await
            .map_err(|_| cursor_forward_error("Cursor AgentService 请求超时"))?
            .map_err(|e| cursor_forward_error(format!("Cursor AgentService 请求失败: {e}")))?;

        Ok(Self {
            writer: Some(tx),
            response,
            parser: ConnectFrameParser::new(),
            trailers: None,
            pending: std::collections::VecDeque::new(),
            closed: false,
            received_any_frame: false,
            timeouts,
            connect_grpc_status: None,
            connect_grpc_message: None,
        })
    }

    pub fn status(&self) -> http::StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    /// Send a Connect-RPC framed payload on the request body. Returns Err if
    /// the writer has been dropped (i.e. the request body has been closed).
    pub fn send_frame(&self, frame: Bytes) -> Result<(), ProxyError> {
        let tx = self
            .writer
            .as_ref()
            .ok_or_else(|| cursor_forward_error("Cursor h2 stream 已关闭，无法继续写入"))?;
        tx.send(frame)
            .map_err(|_| cursor_forward_error("Cursor h2 stream 已关闭，无法继续写入"))
    }

    /// Signal end-of-client-stream. Drops the live mpsc sender so hyper emits
    /// H2 END_STREAM on the request body. After this, [`send_frame`] fails fast.
    pub fn close_writer(&mut self) {
        self.writer = None;
    }

    pub async fn read_body_limited(&mut self, max_bytes: usize) -> Result<Bytes, ProxyError> {
        let mut out = bytes::BytesMut::new();
        let read = async {
            while out.len() < max_bytes {
                let Some(frame) = self.response.body_mut().frame().await else {
                    self.closed = true;
                    break;
                };
                let frame = frame.map_err(|error| {
                    cursor_forward_error(format!("Cursor error body read failed: {error}"))
                })?;
                if frame.is_trailers() {
                    if let Ok(trailers) = frame.into_trailers() {
                        self.trailers = Some(trailers);
                    }
                    continue;
                }
                if let Ok(data) = frame.into_data() {
                    let remaining = max_bytes.saturating_sub(out.len());
                    out.extend_from_slice(&data[..data.len().min(remaining)]);
                }
            }
            Ok::<Bytes, ProxyError>(out.freeze())
        };
        tokio::time::timeout(ERROR_BODY_READ_TIMEOUT, read)
            .await
            .map_err(|_| cursor_forward_error("Cursor error body read timed out"))?
    }

    /// Pull the next decoded Connect-RPC frame from the response body. Returns
    /// `Ok(None)` when the response body has ended cleanly. Trailers are
    /// captured into `self.trailers` and don't surface as frames.
    pub async fn next_frame(&mut self) -> Result<Option<ConnectFrame>, ProxyError> {
        if let Some(frame) = self.pending.pop_front() {
            self.received_any_frame = true;
            return Ok(Some(frame));
        }
        if self.closed {
            return self.terminal_result();
        }

        loop {
            let timeout = if self.received_any_frame {
                self.timeouts.inter_frame
            } else {
                self.timeouts.first_frame
            };
            let frame_result = match timeout {
                Some(timeout) => tokio::time::timeout(timeout, self.response.body_mut().frame())
                    .await
                    .map_err(|_| {
                        let timeout_ms = timeout.as_millis();
                        if !self.received_any_frame {
                            cursor_forward_error(format!(
                                "Cursor AgentService first frame timed out after {timeout_ms}ms"
                            ))
                        } else {
                            cursor_forward_error(format!(
                                "Cursor AgentService stream idled for {timeout_ms}ms"
                            ))
                        }
                    })?,
                None => self.response.body_mut().frame().await,
            };

            let body_frame = match frame_result {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    return Err(cursor_forward_error(format!("Cursor 响应流读取失败: {e}")));
                }
                None => {
                    self.closed = true;
                    return self.terminal_result();
                }
            };

            if body_frame.is_trailers() {
                if let Ok(t) = body_frame.into_trailers() {
                    self.trailers = Some(t);
                }
                continue;
            }
            if let Ok(data) = body_frame.into_data() {
                let new_frames = self.parser.feed(&data).map_err(map_proto_err)?;
                for f in new_frames {
                    if is_end_stream(&f) {
                        self.capture_connect_end_stream(&f);
                        self.closed = true;
                    } else {
                        self.pending.push_back(f);
                    }
                }
                if let Some(f) = self.pending.pop_front() {
                    self.received_any_frame = true;
                    return Ok(Some(f));
                }
                if self.closed {
                    return self.terminal_result();
                }
                // Empty data frame — keep reading.
                continue;
            }
        }
    }

    /// Whether we have received at least one server frame on this stream.
    pub fn received_any_frame(&self) -> bool {
        self.received_any_frame
    }

    /// Trailers captured after the response body ended. `grpc-status` /
    /// `grpc-message` typically live here for Connect-RPC over h2.
    pub fn trailers(&self) -> Option<&HeaderMap> {
        self.trailers.as_ref()
    }

    /// Connect-RPC grpc-status code from trailers. `0` = OK.
    pub fn grpc_status(&self) -> Option<u32> {
        self.connect_grpc_status.or_else(|| {
            self.trailers
                .as_ref()
                .and_then(|t| t.get("grpc-status"))
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse().ok())
        })
    }

    pub fn grpc_message(&self) -> Option<String> {
        self.connect_grpc_message
            .clone()
            .or_else(|| {
                self.trailers
                    .as_ref()
                    .and_then(|t| t.get("grpc-message"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            })
            .map(|message| {
                crate::logging::redact_sensitive_text(&message)
                    .chars()
                    .take(512)
                    .collect()
            })
    }

    fn capture_connect_end_stream(&mut self, frame: &ConnectFrame) {
        for (name, value) in parse_trailers(&frame.payload) {
            match name.as_str() {
                "grpc-status" | "connect-error-code" => {
                    self.connect_grpc_status = value.trim().parse().ok();
                }
                "grpc-message" | "connect-error-message" => {
                    self.connect_grpc_message = Some(value);
                }
                _ => {}
            }
        }
    }

    fn terminal_result(&self) -> Result<Option<ConnectFrame>, ProxyError> {
        match self.grpc_status() {
            Some(0) | None => Ok(None),
            Some(status) => Err(cursor_forward_error(format!(
                "Cursor AgentService terminated with grpc-status {status}: {}",
                self.grpc_message()
                    .as_deref()
                    .unwrap_or("upstream returned no grpc-message")
            ))),
        }
    }
}

fn agentservice_url(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.ends_with(AGENT_PATH) {
        base_url.to_string()
    } else if base_url.is_empty() {
        format!("{DEFAULT_AGENTSERVICE_BASE_URL}{AGENT_PATH}")
    } else {
        format!("{base_url}{AGENT_PATH}")
    }
}

fn map_proto_err(e: ProtoError) -> ProxyError {
    cursor_forward_error(format!("Cursor Connect-RPC 解码失败: {e}"))
}

fn cursor_forward_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: message.into(),
    }
}

/// Drain whatever frames are already in the response body without blocking
/// for more. Returns immediately if no whole frame is currently available.
/// Useful for tests and for non-blocking polling.
#[cfg(test)]
pub async fn try_drain_one(stream: &mut CursorH2Stream) -> Option<ConnectFrame> {
    use futures_util::FutureExt;
    stream
        .next_frame()
        .now_or_never()
        .and_then(Result::ok)
        .flatten()
}

/// Shape of the headers cursor-agent sends on every `agent.v1` request.
/// Auth/machine/checksum specifics come from `identity_headers` in
/// `protocol.rs`; this helper just enforces the Connect-RPC content
/// type and protocol headers.
pub fn agent_connect_headers() -> Vec<(String, String)> {
    vec![
        (
            "content-type".to_string(),
            "application/connect+proto".to_string(),
        ),
        ("connect-protocol-version".to_string(), "1".to_string()),
        // Connect-RPC uses the connect-accept-encoding header (not standard
        // Accept-Encoding). Only advertise gzip — our frame decoder only
        // handles gzip, and brotli-compressed frames would be silently
        // skipped. Matches OmniRoute's cursor executor.
        ("connect-accept-encoding".to_string(), "gzip".to_string()),
        ("user-agent".to_string(), "connect-es/1.6.1".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_connect_headers_include_connect_protocol() {
        let hs = agent_connect_headers();
        assert!(hs
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/connect+proto"));
        assert!(hs.iter().any(|(k, _)| k == "connect-protocol-version"));
        assert!(hs
            .iter()
            .any(|(k, v)| k == "connect-accept-encoding" && v == "gzip"));
        assert!(hs
            .iter()
            .any(|(k, v)| k == "user-agent" && v == "connect-es/1.6.1"));
    }

    #[test]
    fn agentservice_url_accepts_base_or_full_endpoint() {
        assert_eq!(
            agentservice_url("https://agent.example"),
            "https://agent.example/agent.v1.AgentService/Run"
        );
        assert_eq!(
            agentservice_url("https://agent.example/agent.v1.AgentService/Run"),
            "https://agent.example/agent.v1.AgentService/Run"
        );
        assert_eq!(
            agentservice_url(""),
            "https://agentn.global.api5.cursor.sh/agent.v1.AgentService/Run"
        );
    }
}
