use std::collections::BTreeSet;

use bytes::Bytes;
use serde_json::{json, Value};

const DEFAULT_TAIL_BYTES: usize = 200 * 1024;
const DEFAULT_SUMMARY_INPUT_BYTES: usize = 512 * 1024;
const MIN_INPUT_ITEMS: usize = 4;
const ITEM_TEXT_CAP_BYTES: usize = 8 * 1024;
const SUMMARY_OUTPUT_CAP_BYTES: usize = 64 * 1024;
const TRUNCATION_MARKER: &str = "...[truncated]";
const SUMMARY_PREFIX: &str = "[Conversation summary from earlier turns]\n";
const OMITTED_MARKER: &str =
    "[Earlier conversation turns were omitted because the input exceeded the model context window.]";
const SUMMARY_INSTRUCTION: &str = "You are a conversation compaction assistant. Summarize the earlier conversation transcript provided by the user into a dense briefing for the assistant that will continue this conversation. Preserve: key facts and decisions, user goals and constraints, file paths, code identifiers, tool results that matter, and unresolved tasks. Output only the summary text.";

pub(super) const SUMMARY_DATA_SOURCE: &str =
    crate::domain::usage::store::CODEX_OVERFLOW_COMPACT_SUMMARY_DATA_SOURCE;

#[derive(Debug)]
pub(super) struct OverflowCompactPlan {
    body: Value,
    model: String,
    transcript: String,
    summary_index: usize,
    removed_items: usize,
    retained_items: usize,
}

impl OverflowCompactPlan {
    pub(super) fn summary_request_body(&self) -> Option<Bytes> {
        if self.model.trim().is_empty() || self.transcript.trim().is_empty() {
            return None;
        }
        serde_json::to_vec(&json!({
            "model": self.model,
            "stream": true,
            "reasoning": {"effort": "low"},
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": SUMMARY_INSTRUCTION}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": self.transcript}]
                }
            ]
        }))
        .ok()
        .map(Bytes::from)
    }

    pub(super) fn finish(mut self, summary: Option<&str>) -> Option<Bytes> {
        let replacement = summary
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .map(|summary| {
                format!(
                    "{SUMMARY_PREFIX}{}",
                    truncate_utf8(summary, SUMMARY_OUTPUT_CAP_BYTES)
                )
            })
            .unwrap_or_else(|| OMITTED_MARKER.to_string());
        let input = self.body.get_mut("input")?.as_array_mut()?;
        let summary_item = input.get_mut(self.summary_index)?;
        summary_item["content"][0]["text"] = Value::String(replacement);
        serde_json::to_vec(&self.body).ok().map(Bytes::from)
    }

    pub(super) fn removed_items(&self) -> usize {
        self.removed_items
    }

    pub(super) fn retained_items(&self) -> usize {
        self.retained_items
    }
}

pub(super) fn enabled() -> bool {
    enabled_value(
        std::env::var("CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT")
            .ok()
            .as_deref(),
    )
}

pub(super) fn prepare(body: &[u8]) -> Option<OverflowCompactPlan> {
    let mut body = serde_json::from_slice::<Value>(body).ok()?;
    let input = body.get("input")?.as_array()?;
    if input.len() < MIN_INPUT_ITEMS {
        return None;
    }

    let mut cut = input.len();
    let mut used = 0usize;
    for index in (0..input.len()).rev() {
        let item_bytes = serde_json::to_vec(&input[index]).ok()?.len();
        if cut == input.len() && item_bytes > DEFAULT_TAIL_BYTES {
            return None;
        }
        if used.saturating_add(item_bytes) > DEFAULT_TAIL_BYTES && cut < input.len() {
            break;
        }
        used = used.saturating_add(item_bytes);
        cut = index;
    }

    let preserves_leading_instruction = input
        .first()
        .filter(|item| is_message_item(item))
        .and_then(|item| item.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|role| matches!(role.trim(), "developer" | "system"));
    let head_start = if preserves_leading_instruction { 1 } else { 0 };
    if cut <= head_start {
        return None;
    }

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let transcript = flatten_transcript(&input[head_start..cut], DEFAULT_SUMMARY_INPUT_BYTES);
    let removed_items = cut.saturating_sub(head_start);
    let retained_items = input.len().saturating_sub(cut) + head_start;
    let mut replacement = Vec::with_capacity(retained_items.saturating_add(1));
    replacement.extend_from_slice(&input[..head_start]);
    let summary_index = replacement.len();
    replacement.push(json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": OMITTED_MARKER}]
    }));
    replacement.extend_from_slice(&input[cut..]);
    repair_tool_pairing(&mut replacement);
    body["input"] = Value::Array(replacement);

    Some(OverflowCompactPlan {
        body,
        model,
        transcript,
        summary_index,
        removed_items,
        retained_items,
    })
}

