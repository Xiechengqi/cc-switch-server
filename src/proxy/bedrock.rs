//! Bedrock Converse responses on the two explicitly selected static rails.
//! Only the byte framing is shared with Kiro; its events and identities are not.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Map, Value};

use super::kiro::wire::EventStreamDecoder;
use super::request_memory::{
    json_parse_working_bytes, retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent,
    RequestMemoryReservation,
};
use super::ProxyError;

pub(super) fn is_profile(stored: &crate::domain::providers::store::StoredProvider) -> bool {
    stored.resource.profile_id.as_ref().is_some_and(|profile| {
        matches!(
            profile.as_str(),
            "claude.aws_bedrock_aksk" | "claude.aws_bedrock_api_key"
        )
    })
}

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_HEADER_BYTES: usize = 4096;
const MAX_BLOCKS: usize = 128;
const MAX_TOOL_BYTES: usize = 1024 * 1024;

fn invalid() -> ProxyError {
    ProxyError::bad_gateway("invalid or incomplete Bedrock Converse response")
}

pub(super) fn http_error_body() -> Bytes {
    Bytes::from_static(
        br#"{"type":"error","error":{"type":"api_error","message":"Bedrock request failed"}}"#,
    )
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProxyError> {
    value.get(key).and_then(Value::as_str).ok_or_else(invalid)
}

fn stop_reason(value: &Value) -> Result<&'static str, ProxyError> {
    match string(value, "stopReason")? {
        "end_turn" => Ok("end_turn"),
        "tool_use" => Ok("tool_use"),
        "max_tokens" => Ok("max_tokens"),
        "model_context_window_exceeded" => Ok("model_context_window_exceeded"),
        "stop_sequence" => Ok("stop_sequence"),
        "guardrail_intervened" | "content_filtered" => Ok("refusal"),
        _ => Err(invalid()),
    }
}

fn observed_stop_sequence(value: &Value) -> Result<Option<String>, ProxyError> {
    match value.pointer("/additionalModelResponseFields/stop_sequence") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(sequence)) => Ok(Some(sequence.clone())),
        Some(_) => Err(invalid()),
    }
}

fn usage(value: &Value) -> Result<Value, ProxyError> {
    let object = value.as_object().ok_or_else(invalid)?;
    let mut result = Map::new();
    for (source, target, required) in [
        ("inputTokens", "input_tokens", true),
        ("outputTokens", "output_tokens", true),
        ("totalTokens", "total_tokens", true),
        ("cacheReadInputTokens", "cache_read_input_tokens", false),
        (
            "cacheWriteInputTokens",
            "cache_creation_input_tokens",
            false,
        ),
    ] {
        match object.get(source) {
            Some(value) => {
                let value = value.as_u64().ok_or_else(invalid)?;
                result.insert(target.to_string(), json!(value));
            }
            None if required => return Err(invalid()),
            None => {}
        }
    }
    Ok(Value::Object(result))
}

fn content(block: &Value) -> Result<Value, ProxyError> {
    let object = block.as_object().ok_or_else(invalid)?;
    if object.len() != 1 {
        return Err(invalid());
    }
    if let Some(text) = block.get("text") {
        return Ok(json!({"type":"text", "text":text.as_str().ok_or_else(invalid)?}));
    }
    if let Some(tool) = block.get("toolUse") {
        let id = string(tool, "toolUseId")?;
        let name = string(tool, "name")?;
        let input = tool
            .get("input")
            .filter(|value| value.is_object())
            .ok_or_else(invalid)?;
        if id.is_empty() || name.is_empty() {
            return Err(invalid());
        }
        return Ok(json!({"type":"tool_use", "id":id, "name":name, "input":input}));
    }
    if let Some(reasoning) = block.get("reasoningContent") {
        if let Some(text) = reasoning.get("reasoningText") {
            let mut result = json!({"type":"thinking", "thinking":string(text,"text")?});
            if let Some(signature) = text.get("signature") {
                result["signature"] = json!(signature.as_str().ok_or_else(invalid)?);
            }
            return Ok(result);
        }
        if let Some(redacted) = reasoning.get("redactedContent") {
            return Ok(
                json!({"type":"redacted_thinking", "data":redacted.as_str().ok_or_else(invalid)?}),
            );
        }
    }
    // Unknown content is never silently dropped or converted to prose.
    Err(invalid())
}

