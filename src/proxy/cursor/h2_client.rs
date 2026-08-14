//! Bidirectional HTTP/2 client for Cursor's runtime-configured AgentService method.
//!
//! Unlike the existing `protocol::send_cursor_request` path — which sends
//! one fixed-size protobuf and reads the response stream — this method is
//! **client-streaming + server-streaming**. After the initial RunRequest
//! frame we may still need to write additional Connect-RPC frames (e.g.
//! `ExecClientMessage.RequestContextResult`, `McpResult`, `KvClient` blob
//! replies) while continuing to read server frames on the same h2 stream.
//!
//! This module sets up a hyper-1.x h2 client with a `StreamBody` whose stream
//! is fed by an mpsc channel, so the caller can `send_frame()` at any point
//! before closing the request side. The response body is parsed incrementally
//! through `ConnectFrameParser`.

use super::agent_proto::{is_end_stream, ConnectFrame, ConnectFrameParser, ProtoError};
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
use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::time::Instant as TokioInstant;

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
    pending: VecDeque<ConnectFrame>,
    closed: bool,
    received_any_frame: bool,
    output_phase: CursorOutputPhase,
    connect_end_stream: Option<Result<(), String>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorOutputDeadlineKind {
    FirstBusinessOutput,
    Idle,
}

#[derive(Debug, Clone, Copy)]
struct CursorOutputPhase {
    first_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    first_deadline: Option<TokioInstant>,
    idle_deadline: Option<TokioInstant>,
    awaiting_business_output: bool,
}

impl CursorOutputPhase {
    fn new(timeouts: CursorH2Timeouts, started_at: TokioInstant) -> Self {
        Self {
            first_timeout: timeouts.first_frame,
            idle_timeout: timeouts.inter_frame,
            first_deadline: timeouts.first_frame.map(|timeout| started_at + timeout),
            idle_deadline: None,
            awaiting_business_output: true,
        }
    }

    fn deadline(&self) -> Option<(TokioInstant, CursorOutputDeadlineKind, Duration)> {
        if self.awaiting_business_output {
            self.first_deadline
                .zip(self.first_timeout)
                .map(|(deadline, timeout)| {
                    (
                        deadline,
                        CursorOutputDeadlineKind::FirstBusinessOutput,
                        timeout,
                    )
                })
        } else {
            self.idle_deadline
                .zip(self.idle_timeout)
                .map(|(deadline, timeout)| (deadline, CursorOutputDeadlineKind::Idle, timeout))
        }
    }

    fn on_complete_protocol_frame(&mut self, now: TokioInstant) {
        if !self.awaiting_business_output {
            self.idle_deadline = self.idle_timeout.map(|timeout| now + timeout);
        }
    }

    fn mark_business_output(&mut self, now: TokioInstant) {
        if self.awaiting_business_output {
            self.awaiting_business_output = false;
            self.first_deadline = None;
        }
        self.idle_deadline = self.idle_timeout.map(|timeout| now + timeout);
    }