pub(super) fn is_context_length_exceeded_body(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    [
        "/error/code",
        "/error/type",
        "/code",
        "/detail/code",
        "/response/error/code",
        "/response/error/type",
    ]
    .into_iter()
    .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .any(context_code_matches)
        || [
            "/error/message",
            "/message",
            "/detail",
            "/response/error/message",
        ]
        .into_iter()
        .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .any(context_message_matches)
}

pub(super) fn is_context_length_failure(code: &str, message: &str) -> bool {
    context_code_matches(code) || context_message_matches(message)
}

pub(super) fn extract_summary_output(body: &[u8]) -> Option<String> {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        return output_text_from_value(&value);
    }

    let text = String::from_utf8_lossy(body);
    let mut deltas = String::new();
    let mut completed = None;
    for line in text.lines() {
        let payload = line
            .trim_end_matches('\r')
            .strip_prefix("data:")
            .map(str::trim);
        let Some(payload) = payload.filter(|payload| *payload != "[DONE]") else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    deltas.push_str(delta);
                }
            }
            Some("response.completed") => completed = output_text_from_value(&value),
            _ => {}
        }
    }
    completed
        .filter(|output| !output.trim().is_empty())
        .or_else(|| (!deltas.trim().is_empty()).then(|| deltas.trim().to_string()))
}

fn enabled_value(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no" | "disabled"
            )
        })
}

fn context_code_matches(value: &str) -> bool {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .contains("context_length")
}

fn context_message_matches(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.contains("exceeds the context window")
        || value.contains("context length")
        || value.contains("maximum context")
        || value.contains("context window is too long")
}

fn flatten_transcript(items: &[Value], cap_bytes: usize) -> String {
    let mut lines = Vec::new();
    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if is_message_item(item) {
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .filter(|role| !role.trim().is_empty())
                .unwrap_or("user");
            let text = flatten_content_text(item.get("content"));
            if !text.is_empty() {
                lines.push(format!(
                    "{role}: {}",
                    truncate_utf8(&text, ITEM_TEXT_CAP_BYTES)
                ));
            }
        } else if is_tool_call_type(item_type) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            lines.push(format!(
                "assistant tool call {name}({})",
                truncate_utf8(arguments, 1024)
            ));
        } else if is_tool_output_type(item_type) {
            lines.push(format!(
                "tool output: {}",
                truncate_utf8(
                    &flatten_tool_output(item.get("output")),
                    ITEM_TEXT_CAP_BYTES
                )
            ));
        }
    }
    truncate_middle_utf8(&lines.join("\n"), cap_bytes)
}

fn is_message_item(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => true,
        Some("") | None => item
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| matches!(role, "developer" | "system" | "user" | "assistant")),
        _ => false,
    }
}

fn flatten_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn flatten_tool_output(output: Option<&Value>) -> String {
    match output {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                serde_json::to_string(parts).unwrap_or_default()
            } else {
                text
            }
        }
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn repair_tool_pairing(input: &mut Vec<Value>) {
    let call_ids = input
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(is_tool_call_type)
        })
        .filter_map(call_id)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let mut output_ids = input
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(is_tool_output_type)
        })
        .filter_map(call_id)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    let mut repaired = Vec::with_capacity(input.len());
    for item in std::mem::take(input) {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let item_call_id = call_id(&item).map(str::to_string);
        if is_tool_output_type(item_type)
            && item_call_id
                .as_deref()
                .is_none_or(|call_id| !call_ids.contains(call_id))
        {
            let label = item_call_id.as_deref().map_or_else(
                || "[Tool output from an earlier turn]".to_string(),
                |call_id| format!("[Tool output from an earlier turn, call_id {call_id}]"),
            );
            let output = flatten_tool_output(item.get("output"));
            repaired.push(json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": format!("{label}\n{output}")}]
            }));
            continue;
        }

        let placeholder_output_type = item_call_id.as_deref().and_then(|call_id| {
            (!output_ids.contains(call_id))
                .then(|| output_type_for_call(item_type).map(|output_type| (call_id, output_type)))
                .flatten()
        });
        repaired.push(item);
        if let Some((call_id, output_type)) = placeholder_output_type {
            repaired.push(json!({
                "type": output_type,
                "call_id": call_id,
                "output": "[tool output was not recorded]"
            }));
            output_ids.insert(call_id.to_string());
        }
    }
    *input = repaired;
}

