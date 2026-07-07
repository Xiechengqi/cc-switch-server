use crate::proxy::streaming::SseLineBuffer;
use crate::proxy::ProxyError;
use async_stream::stream;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

mod sse {
    use serde_json::Value;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum DeepSeekEvent {
        Text(String),
        Done,
        Ignored,
    }

    pub(super) fn parse_sse_data_line(line: &str) -> DeepSeekEvent {
        let data = line.strip_prefix("data:").unwrap_or(line).trim();
        if data.is_empty() {
            return DeepSeekEvent::Ignored;
        }
        if data == "[DONE]" {
            return DeepSeekEvent::Done;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return DeepSeekEvent::Ignored;
        };
        if is_finished_event(&value) {
            return DeepSeekEvent::Done;
        }
        extract_text(&value)
            .map(DeepSeekEvent::Text)
            .unwrap_or(DeepSeekEvent::Ignored)
    }

    fn extract_text(value: &Value) -> Option<String> {
        if let Some(path) = value.get("p").and_then(Value::as_str).map(str::trim) {
            if is_finished_status(path, value.get("v")) || should_skip_path(path) {
                return None;
            }
            if path == "response/content" || path.ends_with("/content") {
                return string_value(value.get("v"));
            }
            if path == "response/fragments" {
                return fragment_append_text(value);
            }
            return None;
        }

        [
            pointer_string(value, "/choices/0/delta/content"),
            pointer_string(value, "/choices/0/message/content"),
            pointer_string(value, "/delta/content"),
            pointer_string(value, "/content"),
            pointer_string(value, "/text"),
            pointer_string(value, "/response/content"),
            pointer_string(value, "/response/text"),
            pointer_string(value, "/v").filter(|text| !text.eq_ignore_ascii_case("FINISHED")),
        ]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty())
    }

    fn is_finished_event(value: &Value) -> bool {
        value
            .get("p")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|path| is_finished_status(path, value.get("v")))
    }

    fn is_finished_status(path: &str, v: Option<&Value>) -> bool {
        matches!(path, "" | "status" | "response/status")
            && v.and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|s| s.eq_ignore_ascii_case("FINISHED"))
    }

    fn should_skip_path(path: &str) -> bool {
        path.contains("quasi_status")
            || path.contains("elapsed_secs")
            || path.contains("pending_fragment")
            || path.contains("conversation_mode")
            || path == "response/search_status"
            || (path.starts_with("response/fragments/") && path.ends_with("/status"))
    }

    fn string_value(value: Option<&Value>) -> Option<String> {
        match value {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Object(obj)) => obj
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    obj.get("content")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
            _ => None,
        }
    }

    fn fragment_append_text(value: &Value) -> Option<String> {
        if value.get("o").and_then(Value::as_str) != Some("APPEND") {
            return None;
        }
        let mut out = String::new();
        for item in value.get("v")?.as_array()? {
            let item_type = item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_uppercase();
            if item_type == "RESPONSE" {
                if let Some(content) = item.get("content").and_then(Value::as_str) {
                    out.push_str(content);
                }
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn pointer_string(value: &Value, pointer: &str) -> Option<String> {
        value.pointer(pointer).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    }
}
use sse::{parse_sse_data_line, DeepSeekEvent};

static MESSAGE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn map_model(model: &str) -> String {
    match model {
        "claude-sonnet-4-5" | "claude-sonnet-4-6" | "claude-sonnet-4-7" | "claude-3-5-sonnet" => {
            "deepseek-v4-flash".to_string()
        }
        "claude-opus-4-5" | "claude-opus-4-6" | "claude-opus-4-7" | "claude-3-opus" => {
            "deepseek-v4-pro".to_string()
        }
        m if m.starts_with("deepseek-") => m.to_string(),
        _ => "deepseek-v4-flash".to_string(),
    }
}

pub fn build_prompt(body: &Value) -> Result<String, ProxyError> {
    let mut parts = Vec::new();
    if let Some(system) = body.get("system") {
        let text = text_from_content(system);
        if !text.trim().is_empty() {
            parts.push(format!("<system>\n{}\n</system>", text.trim()));
        }
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("messages must be an array"))?;
    for message in messages {
        let role = match message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
        {
            "assistant" => "Assistant",
            "user" => "User",
            other => other,
        };
        let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
        if !text.trim().is_empty() {
            parts.push(format!("{role}: {}", text.trim()));
        }
    }
    let prompt = parts.join("\n\n");
    if prompt.trim().is_empty() {
        return Err(ProxyError::bad_request("text prompt is empty"));
    }
    Ok(prompt)
}

pub fn estimate_billable_user_input_tokens(body: &Value) -> u32 {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return 0;
    };

    messages
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| text_from_content(message.get("content").unwrap_or(&Value::Null)))
        .find(|text| !text.trim().is_empty())
        .map(|text| estimate_tokens(&text))
        .unwrap_or(0)
}

pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().filter(|c| !c.is_whitespace()).count() as u32;
    if chars == 0 {
        0
    } else {
        chars.div_ceil(4).max(1)
    }
}

pub fn collect_text_from_sse_body(body: &str) -> String {
    let mut out = String::new();
    for line in body
        .lines()
        .filter(|line| line.trim_start().starts_with("data:"))
    {
        match parse_sse_data_line(line) {
            DeepSeekEvent::Text(text) => out.push_str(&text),
            DeepSeekEvent::Done => break,
            DeepSeekEvent::Ignored => {}
        }
    }
    out
}