    fn rearm(&mut self, now: TokioInstant) {
        self.awaiting_business_output = true;
        self.first_deadline = self.first_timeout.map(|timeout| now + timeout);
        self.idle_deadline = None;
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
        let uri = base_url.trim().parse::<http::Uri>().map_err(|e| {
            cursor_forward_error(format!(
                "Cursor AgentService endpoint is invalid: {}",
                cursor_transport_diagnostic(&e)
            ))
        })?;

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

        let sent_at = TokioInstant::now();
        let output_phase = CursorOutputPhase::new(timeouts, sent_at);
        let request_deadline = sent_at + timeouts.request;
        let (header_deadline, header_timeout_kind, header_timeout) = output_phase
            .deadline()
            .filter(|(deadline, _, _)| *deadline <= request_deadline)
            .unwrap_or((
                request_deadline,
                CursorOutputDeadlineKind::Idle,
                timeouts.request,
            ));
        let response = tokio::time::timeout_at(header_deadline, client.request(req))
            .await
            .map_err(|_| {
                if header_timeout_kind == CursorOutputDeadlineKind::FirstBusinessOutput {
                    cursor_output_timeout(header_timeout_kind, header_timeout, "response headers")
                } else {
                    cursor_timeout_error(format!(
                        "Cursor AgentService request timed out after {}ms",
                        header_timeout.as_millis()
                    ))
                }
            })?
            .map_err(|e| {
                cursor_forward_error(format!(
                    "Cursor AgentService request failed: {}",
                    cursor_transport_diagnostic(&e)
                ))
            })?;

        Ok(Self {
            writer: Some(tx),
            response,
            parser: ConnectFrameParser::new(),
            trailers: None,
            pending: std::collections::VecDeque::new(),
            closed: false,
            received_any_frame: false,
            output_phase,
            connect_end_stream: None,
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

    /// Confirm that the driver decoded a client-visible/business event.
    /// Protocol/control frames alone never call this method.
    pub fn mark_business_output(&mut self) {
        self.output_phase.mark_business_output(TokioInstant::now());
    }

    /// Start a fresh first-business-output phase after MCP results are written
    /// to the same parked h2 stream.
    pub fn rearm_business_output_phase(&mut self) {
        self.output_phase.rearm(TokioInstant::now());
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
                    cursor_forward_error(format!(
                        "Cursor error body read failed: {}",
                        cursor_transport_diagnostic(&error)
                    ))
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
    /// `Ok(None)` after a validated Connect terminal envelope or a clean HTTP
    /// EOF with successful trailers. Trailers are captured into `self.trailers`
    /// and don't surface as frames.
    pub async fn next_frame(&mut self) -> Result<Option<ConnectFrame>, ProxyError> {
        if let Some(frame) = self.pending.pop_front() {
            self.note_complete_protocol_frame();
            return Ok(Some(frame));
        }
        if self.closed {
            return self.terminal_result();
        }

        loop {
            let deadline = self.output_phase.deadline();
            let frame_result = match deadline {
                Some((deadline, kind, timeout)) => {
                    tokio::time::timeout_at(deadline, self.response.body_mut().frame())
                        .await
                        .map_err(|_| cursor_output_timeout(kind, timeout, "response body"))?
                }
                None => self.response.body_mut().frame().await,
            };

            let body_frame = match frame_result {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    return Err(cursor_forward_error(format!(
                        "Cursor response stream read failed: {}",
                        cursor_transport_diagnostic(&e)
                    )));
                }
                None => {
                    self.closed = true;
                    self.parser.finish().map_err(map_proto_err)?;
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
                ingest_connect_frames(&mut self.pending, &mut self.connect_end_stream, new_frames)?;
                // A Connect end-stream envelope is the protocol terminal. Do
                // not wait for a separate HTTP EOF that some upstreams keep
                // open after the envelope has already completed the RPC.
                if self.connect_end_stream.is_some() {
                    self.closed = true;
                }
                if let Some(f) = self.pending.pop_front() {
                    self.note_complete_protocol_frame();
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

    fn note_complete_protocol_frame(&mut self) {
        self.received_any_frame = true;
        self.output_phase
            .on_complete_protocol_frame(TokioInstant::now());
    }

    /// Trailers captured after the response body ended. `grpc-status` /
    /// `grpc-message` typically live here for Connect-RPC over h2.
    pub fn trailers(&self) -> Option<&HeaderMap> {
        self.trailers.as_ref()
    }

    /// Connect-RPC grpc-status code from trailers. `0` = OK.
    pub fn grpc_status(&self) -> Option<u32> {
        self.trailers
            .as_ref()
            .and_then(|t| t.get("grpc-status"))
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse().ok())
    }

    pub fn grpc_message(&self) -> Option<String> {
        self.trailers
            .as_ref()
            .and_then(|t| t.get("grpc-message"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .map(|message| cursor_transport_diagnostic(&message))
    }

    fn terminal_result(&self) -> Result<Option<ConnectFrame>, ProxyError> {
        self.parser.finish().map_err(map_proto_err)?;
        validate_cursor_terminal(
            self.grpc_status(),
            self.connect_end_stream.as_ref(),
            self.grpc_message().as_deref(),
        )?;
        Ok(None)
    }
}

fn ingest_connect_frames(
    pending: &mut VecDeque<ConnectFrame>,
    connect_end_stream: &mut Option<Result<(), String>>,
    frames: Vec<ConnectFrame>,
) -> Result<(), ProxyError> {
    for frame in frames {
        if is_end_stream(&frame) {
            if connect_end_stream.is_some() {
                *connect_end_stream = Some(Err(
                    "multiple Connect-RPC terminal envelopes were received".to_string(),
                ));
            } else {
                *connect_end_stream = Some(parse_connect_end_stream(&frame.payload));
            }
        } else if connect_end_stream.is_some() {
            *connect_end_stream = Some(Err(
                "data frame received after the Connect-RPC terminal envelope".to_string(),
            ));
        } else {
            pending.push_back(frame);
        }
    }
    if connect_end_stream.as_ref().is_some_and(Result::is_err) {
        validate_cursor_terminal(None, connect_end_stream.as_ref(), None)?;
    }
    Ok(())
}

fn validate_cursor_terminal(
    grpc_status: Option<u32>,
    connect_end_stream: Option<&Result<(), String>>,
    grpc_message: Option<&str>,
) -> Result<(), ProxyError> {
    if let Some(status) = grpc_status {
        if status != 0 {
            return Err(cursor_forward_error(format!(
                "Cursor AgentService terminated with grpc-status {status}: {}",
                grpc_message.unwrap_or("upstream returned no grpc-message")
            )));
        }
    }
    if let Some(Err(message)) = connect_end_stream {
        let message = cursor_transport_diagnostic(message);
        return Err(cursor_forward_error(format!(
            "Cursor AgentService returned an invalid or failed Connect-RPC terminal envelope: {}",
            message.chars().take(512).collect::<String>()
        )));
    }
    if grpc_status == Some(0) || matches!(connect_end_stream, Some(Ok(()))) {
        return Ok(());
    }
    Err(cursor_forward_error(
        "Cursor AgentService response ended without a valid Connect-RPC terminal envelope or grpc-status",
    ))
}

fn parse_connect_end_stream(payload: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(payload)
        .map_err(|error| format!("terminal envelope is not valid UTF-8: {error}"))?
        .trim();
    if text.is_empty() {
        return Err("terminal envelope is empty".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| format!("terminal envelope is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "terminal envelope must be a JSON object".to_string())?;
    if object
        .get("metadata")
        .is_some_and(|metadata| !metadata.is_object() && !metadata.is_null())
    {
        return Err("terminal envelope metadata must be an object".to_string());
    }
    let Some(error) = object.get("error").filter(|error| !error.is_null()) else {
        return Ok(());
    };
    let error = error
        .as_object()
        .ok_or_else(|| "terminal envelope error must be an object".to_string())?;
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Err(match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "upstream returned a Connect-RPC error".to_string(),
    })
}

fn map_proto_err(e: ProtoError) -> ProxyError {
    cursor_forward_error(format!("Cursor Connect-RPC 解码失败: {e}"))
}

pub(super) fn cursor_transport_diagnostic(error: impl std::fmt::Display) -> String {
    let value = crate::logging::redact_sensitive_text(&error.to_string());
    redact_absolute_urls(&value).chars().take(512).collect()
}

fn redact_absolute_urls(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let remaining = &value[cursor..];
        let next_http = find_ascii_case_insensitive(remaining, b"http://");
        let next_https = find_ascii_case_insensitive(remaining, b"https://");
        let Some(relative_start) = next_http.into_iter().chain(next_https).min() else {
            output.push_str(remaining);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let end = value[start..]
            .char_indices()
            .find(|(_, character)| {
                character.is_whitespace()
                    || matches!(
                        character,
                        '"' | '\''
                            | '<'
                            | '>'
                            | ')'
                            | ']'
                            | '}'
                            | '，'
                            | '。'
                            | '；'
                            | '！'
                            | '？'
                            | '）'
                            | '】'
                            | '》'
                            | '」'
                            | '』'
                    )
            })
            .map(|(offset, _)| start + offset)
            .unwrap_or(value.len());
        output.push_str("[REDACTED_CURSOR_URL]");
        cursor = end;
    }
    output
}

fn find_ascii_case_insensitive(haystack: &str, needle: &[u8]) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn cursor_forward_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: message.into(),
    }
}

fn cursor_timeout_error(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::GATEWAY_TIMEOUT,
        message: message.into(),
    }
}

fn cursor_output_timeout(
    kind: CursorOutputDeadlineKind,
    timeout: Duration,
    stage: &str,
) -> ProxyError {
    let timeout_ms = timeout.as_millis();
    match kind {
        CursorOutputDeadlineKind::FirstBusinessOutput => cursor_timeout_error(format!(
            "Cursor AgentService first business output timed out after {timeout_ms}ms while waiting for {stage}"
        )),
        CursorOutputDeadlineKind::Idle => cursor_timeout_error(format!(
            "Cursor AgentService stream idled for {timeout_ms}ms while waiting for {stage}"
        )),
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
/// Credential-rail identity headers are added by `identity.rs`; this helper
/// only enforces the Connect-RPC content type and protocol headers.
pub fn agent_connect_headers(accept_gzip: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "content-type".to_string(),
            "application/connect+proto".to_string(),
        ),
        ("connect-protocol-version".to_string(), "1".to_string()),
        // Connect-RPC uses the connect-accept-encoding header (not standard
        // Accept-Encoding). Only advertise gzip — our frame decoder only
        // handles gzip, and brotli-compressed frames would be silently
        // skipped. Matches OmniRoute's cursor executor.
        ("user-agent".to_string(), "connect-es/1.6.1".to_string()),
    ];
    if accept_gzip {
        headers.push(("connect-accept-encoding".to_string(), "gzip".to_string()));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_connect_headers_include_connect_protocol() {
        let hs = agent_connect_headers(true);
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
    fn sdk_headers_do_not_advertise_cli_only_compression() {
        let headers = agent_connect_headers(false);
        assert!(!headers
            .iter()
            .any(|(key, _)| key == "connect-accept-encoding"));
    }

    #[test]
    fn cursor_transport_diagnostics_hide_absolute_urls_and_secrets() {
        let diagnostic = cursor_transport_diagnostic(
            "request https://private.example/internal/run?token=secret failed; api_key=hidden",
        );
        assert!(diagnostic.contains("[REDACTED_CURSOR_URL]"));
        assert!(!diagnostic.contains("private.example"));
        assert!(!diagnostic.contains("/internal/run"));
        assert!(!diagnostic.contains("secret"));
        assert!(!diagnostic.contains("hidden"));
    }

    #[test]
    fn absolute_url_redaction_handles_mixed_case_utf8_and_punctuation() {
        let diagnostic =
            redact_absolute_urls("前缀 HTTPS://private.example/私有路径?token=秘密），后缀");
        assert_eq!(diagnostic, "前缀 [REDACTED_CURSOR_URL]），后缀");
    }

    #[test]
    fn plain_h2_eof_is_not_a_success_terminal() {
        let error = validate_cursor_terminal(None, None, None).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error
            .message
            .contains("without a valid Connect-RPC terminal"));
        validate_cursor_terminal(None, Some(&Ok(())), None).unwrap();
        validate_cursor_terminal(Some(0), None, None).unwrap();
    }

    #[test]
    fn connect_terminal_envelope_is_strict_and_surfaces_errors() {
        assert_eq!(parse_connect_end_stream(br#"{}"#), Ok(()));
        assert_eq!(
            parse_connect_end_stream(br#"{"metadata":{"request-id":["id"]}}"#),
            Ok(())
        );
        for payload in [b"".as_slice(), b"not-json", br#"{"metadata":true}"#] {
            let terminal = parse_connect_end_stream(payload);
            assert!(terminal.is_err());
            assert!(validate_cursor_terminal(None, Some(&terminal), None).is_err());
        }
        let terminal = parse_connect_end_stream(
            br#"{"error":{"code":"unavailable","message":"retry later"}}"#,
        );
        let error = validate_cursor_terminal(None, Some(&terminal), None).unwrap_err();
        assert!(error.message.contains("unavailable: retry later"));
        assert!(validate_cursor_terminal(Some(7), Some(&Ok(())), Some("denied")).is_err());
    }

    #[test]
    fn frame_ingest_state_rejects_data_and_duplicate_envelopes_after_terminal() {
        let terminal = || ConnectFrame {
            flags: 0x02,
            payload: Bytes::from_static(br#"{}"#),
        };
        let data = || ConnectFrame {
            flags: 0,
            payload: Bytes::from_static(b"payload"),
        };

        let mut pending = VecDeque::new();
        let mut end_stream = None;
        ingest_connect_frames(&mut pending, &mut end_stream, vec![terminal()]).unwrap();
        let error = ingest_connect_frames(&mut pending, &mut end_stream, vec![data()])
            .expect_err("data after terminal must fail");
        assert!(error.message.contains("data frame received after"));

        let mut pending = VecDeque::new();
        let mut end_stream = None;
        ingest_connect_frames(&mut pending, &mut end_stream, vec![terminal()]).unwrap();
        let error = ingest_connect_frames(&mut pending, &mut end_stream, vec![terminal()])
            .expect_err("a second terminal must fail");
        assert!(error.message.contains("multiple Connect-RPC terminal"));
    }

    #[test]
    fn malformed_terminal_is_not_hidden_by_queued_business_data() {
        let mut pending = VecDeque::new();
        let mut end_stream = None;
        let error = ingest_connect_frames(
            &mut pending,
            &mut end_stream,
            vec![
                ConnectFrame {
                    flags: 0,
                    payload: Bytes::from_static(b"business"),
                },
                ConnectFrame {
                    flags: 0x02,
                    payload: Bytes::from_static(b"not-json"),
                },
            ],
        )
        .expect_err("terminal validation must precede queued frame delivery");
        assert!(error
            .message
            .contains("terminal envelope is not valid JSON"));
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn control_frames_and_partial_progress_do_not_extend_first_output_deadline() {
        let started = TokioInstant::now();
        let timeout = Duration::from_millis(80);
        let mut phase = CursorOutputPhase::new(
            CursorH2Timeouts {
                request: Duration::from_secs(1),
                first_frame: Some(timeout),
                inter_frame: Some(Duration::from_millis(200)),
            },
            started,
        );
        let initial_deadline = phase.deadline().unwrap();
        assert_eq!(
            initial_deadline.1,
            CursorOutputDeadlineKind::FirstBusinessOutput
        );

        // A complete heartbeat/KV/request-context frame calls this hook. Raw
        // partial bytes do not call any hook at all. Neither may move the
        // first-business-output deadline.
        phase.on_complete_protocol_frame(started + Duration::from_millis(30));
        phase.on_complete_protocol_frame(started + Duration::from_millis(60));
        assert_eq!(phase.deadline().unwrap().0, initial_deadline.0);
        assert!(started + Duration::from_millis(81) > phase.deadline().unwrap().0);

        let error = cursor_output_timeout(
            phase.deadline().unwrap().1,
            phase.deadline().unwrap().2,
            "response body",
        );
        assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
        assert!(error.message.contains("first business output"));
    }

    #[test]
    fn business_output_starts_idle_phase_and_complete_frames_reset_idle_only() {
        let started = TokioInstant::now();
        let mut phase = CursorOutputPhase::new(
            CursorH2Timeouts {
                request: Duration::from_secs(1),
                first_frame: Some(Duration::from_millis(80)),
                inter_frame: Some(Duration::from_millis(200)),
            },
            started,
        );
        phase.mark_business_output(started + Duration::from_millis(25));
        let first_idle_deadline = phase.deadline().unwrap();
        assert_eq!(first_idle_deadline.1, CursorOutputDeadlineKind::Idle);
        assert_eq!(first_idle_deadline.0, started + Duration::from_millis(225));

        phase.on_complete_protocol_frame(started + Duration::from_millis(90));
        assert_eq!(
            phase.deadline().unwrap().0,
            started + Duration::from_millis(290)
        );
    }

    #[test]
    fn resumed_tool_phase_gets_new_absolute_first_output_deadline() {
        let started = TokioInstant::now();
        let timeout = Duration::from_millis(80);
        let mut phase = CursorOutputPhase::new(
            CursorH2Timeouts {
                request: Duration::from_secs(1),
                first_frame: Some(timeout),
                inter_frame: Some(Duration::from_millis(200)),
            },
            started,
        );
        phase.mark_business_output(started + Duration::from_millis(20));
        let resumed_at = started + Duration::from_secs(2);
        phase.rearm(resumed_at);
        let resumed_deadline = phase.deadline().unwrap();
        assert_eq!(
            resumed_deadline.1,
            CursorOutputDeadlineKind::FirstBusinessOutput
        );
        assert_eq!(resumed_deadline.0, resumed_at + timeout);

        phase.on_complete_protocol_frame(resumed_at + Duration::from_millis(60));
        assert_eq!(phase.deadline().unwrap().0, resumed_at + timeout);
        assert!(resumed_at + Duration::from_millis(81) > phase.deadline().unwrap().0);
    }
}