fn is_tool_call_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "function_call"
            | "tool_call"
            | "local_shell_call"
            | "shell_call"
            | "apply_patch_call"
            | "tool_search_call"
            | "custom_tool_call"
            | "mcp_tool_call"
    )
}

fn is_tool_output_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "function_call_output"
            | "tool_call_output"
            | "local_shell_call_output"
            | "shell_call_output"
            | "apply_patch_call_output"
            | "tool_search_call_output"
            | "custom_tool_call_output"
            | "mcp_tool_call_output"
    )
}

fn output_type_for_call(item_type: &str) -> Option<&'static str> {
    match item_type {
        "function_call" => Some("function_call_output"),
        "tool_call" => Some("tool_call_output"),
        "local_shell_call" => Some("local_shell_call_output"),
        "shell_call" => Some("shell_call_output"),
        "apply_patch_call" => Some("apply_patch_call_output"),
        "tool_search_call" => Some("tool_search_call_output"),
        "custom_tool_call" => Some("custom_tool_call_output"),
        "mcp_tool_call" => Some("mcp_tool_call_output"),
        _ => None,
    }
}

fn call_id(item: &Value) -> Option<&str> {
    item.get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id| !call_id.is_empty())
}

fn output_text_from_value(value: &Value) -> Option<String> {
    let response = value.get("response").unwrap_or(value);
    let direct = response
        .get("output_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    if direct.is_some() {
        return direct;
    }
    let text = response
        .get("output")
        .and_then(Value::as_array)?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
        })
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let available = max_bytes.saturating_sub(TRUNCATION_MARKER.len());
    let end = char_boundary_at_or_before(value, available);
    format!("{}{TRUNCATION_MARKER}", &value[..end])
}

fn truncate_middle_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let marker = "\n[... middle of the transcript truncated ...]\n";
    let available = max_bytes.saturating_sub(marker.len());
    let head_end = char_boundary_at_or_before(value, available / 2);
    let tail_start_target = value
        .len()
        .saturating_sub(available.saturating_sub(head_end));
    let tail_start = char_boundary_at_or_after(value, tail_start_target);
    format!("{}{}{}", &value[..head_end], marker, &value[tail_start..])
}

