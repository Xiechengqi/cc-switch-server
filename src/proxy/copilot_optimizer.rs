use std::collections::HashSet;

use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub(super) struct CopilotOptimizerConfig {
    pub enabled: bool,
    pub request_classification: bool,
    pub tool_result_merging: bool,
    pub compact_detection: bool,
    pub deterministic_request_id: bool,
    pub subagent_detection: bool,
    pub warmup_downgrade: bool,
    pub warmup_model: String,
    pub strip_thinking: bool,
}

impl Default for CopilotOptimizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            request_classification: true,
            tool_result_merging: true,
            compact_detection: true,
            deterministic_request_id: true,
            subagent_detection: true,
            warmup_downgrade: true,
            warmup_model: "gpt-5-mini".to_string(),
            strip_thinking: true,
        }
    }
}

impl CopilotOptimizerConfig {
    pub(super) fn from_settings(settings: &Value) -> Self {
        let Some(value) = settings
            .get("copilotOptimizer")
            .or_else(|| settings.get("copilot_optimizer"))
            .or_else(|| settings.get("copilot"))
        else {
            return Self::default();
        };
        if let Some(enabled) = value_as_bool(value) {
            return Self {
                enabled,
                ..Self::default()
            };
        }
        let mut config = Self::default();
        if let Some(enabled) = bool_field(value, &["enabled"]) {
            config.enabled = enabled;
        }
        if let Some(enabled) =
            bool_field(value, &["requestClassification", "request_classification"])
        {
            config.request_classification = enabled;
        }
        if let Some(enabled) = bool_field(value, &["toolResultMerging", "tool_result_merging"]) {
            config.tool_result_merging = enabled;
        }
        if let Some(enabled) = bool_field(value, &["compactDetection", "compact_detection"]) {
            config.compact_detection = enabled;
        }
        if let Some(enabled) = bool_field(
            value,
            &["deterministicRequestId", "deterministic_request_id"],
        ) {
            config.deterministic_request_id = enabled;
        }
        if let Some(enabled) = bool_field(value, &["subagentDetection", "subagent_detection"]) {
            config.subagent_detection = enabled;
        }
        if let Some(enabled) = bool_field(value, &["warmupDowngrade", "warmup_downgrade"]) {
            config.warmup_downgrade = enabled;
        }
        if let Some(enabled) = bool_field(value, &["stripThinking", "strip_thinking"]) {
            config.strip_thinking = enabled;
        }
        if let Some(model) = string_field(value, &["warmupModel", "warmup_model"]) {
            config.warmup_model = model;
        }
        config
    }
}