pub fn claude_message_json(
    text: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> Value {
    json!({
        "id": next_message_id(),
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
        "model": model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
    })
}

pub fn deepseek_bytes_stream_to_claude_sse<S>(
    upstream: S,
    response_model: String,
    input_tokens: u32,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    stream! {
        yield Ok(sse_event("message_start", &json!({
            "type": "message_start",
            "message": {
                "id": next_message_id(),
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": response_model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": input_tokens, "output_tokens": 0}
            }
        })));
        yield Ok(sse_event("content_block_start", &json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        })));

        let mut line_buffer = SseLineBuffer::new();
        let mut output_text = String::new();
        let mut done = false;
        tokio::pin!(upstream);
        while let Some(item) = upstream.next().await {
            if done {
                break;
            }
            let bytes = match item {
                Ok(bytes) => bytes,
                Err(error) => {
                    yield Ok(sse_event("error", &json!({
                        "type": "error",
                        "error": {"type": "api_error", "message": error.to_string()}
                    })));
                    break;
                }
            };
            for line in line_buffer.push_chunk(&bytes) {
                if !line.trim_start().starts_with("data:") {
                    continue;
                }
                match parse_sse_data_line(&line) {
                    DeepSeekEvent::Text(text) if !text.is_empty() => {
                        output_text.push_str(&text);
                        yield Ok(sse_event("content_block_delta", &json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "text_delta", "text": text}
                        })));
                    }
                    DeepSeekEvent::Done => {
                        done = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        if let Some(tail) = line_buffer.finish() {
            if tail.trim_start().starts_with("data:") {
                if let DeepSeekEvent::Text(text) = parse_sse_data_line(&tail) {
                    if !text.is_empty() {
                        output_text.push_str(&text);
                        yield Ok(sse_event("content_block_delta", &json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "text_delta", "text": text}
                        })));
                    }
                }
            }
        }

        let output_tokens = estimate_tokens(&output_text);
        yield Ok(sse_event("content_block_stop", &json!({"type": "content_block_stop", "index": 0})));
        yield Ok(sse_event("message_delta", &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": {"output_tokens": output_tokens}
        })));
        yield Ok(sse_event("message_stop", &json!({"type": "message_stop"})));
    }
}

fn sse_event(event: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn next_message_id() -> String {
    let counter = MESSAGE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("msg_deepseek_{counter}")
}

fn text_from_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_claude_model_mapping_covers_4_7_tiers() {
        assert_eq!(map_model("claude-opus-4-7"), "deepseek-v4-pro");
        assert_eq!(map_model("claude-sonnet-4-7"), "deepseek-v4-flash");
    }

    #[test]
    fn build_prompt_includes_system_and_user_messages() {
        let body = json!({
            "system": "Be concise.",
            "messages": [{"role": "user", "content": "hello world"}]
        });
        let prompt = build_prompt(&body).unwrap();
        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("User: hello world"));
    }

    #[test]
    fn estimate_billable_user_input_tokens_uses_latest_user_text() {
        let body = json!({
            "messages": [
                {"role": "user", "content": "old"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "new question"}
            ]
        });
        assert_eq!(
            estimate_billable_user_input_tokens(&body),
            estimate_tokens("new question")
        );
    }

    #[test]
    fn collect_text_from_sse_body_joins_delta_chunks() {
        let body = "data: {\"p\":\"response/content\",\"v\":\"hello\"}\ndata: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n";
        assert_eq!(collect_text_from_sse_body(body), "hello");
    }

    #[tokio::test]
    async fn deepseek_stream_fixture_emits_claude_sse_across_chunk_boundaries() {
        use futures_util::stream;
        use futures_util::StreamExt;

        let fixture = concat!(
            "data: {\"p\":\"response/content\",\"v\":\"hel\"}\n",
            "data: {\"p\":\"response/content\",\"v\":\"lo\"}\n",
            "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n",
        );
        let split = fixture.len() - 11;
        let upstream = stream::iter(vec![
            Ok(bytes::Bytes::from_static(&fixture.as_bytes()[..split])),
            Ok(bytes::Bytes::from_static(&fixture.as_bytes()[split..])),
        ]);
        let out = deepseek_bytes_stream_to_claude_sse(upstream, "claude-sonnet-4-7".to_string(), 5);
        tokio::pin!(out);
        let mut chunks = Vec::new();
        while let Some(chunk) = out.next().await {
            chunks.push(chunk.expect("stream chunk"));
        }
        let merged = chunks.concat();
        let text = String::from_utf8_lossy(&merged);
        assert!(text.contains("event: message_start"));
        assert!(text.contains("text_delta"));
        assert!(text.contains(r#""text":"hel""#));
        assert!(text.contains(r#""text":"lo""#));
        assert!(text.contains("event: message_stop"));
    }

    #[test]
    fn claude_message_json_includes_estimated_usage() {
        let value = claude_message_json("hello", "claude-sonnet-4-7", 5, 2);
        assert_eq!(
            value.pointer("/content/0/text").and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            value.pointer("/usage/input_tokens").and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            value
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64),
            Some(2)
        );
    }
}