fn char_boundary_at_or_before(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn char_boundary_at_or_after(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_compact_is_disabled_without_an_explicit_truthy_value() {
        assert!(!enabled_value(None));
        assert!(!enabled_value(Some("")));
        assert!(!enabled_value(Some("false")));
        assert!(!enabled_value(Some("0")));
        assert!(enabled_value(Some("true")));
        assert!(enabled_value(Some("1")));
    }

    #[test]
    fn detects_structured_and_message_context_overflow_errors() {
        assert!(is_context_length_exceeded_body(
            br#"{"error":{"code":"context_length_exceeded"}}"#
        ));
        assert!(is_context_length_exceeded_body(
            br#"{"response":{"error":{"message":"Input exceeds the context window"}}}"#
        ));
        assert!(!is_context_length_exceeded_body(
            br#"{"error":{"code":"invalid_tool"}}"#
        ));
    }

    #[test]
    fn compacts_old_items_and_preserves_system_and_recent_tail() {
        let large = "x".repeat(110 * 1024);
        let body = serde_json::to_vec(&json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"old question"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"old answer"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":large}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":large}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"latest"}]}
            ]
        }))
        .unwrap();
        let plan = prepare(&body).unwrap();
        assert!(plan.removed_items() >= 2);
        assert!(plan.retained_items() >= 2);
        let compacted = plan.finish(Some("decisions and constraints")).unwrap();
        let compacted: Value = serde_json::from_slice(&compacted).unwrap();
        assert_eq!(compacted["input"][0]["content"][0]["text"], "rules");
        assert_eq!(compacted["input"][0]["role"], "developer");
        assert_eq!(compacted["input"][1]["role"], "user");
        assert!(compacted["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.to_string().contains("decisions and constraints")));
        assert!(compacted["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.to_string().contains("latest")));
    }

    #[test]
    fn skips_compaction_when_the_latest_item_exceeds_the_tail_budget() {
        let body = serde_json::to_vec(&json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"old question"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"old answer"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"x".repeat(DEFAULT_TAIL_BYTES + 1)}]}
            ]
        }))
        .unwrap();

        assert!(prepare(&body).is_none());
    }

    #[test]
    fn bounds_generated_summary_text() {
        let large = "x".repeat(110 * 1024);
        let body = serde_json::to_vec(&json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"old"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":large}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":large}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"latest"}]}
            ]
        }))
        .unwrap();
        let compacted = prepare(&body)
            .unwrap()
            .finish(Some(&"summary".repeat(SUMMARY_OUTPUT_CAP_BYTES)))
            .unwrap();
        let compacted: Value = serde_json::from_slice(&compacted).unwrap();
        let summary = compacted["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap();

        assert_eq!(compacted["input"][0]["role"], "user");
        assert!(summary.len() <= SUMMARY_PREFIX.len() + SUMMARY_OUTPUT_CAP_BYTES);
        assert!(summary.ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn summary_failure_uses_omission_marker_and_repairs_orphan_output() {
        let large = "x".repeat(210 * 1024);
        let body = serde_json::to_vec(&json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"function_call","call_id":"removed","name":"lookup","arguments":"{}"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":large}]},
                {"type":"function_call_output","call_id":"removed","output":"important result"},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"latest"}]}
            ]
        }))
        .unwrap();
        let compacted = prepare(&body).unwrap().finish(None).unwrap();
        let compacted = String::from_utf8(compacted.to_vec()).unwrap();
        assert!(compacted.contains(OMITTED_MARKER));
        assert!(!compacted.contains("function_call_output"));
        assert!(compacted.contains("Tool output from an earlier turn"));
        assert!(compacted.contains("important result"));
    }

    #[test]
    fn repair_adds_outputs_for_every_supported_tool_call_type() {
        let pairs = [
            ("function_call", "function_call_output"),
            ("tool_call", "tool_call_output"),
            ("local_shell_call", "local_shell_call_output"),
            ("shell_call", "shell_call_output"),
            ("apply_patch_call", "apply_patch_call_output"),
            ("tool_search_call", "tool_search_call_output"),
            ("custom_tool_call", "custom_tool_call_output"),
            ("mcp_tool_call", "mcp_tool_call_output"),
        ];
        let mut input = pairs
            .iter()
            .enumerate()
            .map(|(index, (call_type, _))| {
                json!({"type": call_type, "call_id": format!("call-{index}")})
            })
            .collect::<Vec<_>>();

        repair_tool_pairing(&mut input);

        assert_eq!(input.len(), pairs.len() * 2);
        for (index, (call_type, output_type)) in pairs.iter().enumerate() {
            let call = &input[index * 2];
            let output = &input[index * 2 + 1];
            assert_eq!(call.get("type").and_then(Value::as_str), Some(*call_type));
            assert_eq!(
                output.get("type").and_then(Value::as_str),
                Some(*output_type)
            );
            assert_eq!(output.get("call_id"), call.get("call_id"));
        }
    }

    #[test]
    fn extracts_summary_from_json_and_sse() {
        let json =
            br#"{"output":[{"type":"message","content":[{"type":"output_text","text":"brief"}]}]}"#;
        assert_eq!(extract_summary_output(json).as_deref(), Some("brief"));
        let sse = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"dense \"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"brief\"}\n\ndata: [DONE]\n\n";
        assert_eq!(extract_summary_output(sse).as_deref(), Some("dense brief"));
    }

    #[test]
    fn truncation_respects_utf8_boundaries() {
        let value = "前文".repeat(10_000);
        let truncated = truncate_middle_utf8(&value, 1024);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.len() <= 1024);
    }
}