#[derive(Debug, Clone)]
pub(super) struct CopilotClassification {
    pub initiator: &'static str,
    pub is_warmup: bool,
    pub is_compact: bool,
    pub is_subagent: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CopilotOptimization {
    pub headers: Vec<(&'static str, String)>,
    pub model_source: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CopilotRequestMetadata {
    pub has_anthropic_beta: bool,
    pub session_id: Option<String>,
}

pub(super) fn optimize_request(
    body: &mut Value,
    config: &CopilotOptimizerConfig,
    metadata: &CopilotRequestMetadata,
) -> CopilotOptimization {
    if !config.enabled {
        return CopilotOptimization {
            headers: copilot_static_headers(),
            model_source: None,
        };
    }

    let classification = classify_request(
        body,
        metadata.has_anthropic_beta,
        config.compact_detection,
        config.subagent_detection,
    );
    let mut model_source = None;
    *body = sanitize_orphan_tool_results(std::mem::take(body));
    if config.tool_result_merging {
        *body = merge_tool_results(std::mem::take(body));
    }
    if config.strip_thinking {
        *body = strip_thinking_blocks(std::mem::take(body));
    }
    if config.warmup_downgrade && classification.is_warmup {
        body["model"] = Value::String(config.warmup_model.clone());
        model_source = Some("copilot_warmup");
    }

    let mut headers = copilot_static_headers();
    if config.request_classification {
        upsert_header(
            &mut headers,
            "x-initiator",
            classification.initiator.to_string(),
        );
    }
    if classification.is_subagent {
        upsert_header(
            &mut headers,
            "x-interaction-type",
            "conversation-subagent".to_string(),
        );
    }
    let session_id = metadata
        .session_id
        .as_deref()
        .or_else(|| session_id_from_body(body));
    if config.deterministic_request_id {
        let request_id = deterministic_request_id(body, session_id.unwrap_or_default());
        upsert_header(&mut headers, "x-request-id", request_id.clone());
        upsert_header(&mut headers, "x-agent-task-id", request_id);
    }
    if let Some(interaction_id) = session_id.and_then(deterministic_interaction_id) {
        upsert_header(&mut headers, "x-interaction-id", interaction_id);
    }

    tracing::debug!(
        initiator = classification.initiator,
        is_warmup = classification.is_warmup,
        is_compact = classification.is_compact,
        is_subagent = classification.is_subagent,
        "applied GitHub Copilot static request optimizer"
    );

    CopilotOptimization {
        headers,
        model_source,
    }
}

pub(super) fn classify_request(
    body: &Value,
    has_anthropic_beta: bool,
    compact_detection: bool,
    subagent_detection: bool,
) -> CopilotClassification {
    let is_compact = compact_detection && is_compact_request(body);
    let is_subagent = subagent_detection && detect_subagent(body);
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return CopilotClassification {
            initiator: "user",
            is_warmup: is_warmup_request(body, has_anthropic_beta, false),
            is_compact: false,
            is_subagent,
        };
    };
    let Some(last_msg) = messages.last() else {
        return CopilotClassification {
            initiator: "user",
            is_warmup: is_warmup_request(body, has_anthropic_beta, false),
            is_compact: false,
            is_subagent,
        };
    };
    if last_msg.get("role").and_then(Value::as_str) != Some("user") {
        return CopilotClassification {
            initiator: if is_subagent { "agent" } else { "user" },
            is_warmup: false,
            is_compact,
            is_subagent,
        };
    }

    let is_user_initiated = match last_msg.get("content") {
        Some(Value::Array(blocks)) => !blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result")),
        Some(Value::String(_)) => true,
        _ => false,
    };
    let initiator = if is_subagent || !is_user_initiated || is_compact {
        "agent"
    } else {
        "user"
    };
    CopilotClassification {
        initiator,
        is_warmup: initiator == "user" && is_warmup_request(body, has_anthropic_beta, is_compact),
        is_compact,
        is_subagent,
    }
}

pub(super) fn sanitize_orphan_tool_results(mut body: Value) -> Value {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return body;
    };
    if messages.len() < 2 {
        return body;
    }

    for i in 1..messages.len() {
        if messages[i].get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let prev_tool_use_ids: HashSet<String> =
            if messages[i - 1].get("role").and_then(Value::as_str) == Some("assistant") {
                messages[i - 1]
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|block| {
                                block.get("type").and_then(Value::as_str) == Some("tool_use")
                            })
                            .filter_map(|block| {
                                block.get("id").and_then(Value::as_str).map(String::from)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                HashSet::new()
            };
        let Some(content) = messages[i].get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if tool_use_id.is_empty() || !prev_tool_use_ids.contains(tool_use_id) {
                let content_text = match block.get("content") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                *block = serde_json::json!({
                    "type": "text",
                    "text": format!("[Tool result for {}]: {}", tool_use_id, content_text)
                });
            }
        }
    }
    body
}

pub(super) fn merge_tool_results(mut body: Value) -> Value {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return body;
    };
    for msg in messages.iter_mut() {
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = msg.get("content").and_then(Value::as_array) else {
            continue;
        };
        let mut tool_results = Vec::new();
        let mut text_blocks = Vec::new();
        let mut valid = true;
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => tool_results.push(block.clone()),
                Some("text") => text_blocks.push(block.clone()),
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        if valid && !tool_results.is_empty() && !text_blocks.is_empty() {
            msg["content"] =
                Value::Array(merge_blocks_into_tool_results(tool_results, text_blocks));
        }
    }

