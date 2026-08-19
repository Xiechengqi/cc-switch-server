use bytes::Bytes;
use serde_json::{json, Value};

use super::response_semantics::SemanticFailure;

pub(super) const OPENAI_CAPACITY_SHED_RETRYABLE_CLIENT_CODE: &str = "server_error";
const CAPACITY_SHED_CODES: &[&str] = &["server_is_overloaded", "slow_down"];

pub(super) fn is_openai_capacity_shed_failure(failure: &SemanticFailure) -> bool {
    is_openai_capacity_shed_code_or_message(&failure.code, &failure.message)
}

pub(super) fn is_openai_capacity_shed_value(value: &Value) -> bool {
    let (code, message) = capacity_shed_code_and_message(value);
    is_openai_capacity_shed_code_or_message(&code, &message)
}

pub(super) fn capacity_shed_retry_source(failure: &SemanticFailure) -> &'static str {
    match normalize_error_token(&failure.code).as_str() {
        "slow_down" => "slow_down",
        _ => "server_is_overloaded",
    }
}

fn is_openai_capacity_shed_code_or_message(code: &str, message: &str) -> bool {
    let code = normalize_error_token(code);
    if code.contains("rate_limit")
        || code.starts_with("invalid_request")
        || code.contains("content_policy")
    {
        return false;
    }
    if CAPACITY_SHED_CODES.contains(&code.as_str()) {
        return true;
    }
    is_openai_capacity_shed_message(message)
}

fn is_openai_capacity_shed_message(message: &str) -> bool {
    let message = message.trim().to_ascii_lowercase();
    if message.is_empty() || message.contains("rate limit") || message.contains("rate_limit") {
        return false;
    }
    message.contains("currently overloaded")
        || message.contains("server is overloaded")
        || message.contains("servers are currently overloaded")
        || (message.contains("overloaded") && message.contains("try again later"))
}

fn normalize_error_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', '.'], "_")
}

fn capacity_shed_code_and_message(value: &Value) -> (String, String) {
    let error = nested_error(value);
    let code = string_field(error, "code")
        .or_else(|| string_field(value.get("response"), "code"))
        .unwrap_or_default();
    let message = string_field(error, "message")
        .or_else(|| string_field(Some(value), "message"))
        .unwrap_or_default();
    (code, message)
}

fn nested_error(value: &Value) -> Option<&Value> {
    value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
        .filter(|error| !error.is_null())
}

fn string_field(value: Option<&Value>, field: &str) -> Option<String> {
    value
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn openai_payload_starts_client_output(value: &Value) -> bool {
    if value.as_str() == Some("[DONE]") {
        return true;
    }
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "response.created" | "response.in_progress" | "response.queued" | "response.failed" => {
            false
        }
        "error" => !is_openai_capacity_shed_value(value),
        "response.output_item.added" => output_item_starts_client_output(value.get("item")),
        "response.content_part.added" => content_part_starts_client_output(value.get("part")),
        "response.reasoning_summary_part.added" => {
            summary_part_starts_client_output(value.get("part"))
        }
        "response.output_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary_text.delta"
        | "response.audio_transcript.delta"
        | "response.function_call_arguments.delta"
        | "response.custom_tool_call_input.delta" => non_empty_delta(value),
        "response.output_text.done"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_text.done"
        | "response.audio_transcript.done" => !string_field(Some(value), "text")
            .unwrap_or_default()
            .is_empty(),
        "response.function_call_arguments.done" => !string_field(Some(value), "arguments")
            .unwrap_or_default()
            .is_empty(),
        "response.custom_tool_call_input.done" => !string_field(Some(value), "input")
            .unwrap_or_default()
            .is_empty(),
        "response.image_generation_call.partial_image" => {
            !string_field(Some(value), "partial_image_b64")
                .unwrap_or_default()
                .is_empty()
        }
        "response.output_item.done" => output_item_starts_client_output(value.get("item")),
        "response.content_part.done" => content_part_starts_client_output(value.get("part")),
        "response.reasoning_summary_part.done" => {
            summary_part_starts_client_output(value.get("part"))
        }
        "response.completed" | "response.done" => value
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| output_item_starts_client_output(Some(item)))
            }),
        "" => false,
        _ => true,
    }
}

