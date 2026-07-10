use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;
use sha2::{Digest, Sha256};
use twox_hash::XxHash64;

use crate::domain::claude_cli::{
    claude_billing_header_text, claude_cch_seed, claude_cli_user_agent, claude_stainless_arch,
    claude_stainless_os, claude_stainless_runtime, claude_stainless_runtime_version,
    CLAUDE_CODE_IDENTITY_TEXT, DEFAULT_STAINLESS_PACKAGE_VERSION,
};

use super::ProxyError;

const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const COMPUTER_USE_BETA: &str = "computer-use-2024-10-22";
const BILLING_PREFIX: &str = "x-anthropic-billing-header:";
const CLAUDE_CODE_PROMPT_MATCH_THRESHOLD: f64 = 0.5;
pub(crate) const CLAUDE_BODY_RETRY_STAGE_HEADER: &str = "x-cc-switch-claude-body-retry";

pub(crate) struct ClaudeForwardContract {
    pub headers: Vec<(&'static str, String)>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeBodyRetryStage {
    Thinking,
    SignatureSensitive,
    WebSearchHistory,
}

impl ClaudeBodyRetryStage {
    pub(crate) fn as_header_value(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::SignatureSensitive => "signature_sensitive",
            Self::WebSearchHistory => "web_search_history",
        }
    }

    pub(crate) fn from_header_value(value: &str) -> Option<Self> {
        match value.trim() {
            "thinking" => Some(Self::Thinking),
            "signature_sensitive" => Some(Self::SignatureSensitive),
            "web_search_history" => Some(Self::WebSearchHistory),
            _ => None,
        }
    }

    fn from_headers(headers: &HeaderMap) -> Option<Self> {
        headers
            .get(CLAUDE_BODY_RETRY_STAGE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(Self::from_header_value)
    }
}

pub(crate) fn apply_forward_contract(
    url: &mut String,
    body: &mut Bytes,
    client_headers: &HeaderMap,
    identity_seed: &str,
) -> Result<ClaudeForwardContract, ProxyError> {
    *url = ensure_claude_oauth_beta_query(url);
    let retry_stage = ClaudeBodyRetryStage::from_headers(client_headers);
    let mut session_id = claude_session_id_from_headers(client_headers);
    let mut body_shape = None;
    if !body.is_empty() {
        let mut value = serde_json::from_slice(body).map_err(|error| {
            ProxyError::bad_request(format!(
                "claude oauth request body must be valid json: {error}"
            ))
        })?;
        session_id = session_id
            .or_else(|| claude_session_id_from_body_value(&value))
            .or_else(|| Some(synth_session_id(identity_seed, &value)));
        if let Some(session_id) = session_id.as_deref() {
            ensure_claude_metadata_user_id(&mut value, identity_seed, session_id);
        }
        value = ensure_claude_code_identity(value);
        if let Some(stage) = retry_stage {
            value = apply_body_retry_stage(value, stage);
        }
        body_shape = Some(value.clone());
        *body = Bytes::from(serde_json::to_vec(&value).map_err(|error| {
            ProxyError::bad_request(format!("claude oauth request body encode failed: {error}"))
        })?);
    }
    let mut headers = claude_cli_headers(session_id.as_deref(), identity_seed, body_shape.as_ref());
    headers.push(anthropic_beta_header(client_headers, body_shape.as_ref()));
    Ok(ClaudeForwardContract {
        headers,
        session_id,
    })
}

pub(super) fn anthropic_beta_header(
    client_headers: &HeaderMap,
    body: Option<&Value>,
) -> (&'static str, String) {
    (
        "anthropic-beta",
        build_anthropic_beta_value(client_headers, body, true),
    )
}

fn claude_cli_headers(
    session_id: Option<&str>,
    identity_seed: &str,
    body: Option<&Value>,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("user-agent", claude_cli_user_agent()),
        ("x-app", "cli".to_string()),
        (
            "anthropic-dangerous-direct-browser-access",
            "true".to_string(),
        ),
        ("sec-fetch-mode", "cors".to_string()),
        ("x-stainless-lang", "js".to_string()),
        (
            "x-stainless-package-version",
            DEFAULT_STAINLESS_PACKAGE_VERSION.to_string(),
        ),
        ("x-stainless-os", claude_stainless_os(Some(identity_seed))),
        (
            "x-stainless-arch",
            claude_stainless_arch(Some(identity_seed)),
        ),
        ("x-stainless-runtime", claude_stainless_runtime()),
        (
            "x-stainless-runtime-version",
            claude_stainless_runtime_version(),
        ),
        ("x-stainless-retry-count", "0".to_string()),
        ("x-stainless-timeout", stainless_timeout_for_body(body)),
    ];
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        headers.push(("x-claude-code-session-id", session_id.to_string()));
    }
    headers
}