    let Some(messages) = body.get("messages").and_then(Value::as_array).cloned() else {
        return body;
    };
    if messages.len() <= 1 {
        return body;
    }
    let mut merged = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        if is_tool_result_only_message(&messages[i]) {
            let mut combined = Vec::new();
            while i < messages.len() && is_tool_result_only_message(&messages[i]) {
                if let Some(content) = messages[i].get("content").and_then(Value::as_array) {
                    combined.extend(content.iter().cloned());
                }
                i += 1;
            }
            if !combined.is_empty() {
                merged.push(serde_json::json!({"role": "user", "content": combined}));
            }
        } else {
            merged.push(messages[i].clone());
            i += 1;
        }
    }
    body["messages"] = Value::Array(merged);
    body
}

pub(super) fn strip_thinking_blocks(mut body: Value) -> Value {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return body;
    };
    for msg in messages {
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = msg.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        content.retain(|block| {
            !matches!(
                block.get("type").and_then(Value::as_str),
                Some("thinking") | Some("redacted_thinking")
            )
        });
    }
    body
}

fn is_warmup_request(body: &Value, has_anthropic_beta: bool, is_compact: bool) -> bool {
    has_anthropic_beta
        && !is_compact
        && body
            .get("tools")
            .and_then(Value::as_array)
            .is_none_or(|tools| tools.is_empty())
}

fn is_compact_request(body: &Value) -> bool {
    if extract_system_text(body)
        .starts_with("You are a helpful AI assistant tasked with summarizing conversations")
    {
        return true;
    }
    let Some(last_msg) = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
    else {
        return false;
    };
    if last_msg.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = extract_text_from_message(last_msg);
    text.contains("CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.")
        || (text.contains("Pending Tasks:") && text.contains("Current Work:"))
}

fn detect_subagent(body: &Value) -> bool {
    if extract_system_text(body).contains("__SUBAGENT_MARKER__") {
        return true;
    }
    if body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message.get("role").and_then(Value::as_str) == Some("user")
                    && extract_text_from_message(message).contains("__SUBAGENT_MARKER__")
            })
        })
    {
        return true;
    }
    body.pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .is_some_and(|user_id| user_id.contains("_agent_"))
}

fn deterministic_request_id(body: &Value, session_id: &str) -> String {
    let Some(last_user_content) = find_last_user_content(body) else {
        return random_uuid_like();
    };
    stable_uuid_like(&format!("{session_id}:{last_user_content}"))
}

fn deterministic_interaction_id(session_id: &str) -> Option<String> {
    let session_id = session_id.trim();
    (!session_id.is_empty()).then(|| stable_uuid_like(&format!("interaction:{session_id}")))
}

fn copilot_static_headers() -> Vec<(&'static str, String)> {
    vec![
        ("user-agent", "GitHubCopilotChat/0.38.2".to_string()),
        ("editor-version", "vscode/1.110.1".to_string()),
        ("editor-plugin-version", "copilot-chat/0.38.2".to_string()),
        ("copilot-integration-id", "vscode-chat".to_string()),
        ("x-github-api-version", "2025-10-01".to_string()),
        ("openai-intent", "conversation-panel".to_string()),
        ("x-initiator", "user".to_string()),
        (
            "x-vscode-user-agent-library-version",
            "electron-fetch".to_string(),
        ),
        ("x-interaction-type", "conversation".to_string()),
        ("x-request-id", random_uuid_like()),
        ("x-agent-task-id", random_uuid_like()),
    ]
}