fn non_empty_delta(value: &Value) -> bool {
    match value.get("delta") {
        Some(Value::String(delta)) => !delta.is_empty(),
        Some(Value::Null) | None => false,
        Some(other) => !other.is_null(),
    }
}

fn output_item_starts_client_output(item: Option<&Value>) -> bool {
    let Some(item) = item.filter(|item| item.is_object()) else {
        return true;
    };
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "reasoning" => {
            if !string_field(Some(item), "encrypted_content")
                .unwrap_or_default()
                .is_empty()
            {
                return true;
            }
            let Some(summary) = item.get("summary").and_then(Value::as_array) else {
                return false;
            };
            summary.iter().any(|part| {
                part.get("type").and_then(Value::as_str) != Some("summary_text")
                    || !string_field(Some(part), "text")
                        .unwrap_or_default()
                        .is_empty()
            })
        }
        "message" => {
            let Some(content) = item.get("content").and_then(Value::as_array) else {
                return false;
            };
            content
                .iter()
                .any(|part| match part.get("type").and_then(Value::as_str) {
                    Some("output_text") => !string_field(Some(part), "text")
                        .unwrap_or_default()
                        .is_empty(),
                    Some("refusal") => !string_field(Some(part), "refusal")
                        .unwrap_or_default()
                        .is_empty(),
                    _ => true,
                })
        }
        "function_call" => !string_field(Some(item), "arguments")
            .unwrap_or_default()
            .is_empty(),
        "custom_tool_call" => !string_field(Some(item), "input")
            .unwrap_or_default()
            .is_empty(),
        "compaction" => !string_field(Some(item), "encrypted_content")
            .unwrap_or_default()
            .is_empty(),
        _ => true,
    }
}

fn content_part_starts_client_output(part: Option<&Value>) -> bool {
    let Some(part) = part.filter(|part| part.is_object()) else {
        return true;
    };
    match part.get("type").and_then(Value::as_str).unwrap_or_default() {
        "output_text" => !string_field(Some(part), "text")
            .unwrap_or_default()
            .is_empty(),
        "refusal" => !string_field(Some(part), "refusal")
            .unwrap_or_default()
            .is_empty(),
        _ => true,
    }
}

fn summary_part_starts_client_output(part: Option<&Value>) -> bool {
    let Some(part) = part.filter(|part| part.is_object()) else {
        return true;
    };
    if part.get("type").and_then(Value::as_str) != Some("summary_text") {
        return true;
    }
    !string_field(Some(part), "text")
        .unwrap_or_default()
        .is_empty()
}

pub(super) fn openai_stream_bytes_start_client_output(bytes: &[u8]) -> bool {
    for payload in iter_stream_payloads(bytes) {
        if payload == "[DONE]" {
            return true;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            if openai_payload_starts_client_output(&value) {
                return true;
            }
        }
    }
    false
}

pub(super) fn sanitize_openai_capacity_shed_json_bytes(payload: &[u8]) -> (Vec<u8>, bool) {
    let Ok(mut value) = serde_json::from_slice::<Value>(payload) else {
        return (payload.to_vec(), false);
    };
    if !sanitize_openai_capacity_shed_value(&mut value) {
        return (payload.to_vec(), false);
    }
    match serde_json::to_vec(&value) {
        Ok(bytes) => (bytes, true),
        Err(_) => (payload.to_vec(), false),
    }
}

pub(super) fn sanitize_openai_capacity_shed_json_text(payload: &str) -> (String, bool) {
    let (bytes, changed) = sanitize_openai_capacity_shed_json_bytes(payload.as_bytes());
    (
        String::from_utf8(bytes).unwrap_or_else(|_| payload.to_string()),
        changed,
    )
}