fn ensure_claude_oauth_beta_query(url: &str) -> String {
    let (base, query) = split_endpoint_and_query(url);
    match query {
        Some(query) if !query.is_empty() => {
            if query.split('&').any(|part| part == "beta=true") {
                url.to_string()
            } else {
                format!("{base}?beta=true&{query}")
            }
        }
        _ => format!("{base}?beta=true"),
    }
}

fn split_endpoint_and_query(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    }
}

fn sign_claude_oauth_messages_body(mut body: Value) -> Value {
    let Some(system) = body.get("system").and_then(|value| value.as_array()) else {
        return body;
    };
    let Some(first_block) = system.first() else {
        return body;
    };
    let Some(text) = first_block.get("text").and_then(|value| value.as_str()) else {
        return body;
    };
    if !text.starts_with(BILLING_PREFIX) {
        return body;
    }
    if !cch_signature_present(text) {
        return body;
    }

    let unsigned_text = replace_cch_value(text, "00000");
    body["system"][0]["text"] = Value::String(unsigned_text.clone());

    let Ok(unsigned_body) = serde_json::to_vec(&body) else {
        return body;
    };

    let mut hasher = XxHash64::with_seed(claude_cch_seed());
    hasher.write(&unsigned_body);
    let cch = format!("{:05x}", hasher.finish() & 0xFFFFF);
    let signed_text = replace_cch_value(&unsigned_text, &cch);
    body["system"][0]["text"] = Value::String(signed_text);
    body
}

fn ensure_claude_code_identity(mut body: Value) -> Value {
    if body
        .get("system")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t.starts_with(BILLING_PREFIX))
    {
        ensure_claude_tools_array(&mut body);
        return sign_claude_oauth_messages_body(body);
    }

    let is_claude_code_system = system_matches_claude_code_template(&body);
    let existing_system = if is_claude_code_system {
        None
    } else {
        body.as_object_mut()
            .and_then(|object| object.remove("system"))
    };

    if let Some(system) = existing_system {
        migrate_system_to_messages(&mut body, system);
    }

    let mut blocks = Vec::new();
    blocks.push(claude_billing_block());
    if is_claude_code_system {
        if let Some(existing) = body
            .as_object_mut()
            .and_then(|object| object.remove("system"))
        {
            append_system_blocks(&mut blocks, existing);
        }
    } else {
        blocks.push(claude_identity_block());
    }

    body["system"] = Value::Array(blocks);
    ensure_claude_tools_array(&mut body);
    sign_claude_oauth_messages_body(body)
}

fn ensure_claude_oauth_billing_header_system(body: Value) -> Value {
    ensure_claude_code_identity(body)
}

fn apply_body_retry_stage(mut body: Value, stage: ClaudeBodyRetryStage) -> Value {
    match stage {
        ClaudeBodyRetryStage::Thinking => {
            downgrade_thinking_blocks_for_retry(&mut body);
        }
        ClaudeBodyRetryStage::SignatureSensitive => {
            downgrade_thinking_blocks_for_retry(&mut body);
            downgrade_signature_sensitive_blocks_for_retry(&mut body);
        }
        ClaudeBodyRetryStage::WebSearchHistory => {
            downgrade_thinking_blocks_for_retry(&mut body);
            downgrade_signature_sensitive_blocks_for_retry(&mut body);
            filter_web_search_history_blocks(&mut body);
        }
    }
    sign_claude_oauth_messages_body(body)
}

fn ensure_claude_tools_array(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
}

fn claude_billing_block() -> Value {
    serde_json::json!({
        "type": "text",
        "text": claude_billing_header_text(),
        "cache_control": {"type": "ephemeral"}
    })
}

fn claude_identity_block() -> Value {
    serde_json::json!({
        "type": "text",
        "text": CLAUDE_CODE_IDENTITY_TEXT,
        "cache_control": {"type": "ephemeral"}
    })
}