pub(super) fn converse_json(body: &[u8], model: &str) -> Result<Bytes, ProxyError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
    let message = value.pointer("/output/message").ok_or_else(invalid)?;
    if string(message, "role")? != "assistant" {
        return Err(invalid());
    }
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if blocks.len() > MAX_BLOCKS {
        return Err(invalid());
    }
    let result = json!({
        "id":format!("msg_{:032x}",rand::random::<u128>()), "type":"message",
        "role":"assistant", "model":model,
        "content":blocks.iter().map(content).collect::<Result<Vec<_>,_>>()?,
        "stop_reason":stop_reason(&value)?, "stop_sequence":observed_stop_sequence(&value)?,
        "usage":usage(value.get("usage").ok_or_else(invalid)?)?
    });
    serde_json::to_vec(&result)
        .map(Bytes::from)
        .map_err(|_| invalid())
}

#[derive(Debug)]
enum Block {
    Pending,
    Text,
    Thinking,
    Redacted,
    Tool(String),
    Closed,
}

pub(super) struct ConverseStream {
    decoder: EventStreamDecoder,
    model: String,
    started: bool,
    stop: Option<&'static str>,
    stop_sequence: Option<String>,
    usage: Option<Value>,
    blocks: BTreeMap<u64, Block>,
    finished: bool,
    failed: bool,
    memory: Option<RequestMemoryReservation>,
    budget: Option<RequestMemoryBudget>,
}

fn event(output: &mut Vec<u8>, value: Value) -> Result<(), ProxyError> {
    let kind = string(&value, "type")?;
    output.extend_from_slice(b"event: ");
    output.extend_from_slice(kind.as_bytes());
    output.extend_from_slice(b"\ndata: ");
    serde_json::to_writer(&mut *output, &value).map_err(|_| invalid())?;
    output.extend_from_slice(b"\n\n");
    Ok(())
}

impl ConverseStream {
    pub(super) fn new(model: String) -> Self {
        Self {
            decoder: EventStreamDecoder::bounded(
                MAX_FRAME_BYTES,
                MAX_FRAME_BYTES + 12,
                MAX_HEADER_BYTES,
            ),
            model,
            started: false,
            stop: None,
            stop_sequence: None,
            usage: None,
            blocks: BTreeMap::new(),
            finished: false,
            failed: false,
            memory: None,
            budget: None,
        }
    }

    pub(super) fn with_memory(
        mut self,
        budget: Option<&RequestMemoryBudget>,
    ) -> Result<Self, ProxyError> {
        self.budget = budget.cloned();
        self.memory = budget
            .map(|budget| {
                budget
                    .reserve(
                        RequestMemoryComponent::StreamRetainedState,
                        self.retained_bytes(),
                    )
                    .map_err(|error| error.into_proxy_error())
            })
            .transpose()?;
        Ok(self)
    }

    fn resize(&self, extra: usize) -> Result<(), ProxyError> {
        if let Some(memory) = self.memory.as_ref() {
            memory
                .resize(self.retained_bytes().saturating_add(extra))
                .map_err(|error| error.into_proxy_error())?;
        }
        Ok(())
    }