fn upsert_header(headers: &mut Vec<(&'static str, String)>, name: &'static str, value: String) {
    if let Some((_, current)) = headers
        .iter_mut()
        .find(|(current_name, _)| current_name.eq_ignore_ascii_case(name))
    {
        *current = value;
    } else {
        headers.push((name, value));
    }
}

fn session_id_from_body(body: &Value) -> Option<&str> {
    body.pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .and_then(parse_session_from_user_id)
        .or_else(|| {
            body.pointer("/metadata/session_id")
                .or_else(|| body.pointer("/metadata/sessionId"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            body.pointer("/metadata/user_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
}

fn parse_session_from_user_id(user_id: &str) -> Option<&str> {
    let session_id = user_id.split_once("_session_")?.1.trim();
    (!session_id.is_empty()).then_some(session_id)
}

fn extract_system_text(body: &Value) -> String {
    match body.get("system") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn find_last_user_content(body: &Value) -> Option<String> {
    let messages = body.get("messages").and_then(Value::as_array)?;
    for msg in messages.iter().rev() {
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let content = msg.get("content")?;
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(blocks) = content.as_array() {
            let filtered = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"))
                .map(|block| {
                    let mut block = block.clone();
                    if let Some(object) = block.as_object_mut() {
                        object.remove("cache_control");
                    }
                    block
                })
                .collect::<Vec<_>>();
            if !filtered.is_empty() {
                return Some(serde_json::to_string(&filtered).unwrap_or_default());
            }
        }
    }
    None
}

fn merge_blocks_into_tool_results(
    mut tool_results: Vec<Value>,
    text_blocks: Vec<Value>,
) -> Vec<Value> {
    if tool_results.len() == text_blocks.len() {
        for (tool_result, text_block) in tool_results.iter_mut().zip(text_blocks.iter()) {
            append_text_to_tool_result(tool_result, text_block);
        }
    } else if let Some(last_tool_result) = tool_results.last_mut() {
        for text_block in &text_blocks {
            append_text_to_tool_result(last_tool_result, text_block);
        }
    }
    tool_results
}

fn append_text_to_tool_result(tool_result: &mut Value, text_block: &Value) {
    let text = text_block.get("text").and_then(Value::as_str).unwrap_or("");
    if text.trim().is_empty() {
        return;
    }
    match tool_result.get_mut("content") {
        Some(Value::String(existing)) => {
            existing.push('\n');
            existing.push_str(text);
        }
        Some(Value::Array(items)) => {
            items.push(serde_json::json!({"type": "text", "text": text}));
        }
        _ => {
            tool_result["content"] = Value::String(text.to_string());
        }
    }
}

fn extract_text_from_message(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn is_tool_result_only_message(msg: &Value) -> bool {
    msg.get("role").and_then(Value::as_str) == Some("user")
        && msg
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                !blocks.is_empty()
                    && blocks.iter().all(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                    })
            })
}

fn stable_uuid_like(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid_like_from_bytes(bytes)
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    uuid_like_from_bytes(bytes)
}

fn uuid_like_from_bytes(mut bytes: [u8; 16]) -> String {
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

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    })
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_as_bool))
}

fn value_as_bool(value: &Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    let value = value.as_str()?.trim().to_ascii_lowercase();
    match value.as_str() {
        "true" | "1" | "yes" | "on" | "enabled" => Some(true),
        "false" | "0" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_tool_result_as_agent_before_sanitize() {
        let body = json!({
            "messages": [
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "missing", "content": "result"},
                    {"type": "text", "text": "next"}
                ]}
            ]
        });
        let classification = classify_request(&body, false, true, false);
        assert_eq!(classification.initiator, "agent");
    }

    #[test]
    fn detects_compact_without_generic_summary_false_positive() {
        let compact = json!({
            "messages": [{"role": "user", "content": "Pending Tasks:\n- a\n\nCurrent Work:\n- b"}]
        });
        let generic = json!({
            "messages": [{"role": "user", "content": "Please summarize this conversation."}]
        });
        assert!(classify_request(&compact, false, true, false).is_compact);
        assert!(!classify_request(&generic, false, true, false).is_compact);
    }

    #[test]
    fn optimizer_injects_agent_headers_and_strips_thinking() {
        let mut body = json!({
            "metadata": {"session_id": "s1"},
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hidden"},
                    {"type": "text", "text": "visible"}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "missing", "content": "result"}
                ]}
            ]
        });
        let result = optimize_request(
            &mut body,
            &CopilotOptimizerConfig::default(),
            &CopilotRequestMetadata::default(),
        );
        assert!(result
            .headers
            .iter()
            .any(|item| item == &("x-initiator", "agent".to_string())));
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    }

    #[test]
    fn warmup_can_downgrade_model() {
        let mut body = json!({
            "model": "claude-sonnet-4.6",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let metadata = CopilotRequestMetadata {
            has_anthropic_beta: true,
            session_id: None,
        };
        let result = optimize_request(&mut body, &CopilotOptimizerConfig::default(), &metadata);
        assert_eq!(body["model"], "gpt-5-mini");
        assert_eq!(result.model_source, Some("copilot_warmup"));
    }
}