fn migrate_system_to_messages(body: &mut Value, system: Value) {
    let Some(content) = system_to_user_message_content(system) else {
        return;
    };
    let message = serde_json::json!({
        "role": "user",
        "content": content
    });
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match object.get_mut("messages") {
        Some(Value::Array(messages)) => messages.insert(0, message),
        _ => {
            object.insert("messages".to_string(), Value::Array(vec![message]));
        }
    }
}

fn system_to_user_message_content(system: Value) -> Option<Value> {
    match system {
        Value::String(text) if !text.trim().is_empty() => Some(Value::String(text)),
        Value::Array(blocks) if !blocks.is_empty() => Some(Value::Array(blocks)),
        Value::Object(object) if !object.is_empty() => {
            Some(Value::Array(vec![Value::Object(object)]))
        }
        _ => None,
    }
}

fn append_system_blocks(blocks: &mut Vec<Value>, system: Value) {
    match system {
        Value::String(text) if !text.trim().is_empty() => {
            let block = serde_json::json!({"type": "text", "text": text});
            push_system_block_deduping_billing(blocks, block);
        }
        Value::Array(existing) => {
            for block in existing {
                push_system_block_deduping_billing(blocks, block);
            }
        }
        Value::Object(object) if !object.is_empty() => {
            push_system_block_deduping_billing(blocks, Value::Object(object));
        }
        _ => {}
    }
}

fn push_system_block_deduping_billing(blocks: &mut Vec<Value>, block: Value) {
    if is_billing_block(&block) && blocks.iter().any(is_billing_block) {
        return;
    }
    blocks.push(block);
}

fn is_billing_block(block: &Value) -> bool {
    block
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.starts_with(BILLING_PREFIX))
}

fn downgrade_thinking_blocks_for_retry(body: &mut Value) -> bool {
    let mut modified = false;
    if body
        .as_object_mut()
        .and_then(|object| object.remove("thinking"))
        .is_some()
    {
        modified = true;
    }
    modified
        | rewrite_message_content_blocks(body, |block| match block_type(block) {
            Some("thinking") => {
                let text = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .unwrap_or("(thinking omitted)");
                Some(Some(text_block(text)))
            }
            Some("redacted_thinking") => Some(None),
            _ => None,
        })
}

fn downgrade_signature_sensitive_blocks_for_retry(body: &mut Value) -> bool {
    rewrite_message_content_blocks(body, |block| match block_type(block) {
        Some("tool_use") => Some(Some(text_block(&tool_use_retry_text(block)))),
        Some("tool_result") => Some(Some(text_block(&tool_result_retry_text(block)))),
        _ => {
            if block.get("signature").is_some() {
                let mut next = block.clone();
                if let Some(object) = next.as_object_mut() {
                    object.remove("signature");
                }
                Some(Some(next))
            } else {
                None
            }
        }
    })
}

pub(crate) fn filter_web_search_history_blocks(body: &mut Value) -> bool {
    rewrite_message_content_blocks(body, |block| match block_type(block) {
        Some("server_tool_use") if is_web_search_server_tool_use(block) => Some(None),
        Some("web_search_tool_result") => Some(None),
        _ => None,
    })
}

pub(crate) fn body_contains_web_search_history_blocks(body: &[u8]) -> bool {
    body.windows(b"\"server_tool_use\"".len())
        .any(|window| window == b"\"server_tool_use\"")
        || body
            .windows(b"\"web_search_tool_result\"".len())
            .any(|window| window == b"\"web_search_tool_result\"")
}

fn rewrite_message_content_blocks(
    body: &mut Value,
    mut rewrite: impl FnMut(&Value) -> Option<Option<Value>>,
) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut modified = false;
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut next_content = Vec::with_capacity(content.len());
        let mut changed = false;
        for block in content.iter() {
            match rewrite(block) {
                Some(Some(next)) => {
                    next_content.push(next);
                    changed = true;
                }
                Some(None) => {
                    changed = true;
                }
                None => next_content.push(block.clone()),
            }
        }
        if changed {
            if next_content.is_empty() {
                let placeholder = if role == "assistant" {
                    "(assistant content removed)"
                } else {
                    "(content removed)"
                };
                next_content.push(text_block(placeholder));
            }
            *content = next_content;
            modified = true;
        }
    }
    modified
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