pub(super) fn sanitize_openai_capacity_shed_sse_bytes(input: &[u8]) -> Bytes {
    let mut output = Vec::with_capacity(input.len());
    let mut rest = input;
    while let Some((end, delim)) = next_event_boundary(rest) {
        output.extend_from_slice(&rewrite_sse_event(&rest[..end]));
        output.extend_from_slice(&rest[end..end + delim]);
        rest = &rest[end + delim..];
    }
    output.extend_from_slice(rest);
    Bytes::from(output)
}

fn sanitize_openai_capacity_shed_value(value: &mut Value) -> bool {
    if !is_openai_capacity_shed_value(value) {
        return false;
    }
    let mut changed = false;
    for pointer in ["/response/error", "/error"] {
        let Some(error) = value.pointer_mut(pointer) else {
            continue;
        };
        if !error.is_object() {
            continue;
        }
        let current = error
            .get("code")
            .and_then(Value::as_str)
            .map(normalize_error_token)
            .unwrap_or_default();
        if current.is_empty() || CAPACITY_SHED_CODES.contains(&current.as_str()) {
            error["code"] = json!(OPENAI_CAPACITY_SHED_RETRYABLE_CLIENT_CODE);
            changed = true;
        }
    }
    changed
}

fn rewrite_sse_event(event: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(event) else {
        return event.to_vec();
    };
    let mut prefix = String::new();
    let mut data_lines = Vec::new();
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if let Some(data) = content.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data).to_string());
        } else {
            prefix.push_str(line);
        }
    }
    if data_lines.is_empty() {
        return event.to_vec();
    }
    let payload = data_lines.join("\n");
    if payload == "[DONE]" {
        return event.to_vec();
    }
    let (rewritten, changed) = sanitize_openai_capacity_shed_json_text(&payload);
    if !changed {
        return event.to_vec();
    }
    let mut output = prefix.into_bytes();
    for (index, line) in rewritten.split('\n').enumerate() {
        if index > 0 {
            output.extend_from_slice(b"\n");
        }
        output.extend_from_slice(b"data: ");
        output.extend_from_slice(line.as_bytes());
    }
    if text.ends_with('\n') && !rewritten.ends_with('\n') {
        output.push(b'\n');
    }
    output
}

fn iter_stream_payloads(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut payloads = Vec::new();
    let mut rest = text.as_bytes();
    while let Some((end, delim)) = next_event_boundary(rest) {
        if let Some(payload) = sse_event_payload(&rest[..end]) {
            payloads.push(payload);
        }
        rest = &rest[end + delim..];
    }
    if !rest.is_empty() {
        if let Some(payload) = sse_event_payload(rest) {
            payloads.push(payload);
        } else if let Ok(payload) = std::str::from_utf8(rest) {
            let payload = payload.trim();
            if !payload.is_empty() {
                payloads.push(payload.to_string());
            }
        }
    }
    payloads
}

fn sse_event_payload(event: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(event).ok()?;
    let data = text
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>();
    if data.is_empty() {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        Some(data.join("\n").trim().to_string()).filter(|payload| !payload.is_empty())
    }
}