    fn deliver(&self, output: Vec<u8>) -> Result<Bytes, ProxyError> {
        self.resize(output.capacity())?;
        let output = Bytes::from(output);
        let output = match self.budget.as_ref() {
            Some(budget) => budget
                .retain_bytes(RequestMemoryComponent::NormalizedEvent, output)
                .map_err(|error| error.into_proxy_error())?,
            None => output,
        };
        self.resize(0)?;
        Ok(output)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.decoder
            .retained_bytes()
            .saturating_add(self.stop_sequence.as_ref().map_or(0, String::capacity))
            .saturating_add(self.model.capacity())
            .saturating_add(
                self.blocks
                    .len()
                    .saturating_mul(std::mem::size_of::<(u64, Block)>() + 32),
            )
            .saturating_add(
                self.blocks
                    .values()
                    .map(|block| match block {
                        Block::Tool(args) => args.capacity(),
                        _ => 0,
                    })
                    .sum::<usize>(),
            )
            .saturating_add(self.usage.as_ref().map_or(0, retained_json_bytes))
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Bytes, ProxyError> {
        if self.finished || self.failed {
            return Err(invalid());
        }
        let result = self.push_inner(chunk);
        self.failed |= result.is_err();
        result
    }

    fn push_inner(&mut self, chunk: &[u8]) -> Result<Bytes, ProxyError> {
        // Reserve header-map indexes, duplicate wire buffers and parser scratch
        // before the framing decoder allocates its Frame collection.
        self.resize(
            json_parse_working_bytes(chunk)
                .saturating_add(chunk.len().saturating_mul(64))
                .saturating_add(self.decoder.retained_bytes().saturating_mul(3))
                .saturating_add(MAX_HEADER_BYTES.saturating_mul(64))
                .saturating_add(self.model.capacity().saturating_mul(4)),
        )?;
        let frames = self.decoder.feed(chunk).map_err(|_| invalid())?;
        let mut output = Vec::new();
        let frame_bytes = frames
            .iter()
            .map(|frame| frame.retained_bytes())
            .sum::<usize>();
        for frame in frames {
            self.resize(
                frame_bytes
                    .saturating_add(json_parse_working_bytes(frame.payload()).saturating_mul(2))
                    .saturating_add(self.model.capacity().saturating_mul(4))
                    .saturating_add(output.capacity()),
            )?;
            match frame.string_header(":message-type") {
                Some("exception" | "error") => {
                    let code = frame
                        .string_header(":exception-type")
                        .or_else(|| frame.string_header(":error-code"))
                        .unwrap_or_default();
                    let status = match code {
                        "throttlingException" => 429,
                        "validationException" => 400,
                        "serviceUnavailableException" => 503,
                        "internalServerException" => 500,
                        _ => 502,
                    };
                    // Provider payloads may contain user content or credentials.
                    return Err(ProxyError {
                        status: axum::http::StatusCode::from_u16(status).unwrap(),
                        message: "Bedrock Converse stream exception".to_string(),
                    });
                }
                Some("event") => {}
                _ => return Err(invalid()),
            }
            if frame.string_header(":content-type") != Some("application/json") {
                return Err(invalid());
            }
            let kind = frame.string_header(":event-type").ok_or_else(invalid)?;
            let value: Value = serde_json::from_slice(frame.payload()).map_err(|_| invalid())?;
            self.accept(kind, &value, &mut output)?;
        }
        self.deliver(output)
    }

    fn accept(
        &mut self,
        kind: &str,
        value: &Value,
        output: &mut Vec<u8>,
    ) -> Result<(), ProxyError> {
        if self.usage.is_some() {
            return Err(invalid());
        }
        if kind == "messageStart" {
            if self.started || string(value, "role")? != "assistant" {
                return Err(invalid());
            }
            self.started = true;
            return event(
                output,
                json!({"type":"message_start","message":{
                    "id":format!("msg_{:032x}",rand::random::<u128>()),"type":"message","role":"assistant",
                    "model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,
                    "usage":{"input_tokens":0,"output_tokens":0}
                }}),
            );
        }
        if !self.started {
            return Err(invalid());
        }
        if kind == "metadata" {
            if self.stop.is_none() {
                return Err(invalid());
            }
            self.usage = Some(usage(value.get("usage").ok_or_else(invalid)?)?);
            return Ok(());
        }
        if self.stop.is_some() {
            return Err(invalid());
        }
        if kind == "messageStop" {
            if self
                .blocks
                .values()
                .any(|block| !matches!(block, Block::Closed))
            {
                return Err(invalid());
            }
            self.stop = Some(stop_reason(value)?);
            self.stop_sequence = observed_stop_sequence(value)?;
            return Ok(());
        }
        let index = value
            .get("contentBlockIndex")
            .and_then(Value::as_u64)
            .filter(|v| *v < MAX_BLOCKS as u64)
            .ok_or_else(invalid)?;
        match kind {
            "contentBlockStart" => {
                if self.blocks.contains_key(&index) {
                    return Err(invalid());
                }
                let start = value
                    .get("start")
                    .and_then(Value::as_object)
                    .ok_or_else(invalid)?;
                if start.is_empty() {
                    self.blocks.insert(index, Block::Pending);
                } else {
                    if start.len() != 1 {
                        return Err(invalid());
                    }
                    let tool = start.get("toolUse").ok_or_else(invalid)?;
                    let id = string(tool, "toolUseId")?;
                    let name = string(tool, "name")?;
                    if id.is_empty() || name.is_empty() {
                        return Err(invalid());
                    }
                    self.blocks.insert(index, Block::Tool(String::new()));
                    event(
                        output,
                        json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
                    )?;
                }
            }
            "contentBlockDelta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_object)
                    .filter(|v| v.len() == 1)
                    .ok_or_else(invalid)?;
                if let Some(text) = delta.get("text") {
                    self.ensure_block(index, Block::Text, output)?;
                    event(
                        output,
                        json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text.as_str().ok_or_else(invalid)?}}),
                    )?;
                } else if let Some(tool) = delta.get("toolUse") {
                    let text = string(tool, "input")?;
                    let Some(Block::Tool(args)) = self.blocks.get_mut(&index) else {
                        return Err(invalid());
                    };
                    if args.len().saturating_add(text.len()) > MAX_TOOL_BYTES {
                        return Err(invalid());
                    }
                    args.push_str(text);
                    event(
                        output,
                        json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":text}}),
                    )?;
                } else if let Some(reasoning) = delta.get("reasoningContent") {
                    let fields = reasoning
                        .as_object()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(invalid)?;
                    if fields.keys().any(|key| {
                        !matches!(key.as_str(), "text" | "signature" | "redactedContent")
                    }) {
                        return Err(invalid());
                    }
                    if let Some(data) = reasoning.get("redactedContent") {
                        if fields.len() != 1 || self.blocks.contains_key(&index) {
                            return Err(invalid());
                        }
                        self.blocks.insert(index, Block::Redacted);
                        event(
                            output,
                            json!({"type":"content_block_start","index":index,"content_block":{"type":"redacted_thinking","data":data.as_str().ok_or_else(invalid)?}}),
                        )?;
                    } else {
                        self.ensure_block(index, Block::Thinking, output)?;
                        for (key, kind) in [("text", "thinking"), ("signature", "signature")] {
                            if let Some(value) = reasoning.get(key) {
                                let mut delta = json!({"type":format!("{kind}_delta")});
                                delta[kind] = json!(value.as_str().ok_or_else(invalid)?);
                                event(
                                    output,
                                    json!({"type":"content_block_delta","index":index,"delta":delta}),
                                )?;
                            }
                        }
                    }
                } else {
                    return Err(invalid());
                }
            }
            "contentBlockStop" => {
                // Complete arguments can be much larger than this stop frame.
                // Charge their JSON working set alongside all live frames.
                let _argument_parse_memory = match self.blocks.get(&index) {
                    Some(Block::Tool(args)) => self
                        .budget
                        .as_ref()
                        .map(|budget| {
                            budget
                                .reserve(
                                    RequestMemoryComponent::ToolArguments,
                                    json_parse_working_bytes(args.as_bytes()),
                                )
                                .map_err(|error| error.into_proxy_error())
                        })
                        .transpose()?,
                    _ => None,
                };
                let block = self.blocks.get_mut(&index).ok_or_else(invalid)?;
                if matches!(block, Block::Closed | Block::Pending) {
                    return Err(invalid());
                }
                if let Block::Tool(args) = block {
                    let args: Value = serde_json::from_str(args).map_err(|_| invalid())?;
                    if !args.is_object() {
                        return Err(invalid());
                    }
                }
                *block = Block::Closed;
                event(output, json!({"type":"content_block_stop","index":index}))?;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }

    fn ensure_block(
        &mut self,
        index: u64,
        kind: Block,
        output: &mut Vec<u8>,
    ) -> Result<(), ProxyError> {
        let current = self.blocks.entry(index).or_insert(Block::Pending);
        if matches!(current, Block::Pending) {
            let content = match kind {
                Block::Text => json!({"type":"text","text":""}),
                Block::Thinking => json!({"type":"thinking","thinking":""}),
                _ => return Err(invalid()),
            };
            *current = kind;
            event(
                output,
                json!({"type":"content_block_start","index":index,"content_block":content}),
            )?;
        } else if !matches!(
            (&kind, current),
            (Block::Text, Block::Text) | (Block::Thinking, Block::Thinking)
        ) {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<Bytes, ProxyError> {
        if self.finished || self.failed {
            return Err(invalid());
        }
        self.finished = true;
        self.decoder.finish().map_err(|_| invalid())?;
        let stop = self.stop.ok_or_else(invalid)?;
        let usage_bytes = self
            .usage
            .as_ref()
            .map(retained_json_bytes)
            .ok_or_else(invalid)?;
        // Escaping a retained stop sequence can expand it sixfold, and Vec
        // growth and the terminal Value coexist. Admit that work before moving
        // the observed fields or allocating the success frames.
        self.resize(
            self.stop_sequence
                .as_ref()
                .map_or(0, |value| value.capacity().saturating_mul(16))
                .saturating_add(usage_bytes.saturating_mul(4))
                .saturating_add(1024),
        )?;
        let usage = self.usage.take().ok_or_else(invalid)?;
        let mut output = Vec::new();
        event(
            &mut output,
            json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":self.stop_sequence.take()},"usage":usage}),
        )?;
        event(&mut output, json!({"type":"message_stop"}))?;
        self.deliver(output)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn frame(kind: &str, value: Value) -> Vec<u8> {
        frame_with_headers(
            &[
                (":message-type", "event"),
                (":event-type", kind),
                (":content-type", "application/json"),
            ],
            value,
        )
    }

    fn frame_with_headers(headers: &[(&str, &str)], value: Value) -> Vec<u8> {
        let mut encoded = Vec::new();
        for (name, value) in headers {
            encoded.push(name.len() as u8);
            encoded.extend_from_slice(name.as_bytes());
            encoded.push(7);
            encoded.extend_from_slice(&(value.len() as u16).to_be_bytes());
            encoded.extend_from_slice(value.as_bytes());
        }
        let payload = serde_json::to_vec(&value).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(&((16 + encoded.len() + payload.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        out.extend_from_slice(&crate::proxy::kiro::wire::crc32(&out).to_be_bytes());
        out.extend(encoded);
        out.extend(payload);
        out.extend_from_slice(&crate::proxy::kiro::wire::crc32(&out).to_be_bytes());
        out
    }

    pub(crate) fn text_stream() -> Vec<u8> {
        [frame("messageStart",json!({"role":"assistant"})),
         frame("contentBlockDelta",json!({"contentBlockIndex":0,"delta":{"text":"hello"}})),
         frame("contentBlockStop",json!({"contentBlockIndex":0})),
         frame("messageStop",json!({"stopReason":"end_turn"})),
         frame("metadata",json!({"usage":{"inputTokens":11,"outputTokens":2,"totalTokens":13,"cacheReadInputTokens":7,"cacheWriteInputTokens":3}}))].concat()
    }

    #[test]
    fn bedrock_r5_stream_handles_every_byte_boundary_and_delays_success_until_eof() {
        let bytes = text_stream();
        for split in 0..=bytes.len() {
            let mut stream = ConverseStream::new("exact-model".to_string());
            let mut out = stream.push(&bytes[..split]).unwrap().to_vec();
            out.extend_from_slice(&stream.push(&bytes[split..]).unwrap());
            assert!(!String::from_utf8_lossy(&out).contains("\"type\":\"message_stop\""));
            out.extend_from_slice(&stream.finish().unwrap());
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text.matches("event: message_stop\n").count(), 1);
            assert!(text.contains("hello"));
            assert!(text.contains("\"cache_read_input_tokens\":7"));
            let mut inspector = crate::proxy::anthropic_semantics::AnthropicSseInspector::default();
            inspector.push(text.as_bytes()).unwrap();
            inspector.finish().unwrap();
        }
    }

    #[test]
    fn bedrock_r5_stream_preserves_signed_thinking_and_fragmented_tool_arguments() {
        let frames = [
            frame("messageStart", json!({"role":"assistant"})),
            frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":0,"delta":{"reasoningContent":{"text":"thought","signature":"opaque-signature"}}}),
            ),
            frame("contentBlockStop", json!({"contentBlockIndex":0})),
            frame(
                "contentBlockStart",
                json!({"contentBlockIndex":1,"start":{"toolUse":{"toolUseId":"call-exact","name":"Read"}}}),
            ),
            frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":1,"delta":{"toolUse":{"input":"{\"path\":"}}}),
            ),
            frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":1,"delta":{"toolUse":{"input":"\"a\"}"}}}),
            ),
            frame("contentBlockStop", json!({"contentBlockIndex":1})),
            frame("messageStop", json!({"stopReason":"tool_use"})),
            frame(
                "metadata",
                json!({"usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3}}),
            ),
        ];
        let mut stream = ConverseStream::new("exact-model".to_string());
        let mut out = stream.push(&frames.concat()).unwrap().to_vec();
        out.extend_from_slice(&stream.finish().unwrap());
        let text = String::from_utf8(out).unwrap();
        for term in [
            "opaque-signature",
            "thinking_delta",
            "input_json_delta",
            "call-exact",
            "Read",
            "tool_use",
        ] {
            assert!(text.contains(term), "{term}");
        }
        assert!(stream.retained_bytes() < MAX_TOOL_BYTES);
    }

    #[test]
    fn bedrock_r5_stream_rejects_crc_truncation_missing_terminal_and_post_terminal_data() {
        let good = text_stream();
        let mut corrupt = good.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        let missing = [
            frame("messageStart", json!({"role":"assistant"})),
            frame(
                "metadata",
                json!({"usage":{"inputTokens":1,"outputTokens":1}}),
            ),
        ]
        .concat();
        let duplicate = [
            good.clone(),
            frame("messageStop", json!({"stopReason":"end_turn"})),
        ]
        .concat();
        for bytes in [corrupt, good[..good.len() - 1].to_vec(), missing, duplicate] {
            let mut stream = ConverseStream::new("m".to_string());
            assert!(stream.push(&bytes).and_then(|_| stream.finish()).is_err());
            assert!(stream.push(&[]).is_err());
        }
        let no_metadata = [
            frame("messageStart", json!({"role":"assistant"})),
            frame("messageStop", json!({"stopReason":"end_turn"})),
        ]
        .concat();
        let mut stream = ConverseStream::new("m".to_string());
        stream.push(&no_metadata).unwrap();
        assert!(stream.finish().is_err());
    }

    #[test]
    fn bedrock_r5_stream_exception_is_sanitized_and_never_a_success_terminal() {
        for (kind, status) in [
            ("throttlingException", 429),
            ("validationException", 400),
            ("serviceUnavailableException", 503),
            ("internalServerException", 500),
            ("unknown-provider-code", 502),
        ] {
            let bytes = frame_with_headers(
                &[(":message-type", "exception"), (":exception-type", kind)],
                json!({"message":"secret token user text"}),
            );
            let mut stream = ConverseStream::new("m".to_string());
            let error = stream.push(&bytes).unwrap_err();
            assert_eq!(error.status.as_u16(), status);
            assert!(!error.message.contains("secret"));
            assert!(stream.finish().is_err());
        }
    }

    #[test]
    fn bedrock_r5_stream_rejects_bad_tools_indices_usage_and_unknown_content() {
        let cases = [
            vec![frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":MAX_BLOCKS,"delta":{"text":"x"}}),
            )],
            vec![frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":0,"delta":{"unknown":"x"}}),
            )],
            vec![frame(
                "contentBlockDelta",
                json!({"contentBlockIndex":0,"delta":{"toolUse":{"input":"{}"}}}),
            )],
            vec![
                frame(
                    "contentBlockStart",
                    json!({"contentBlockIndex":0,"start":{"toolUse":{"toolUseId":"c","name":"f"}}}),
                ),
                frame(
                    "contentBlockDelta",
                    json!({"contentBlockIndex":0,"delta":{"toolUse":{"input":"[]"}}}),
                ),
                frame("contentBlockStop", json!({"contentBlockIndex":0})),
            ],
            vec![
                frame("messageStop", json!({"stopReason":"end_turn"})),
                frame(
                    "metadata",
                    json!({"usage":{"inputTokens":1.5,"outputTokens":1}}),
                ),
            ],
        ];
        for frames in cases {
            let mut stream = ConverseStream::new("m".to_string());
            stream
                .push(&frame("messageStart", json!({"role":"assistant"})))
                .unwrap();
            assert!(stream.push(&frames.concat()).is_err());
        }
        let mut stream = ConverseStream::new("m".to_string());
        let oversized_header = "x".repeat(MAX_HEADER_BYTES);
        let bytes = frame_with_headers(
            &[
                (":message-type", "event"),
                (":event-type", "messageStart"),
                (":content-type", "application/json"),
                ("large", &oversized_header),
            ],
            json!({"role":"assistant"}),
        );
        let mut header_stream = ConverseStream::new("m".to_string());
        assert!(header_stream.push(&bytes).is_err());
        let mut huge = vec![0; 12];
        huge[..4].copy_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        assert!(stream.push(&huge).is_err());
    }

    #[test]
    fn bedrock_r5_json_preserves_reasoning_tools_and_observed_cache_counters() {
        let raw = json!({"output":{"message":{"role":"assistant","content":[
            {"reasoningContent":{"reasoningText":{"text":"thought","signature":"sig"}}},
            {"reasoningContent":{"redactedContent":"b3BhcXVl"}},
            {"toolUse":{"toolUseId":"c","name":"f","input":{"x":1}}}
        ]}},"stopReason":"tool_use","usage":{"inputTokens":2,"outputTokens":3,"totalTokens":5,"cacheReadInputTokens":7}});
        let result: Value = serde_json::from_slice(
            &converse_json(&serde_json::to_vec(&raw).unwrap(), "exact-model").unwrap(),
        )
        .unwrap();
        assert_eq!(result["model"], "exact-model");
        assert_eq!(result["content"][0]["signature"], "sig");
        assert_eq!(result["content"][1]["data"], "b3BhcXVl");
        assert_eq!(result["content"][2]["input"]["x"], 1);
        assert_eq!(result["usage"]["cache_read_input_tokens"], 7);
        assert!(result["usage"].get("cache_creation_input_tokens").is_none());
    }
    #[test]
    fn bedrock_r5_legal_context_limit_and_observed_stop_sequence_are_not_forged_or_lost() {
        let raw = json!({"output":{"message":{"role":"assistant","content":[{"text":"partial"}]}},"stopReason":"model_context_window_exceeded","usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3}});
        let value: Value = serde_json::from_slice(
            &converse_json(&serde_json::to_vec(&raw).unwrap(), "m").unwrap(),
        )
        .unwrap();
        assert_eq!(value["stop_reason"], "model_context_window_exceeded");
        for malformed in ["malformed_model_output", "malformed_tool_use", "unknown"] {
            let mut value = raw.clone();
            value["stopReason"] = json!(malformed);
            assert!(converse_json(&serde_json::to_vec(&value).unwrap(), "m").is_err());
        }
        let mut sequence = raw;
        sequence["stopReason"] = json!("stop_sequence");
        sequence["additionalModelResponseFields"] = json!({"stop_sequence":"exact-stop"});
        let value: Value = serde_json::from_slice(
            &converse_json(&serde_json::to_vec(&sequence).unwrap(), "m").unwrap(),
        )
        .unwrap();
        assert_eq!(value["stop_sequence"], "exact-stop");
        let bytes=[frame("messageStart",json!({"role":"assistant"})),frame("messageStop",json!({"stopReason":"stop_sequence","additionalModelResponseFields":{"stop_sequence":"exact-stop"}})),frame("metadata",json!({"usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3}}))].concat();
        let mut stream = ConverseStream::new("m".to_string());
        stream.push(&bytes).unwrap();
        let tail = stream.finish().unwrap();
        assert!(String::from_utf8_lossy(&tail).contains("\"stop_sequence\":\"exact-stop\""));
    }
    #[test]
    fn bedrock_r5_completed_tool_json_parse_is_admitted_before_allocating_the_object() {
        let parent =
            crate::infra::resident_memory::ResidentMemoryBudget::new(Some(8 * 1024 * 1024));
        let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
        let mut stream = ConverseStream::new("m".to_string())
            .with_memory(Some(&budget))
            .unwrap();
        drop(
            stream
                .push(&frame("messageStart", json!({"role":"assistant"})))
                .unwrap(),
        );
        drop(
            stream
                .push(&frame(
                    "contentBlockStart",
                    json!({"contentBlockIndex":0,"start":{"toolUse":{"toolUseId":"c","name":"f"}}}),
                ))
                .unwrap(),
        );
        let args = Value::Object(
            (0..10000)
                .map(|index| (format!("k{index}"), json!(0)))
                .collect(),
        );
        let text = serde_json::to_string(&args).unwrap();
        for part in text.as_bytes().chunks(1000) {
            let part = std::str::from_utf8(part).unwrap();
            drop(
                stream
                    .push(&frame(
                        "contentBlockDelta",
                        json!({"contentBlockIndex":0,"delta":{"toolUse":{"input":part}}}),
                    ))
                    .unwrap(),
            );
        }
        let error = stream
            .push(&frame("contentBlockStop", json!({"contentBlockIndex":0})))
            .unwrap_err();
        assert!(error.is_request_memory_exhausted());
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        drop(stream);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[test]
    fn bedrock_r5_terminal_encoding_is_admitted_before_moving_observed_fields() {
        let limit = 8 * 1024 * 1024;
        let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(limit));
        let budget = RequestMemoryBudget::with_parent(limit, parent.clone());
        let mut stream = ConverseStream::new("m".to_string())
            .with_memory(Some(&budget))
            .unwrap();
        let sequence = "\0".repeat(4096);
        let bytes = [
            frame("messageStart", json!({"role":"assistant"})),
            frame("messageStop", json!({"stopReason":"stop_sequence","additionalModelResponseFields":{"stop_sequence":sequence}})),
            frame("metadata", json!({"usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3}})),
        ].concat();
        drop(stream.push(&bytes).unwrap());
        let pending_output = budget
            .reserve(
                RequestMemoryComponent::NormalizedEvent,
                limit - budget.snapshot().used_bytes - 512,
            )
            .unwrap();
        let error = stream.finish().unwrap_err();
        assert!(error.is_request_memory_exhausted());
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(stream.stop_sequence.as_deref(), Some(sequence.as_str()));
        assert!(stream.usage.is_some());
        drop(pending_output);
        drop(stream);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