fn text_block(text: &str) -> Value {
    serde_json::json!({"type": "text", "text": text})
}

fn tool_use_retry_text(block: &Value) -> String {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    if id.is_empty() {
        format!("(tool_use) name={name} input={input}")
    } else {
        format!("(tool_use) id={id} name={name} input={input}")
    }
}

fn tool_result_retry_text(block: &Value) -> String {
    let tool_use_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = block.get("content").cloned().unwrap_or(Value::Null);
    if tool_use_id.is_empty() {
        format!("(tool_result) content={content}")
    } else {
        format!("(tool_result) tool_use_id={tool_use_id} content={content}")
    }
}

fn is_web_search_server_tool_use(block: &Value) -> bool {
    block
        .get("name")
        .or_else(|| block.get("tool_name"))
        .and_then(Value::as_str)
        .is_some_and(|name| name.contains("web_search"))
        || block
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("srvtoolu_ws_"))
}

fn system_matches_claude_code_template(body: &Value) -> bool {
    let Some(text) = first_system_text(body) else {
        return false;
    };
    if text.starts_with(BILLING_PREFIX) || text.contains(CLAUDE_CODE_IDENTITY_TEXT) {
        return true;
    }
    dice_coefficient(&text, CLAUDE_CODE_IDENTITY_TEXT) >= CLAUDE_CODE_PROMPT_MATCH_THRESHOLD
}

fn first_system_text(body: &Value) -> Option<String> {
    match body.get("system")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => blocks
            .first()
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn dice_coefficient(left: &str, right: &str) -> f64 {
    let left = normalize_prompt_text(left);
    let right = normalize_prompt_text(right);
    if left == right {
        return 1.0;
    }
    let left_bigrams = bigram_counts(&left);
    let right_bigrams = bigram_counts(&right);
    if left_bigrams.is_empty() || right_bigrams.is_empty() {
        return 0.0;
    }
    let intersection = left_bigrams
        .iter()
        .map(|(bigram, left_count)| {
            right_bigrams
                .get(bigram)
                .map(|right_count| (*left_count).min(*right_count))
                .unwrap_or(0)
        })
        .sum::<usize>();
    (2.0 * intersection as f64) / ((left.len() - 1 + right.len() - 1) as f64)
}

fn normalize_prompt_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn bigram_counts(text: &str) -> HashMap<(char, char), usize> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut counts = HashMap::new();
    for pair in chars.windows(2) {
        *counts.entry((pair[0], pair[1])).or_insert(0) += 1;
    }
    counts
}

fn build_anthropic_beta_value(
    headers: &HeaderMap,
    body: Option<&Value>,
    is_claude_oauth: bool,
) -> String {
    let mut betas = vec![CLAUDE_CODE_BETA.to_string()];
    if is_claude_oauth {
        betas.push(CLAUDE_OAUTH_BETA.to_string());
    }

    if let Some(beta) = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
    {
        for item in beta
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if !betas.iter().any(|existing| existing == item) {
                betas.push(item.to_string());
            }
        }
    }

    if is_claude_oauth {
        if body.is_some_and(body_has_thinking) {
            push_beta(&mut betas, INTERLEAVED_THINKING_BETA);
        }
        if body.is_some_and(body_has_streaming_tools) {
            push_beta(&mut betas, FINE_GRAINED_TOOL_STREAMING_BETA);
        }
        if body.is_some_and(body_has_computer_use_tool) {
            push_beta(&mut betas, COMPUTER_USE_BETA);
        }
    }

    betas.join(",")
}

fn push_beta(betas: &mut Vec<String>, beta: &str) {
    if !betas.iter().any(|item| item == beta) {
        betas.push(beta.to_string());
    }
}

fn body_has_thinking(body: &Value) -> bool {
    body.get("thinking").is_some_and(|value| !value.is_null())
}

fn body_has_streaming_tools(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
        && body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
}

fn body_has_computer_use_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| tool_type.contains("computer"))
            })
        })
}

fn stainless_timeout_for_body(body: Option<&Value>) -> String {
    if body
        .and_then(|body| body.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "600".to_string()
    } else {
        "60".to_string()
    }
}