fn next_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            return Some((index, 2));
        }
        if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_curl_sequence_is_capacity_shed_before_output() {
        let created =
            json!({"type":"response.created","response":{"id":"resp_1","status":"in_progress"}});
        let in_progress = json!({"type":"response.in_progress","response":{"id":"resp_1","status":"in_progress"}});
        let error = json!({
            "type":"error",
            "error":{
                "type":"service_unavailable_error",
                "code":"server_is_overloaded",
                "message":"Our servers are currently overloaded. Please try again later.",
                "param":null
            },
            "sequence_number":2
        });
        assert!(!openai_payload_starts_client_output(&created));
        assert!(!openai_payload_starts_client_output(&in_progress));
        assert!(!openai_payload_starts_client_output(&error));
        assert!(is_openai_capacity_shed_value(&error));
        assert!(!openai_stream_bytes_start_client_output(
            br#"event: response.created
data: {"type":"response.created","response":{"id":"resp_1"}}

event: response.in_progress
data: {"type":"response.in_progress","response":{"id":"resp_1"}}

event: error
data: {"type":"error","error":{"type":"service_unavailable_error","code":"server_is_overloaded","message":"Our servers are currently overloaded. Please try again later."}}

"#
        ));
    }

    #[test]
    fn empty_reasoning_and_summary_do_not_start_output() {
        assert!(!openai_payload_starts_client_output(&json!({
            "type":"response.output_item.added",
            "item":{"type":"reasoning","summary":[]}
        })));
        assert!(!openai_payload_starts_client_output(&json!({
            "type":"response.reasoning_summary_part.added",
            "part":{"type":"summary_text","text":""}
        })));
        assert!(openai_payload_starts_client_output(&json!({
            "type":"response.output_item.added",
            "item":{"type":"reasoning","encrypted_content":"ciphertext"}
        })));
        assert!(openai_payload_starts_client_output(&json!({
            "type":"response.output_text.delta",
            "delta":"hi"
        })));
    }

    #[test]
    fn same_batch_delta_then_error_has_started_output() {
        assert!(openai_stream_bytes_start_client_output(
            br#"event: response.created
data: {"type":"response.created","response":{"id":"resp_1"}}

event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"partial"}

event: error
data: {"type":"error","error":{"code":"server_is_overloaded","message":"overloaded"}}

"#
        ));
    }

    #[test]
    fn sanitizes_capacity_codes_and_leaves_rate_limit_alone() {
        let (out, changed) = sanitize_openai_capacity_shed_json_text(
            r#"{"type":"response.failed","response":{"error":{"code":"server_is_overloaded","message":"overloaded"}}}"#,
        );
        assert!(changed);
        assert!(out.contains(r#""code":"server_error""#));
        assert!(!out.contains("server_is_overloaded"));
        assert!(out.contains("overloaded"));

        let (out, changed) = sanitize_openai_capacity_shed_json_text(
            r#"{"type":"error","error":{"code":"slow_down","message":"slow down"}}"#,
        );
        assert!(changed);
        assert!(out.contains(r#""code":"server_error""#));

        let (out, changed) = sanitize_openai_capacity_shed_json_text(
            r#"{"type":"error","error":{"message":"Our servers are currently overloaded. Please try again later."}}"#,
        );
        assert!(changed);
        assert!(out.contains(r#""code":"server_error""#));

        let (out, changed) = sanitize_openai_capacity_shed_json_text(
            r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"try again in 3s"}}}"#,
        );
        assert!(!changed);
        assert!(out.contains("rate_limit_exceeded"));
    }

    #[test]
    fn sanitizes_sse_error_frames_in_place() {
        let input = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"Our servers are currently overloaded. Please try again later.\"}}\n",
            "\n"
        );
        let output = sanitize_openai_capacity_shed_sse_bytes(input.as_bytes());
        let text = String::from_utf8(output.to_vec()).unwrap();
        assert!(text.contains(r#""code":"server_error""#));
        assert!(!text.contains("server_is_overloaded"));
        assert!(text.contains("Our servers are currently overloaded"));
    }

    #[test]
    fn rate_limit_and_invalid_request_are_not_capacity_shed() {
        assert!(!is_openai_capacity_shed_value(&json!({
            "type":"error",
            "error":{"code":"rate_limit_exceeded","message":"try again later"}
        })));
        assert!(!is_openai_capacity_shed_value(&json!({
            "type":"error",
            "error":{"type":"invalid_request_error","message":"bad tool"}
        })));
        assert!(openai_payload_starts_client_output(&json!({
            "type":"error",
            "error":{"type":"invalid_request_error","code":"content_policy_violation","message":"blocked"}
        })));
    }
}