fn ensure_claude_metadata_user_id(body: &mut Value, identity_seed: &str, session_id: &str) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let metadata = object
        .entry("metadata")
        .or_insert_with(|| serde_json::json!({}));
    if !metadata.is_object() {
        return;
    }
    let Some(metadata) = metadata.as_object_mut() else {
        return;
    };
    if metadata
        .get("user_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }
    let account_fingerprint = stable_hex(identity_seed, 16);
    metadata.insert(
        "user_id".to_string(),
        Value::String(format!(
            "user_{account_fingerprint}_account__session_{session_id}"
        )),
    );
}

fn claude_session_id_from_headers(headers: &HeaderMap) -> Option<String> {
    ["x-claude-code-session-id", "claude-code-session-id"]
        .into_iter()
        .find_map(|name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

fn claude_session_id_from_body_value(body: &Value) -> Option<String> {
    body.pointer("/metadata/user_id")
        .and_then(Value::as_str)
        .and_then(parse_session_from_user_id)
        .or_else(|| {
            ["/metadata/session_id", "/metadata/sessionId"]
                .into_iter()
                .find_map(|pointer| {
                    body.pointer(pointer)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
        })
}

fn parse_session_from_user_id(user_id: &str) -> Option<String> {
    let session_id = user_id.split_once("_session_")?.1.trim();
    (!session_id.is_empty()).then(|| session_id.to_string())
}

fn synth_session_id(identity_seed: &str, body: &Value) -> String {
    if let Some(first_user_text) = first_user_text_for_session_seed(body) {
        return stable_uuid(&format!("{identity_seed}:first_user:{first_user_text}"));
    }
    let day_bucket = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    stable_uuid(&format!("{identity_seed}:{day_bucket}"))
}

fn first_user_text_for_session_seed(body: &Value) -> Option<String> {
    let messages = body.get("messages").and_then(Value::as_array)?;
    messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .or_else(|| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(content_text_for_seed)
}

fn content_text_for_seed(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .or_else(|| block.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(256).collect())
    }
}

fn stable_uuid(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
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

fn stable_hex(seed: &str, max_chars: usize) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut output = String::with_capacity(max_chars);
    for byte in digest {
        if output.len() >= max_chars {
            break;
        }
        output.push_str(&format!("{byte:02x}"));
    }
    output.truncate(max_chars);
    output
}

fn cch_signature_present(text: &str) -> bool {
    find_cch_range(text).is_some()
}

fn replace_cch_value(text: &str, replacement: &str) -> String {
    let Some((start, end)) = find_cch_range(text) else {
        return text.to_string();
    };
    let mut output = String::with_capacity(text.len() - (end - start) + replacement.len());
    output.push_str(&text[..start]);
    output.push_str("cch=");
    output.push_str(replacement);
    output.push(';');
    output.push_str(&text[end..]);
    output
}

fn find_cch_range(text: &str) -> Option<(usize, usize)> {
    static NEEDLE: OnceLock<&'static str> = OnceLock::new();
    let needle = NEEDLE.get_or_init(|| "cch=");
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(needle) {
        let start = search_from + rel;
        if start > 0 {
            let prev = bytes[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                search_from = start + 1;
                continue;
            }
        }
        let hex_start = start + needle.len();
        if hex_start + 6 > bytes.len() {
            return None;
        }
        let hex_part = &text[hex_start..hex_start + 5];
        if hex_part.chars().all(|c| c.is_ascii_hexdigit()) && bytes[hex_start + 5] == b';' {
            return Some((start, hex_start + 6));
        }
        search_from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_beta_query_appends_or_merges() {
        assert_eq!(
            ensure_claude_oauth_beta_query("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
        assert_eq!(
            ensure_claude_oauth_beta_query(
                "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
            ),
            "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
        );
        assert_eq!(
            ensure_claude_oauth_beta_query("https://api.anthropic.com/v1/messages?foo=bar"),
            "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
        );
    }

    #[test]
    fn inject_billing_header_when_no_system() {
        let body = json!({"model": "claude-opus-4-7", "max_tokens": 16, "messages": []});
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 2);
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(BILLING_PREFIX));
        assert_eq!(
            system[1]["text"].as_str().unwrap_or(""),
            CLAUDE_CODE_IDENTITY_TEXT
        );
        assert_eq!(result["tools"], json!([]));
    }

    #[test]
    fn non_claude_code_string_system_moves_to_first_user_message() {
        let body = json!({"model": "x", "max_tokens": 1, "system": "Be helpful.", "messages": []});
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 2);
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(BILLING_PREFIX));
        assert_eq!(
            system[1]["text"].as_str().unwrap_or(""),
            CLAUDE_CODE_IDENTITY_TEXT
        );
        assert_eq!(
            result["messages"][0]["content"].as_str().unwrap_or(""),
            "Be helpful."
        );
    }

    #[test]
    fn existing_billing_header_is_re_signed_without_adding_blocks() {
        let original_text =
            "x-anthropic-billing-header: cc_version=2.1; cch=abcde;\n\nYou are Claude Code.";
        let body = json!({
            "model": "x",
            "max_tokens": 1,
            "system": [{"type": "text", "text": original_text}],
            "messages": []
        });
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 1);
        let text = system[0]["text"].as_str().unwrap_or("");
        assert!(text.starts_with("x-anthropic-billing-header: cc_version=2.1; cch="));
        assert!(!text.contains("cch=abcde;"));
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_includes_oauth_marker() {
        let headers = HeaderMap::new();
        let beta = build_anthropic_beta_value(&headers, None, true);
        assert_eq!(beta, "claude-code-20250219,oauth-2025-04-20");
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_merges_existing_markers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("custom-beta,claude-code-20250219"),
        );
        let body = json!({"thinking": {"type": "enabled"}});
        let beta = build_anthropic_beta_value(&headers, Some(&body), true);
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,custom-beta,interleaved-thinking-2025-05-14"
        );
    }

    #[test]
    fn claude_code_like_system_keeps_system_and_adds_billing() {
        let body = json!({
            "model": "x",
            "max_tokens": 1,
            "system": CLAUDE_CODE_IDENTITY_TEXT,
            "messages": [{"role": "user", "content": "hi"}]
        });
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");

        assert_eq!(system.len(), 2);
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(BILLING_PREFIX));
        assert_eq!(
            system[1]["text"].as_str().unwrap_or(""),
            CLAUDE_CODE_IDENTITY_TEXT
        );
        assert_eq!(result["messages"][0]["content"], json!("hi"));
    }

    #[test]
    fn claude_code_like_system_dedupes_existing_billing_block() {
        let existing_billing =
            "x-anthropic-billing-header: cc_version=2.1.100.47e; cc_entrypoint=cli; cch=00000;";
        let body = json!({
            "model": "x",
            "max_tokens": 1,
            "system": [
                {"type": "text", "text": CLAUDE_CODE_IDENTITY_TEXT},
                {"type": "text", "text": existing_billing},
                {"type": "text", "text": "Use concise answers."}
            ],
            "messages": [{"role": "user", "content": "hi"}]
        });
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        let billing_count = system
            .iter()
            .filter(|block| is_billing_block(block))
            .count();

        assert_eq!(billing_count, 1);
        assert_eq!(
            system.last().unwrap()["text"],
            json!("Use concise answers.")
        );
    }

    #[test]
    fn apply_forward_contract_injects_cli_headers_session_and_user_id() {
        let headers = HeaderMap::new();
        let mut url = "https://api.anthropic.com/v1/messages".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#,
        );

        let contract =
            apply_forward_contract(&mut url, &mut body, &headers, "account-123").unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let session_id = contract.session_id.as_deref().unwrap();

        assert!(url.ends_with("?beta=true"));
        assert!(contract
            .headers
            .iter()
            .any(|(name, value)| *name == "user-agent" && value.starts_with("claude-cli/")));
        assert!(contract
            .headers
            .iter()
            .any(|(name, value)| *name == "x-claude-code-session-id" && value == session_id));
        assert!(value
            .pointer("/metadata/user_id")
            .and_then(Value::as_str)
            .is_some_and(|user_id| user_id.ends_with(&format!("_session_{session_id}"))));
        assert!(value["system"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .starts_with(BILLING_PREFIX));
        assert_eq!(value["tools"], json!([]));
    }

    #[test]
    fn cli_headers_use_stream_sensitive_timeout() {
        let streaming = json!({"stream": true});
        let non_streaming = json!({"stream": false});

        assert!(claude_cli_headers(None, "account-1", Some(&streaming))
            .iter()
            .any(|(name, value)| *name == "x-stainless-timeout" && value == "600"));
        assert!(claude_cli_headers(None, "account-1", Some(&non_streaming))
            .iter()
            .any(|(name, value)| *name == "x-stainless-timeout" && value == "60"));
        assert!(claude_cli_headers(None, "account-1", None)
            .iter()
            .any(|(name, value)| *name == "x-stainless-timeout" && value == "60"));
    }

    #[test]
    fn apply_forward_contract_preserves_input_field_order() {
        let headers = HeaderMap::new();
        let mut url = "https://api.anthropic.com/v1/messages".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-4-6","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"stream":false}"#,
        );

        apply_forward_contract(&mut url, &mut body, &headers, "account-123").unwrap();
        let text = std::str::from_utf8(&body).unwrap();

        assert!(text.find("\"model\"").unwrap() < text.find("\"max_tokens\"").unwrap());
        assert!(text.find("\"max_tokens\"").unwrap() < text.find("\"messages\"").unwrap());
        assert!(text.find("\"messages\"").unwrap() < text.find("\"stream\"").unwrap());
    }

    #[test]
    fn synth_session_id_uses_first_user_text_when_available() {
        let first = json!({
            "messages": [{"role": "user", "content": "same conversation"}]
        });
        let second = json!({
            "messages": [{"role": "user", "content": [{"type": "text", "text": "same conversation"}]}]
        });
        let different = json!({
            "messages": [{"role": "user", "content": "different conversation"}]
        });

        assert_eq!(
            synth_session_id("account-1", &first),
            synth_session_id("account-1", &second)
        );
        assert_ne!(
            synth_session_id("account-1", &first),
            synth_session_id("account-1", &different)
        );
    }

    #[test]
    fn retry_stage_thinking_downgrades_thinking_blocks_to_text() {
        let body = json!({
            "thinking": {"type": "enabled"},
            "system": [{"type": "text", "text": claude_billing_header_text()}],
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "keep this", "signature": "bad"},
                    {"type": "redacted_thinking", "data": "secret"},
                    {"type": "text", "text": "visible"}
                ]
            }]
        });

        let result = apply_body_retry_stage(body, ClaudeBodyRetryStage::Thinking);
        let content = result["messages"][0]["content"].as_array().unwrap();
        assert!(result.get("thinking").is_none());
        assert_eq!(content[0]["type"], json!("text"));
        assert_eq!(content[0]["text"], json!("keep this"));
        assert_eq!(content[1]["text"], json!("visible"));
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn retry_stage_signature_sensitive_downgrades_tool_blocks() {
        let body = json!({
            "system": [{"type": "text", "text": claude_billing_header_text()}],
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "x"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}
            ]
        });

        let result = apply_body_retry_stage(body, ClaudeBodyRetryStage::SignatureSensitive);
        assert_eq!(result["messages"][0]["content"][0]["type"], json!("text"));
        assert!(result["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("(tool_use)"));
        assert_eq!(result["messages"][1]["content"][0]["type"], json!("text"));
    }

    #[test]
    fn retry_stage_web_search_removes_history_blocks() {
        let body = json!({
            "system": [{"type": "text", "text": claude_billing_header_text()}],
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "server_tool_use", "id": "srvtoolu_ws_1", "name": "web_search", "input": {"query": "q"}},
                    {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_ws_1", "content": []},
                    {"type": "text", "text": "summary"}
                ]
            }]
        });

        let result = apply_body_retry_stage(body, ClaudeBodyRetryStage::WebSearchHistory);
        let content = result["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], json!("summary"));
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_is_request_shape_driven() {
        let headers = HeaderMap::new();
        let body = json!({
            "stream": true,
            "thinking": {"type": "enabled"},
            "tools": [
                {"name": "computer", "type": "computer_use_20250124"}
            ]
        });
        let beta = build_anthropic_beta_value(&headers, Some(&body), true);

        assert!(beta.contains(INTERLEAVED_THINKING_BETA));
        assert!(beta.contains(FINE_GRAINED_TOOL_STREAMING_BETA));
        assert!(beta.contains(COMPUTER_USE_BETA));
    }

    #[test]
    fn sign_claude_oauth_messages_body_recomputes_cch() {
        let body = json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.195.47e; cc_entrypoint=cli; cch=00000;"
            }],
            "messages": [{"role": "user", "content": "hi"}]
        });
        let signed = sign_claude_oauth_messages_body(body);
        let text = signed["system"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("cch="));
        assert!(!text.contains("cch=00000;"));
    }
}
