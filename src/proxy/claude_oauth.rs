use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;
use sha2::{Digest, Sha256};
use twox_hash::XxHash64;

#[cfg(test)]
use crate::domain::claude_cli::claude_billing_header_text;
use crate::domain::claude_cli::{
    claude_billing_header_text_for_prompt, claude_cch_seed, claude_cli_user_agent,
    claude_stainless_arch, claude_stainless_os, claude_stainless_package_version,
    claude_stainless_runtime, claude_stainless_runtime_version, CLAUDE_CODE_IDENTITY_TEXT,
};

use super::anthropic_cache_control::{
    normalize_anthropic_cache_control, reconcile_forced_tool_choice,
};
use super::anthropic_dateline::{dateline_normalization_enabled, normalize_anthropic_dateline};
use super::claude_client_detection::{detect_claude_client, ClaudeClientClass};
use super::ProxyError;

const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
const CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";
const EFFORT_BETA: &str = "effort-2025-11-24";
const EXTENDED_CACHE_TTL_BETA: &str = "extended-cache-ttl-2025-04-11";
const TOKEN_COUNTING_BETA: &str = "token-counting-2024-11-01";
const REDACT_THINKING_BETA: &str = "redact-thinking-2026-02-12";
const THINKING_DISPLAY_UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";
const THINKING_TOKEN_COUNT_BETA: &str = "thinking-token-count-2026-05-13";
const PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";
const MID_CONVERSATION_SYSTEM_BETA: &str = "mid-conversation-system-2026-04-07";
const ADVISOR_TOOL_BETA: &str = "advisor-tool-2026-03-01";
const ADVANCED_TOOL_USE_BETA: &str = "advanced-tool-use-2025-11-20";
const SERVER_SIDE_FALLBACK_ARRAY_BETA: &str = "server-side-fallback-2026-06-01";
const SERVER_SIDE_FALLBACK_DEFAULT_BETA: &str = "server-side-fallback-2026-07-01";
const FALLBACK_CREDIT_BETA: &str = "fallback-credit-2026-06-01";
const STRUCTURED_OUTPUTS_BETA: &str = "structured-outputs-2025-12-15";
const FAST_MODE_BETA: &str = "fast-mode-2026-02-01";
const CACHE_DIAGNOSIS_BETA: &str = "cache-diagnosis-2026-04-07";
const MESSAGES_CLIENT_BETAS: &[&str] = &[
    "prompt-caching-2024-07-31",
    "token-efficient-tools-2025-02-19",
];
const COUNT_TOKENS_CLIENT_BETAS: &[&str] = MESSAGES_CLIENT_BETAS;
const BILLING_PREFIX: &str = "x-anthropic-billing-header:";
const CLAUDE_CACHE_TTL_ENV: &str = "CC_SWITCH_CLAUDE_CACHE_TTL";
const CLAUDE_CCH_POLICY_ENV: &str = "CC_SWITCH_CLAUDE_CCH_POLICY";
const CLAUDE_NATIVE_PASSTHROUGH_ENV: &str = "CC_SWITCH_CLAUDE_NATIVE_PASSTHROUGH";
const CLAUDE_CACHE_REWRITE_ENV: &str = "CC_SWITCH_CLAUDE_CACHE_REWRITE";
const CLAUDE_CUSTOM_TOOL_ALIAS_ENV: &str = "CC_SWITCH_CLAUDE_CUSTOM_TOOL_ALIAS";
const CLAUDE_BETA_PROFILE_ENV: &str = "CC_SWITCH_CLAUDE_BETA_PROFILE";
const CLAUDE_CODE_PROMPT_MATCH_THRESHOLD: f64 = 0.5;
const CLAUDE_CODE_TOOL_NAMES: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

pub(crate) struct ClaudeForwardContract {
    pub headers: Vec<(&'static str, String)>,
    pub session_id: Option<String>,
    pub tool_name_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeBodyRetryStage {
    Thinking,
    SignatureSensitive,
    WebSearchHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeBetaOperation {
    Messages,
    CountTokens,
}

impl ClaudeBetaOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::CountTokens => "count_tokens",
        }
    }

    fn client_betas(self) -> &'static [&'static str] {
        match self {
            Self::Messages => MESSAGES_CLIENT_BETAS,
            Self::CountTokens => COUNT_TOKENS_CLIENT_BETAS,
        }
    }
}

impl ClaudeBodyRetryStage {
    pub(crate) fn as_header_value(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::SignatureSensitive => "signature_sensitive",
            Self::WebSearchHistory => "web_search_history",
        }
    }
}

pub(crate) fn apply_forward_contract(
    url: &mut String,
    body: &mut Bytes,
    client_headers: &HeaderMap,
    identity_seed: &str,
    context_1m_requested: bool,
    retry_stage: Option<ClaudeBodyRetryStage>,
) -> Result<ClaudeForwardContract, ProxyError> {
    apply_forward_contract_inner(
        url,
        body,
        client_headers,
        identity_seed,
        context_1m_requested,
        retry_stage,
        false,
    )
}

pub(crate) fn apply_count_tokens_forward_contract(
    url: &mut String,
    body: &mut Bytes,
    client_headers: &HeaderMap,
    identity_seed: &str,
    context_1m_requested: bool,
) -> Result<ClaudeForwardContract, ProxyError> {
    apply_forward_contract_inner(
        url,
        body,
        client_headers,
        identity_seed,
        context_1m_requested,
        None,
        true,
    )
}

pub(crate) fn normalize_count_tokens_body(body: &mut Bytes) -> Result<(), ProxyError> {
    if body.is_empty() {
        return Ok(());
    }
    let mut value = serde_json::from_slice(body).map_err(|error| {
        ProxyError::bad_request(format!(
            "Claude count_tokens request body must be valid json: {error}"
        ))
    })?;
    let _ = take_internal_anthropic_betas(&mut value);
    remove_generation_fields_for_count_tokens(&mut value);
    *body = Bytes::from(serde_json::to_vec(&value).map_err(|error| {
        ProxyError::bad_request(format!(
            "Claude count_tokens request body encode failed: {error}"
        ))
    })?);
    Ok(())
}

fn apply_forward_contract_inner(
    url: &mut String,
    body: &mut Bytes,
    client_headers: &HeaderMap,
    identity_seed: &str,
    context_1m_requested: bool,
    retry_stage: Option<ClaudeBodyRetryStage>,
    is_count_tokens: bool,
) -> Result<ClaudeForwardContract, ProxyError> {
    let beta_operation = if is_count_tokens {
        ClaudeBetaOperation::CountTokens
    } else {
        ClaudeBetaOperation::Messages
    };
    *url = ensure_claude_oauth_beta_query(url);
    let mut session_id = claude_session_id_from_headers(client_headers);
    let mut body_shape = None;
    let mut internal_betas = Vec::new();
    let mut tool_name_map = BTreeMap::new();
    let mut client_class = ClaudeClientClass::ThirdPartyAnthropic;
    if !body.is_empty() {
        let mut value = serde_json::from_slice(body).map_err(|error| {
            ProxyError::bad_request(format!(
                "claude oauth request body must be valid json: {error}"
            ))
        })?;
        let billing_header = claude_billing_header_text_for_prompt(
            first_user_text_for_billing(&value).unwrap_or_default(),
        );
        client_class = detect_claude_client(client_headers, &value, is_count_tokens);
        if !feature_enabled(CLAUDE_NATIVE_PASSTHROUGH_ENV, true) {
            client_class = ClaudeClientClass::ThirdPartyAnthropic;
        }
        session_id = session_id.or_else(|| claude_session_id_from_body_value(&value));
        let client_session_id = session_id.clone();
        if !client_class.is_confirmed_native() {
            session_id = session_id.or_else(|| Some(synth_session_id(identity_seed, &value)));
            if let Some(session_id) = session_id.as_deref() {
                ensure_claude_metadata_user_id(&mut value, identity_seed, session_id);
            }
            value = normalize_claude_code_identity(value, &billing_header);
        }
        let tool_alias_seed = (!client_class.is_confirmed_native() && custom_tool_alias_enabled())
            .then_some(client_session_id.as_deref())
            .flatten()
            .map(|session_id| format!("{identity_seed}\0{session_id}"));
        tool_name_map = normalize_claude_oauth_tool_names(&mut value, tool_alias_seed.as_deref())?;
        if tool_name_map
            .iter()
            .any(|(wire, original)| wire != &original.to_ascii_lowercase())
        {
            crate::metrics::record_claude_optional_rewrite("tool_alias");
        }
        if let Some(stage) = retry_stage {
            apply_body_retry_stage_unsigned(&mut value, stage);
        }
        if !client_class.is_confirmed_native() && dateline_normalization_enabled() {
            let hit_count = normalize_anthropic_dateline(&mut value).min(32);
            if hit_count > 0 {
                crate::metrics::record_claude_optional_rewrite("dateline");
                tracing::debug!(hit_count, "normalized bounded Claude dateline fingerprints");
            }
        }
        reconcile_forced_tool_choice(&mut value);
        if !client_class.is_confirmed_native() {
            normalize_claude_sampling(&mut value);
        }
        internal_betas = take_internal_anthropic_betas(&mut value);
        sanitize_claude_fallback_fields(
            &mut value,
            client_headers,
            &internal_betas,
            beta_operation,
            client_class,
        );
        if is_count_tokens {
            remove_generation_fields_for_count_tokens(&mut value);
        }
        if feature_enabled(CLAUDE_CACHE_REWRITE_ENV, true) {
            normalize_anthropic_cache_control(
                &mut value,
                !client_class.is_confirmed_native() && !is_count_tokens,
            );
        }
        value = finalize_claude_cch(value);
        body_shape = Some(value.clone());
        *body = Bytes::from(serde_json::to_vec(&value).map_err(|error| {
            ProxyError::bad_request(format!("claude oauth request body encode failed: {error}"))
        })?);
    }
    let mut headers = claude_forward_headers(
        client_class,
        client_headers,
        session_id.as_deref(),
        identity_seed,
        body_shape.as_ref(),
    );
    headers.push((
        "anthropic-beta",
        build_anthropic_beta_value_for_class(
            client_headers,
            body_shape.as_ref(),
            &internal_betas,
            context_1m_requested,
            true,
            beta_operation,
            client_class,
        ),
    ));
    tracing::debug!(
        claude_client_class = client_class.as_str(),
        operation = beta_operation.as_str(),
        "applied Claude OAuth wire contract"
    );
    crate::metrics::record_claude_client_class(client_class.as_str(), beta_operation.as_str());
    Ok(ClaudeForwardContract {
        headers,
        session_id,
        tool_name_map,
    })
}

fn normalize_claude_oauth_tool_names(
    body: &mut Value,
    custom_alias_seed: Option<&str>,
) -> Result<BTreeMap<String, String>, ProxyError> {
    let mut aliases = BTreeMap::new();
    let mut request_names = BTreeMap::new();
    let mut used_wire_names = std::collections::BTreeSet::new();
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        if tools.len() > 128 {
            return Err(ProxyError::bad_request(
                "Claude custom tool alias map exceeds 128 declarations",
            ));
        }
        for tool in tools {
            let Some(original) = tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            let lookup = original.to_ascii_lowercase();
            if aliases
                .get(&lookup)
                .is_some_and(|existing| existing != &original)
            {
                return Err(ProxyError::bad_request(
                    "Claude tool names collide case-insensitively",
                ));
            }
            let wire_name = CLAUDE_CODE_TOOL_NAMES
                .iter()
                .copied()
                .find(|canonical| canonical.eq_ignore_ascii_case(&original))
                .map(str::to_string)
                .or_else(|| {
                    custom_alias_seed
                        .filter(|_| !reserved_server_tool(tool))
                        .map(|seed| custom_tool_alias(seed, &lookup, &used_wire_names))
                })
                .unwrap_or_else(|| original.clone());
            let wire_lookup = wire_name.to_ascii_lowercase();
            if !used_wire_names.insert(wire_lookup.clone()) {
                return Err(ProxyError::bad_request(
                    "Claude tool names map to the same wire alias",
                ));
            }
            aliases.insert(wire_lookup, original.clone());
            request_names.insert(lookup, wire_name.clone());
            tool["name"] = Value::String(wire_name);
        }
    }
    if let Some(tool_choice) = body.get_mut("tool_choice") {
        rewrite_claude_tool_name_field(tool_choice, "name", &request_names);
    }
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use" | "server_tool_use") => {
                        rewrite_claude_tool_name_field(block, "name", &request_names);
                    }
                    Some("tool_reference") => {
                        rewrite_claude_tool_name_field(block, "tool_name", &request_names);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(aliases)
}

fn custom_tool_alias_enabled() -> bool {
    feature_enabled(CLAUDE_CUSTOM_TOOL_ALIAS_ENV, false)
}

fn feature_enabled(name: &str, default: bool) -> bool {
    let Ok(value) = std::env::var(name) else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => true,
        "0" | "false" | "no" | "off" | "disabled" => false,
        _ => default,
    }
}

fn reserved_server_tool(tool: &Value) -> bool {
    let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
    !tool_type.is_empty()
        && tool_type != "custom"
        && !tool
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("mcp__"))
}

fn custom_tool_alias(
    seed: &str,
    original_lookup: &str,
    used: &std::collections::BTreeSet<String>,
) -> String {
    for counter in 0_u16..=u16::MAX {
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-server:claude-tool-alias:v1\0");
        digest.update(seed.as_bytes());
        digest.update(b"\0");
        digest.update(original_lookup.as_bytes());
        digest.update(counter.to_le_bytes());
        let alias = format!("cc_tool_{}", hex::encode(&digest.finalize()[..8]));
        if !used.contains(&alias) {
            return alias;
        }
    }
    unreachable!("u16 alias collision space exhausted")
}

fn rewrite_claude_tool_name_field(
    value: &mut Value,
    field: &str,
    request_names: &BTreeMap<String, String>,
) {
    let Some(name) = value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    let Some(wire_name) = request_names.get(&name.to_ascii_lowercase()) else {
        return;
    };
    if let Some(object) = value.as_object_mut() {
        object.insert(field.to_string(), Value::String(wire_name.clone()));
    }
}

pub(crate) fn restore_claude_tool_names_in_response_bytes(
    body: Bytes,
    aliases: &BTreeMap<String, String>,
) -> Bytes {
    if aliases.is_empty() {
        return body;
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    if !restore_claude_tool_names_in_value(&mut value, aliases) {
        return body;
    }
    serde_json::to_vec(&value).map(Bytes::from).unwrap_or(body)
}

fn restore_claude_tool_names_in_value(
    value: &mut Value,
    aliases: &BTreeMap<String, String>,
) -> bool {
    let Value::Object(object) = value else {
        if let Value::Array(items) = value {
            return items.iter_mut().fold(false, |changed, item| {
                restore_claude_tool_names_in_value(item, aliases) | changed
            });
        }
        return false;
    };

    let mut changed = false;
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            matches!(
                kind,
                "tool_use"
                    | "server_tool_use"
                    | "function_call"
                    | "custom_tool_call"
                    | "mcp_tool_call"
            )
        })
    {
        changed |= restore_claude_tool_name_field(object, "name", aliases);
    }
    for nested_key in ["function", "functionCall", "function_call"] {
        if let Some(nested) = object.get_mut(nested_key).and_then(Value::as_object_mut) {
            changed |= restore_claude_tool_name_field(nested, "name", aliases);
        }
    }
    for child in object.values_mut() {
        changed |= restore_claude_tool_names_in_value(child, aliases);
    }
    changed
}

fn restore_claude_tool_name_field(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    aliases: &BTreeMap<String, String>,
) -> bool {
    let Some(current) = object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return false;
    };
    let Some(original) = aliases.get(&current.to_ascii_lowercase()) else {
        return false;
    };
    if current == original {
        return false;
    }
    object.insert(field.to_string(), Value::String(original.clone()));
    true
}

#[derive(Debug, Default)]
pub(crate) struct ClaudeToolNameStreamPatcher {
    aliases: BTreeMap<String, String>,
    buffer: Vec<u8>,
}

impl ClaudeToolNameStreamPatcher {
    pub(crate) fn new(aliases: BTreeMap<String, String>) -> Self {
        Self {
            aliases,
            buffer: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, chunk: Bytes) -> Bytes {
        if self.aliases.is_empty() {
            return chunk;
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain(false)
    }

    pub(crate) fn finish(&mut self) -> Bytes {
        if self.aliases.is_empty() {
            return Bytes::new();
        }
        self.drain(true)
    }

    fn drain(&mut self, finish: bool) -> Bytes {
        let mut output = Vec::new();
        while let Some((event_end, delimiter_len)) = claude_sse_event_boundary(&self.buffer) {
            let event = self.buffer[..event_end].to_vec();
            let delimiter = self.buffer[event_end..event_end + delimiter_len].to_vec();
            self.buffer.drain(..event_end + delimiter_len);
            output.extend_from_slice(&rewrite_claude_sse_event(&event, &self.aliases));
            output.extend_from_slice(&delimiter);
        }
        if finish && !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            output.extend_from_slice(&rewrite_claude_sse_event(&event, &self.aliases));
        }
        Bytes::from(output)
    }
}

fn claude_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
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

fn rewrite_claude_sse_event(event: &[u8], aliases: &BTreeMap<String, String>) -> Vec<u8> {
    let Some((payload_start, payload_end)) = single_sse_data_payload_range(event) else {
        let Ok(mut value) = serde_json::from_slice::<Value>(event) else {
            return event.to_vec();
        };
        if !restore_claude_tool_names_in_value(&mut value, aliases) {
            return event.to_vec();
        }
        return serde_json::to_vec(&value).unwrap_or_else(|_| event.to_vec());
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&event[payload_start..payload_end]) else {
        return event.to_vec();
    };
    if !restore_claude_tool_names_in_value(&mut value, aliases) {
        return event.to_vec();
    }
    let Ok(payload) = serde_json::to_vec(&value) else {
        return event.to_vec();
    };
    let mut output = Vec::with_capacity(event.len() + payload.len());
    output.extend_from_slice(&event[..payload_start]);
    output.extend_from_slice(&payload);
    output.extend_from_slice(&event[payload_end..]);
    output
}

fn single_sse_data_payload_range(event: &[u8]) -> Option<(usize, usize)> {
    let mut cursor = 0;
    let mut found = None;
    while cursor < event.len() {
        let line_end = event[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| cursor + offset)
            .unwrap_or(event.len());
        let content_end = line_end
            .checked_sub(1)
            .filter(|index| event.get(*index) == Some(&b'\r'))
            .unwrap_or(line_end);
        let line = &event[cursor..content_end];
        if let Some(payload) = line.strip_prefix(b"data:") {
            if found.is_some() {
                return None;
            }
            let leading_spaces = payload.iter().take_while(|byte| **byte == b' ').count();
            found = Some((cursor + b"data:".len() + leading_spaces, content_end));
        }
        cursor = line_end.saturating_add(1);
    }
    found
}

fn remove_generation_fields_for_count_tokens(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for key in [
        "max_tokens",
        "temperature",
        "top_p",
        "top_k",
        "stream",
        "stop_sequences",
        "service_tier",
        "thinking",
        "output_config",
        "context_management",
        "tool_choice",
        "fallbacks",
        "fallback_credit_token",
    ] {
        object.remove(key);
    }
}

fn claude_cli_headers(
    session_id: Option<&str>,
    identity_seed: &str,
    body: Option<&Value>,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("user-agent", claude_cli_user_agent()),
        ("x-app", "cli".to_string()),
        ("x-stainless-lang", "js".to_string()),
        (
            "x-stainless-package-version",
            claude_stainless_package_version().to_string(),
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

fn claude_forward_headers(
    client_class: ClaudeClientClass,
    client_headers: &HeaderMap,
    session_id: Option<&str>,
    identity_seed: &str,
    body: Option<&Value>,
) -> Vec<(&'static str, String)> {
    let mut headers = claude_cli_headers(session_id, identity_seed, body);
    if !client_class.is_confirmed_native() {
        return headers;
    }

    const PASSTHROUGH: &[&str] = &[
        "user-agent",
        "x-app",
        "x-stainless-lang",
        "x-stainless-package-version",
        "x-stainless-os",
        "x-stainless-arch",
        "x-stainless-runtime",
        "x-stainless-runtime-version",
        "x-stainless-retry-count",
        "x-stainless-timeout",
        "x-stainless-async",
    ];
    for &name in PASSTHROUGH {
        let Some(value) = client_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if let Some((_, existing)) = headers.iter_mut().find(|(header, _)| *header == name) {
            *existing = value.to_string();
        } else {
            headers.push((name, value.to_string()));
        }
    }
    headers
}

fn ensure_claude_oauth_beta_query(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        let (base, query) = split_endpoint_and_query(url);
        return match query {
            Some(query) if !query.is_empty() => format!("{base}?beta=true&{query}"),
            _ => format!("{base}?beta=true"),
        };
    };
    let retained = parsed
        .query_pairs()
        .filter(|(name, _)| name != "beta")
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    parsed.set_query(None);
    {
        let mut query = parsed.query_pairs_mut();
        query.append_pair("beta", "true");
        for (name, value) in retained {
            query.append_pair(&name, &value);
        }
    }
    parsed.to_string()
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

    let mut normalized_body = body.clone();
    normalize_claude_cch_hash_value(&mut normalized_body);
    let Ok(unsigned_body) = serde_json::to_vec(&normalized_body) else {
        return body;
    };

    let mut hasher = XxHash64::with_seed(claude_cch_seed());
    hasher.write(&unsigned_body);
    let cch = format!("{:05x}", hasher.finish() & 0xFFFFF);
    let signed_text = replace_cch_value(&unsigned_text, &cch);
    body["system"][0]["text"] = Value::String(signed_text);
    body
}

fn finalize_claude_cch(mut body: Value) -> Value {
    remove_billing_cache_control(&mut body);
    if std::env::var(CLAUDE_CCH_POLICY_ENV)
        .ok()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("disabled"))
    {
        if let Some(text) = body.pointer_mut("/system/0/text") {
            if let Some(value) = text.as_str().map(remove_cch_member) {
                *text = Value::String(value);
            }
        }
        return body;
    }
    ensure_cch_placeholder_in_billing(&mut body);
    sign_claude_oauth_messages_body(body)
}

fn remove_billing_cache_control(body: &mut Value) {
    let Some(block) = body.pointer_mut("/system/0") else {
        return;
    };
    let is_billing = block
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.starts_with(BILLING_PREFIX));
    if is_billing {
        if let Some(object) = block.as_object_mut() {
            object.remove("cache_control");
        }
    }
}

fn remove_cch_member(text: &str) -> String {
    let Some(start) = text.find("cch=") else {
        return text.to_string();
    };
    let Some(relative_end) = text[start..].find(';') else {
        return text.to_string();
    };
    let mut remove_start = start;
    while remove_start > 0 && text.as_bytes()[remove_start - 1].is_ascii_whitespace() {
        remove_start -= 1;
    }
    let end = start + relative_end + 1;
    let mut output = String::with_capacity(text.len().saturating_sub(end - remove_start));
    output.push_str(&text[..remove_start]);
    output.push_str(&text[end..]);
    output
}

fn ensure_cch_placeholder_in_billing(body: &mut Value) {
    let Some(text) = body
        .pointer("/system/0/text")
        .and_then(Value::as_str)
        .filter(|text| text.starts_with(BILLING_PREFIX))
    else {
        return;
    };
    if cch_signature_present(text) {
        return;
    }
    let Some(entrypoint_start) = text.find("cc_entrypoint=") else {
        return;
    };
    let Some(relative_end) = text[entrypoint_start..].find(';') else {
        return;
    };
    let insert_at = entrypoint_start + relative_end + 1;
    let mut next = String::with_capacity(text.len() + " cch=00000;".len());
    next.push_str(&text[..insert_at]);
    next.push_str(" cch=00000;");
    next.push_str(&text[insert_at..]);
    body["system"][0]["text"] = Value::String(next);
}

fn normalize_claude_cch_hash_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_claude_cch_hash_value(item);
            }
        }
        Value::Object(object) => {
            for key in crate::domain::claude_cli::CLAUDE_WIRE_PROFILE.cch_excluded_keys {
                object.shift_remove(*key);
            }
            for (key, value) in object.iter_mut() {
                if key == "model" && value.is_string() {
                    *value = Value::String(String::new());
                } else {
                    normalize_claude_cch_hash_value(value);
                }
            }
        }
        _ => {}
    }
}

fn normalize_claude_code_identity(mut body: Value, billing_header: &str) -> Value {
    if replace_leading_billing_block(&mut body, billing_header) {
        ensure_claude_tools_array(&mut body);
        ensure_claude_defaults(&mut body);
        return body;
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
    blocks.push(claude_billing_block(billing_header));
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
    ensure_claude_defaults(&mut body);
    body
}

fn replace_leading_billing_block(body: &mut Value, billing_header: &str) -> bool {
    let Some(block) = body.pointer_mut("/system/0") else {
        return false;
    };
    if !block
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.starts_with(BILLING_PREFIX))
    {
        return false;
    }
    let Some(object) = block.as_object_mut() else {
        return false;
    };
    object.insert(
        "text".to_string(),
        Value::String(billing_header.to_string()),
    );
    object.remove("cache_control");
    true
}

fn ensure_claude_code_identity(body: Value) -> Value {
    let billing_header = claude_billing_header_text_for_prompt(
        first_user_text_for_billing(&body).unwrap_or_default(),
    );
    let mut body = normalize_claude_code_identity(body, &billing_header);
    normalize_claude_sampling(&mut body);
    finalize_claude_cch(body)
}

fn ensure_claude_oauth_billing_header_system(body: Value) -> Value {
    ensure_claude_code_identity(body)
}

fn apply_body_retry_stage(mut body: Value, stage: ClaudeBodyRetryStage) -> Value {
    apply_body_retry_stage_unsigned(&mut body, stage);
    finalize_claude_cch(body)
}

fn apply_body_retry_stage_unsigned(body: &mut Value, stage: ClaudeBodyRetryStage) {
    match stage {
        ClaudeBodyRetryStage::Thinking => {
            downgrade_thinking_blocks_for_retry(body);
        }
        ClaudeBodyRetryStage::SignatureSensitive => {
            downgrade_thinking_blocks_for_retry(body);
            downgrade_signature_sensitive_blocks_for_retry(body);
        }
        ClaudeBodyRetryStage::WebSearchHistory => {
            downgrade_thinking_blocks_for_retry(body);
            downgrade_signature_sensitive_blocks_for_retry(body);
            filter_web_search_history_blocks(body);
        }
    }
}

fn ensure_claude_tools_array(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
}

fn ensure_claude_defaults(body: &mut Value) {
    let thinking_uses_context_management = body
        .pointer("/thinking/type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "enabled" | "adaptive"));
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object
        .entry("max_tokens".to_string())
        .or_insert_with(|| serde_json::json!(128_000));
    if thinking_uses_context_management {
        object
            .entry("context_management".to_string())
            .or_insert_with(|| {
                serde_json::json!({
                    "edits": [{"type": "clear_thinking_20251015", "keep": "all"}]
                })
            });
    }
}

fn normalize_claude_sampling(body: &mut Value) {
    let thinking_active = body
        .pointer("/thinking/type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "enabled" | "adaptive" | "auto"));
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if thinking_active {
        object.insert("temperature".to_string(), serde_json::json!(1));
        object.remove("top_p");
        object.remove("top_k");
    } else {
        object
            .entry("temperature".to_string())
            .or_insert_with(|| serde_json::json!(1));
    }
}

fn claude_cache_control() -> Value {
    claude_cache_control_for_ttl(std::env::var(CLAUDE_CACHE_TTL_ENV).ok().as_deref())
}

fn claude_cache_control_for_ttl(ttl: Option<&str>) -> Value {
    match ttl.map(str::trim) {
        Some("1h") => serde_json::json!({"type": "ephemeral", "ttl": "1h"}),
        _ => serde_json::json!({"type": "ephemeral"}),
    }
}

fn claude_billing_block(billing_header: &str) -> Value {
    serde_json::json!({
        "type": "text",
        "text": billing_header
    })
}

fn claude_identity_block() -> Value {
    serde_json::json!({
        "type": "text",
        "text": CLAUDE_CODE_IDENTITY_TEXT,
        "cache_control": claude_cache_control()
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
    internal_betas: &[String],
    context_1m_requested: bool,
    is_claude_oauth: bool,
    operation: ClaudeBetaOperation,
) -> String {
    build_anthropic_beta_value_for_class(
        headers,
        body,
        internal_betas,
        context_1m_requested,
        is_claude_oauth,
        operation,
        ClaudeClientClass::ThirdPartyAnthropic,
    )
}

fn build_anthropic_beta_value_for_class(
    headers: &HeaderMap,
    body: Option<&Value>,
    internal_betas: &[String],
    context_1m_requested: bool,
    is_claude_oauth: bool,
    operation: ClaudeBetaOperation,
    client_class: ClaudeClientClass,
) -> String {
    if client_class.is_confirmed_native() {
        return native_passthrough_betas(headers, body, internal_betas, is_claude_oauth, operation);
    }

    let requested = requested_anthropic_betas(headers, internal_betas);
    let requested_refs = requested.iter().map(String::as_str).collect::<Vec<_>>();
    let mut betas = vec![CLAUDE_CODE_BETA.to_string()];
    if !is_claude_oauth {
        for beta in requested_refs {
            push_beta(&mut betas, beta);
        }
        return betas.join(",");
    }
    if is_claude_oauth {
        betas.push(CLAUDE_OAUTH_BETA.to_string());
    }
    if is_claude_oauth && !current_beta_profile_enabled() {
        if operation == ClaudeBetaOperation::CountTokens {
            push_beta(&mut betas, TOKEN_COUNTING_BETA);
        }
        return betas.join(",");
    }
    if context_1m_requested {
        push_beta(&mut betas, CONTEXT_1M_BETA);
    }

    match operation {
        ClaudeBetaOperation::Messages => {
            push_beta(&mut betas, INTERLEAVED_THINKING_BETA);
            if body.is_some_and(body_has_thinking_display_updates) {
                push_beta(&mut betas, THINKING_DISPLAY_UPDATES_BETA);
            } else if !body.is_some_and(body_has_thinking_display) {
                push_beta(&mut betas, REDACT_THINKING_BETA);
            }
            push_beta(&mut betas, THINKING_TOKEN_COUNT_BETA);
            push_beta(&mut betas, CONTEXT_MANAGEMENT_BETA);
            push_beta(&mut betas, PROMPT_CACHING_SCOPE_BETA);
            if body.is_some_and(body_model_supports_mid_conversation_system) {
                push_beta(&mut betas, MID_CONVERSATION_SYSTEM_BETA);
            }
            if body.is_some_and(body_has_advisor_tool) {
                push_beta(&mut betas, ADVISOR_TOOL_BETA);
            }
            if body.is_some_and(body_has_advanced_tool_use) {
                push_beta(&mut betas, ADVANCED_TOOL_USE_BETA);
            }
            push_beta(&mut betas, EFFORT_BETA);
            if let Some(fallback_beta) = body.and_then(body_server_side_fallback_beta) {
                if requested.iter().any(|requested| requested == fallback_beta) {
                    push_beta(&mut betas, fallback_beta);
                }
            }
            if is_claude_oauth || body.is_some_and(body_has_fallback_credit) {
                push_beta(&mut betas, FALLBACK_CREDIT_BETA);
            }
            if body.is_some_and(body_has_structured_output) {
                push_beta(&mut betas, STRUCTURED_OUTPUTS_BETA);
            }
            if body.is_some_and(body_has_fast_mode) {
                push_beta(&mut betas, FAST_MODE_BETA);
            }
            if is_claude_oauth && current_beta_profile_enabled() {
                push_beta(&mut betas, EXTENDED_CACHE_TTL_BETA);
            }
            if body.is_some_and(body_has_diagnostics) {
                push_beta(&mut betas, CACHE_DIAGNOSIS_BETA);
            }
        }
        ClaudeBetaOperation::CountTokens => {
            push_beta(&mut betas, INTERLEAVED_THINKING_BETA);
            push_beta(&mut betas, CONTEXT_MANAGEMENT_BETA);
            if body.is_some_and(body_has_advisor_tool) {
                push_beta(&mut betas, ADVISOR_TOOL_BETA);
            }
            push_beta(&mut betas, TOKEN_COUNTING_BETA);
        }
    }

    for beta in operation.client_betas() {
        if requested.iter().any(|requested| requested == beta) {
            push_beta(&mut betas, beta);
        }
    }
    let allowed = betas.iter().map(String::as_str).collect::<Vec<_>>();
    let dropped_count = requested
        .iter()
        .filter(|requested| !allowed.contains(&requested.as_str()))
        .count();
    if dropped_count > 0 {
        tracing::debug!(
            dropped_count,
            operation = operation.as_str(),
            "dropping unapproved anthropic-beta values for Claude OAuth"
        );
        for _ in 0..dropped_count {
            crate::metrics::record_claude_beta_decision(operation.as_str(), "dropped_unknown");
        }
    }
    betas.join(",")
}

fn requested_anthropic_betas(headers: &HeaderMap, internal_betas: &[String]) -> Vec<String> {
    headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .chain(internal_betas.iter().cloned())
        .collect()
}

fn current_beta_profile_enabled() -> bool {
    beta_profile_value_is_current(std::env::var(CLAUDE_BETA_PROFILE_ENV).ok().as_deref())
}

fn beta_profile_value_is_current(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "minimal" | "legacy" | "disabled" | "off"
    )
}

fn native_passthrough_betas(
    headers: &HeaderMap,
    body: Option<&Value>,
    internal_betas: &[String],
    is_claude_oauth: bool,
    operation: ClaudeBetaOperation,
) -> String {
    let mut betas = Vec::new();
    for beta in requested_anthropic_betas(headers, internal_betas) {
        let shape_matches = match beta.as_str() {
            SERVER_SIDE_FALLBACK_ARRAY_BETA | SERVER_SIDE_FALLBACK_DEFAULT_BETA => body
                .and_then(body_server_side_fallback_beta)
                .is_some_and(|required| required == beta),
            ADVANCED_TOOL_USE_BETA => body.is_some_and(body_has_advanced_tool_use),
            THINKING_DISPLAY_UPDATES_BETA => body.is_some_and(body_has_thinking_display_updates),
            REDACT_THINKING_BETA => !body.is_some_and(body_has_thinking_display),
            _ => true,
        };
        if native_beta_allowed(&beta, operation) && shape_matches {
            push_beta(&mut betas, &beta);
        } else {
            crate::metrics::record_claude_beta_decision(operation.as_str(), "dropped_unknown");
        }
    }
    if is_claude_oauth && !betas.iter().any(|beta| beta == CLAUDE_OAUTH_BETA) {
        let insert_at = betas
            .iter()
            .position(|beta| beta == CLAUDE_CODE_BETA)
            .map_or(0, |index| index + 1);
        betas.insert(insert_at, CLAUDE_OAUTH_BETA.to_string());
    }
    if is_claude_oauth
        && operation == ClaudeBetaOperation::Messages
        && current_beta_profile_enabled()
    {
        push_beta(&mut betas, EXTENDED_CACHE_TTL_BETA);
    }
    betas.join(",")
}

fn native_beta_allowed(beta: &str, operation: ClaudeBetaOperation) -> bool {
    if matches!(
        beta,
        CLAUDE_CODE_BETA
            | CLAUDE_OAUTH_BETA
            | INTERLEAVED_THINKING_BETA
            | CONTEXT_MANAGEMENT_BETA
            | CONTEXT_1M_BETA
    ) || operation.client_betas().contains(&beta)
    {
        return true;
    }
    match operation {
        ClaudeBetaOperation::Messages => matches!(
            beta,
            EFFORT_BETA
                | EXTENDED_CACHE_TTL_BETA
                | REDACT_THINKING_BETA
                | THINKING_DISPLAY_UPDATES_BETA
                | THINKING_TOKEN_COUNT_BETA
                | PROMPT_CACHING_SCOPE_BETA
                | MID_CONVERSATION_SYSTEM_BETA
                | ADVISOR_TOOL_BETA
                | ADVANCED_TOOL_USE_BETA
                | SERVER_SIDE_FALLBACK_ARRAY_BETA
                | SERVER_SIDE_FALLBACK_DEFAULT_BETA
                | FALLBACK_CREDIT_BETA
                | STRUCTURED_OUTPUTS_BETA
                | FAST_MODE_BETA
                | CACHE_DIAGNOSIS_BETA
        ),
        ClaudeBetaOperation::CountTokens => {
            matches!(beta, TOKEN_COUNTING_BETA | ADVISOR_TOOL_BETA)
        }
    }
}

fn take_internal_anthropic_betas(body: &mut Value) -> Vec<String> {
    let Some(object) = body.as_object_mut() else {
        return Vec::new();
    };
    let mut betas = Vec::new();
    for key in ["anthropic_beta", "betas"] {
        let Some(value) = object.remove(key) else {
            continue;
        };
        match value {
            Value::Array(items) => {
                betas.extend(items.into_iter().filter_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string)
                }));
            }
            Value::String(value) => {
                betas.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string),
                );
            }
            _ => {}
        }
    }
    betas
}

fn push_beta(betas: &mut Vec<String>, beta: &str) {
    if !betas.iter().any(|item| item == beta) {
        betas.push(beta.to_string());
    }
}

fn body_has_thinking(body: &Value) -> bool {
    body.pointer("/thinking/type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "enabled" | "adaptive" | "auto"))
}

fn body_has_effort(body: &Value) -> bool {
    body.pointer("/output_config/effort")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn body_has_extended_cache_ttl(body: &Value) -> bool {
    match body {
        Value::Array(items) => items.iter().any(body_has_extended_cache_ttl),
        Value::Object(object) => {
            object
                .get("cache_control")
                .is_some_and(|cache| cache.get("ttl").and_then(Value::as_str) == Some("1h"))
                || object.values().any(body_has_extended_cache_ttl)
        }
        _ => false,
    }
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

fn body_has_context_management(body: &Value) -> bool {
    body.get("context_management")
        .is_some_and(|value| !value.is_null())
}

fn body_has_thinking_display(body: &Value) -> bool {
    body.pointer("/thinking/display")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn body_has_thinking_display_updates(body: &Value) -> bool {
    body.pointer("/thinking/display").and_then(Value::as_str) == Some("updates")
}

fn body_has_advanced_tool_use(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                matches!(
                    tool.get("type").and_then(Value::as_str),
                    Some("tool_search_tool_regex_20251119" | "tool_search_tool_bm25_20251119")
                ) || tool.get("defer_loading").and_then(Value::as_bool) == Some(true)
                    || tool
                        .pointer("/custom/defer_loading")
                        .and_then(Value::as_bool)
                        == Some(true)
            })
        })
}

fn body_has_advisor_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("type").and_then(Value::as_str) == Some("advisor_20260301"))
        })
}

fn body_model_supports_mid_conversation_system(body: &Value) -> bool {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return false;
    };
    matches!(
        model.trim_end_matches("[1m]"),
        "claude-opus-4-8"
            | "claude-opus-5"
            | "claude-sonnet-5"
            | "claude-fable-5"
            | "claude-fable-5-1"
    )
}

fn body_server_side_fallback_beta(body: &Value) -> Option<&'static str> {
    match body.get("fallbacks") {
        Some(Value::String(value)) if value == "default" => Some(SERVER_SIDE_FALLBACK_DEFAULT_BETA),
        Some(Value::Array(values))
            if !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty())) =>
        {
            Some(SERVER_SIDE_FALLBACK_ARRAY_BETA)
        }
        _ => None,
    }
}

fn sanitize_claude_fallback_fields(
    body: &mut Value,
    headers: &HeaderMap,
    internal_betas: &[String],
    operation: ClaudeBetaOperation,
    client_class: ClaudeClientClass,
) {
    let requested = requested_anthropic_betas(headers, internal_betas);
    let generation_beta_profile_enabled =
        client_class.is_confirmed_native() || current_beta_profile_enabled();
    let keep_fallbacks = operation == ClaudeBetaOperation::Messages
        && generation_beta_profile_enabled
        && body_server_side_fallback_beta(body)
            .is_some_and(|required| requested.iter().any(|requested| requested == required));
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if object.contains_key("fallbacks") && !keep_fallbacks {
        object.remove("fallbacks");
        crate::metrics::record_claude_beta_decision(operation.as_str(), "fallback_stripped");
        tracing::debug!(
            operation = operation.as_str(),
            "stripped Claude fallback field without an exact approved beta/shape pair"
        );
    }
    let valid_fallback_credit = operation == ClaudeBetaOperation::Messages
        && generation_beta_profile_enabled
        && object
            .get("fallback_credit_token")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    if object.contains_key("fallback_credit_token") && !valid_fallback_credit {
        object.remove("fallback_credit_token");
        crate::metrics::record_claude_beta_decision(operation.as_str(), "fallback_credit_stripped");
        tracing::debug!(
            operation = operation.as_str(),
            "stripped invalid Claude fallback credit field"
        );
    }
}

fn body_has_fallback_credit(body: &Value) -> bool {
    body.get("fallback_credit_token")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn body_has_structured_output(body: &Value) -> bool {
    body.get("output_format").is_some_and(Value::is_object)
        || body
            .pointer("/output_config/format")
            .is_some_and(Value::is_object)
}

fn body_has_fast_mode(body: &Value) -> bool {
    body.get("speed")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("fast"))
}

fn body_has_diagnostics(body: &Value) -> bool {
    body.get("diagnostics").is_some_and(Value::is_object)
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

fn first_user_text_for_billing(body: &Value) -> Option<&str> {
    body.get("messages")
        .and_then(Value::as_array)?
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .find_map(|message| {
            let content = message.get("content")?;
            match content {
                Value::String(text) => (!text.is_empty()).then_some(text.as_str()),
                Value::Array(blocks) => blocks.iter().find_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                        .filter(|text| !text.is_empty())
                }),
                _ => None,
            }
        })
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

    const WIRE_PROFILE_JSON: &str =
        include_str!("../../assets/contract/claude-oauth-wire-profile.json");

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
        assert_eq!(
            ensure_claude_oauth_beta_query(
                "https://api.anthropic.com/v1/messages?beta=false&foo=bar&beta=true"
            ),
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
        assert!(system[0].get("cache_control").is_none());
        assert!(system[1].get("cache_control").is_some());
        assert_eq!(result["tools"], json!([]));
    }

    #[test]
    fn claude_defaults_fill_missing_wire_fields() {
        let result = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-sonnet-4-6",
            "messages": [],
            "thinking": {"type": "enabled"}
        }));

        assert_eq!(result["max_tokens"], json!(128_000));
        assert_eq!(result["temperature"], json!(1));
        assert_eq!(
            result["context_management"],
            json!({"edits": [{"type": "clear_thinking_20251015", "keep": "all"}]})
        );
    }

    #[test]
    fn claude_defaults_preserve_explicit_values_except_invalid_thinking_sampling() {
        let result = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 4096,
            "temperature": 0,
            "messages": [],
            "thinking": {"type": "adaptive"},
            "context_management": {"edits": []}
        }));

        assert_eq!(result["max_tokens"], json!(4096));
        assert_eq!(result["temperature"], json!(1));
        assert_eq!(result["context_management"], json!({"edits": []}));
    }

    #[test]
    fn claude_context_management_is_only_added_for_supported_thinking_modes() {
        let result = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-sonnet-4-6",
            "messages": [],
            "thinking": {"type": "disabled"}
        }));

        assert!(result.get("context_management").is_none());
    }

    #[test]
    fn claude_cache_ttl_defaults_to_wire_compatible_five_minutes() {
        assert_eq!(
            claude_cache_control_for_ttl(None),
            json!({"type": "ephemeral"})
        );
        assert_eq!(
            claude_cache_control_for_ttl(Some("5m")),
            json!({"type": "ephemeral"})
        );
        assert_eq!(
            claude_cache_control_for_ttl(Some("invalid")),
            json!({"type": "ephemeral"})
        );
        assert_eq!(
            claude_cache_control_for_ttl(Some("1h")),
            json!({"type": "ephemeral", "ttl": "1h"})
        );
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
    fn billing_fingerprint_uses_original_user_text_before_system_migration() {
        let body = json!({
            "model": "x",
            "max_tokens": 1,
            "system": "This migrated system prompt must not become billing input.",
            "messages": [{"role": "user", "content": "abcdefghijklmnopqrstuvwxyz"}]
        });
        let result = ensure_claude_oauth_billing_header_system(body);
        let billing = result["system"][0]["text"].as_str().unwrap();

        assert!(billing.contains("cc_version=2.1.258.d3d;"));
        assert_eq!(
            result["messages"][0]["content"],
            json!("This migrated system prompt must not become billing input.")
        );
        assert!(result["system"][0].get("cache_control").is_none());
    }

    #[test]
    fn billing_text_extraction_skips_non_text_user_content() {
        let body = json!({
            "messages": [
                {"role": "assistant", "content": "ignore"},
                {"role": "user", "content": [{"type": "tool_result", "content": "ignore"}]},
                {"role": "user", "content": [{"type": "text", "text": "ping"}]}
            ]
        });
        assert_eq!(first_user_text_for_billing(&body), Some("ping"));
    }

    #[test]
    fn haiku_non_cc_client_still_gets_full_mimicry() {
        let result = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-haiku-4-5-20251001",
            "system": "Be concise.",
            "messages": [{"role": "user", "content": "hello"}]
        }));

        let system = result["system"].as_array().unwrap();
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or_default()
            .starts_with(BILLING_PREFIX));
        assert_eq!(system[1]["text"], json!(CLAUDE_CODE_IDENTITY_TEXT));
        assert_eq!(result["messages"][0]["content"], json!("Be concise."));
        assert_eq!(result["tools"], json!([]));
        assert_eq!(result["temperature"], json!(1));
    }

    #[test]
    fn thinking_sampling_is_normalized_without_affecting_non_thinking_requests() {
        let thinking = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-sonnet-4-6",
            "thinking": {"type": "adaptive"},
            "temperature": 0.2,
            "top_p": 0.9,
            "top_k": 40,
            "messages": []
        }));
        assert_eq!(thinking["temperature"], json!(1));
        assert!(thinking.get("top_p").is_none());
        assert!(thinking.get("top_k").is_none());

        let non_thinking = ensure_claude_oauth_billing_header_system(json!({
            "model": "claude-haiku-4-5",
            "temperature": 0.2,
            "top_p": 0.9,
            "messages": []
        }));
        assert_eq!(non_thinking["temperature"], json!(0.2));
        assert_eq!(non_thinking["top_p"], json!(0.9));
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
        assert!(text.starts_with(
            "x-anthropic-billing-header: cc_version=2.1.258.1e2; cc_entrypoint=cli; cch="
        ));
        assert!(!text.contains("cch=abcde;"));
        assert!(system[0].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_includes_oauth_marker() {
        let headers = HeaderMap::new();
        let beta = build_anthropic_beta_value(
            &headers,
            None,
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05,effort-2025-11-24,fallback-credit-2026-06-01,extended-cache-ttl-2025-04-11"
        );
    }

    #[test]
    fn beta_profile_runtime_control_fails_to_current_and_rolls_back_to_minimal() {
        for value in [None, Some("current"), Some("unexpected")] {
            assert!(beta_profile_value_is_current(value), "{value:?}");
        }
        for value in ["minimal", "legacy", "disabled", "off", " MINIMAL "] {
            assert!(!beta_profile_value_is_current(Some(value)), "{value}");
        }
    }

    #[test]
    fn beta_policy_matches_the_wire_profile_asset() {
        fn strings(value: &Value) -> Vec<&str> {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect()
        }

        let profile: Value = serde_json::from_str(WIRE_PROFILE_JSON).unwrap();
        let messages = &profile["betaMatrices"]["messages"];
        let count_tokens = &profile["betaMatrices"]["countTokens"];
        let maximal_messages_body = json!({
            "model": "claude-sonnet-5",
            "stream": true,
            "thinking": {"type": "adaptive", "display": "updates"},
            "tools": [
                {"name": "deferred", "defer_loading": true},
                {"type": "advisor_20260301"}
            ],
            "fallbacks": "default",
            "context_management": {"edits": []},
            "output_config": {"effort": "high", "format": {"type": "json_schema"}},
            "speed": "fast",
            "diagnostics": {},
            "system": [{"cache_control": {"type": "ephemeral", "ttl": "1h"}}]
        });

        let message_betas = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&maximal_messages_body),
            &[SERVER_SIDE_FALLBACK_DEFAULT_BETA.to_string()],
            true,
            true,
            ClaudeBetaOperation::Messages,
        );
        let message_betas = message_betas.split(',').collect::<Vec<_>>();
        for beta in strings(&messages["always"])
            .into_iter()
            .filter(|beta| *beta != REDACT_THINKING_BETA)
            .chain(strings(&messages["shapeGated"]))
        {
            assert!(message_betas.contains(&beta), "missing message beta {beta}");
        }
        assert!(!message_betas.contains(&REDACT_THINKING_BETA));
        assert_eq!(
            strings(&messages["auditedClientCompatibility"]),
            MESSAGES_CLIENT_BETAS
        );
        assert_eq!(
            messages["shapeRules"]["thinkingDisplayUpdates"]["equals"],
            "updates"
        );
        assert_eq!(
            messages["shapeRules"]["thinkingDisplayUpdates"]["excludes"],
            REDACT_THINKING_BETA
        );
        assert_eq!(
            strings(&messages["shapeRules"]["advancedToolUse"]["toolTypes"]),
            [
                "tool_search_tool_regex_20251119",
                "tool_search_tool_bm25_20251119"
            ]
        );
        assert_eq!(
            strings(&messages["shapeRules"]["advancedToolUse"]["booleanPaths"]),
            ["defer_loading", "custom.defer_loading"]
        );
        assert_eq!(
            messages["shapeRules"]["advancedToolUse"]["ordinaryToolsEnable"],
            false
        );
        assert_eq!(
            messages["explicitShapePaired"]["fallbackArray"],
            SERVER_SIDE_FALLBACK_ARRAY_BETA
        );
        assert_eq!(
            messages["explicitShapePaired"]["fallbackDefault"],
            SERVER_SIDE_FALLBACK_DEFAULT_BETA
        );
        assert!(message_betas.contains(&SERVER_SIDE_FALLBACK_DEFAULT_BETA));
        let array_fallback = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&json!({"fallbacks": ["claude-opus-5"]})),
            &[SERVER_SIDE_FALLBACK_ARRAY_BETA.to_string()],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert!(array_fallback
            .split(',')
            .any(|beta| beta == SERVER_SIDE_FALLBACK_ARRAY_BETA));
        let count_token_betas = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&json!({"tools": [{"type": "advisor_20260301"}]})),
            &[],
            true,
            true,
            ClaudeBetaOperation::CountTokens,
        );
        let count_token_betas = count_token_betas.split(',').collect::<Vec<_>>();
        for beta in strings(&count_tokens["always"])
            .into_iter()
            .chain(strings(&count_tokens["shapeGated"]))
        {
            assert!(
                count_token_betas.contains(&beta),
                "missing count_tokens beta {beta}"
            );
        }
        assert_eq!(
            strings(&count_tokens["auditedClientCompatibility"]),
            COUNT_TOKENS_CLIENT_BETAS
        );
        assert_eq!(count_tokens["generationOnlyBetasAllowed"], false);

        let catalog = &profile["modelCatalog"];
        assert_eq!(
            catalog["capabilityPolicy"]["unknownModel"],
            "shape_only_no_model_beta"
        );
        let mut catalog_models = strings(&catalog["models"]);
        catalog_models.sort_unstable();
        let mut capability_models = catalog["capabilitiesByModel"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        capability_models.sort_unstable();
        assert_eq!(catalog_models, capability_models);
        for (model, capabilities) in catalog["capabilitiesByModel"].as_object().unwrap() {
            let body = json!({"model": model});
            assert_eq!(
                body_model_supports_mid_conversation_system(&body),
                capabilities["midConversationSystem"].as_bool().unwrap(),
                "model capability fixture drift for {model}"
            );
        }
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_allows_only_known_safe_client_markers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(
                "custom-beta,prompt-caching-2024-07-31,prompt-caching-scope-2026-01-05,token-efficient-tools-2025-02-19,claude-code-20250219",
            ),
        );
        let body = json!({"thinking": {"type": "enabled"}});
        let beta = build_anthropic_beta_value(
            &headers,
            Some(&body),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05,effort-2025-11-24,fallback-credit-2026-06-01,extended-cache-ttl-2025-04-11,prompt-caching-2024-07-31,token-efficient-tools-2025-02-19"
        );
        assert!(!beta.contains("custom-beta"));
        assert_eq!(beta.matches(PROMPT_CACHING_SCOPE_BETA).count(), 1);
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_merges_repeated_header_fields() {
        let mut headers = HeaderMap::new();
        headers.append(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("prompt-caching-2024-07-31"),
        );
        headers.append(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("unknown-beta,token-efficient-tools-2025-02-19"),
        );

        let beta = build_anthropic_beta_value(
            &headers,
            None,
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(beta.contains("prompt-caching-2024-07-31"));
        assert!(beta.contains("token-efficient-tools-2025-02-19"));
        assert!(!beta.contains("unknown-beta"));
    }

    #[test]
    fn fallback_fields_require_the_exact_explicit_beta_for_their_shape() {
        fn apply(fallbacks: Value, beta: Option<&'static str>) -> (Value, String) {
            let mut headers = HeaderMap::new();
            if let Some(beta) = beta {
                headers.insert("anthropic-beta", axum::http::HeaderValue::from_static(beta));
            }
            let mut url = "https://api.anthropic.com/v1/messages".to_string();
            let mut body = Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-sonnet-5",
                    "fallbacks": fallbacks,
                    "messages": [{"role": "user", "content": "hi"}]
                }))
                .unwrap(),
            );
            let contract =
                apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None)
                    .unwrap();
            let body = serde_json::from_slice(&body).unwrap();
            let beta = contract
                .headers
                .iter()
                .find(|(name, _)| *name == "anthropic-beta")
                .unwrap()
                .1
                .clone();
            (body, beta)
        }

        let (body, beta) = apply(
            json!(["claude-opus-5"]),
            Some(SERVER_SIDE_FALLBACK_ARRAY_BETA),
        );
        assert_eq!(body["fallbacks"], json!(["claude-opus-5"]));
        assert!(beta
            .split(',')
            .any(|value| value == SERVER_SIDE_FALLBACK_ARRAY_BETA));
        assert!(!beta.contains(SERVER_SIDE_FALLBACK_DEFAULT_BETA));

        let (body, beta) = apply(json!("default"), Some(SERVER_SIDE_FALLBACK_DEFAULT_BETA));
        assert_eq!(body["fallbacks"], json!("default"));
        assert!(beta
            .split(',')
            .any(|value| value == SERVER_SIDE_FALLBACK_DEFAULT_BETA));
        assert!(!beta.contains(SERVER_SIDE_FALLBACK_ARRAY_BETA));

        for (fallbacks, beta) in [
            (
                json!(["claude-opus-5"]),
                Some(SERVER_SIDE_FALLBACK_DEFAULT_BETA),
            ),
            (json!("default"), Some(SERVER_SIDE_FALLBACK_ARRAY_BETA)),
            (json!(["claude-opus-5"]), None),
            (json!([]), Some(SERVER_SIDE_FALLBACK_ARRAY_BETA)),
        ] {
            let (body, outbound_beta) = apply(fallbacks, beta);
            assert!(body.get("fallbacks").is_none());
            assert!(!outbound_beta.contains(SERVER_SIDE_FALLBACK_ARRAY_BETA));
            assert!(!outbound_beta.contains(SERVER_SIDE_FALLBACK_DEFAULT_BETA));
        }
    }

    #[test]
    fn fallback_beta_can_be_explicit_in_repeated_headers_or_internal_fields() {
        let mut headers = HeaderMap::new();
        headers.append(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("prompt-caching-2024-07-31"),
        );
        headers.append(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(SERVER_SIDE_FALLBACK_ARRAY_BETA),
        );
        let mut url = "https://api.anthropic.com/v1/messages".to_string();
        let mut body = Bytes::from(
            serde_json::to_vec(&json!({
                "model": "claude-sonnet-5",
                "fallbacks": ["claude-opus-5"],
                "betas": [SERVER_SIDE_FALLBACK_ARRAY_BETA],
                "messages": []
            }))
            .unwrap(),
        );
        let contract =
            apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None)
                .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let beta = contract
            .headers
            .iter()
            .find(|(name, _)| *name == "anthropic-beta")
            .unwrap()
            .1
            .as_str();

        assert!(body.get("betas").is_none());
        assert!(body.get("fallbacks").is_some());
        assert_eq!(beta.matches(SERVER_SIDE_FALLBACK_ARRAY_BETA).count(), 1);
    }

    #[test]
    fn advisor_tool_beta_is_exact_shape_gated_and_ordered() {
        let body = json!({
            "model": "claude-sonnet-5",
            "tools": [{"type": "advisor_20260301", "defer_loading": true}]
        });
        let beta = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&body),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        let betas = beta.split(',').collect::<Vec<_>>();
        let mid = betas
            .iter()
            .position(|beta| *beta == MID_CONVERSATION_SYSTEM_BETA)
            .unwrap();
        let advisor = betas
            .iter()
            .position(|beta| *beta == ADVISOR_TOOL_BETA)
            .unwrap();
        let advanced = betas
            .iter()
            .position(|beta| *beta == ADVANCED_TOOL_USE_BETA)
            .unwrap();
        assert!(mid < advisor && advisor < advanced);

        let ordinary = json!({"tools": [{"type": "advisor"}]});
        let beta = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&ordinary),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert!(!beta.contains(ADVISOR_TOOL_BETA));
    }

    #[test]
    fn advanced_tool_beta_requires_an_audited_deferred_or_search_shape() {
        for ordinary in [
            json!({"tools": [{"name": "Read", "input_schema": {"type": "object"}}]}),
            json!({"tools": [{"name": "computer", "type": "computer_use_20250124"}]}),
            json!({"tools": [{"name": "bad", "defer_loading": "true"}]}),
        ] {
            let beta = build_anthropic_beta_value(
                &HeaderMap::new(),
                Some(&ordinary),
                &[],
                false,
                true,
                ClaudeBetaOperation::Messages,
            );
            assert!(!beta.contains(ADVANCED_TOOL_USE_BETA));
        }

        for advanced in [
            json!({"tools": [{"name": "deferred", "defer_loading": true}]}),
            json!({"tools": [{"name": "deferred", "custom": {"defer_loading": true}}]}),
            json!({"tools": [{"type": "tool_search_tool_regex_20251119"}]}),
            json!({"tools": [{"type": "tool_search_tool_bm25_20251119"}]}),
        ] {
            let beta = build_anthropic_beta_value(
                &HeaderMap::new(),
                Some(&advanced),
                &[],
                false,
                true,
                ClaudeBetaOperation::Messages,
            );
            assert!(beta.contains(ADVANCED_TOOL_USE_BETA));
        }
    }

    #[test]
    fn thinking_display_updates_beta_is_exact_and_excludes_redaction() {
        let updates = json!({"thinking": {"type": "adaptive", "display": "updates"}});
        let beta = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&updates),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert!(beta.contains(THINKING_DISPLAY_UPDATES_BETA));
        assert!(!beta.contains(REDACT_THINKING_BETA));

        let other = json!({"thinking": {"type": "adaptive", "display": "summary"}});
        let beta = build_anthropic_beta_value(
            &HeaderMap::new(),
            Some(&other),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        assert!(!beta.contains(THINKING_DISPLAY_UPDATES_BETA));
    }

    #[test]
    fn count_tokens_supports_advisor_but_strips_generation_fallback_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(SERVER_SIDE_FALLBACK_DEFAULT_BETA),
        );
        let mut url = "https://api.anthropic.com/v1/messages/count_tokens".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-5","tools":[{"type":"advisor_20260301"}],"fallbacks":"default","fallback_credit_token":"secret","messages":[]}"#,
        );
        let contract = apply_count_tokens_forward_contract(
            &mut url,
            &mut body,
            &headers,
            "account-123",
            false,
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let beta = contract
            .headers
            .iter()
            .find(|(name, _)| *name == "anthropic-beta")
            .unwrap()
            .1
            .as_str();

        assert!(body.get("fallbacks").is_none());
        assert!(body.get("fallback_credit_token").is_none());
        assert!(beta.contains(ADVISOR_TOOL_BETA));
        assert!(!beta.contains(SERVER_SIDE_FALLBACK_DEFAULT_BETA));
    }

    #[test]
    fn native_beta_passthrough_drops_values_outside_the_audited_profile() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(
                "claude-code-20250219,unknown-beta,structured-outputs-2025-12-15",
            ),
        );
        let beta = build_anthropic_beta_value_for_class(
            &headers,
            None,
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
            ClaudeClientClass::NativeCli,
        );
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,structured-outputs-2025-12-15,extended-cache-ttl-2025-04-11"
        );
    }

    #[test]
    fn native_passthrough_enforces_new_advanced_and_display_shapes() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(
                "advanced-tool-use-2025-11-20,thinking-display-updates-2026-08-18,redact-thinking-2026-02-12",
            ),
        );
        let ordinary = json!({
            "thinking": {"type": "adaptive"},
            "tools": [{"name": "Read", "input_schema": {"type": "object"}}]
        });
        let beta = build_anthropic_beta_value_for_class(
            &headers,
            Some(&ordinary),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
            ClaudeClientClass::NativeCli,
        );
        assert!(!beta.contains(ADVANCED_TOOL_USE_BETA));
        assert!(!beta.contains(THINKING_DISPLAY_UPDATES_BETA));
        assert!(beta.contains(REDACT_THINKING_BETA));

        let matching = json!({
            "thinking": {"type": "adaptive", "display": "updates"},
            "tools": [{"name": "Read", "defer_loading": true}]
        });
        let beta = build_anthropic_beta_value_for_class(
            &headers,
            Some(&matching),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
            ClaudeClientClass::NativeCli,
        );
        assert!(beta.contains(ADVANCED_TOOL_USE_BETA));
        assert!(beta.contains(THINKING_DISPLAY_UPDATES_BETA));
        assert!(!beta.contains(REDACT_THINKING_BETA));
    }

    #[test]
    fn native_passthrough_also_enforces_fallback_body_beta_consistency() {
        let mut headers = HeaderMap::new();
        headers.insert("x-app", axum::http::HeaderValue::from_static("cli"));
        headers.insert(
            "user-agent",
            axum::http::HeaderValue::from_static("claude-cli/2.1.258 (external, cli)"),
        );
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(SERVER_SIDE_FALLBACK_ARRAY_BETA),
        );
        let mut url = "https://api.anthropic.com/v1/messages".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-5","fallbacks":"default","metadata":{"user_id":"user_a_account__session_abc"},"messages":[]}"#,
        );
        let contract =
            apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None)
                .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let beta = contract
            .headers
            .iter()
            .find(|(name, _)| *name == "anthropic-beta")
            .unwrap()
            .1
            .as_str();

        assert!(body.get("fallbacks").is_none());
        assert!(!beta.contains(SERVER_SIDE_FALLBACK_ARRAY_BETA));
        assert!(!beta.contains(SERVER_SIDE_FALLBACK_DEFAULT_BETA));
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_rejects_shape_betas_without_matching_body() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(
                "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14,computer-use-2024-10-22,context-management-2025-06-27,context-1m-2025-08-07,effort-2025-11-24,extended-cache-ttl-2025-04-11,token-counting-2024-11-01",
            ),
        );
        let body = json!({
            "model": "claude-sonnet-4",
            "messages": [],
            "stream": false
        });

        let beta = build_anthropic_beta_value(
            &headers,
            Some(&body),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(beta.starts_with("claude-code-20250219,oauth-2025-04-20,"));
        assert!(!beta.contains("fine-grained-tool-streaming"));
        assert!(!beta.contains("computer-use"));
        assert!(!beta.contains(CONTEXT_1M_BETA));
        assert!(!beta.contains(TOKEN_COUNTING_BETA));
    }

    #[test]
    fn anthropic_beta_non_oauth_path_preserves_client_markers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("custom-beta"),
        );

        let beta = build_anthropic_beta_value(
            &headers,
            None,
            &[],
            false,
            false,
            ClaudeBetaOperation::Messages,
        );

        assert_eq!(beta, "claude-code-20250219,custom-beta");
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_tracks_context_management() {
        let headers = HeaderMap::new();
        let body = json!({"context_management": {"edits": []}});
        let beta = build_anthropic_beta_value(
            &headers,
            Some(&body),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(beta
            .split(',')
            .any(|value| value == CONTEXT_MANAGEMENT_BETA));
    }

    #[test]
    fn internal_beta_fields_are_removed_and_shape_betas_are_resolved() {
        let headers = HeaderMap::new();
        let mut body = json!({
            "model": "claude-opus-4-6",
            "anthropic_beta": [CONTEXT_1M_BETA, "custom-beta"],
            "betas": [EFFORT_BETA],
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
            "system": [{
                "type": "text",
                "text": "cached",
                "cache_control": {"type": "ephemeral", "ttl": "1h"}
            }]
        });
        let internal = take_internal_anthropic_betas(&mut body);
        let beta = build_anthropic_beta_value(
            &headers,
            Some(&body),
            &internal,
            true,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(body.get("anthropic_beta").is_none());
        assert!(body.get("betas").is_none());
        assert!(beta.contains(CONTEXT_1M_BETA));
        assert!(beta.contains(EFFORT_BETA));
        assert!(beta.contains(EXTENDED_CACHE_TTL_BETA));
        assert!(!beta.contains("custom-beta"));
    }

    #[test]
    fn context_1m_beta_requires_explicit_model_capability() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(CONTEXT_1M_BETA),
        );

        let blocked = build_anthropic_beta_value(
            &headers,
            None,
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );
        let allowed = build_anthropic_beta_value(
            &headers,
            None,
            &[],
            true,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(!blocked.contains(CONTEXT_1M_BETA));
        assert!(allowed.contains(CONTEXT_1M_BETA));
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
            apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None)
                .unwrap();
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
        assert!(!contract.headers.iter().any(|(name, _)| matches!(
            *name,
            "anthropic-dangerous-direct-browser-access" | "sec-fetch-mode"
        )));
        assert!(value
            .pointer("/metadata/user_id")
            .and_then(Value::as_str)
            .is_some_and(|user_id| user_id.ends_with(&format!("_session_{session_id}"))));
        assert!(value["system"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .starts_with(BILLING_PREFIX));
        assert!(value["system"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("cc_version=2.1.258.1e2;"));
        assert!(value["system"][0].get("cache_control").is_none());
        assert_eq!(value["tools"], json!([]));
    }

    #[test]
    fn confirmed_native_and_helper_requests_take_the_minimal_body_path() {
        fn headers(beta: &'static str) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert("x-app", axum::http::HeaderValue::from_static("cli"));
            headers.insert(
                "user-agent",
                axum::http::HeaderValue::from_static(
                    "claude-cli/2.1.258 (external, cli, linux-x64)",
                ),
            );
            headers.insert(
                "x-stainless-package-version",
                axum::http::HeaderValue::from_static("0.112.1"),
            );
            headers.insert("anthropic-beta", axum::http::HeaderValue::from_static(beta));
            headers
        }

        let mut native_url = "https://api.anthropic.com/v1/messages".to_string();
        let mut native_body = Bytes::from_static(
            br#"{"model":"claude-sonnet-5","metadata":{"user_id":"user_a_account__session_abc"},"messages":[{"role":"user","content":"hi"}]}"#,
        );
        let native = apply_forward_contract(
            &mut native_url,
            &mut native_body,
            &headers(CLAUDE_CODE_BETA),
            "account-123",
            false,
            None,
        )
        .unwrap();
        let native_body: Value = serde_json::from_slice(&native_body).unwrap();
        assert!(native_body.get("system").is_none());
        assert!(native_body.get("tools").is_none());
        assert!(native_body.get("max_tokens").is_none());
        assert!(native_body.get("thinking").is_none());
        assert!(native
            .headers
            .iter()
            .any(|(name, value)| { *name == "x-stainless-package-version" && value == "0.112.1" }));

        let helper_beta = "oauth-2025-04-20,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05";
        let mut helper_url = "https://api.anthropic.com/v1/messages".to_string();
        let mut helper_body = Bytes::from_static(
            br#"{"model":"claude-haiku-4-5-20251001","max_tokens":1024,"metadata":{"user_id":"user_a_account__session_abc"},"messages":[{"role":"user","content":"classify"}]}"#,
        );
        apply_forward_contract(
            &mut helper_url,
            &mut helper_body,
            &headers(helper_beta),
            "account-123",
            false,
            None,
        )
        .unwrap();
        let helper_body: Value = serde_json::from_slice(&helper_body).unwrap();
        assert!(helper_body.get("system").is_none());
        assert!(helper_body.get("tools").is_none());
        assert_eq!(helper_body["max_tokens"], 1024);
    }

    #[test]
    fn native_billing_without_cch_is_finalized_without_other_injection() {
        let mut headers = HeaderMap::new();
        headers.insert("x-app", axum::http::HeaderValue::from_static("cli"));
        headers.insert(
            "user-agent",
            axum::http::HeaderValue::from_static(
                "claude-cli/2.1.258 (external, sdk-cli, linux-x64)",
            ),
        );
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static(CLAUDE_CODE_BETA),
        );
        let mut url = "https://api.anthropic.com/v1/messages".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-5","system":[{"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.258.1e2; cc_entrypoint=sdk-cli;"}],"metadata":{"user_id":"user_a_account__session_abc"},"messages":[{"role":"user","content":"hi"}]}"#,
        );
        apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None).unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let billing = body["system"][0]["text"].as_str().unwrap();
        assert!(billing.contains("cch="));
        assert!(!billing.contains("cch=00000;"));
        assert!(body.get("tools").is_none());
        assert!(body.get("max_tokens").is_none());
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

        apply_forward_contract(&mut url, &mut body, &headers, "account-123", false, None).unwrap();
        let text = std::str::from_utf8(&body).unwrap();

        assert!(text.find("\"model\"").unwrap() < text.find("\"max_tokens\"").unwrap());
        assert!(text.find("\"max_tokens\"").unwrap() < text.find("\"messages\"").unwrap());
        assert!(text.find("\"messages\"").unwrap() < text.find("\"stream\"").unwrap());
    }

    #[test]
    fn claude_oauth_tool_names_use_canonical_wire_case_and_restore_declared_case() {
        let mut request = json!({
            "tools": [
                {"name": "read", "input_schema": {"type": "object"}},
                {"name": "CustomLookup", "input_schema": {"type": "object"}}
            ],
            "tool_choice": {"type": "tool", "name": "READ"},
            "messages": [{
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "toolu_1", "name": "rEaD", "input": {}}]
            }]
        });

        let aliases = normalize_claude_oauth_tool_names(&mut request, None).unwrap();
        assert_eq!(request.pointer("/tools/0/name"), Some(&json!("Read")));
        assert_eq!(
            request.pointer("/tools/1/name"),
            Some(&json!("CustomLookup"))
        );
        assert_eq!(request.pointer("/tool_choice/name"), Some(&json!("Read")));
        assert_eq!(
            request.pointer("/messages/0/content/0/name"),
            Some(&json!("Read"))
        );

        let response = Bytes::from_static(
            br#"{"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{}},{"type":"tool_use","id":"toolu_2","name":"customlookup","input":{}}]}"#,
        );
        let restored = restore_claude_tool_names_in_response_bytes(response, &aliases);
        let restored: Value = serde_json::from_slice(&restored).unwrap();
        assert_eq!(restored.pointer("/content/0/name"), Some(&json!("read")));
        assert_eq!(
            restored.pointer("/content/1/name"),
            Some(&json!("CustomLookup"))
        );
    }

    #[test]
    fn claude_oauth_tool_names_reject_case_insensitive_declaration_collisions() {
        let mut request = json!({
            "tools": [
                {"name": "read", "input_schema": {"type": "object"}},
                {"name": "Read", "input_schema": {"type": "object"}}
            ]
        });

        let error = normalize_claude_oauth_tool_names(&mut request, None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.message.contains("collide case-insensitively"));
    }

    #[test]
    fn custom_tool_alias_is_stable_scoped_and_round_trips() {
        let mut request = json!({
            "tools": [
                {"name": "CustomLookup", "input_schema": {"type": "object"}},
                {"name": "mcp__weather__forecast", "type": "mcp_tool", "input_schema": {"type": "object"}},
                {"name": "web_search", "type": "web_search_20250305"}
            ],
            "tool_choice": {"type": "tool", "name": "customlookup"},
            "messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "CustomLookup", "input": {}},
                {"type": "tool_reference", "tool_name": "mcp__weather__forecast"}
            ]}]
        });
        let aliases =
            normalize_claude_oauth_tool_names(&mut request, Some("account-a\0session-a")).unwrap();
        let custom_alias = request["tools"][0]["name"].as_str().unwrap().to_string();
        let mcp_alias = request["tools"][1]["name"].as_str().unwrap().to_string();
        assert!(custom_alias.starts_with("cc_tool_"));
        assert!(mcp_alias.starts_with("cc_tool_"));
        assert_ne!(custom_alias, mcp_alias);
        assert_eq!(request["tools"][2]["name"], "web_search");
        assert_eq!(request["tool_choice"]["name"], custom_alias);
        assert_eq!(request["messages"][0]["content"][0]["name"], custom_alias);
        assert_eq!(request["messages"][0]["content"][1]["tool_name"], mcp_alias);

        let response = Bytes::from(
            serde_json::to_vec(&json!({
                "content": [{"type": "tool_use", "name": custom_alias, "input": {}}]
            }))
            .unwrap(),
        );
        let restored = restore_claude_tool_names_in_response_bytes(response, &aliases);
        let restored: Value = serde_json::from_slice(&restored).unwrap();
        assert_eq!(restored["content"][0]["name"], "CustomLookup");
    }

    #[test]
    fn claude_tool_name_stream_patcher_restores_fragmented_sse_events() {
        let aliases = BTreeMap::from([("read".to_string(), "read".to_string())]);
        let mut patcher = ClaudeToolNameStreamPatcher::new(aliases);

        assert!(patcher
            .push(Bytes::from_static(
                b"event: content_block_start\r\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"name\":\"Re"
            ))
            .is_empty());
        let output = patcher.push(Bytes::from_static(
            b"ad\",\"input\":{}}}\r\n\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
        ));
        let output = std::str::from_utf8(&output).unwrap();
        assert!(output.contains("\"name\":\"read\""));
        assert!(output.contains("event: content_block_start\r\n"));
        assert!(output.contains("data: {\"type\":\"message_stop\"}"));
        assert!(patcher.finish().is_empty());
    }

    #[test]
    fn count_tokens_contract_filters_generation_fields_and_signs_final_body() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("prompt-caching-2024-07-31,unknown-beta"),
        );
        let mut url = "https://api.anthropic.com/v1/messages/count_tokens".to_string();
        let mut body = Bytes::from_static(
            br#"{"model":"claude-sonnet-4-6","max_tokens":4096,"temperature":0.2,"top_p":0.9,"top_k":40,"stream":true,"stop_sequences":["x"],"anthropic_beta":["effort-2025-11-24"],"output_config":{"effort":"high"},"messages":[{"role":"user","content":"hi"}]}"#,
        );

        let contract = apply_count_tokens_forward_contract(
            &mut url,
            &mut body,
            &headers,
            "account-123",
            false,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        for field in [
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "stream",
            "stop_sequences",
            "anthropic_beta",
            "betas",
            "thinking",
            "output_config",
            "context_management",
            "tool_choice",
        ] {
            assert!(value.get(field).is_none(), "unexpected field: {field}");
        }
        let beta = contract
            .headers
            .iter()
            .find(|(name, _)| *name == "anthropic-beta")
            .map(|(_, value)| value.as_str())
            .unwrap();
        assert!(beta.contains(TOKEN_COUNTING_BETA));
        assert!(beta.contains("prompt-caching-2024-07-31"));
        assert!(!beta.contains(EFFORT_BETA));
        assert!(!beta.contains("unknown-beta"));

        let signed_again = sign_claude_oauth_messages_body(value.clone());
        assert_eq!(value, signed_again, "CCH must sign the final filtered body");
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
        let beta = build_anthropic_beta_value(
            &headers,
            Some(&body),
            &[],
            false,
            true,
            ClaudeBetaOperation::Messages,
        );

        assert!(beta.contains(INTERLEAVED_THINKING_BETA));
        assert!(!beta.contains(ADVANCED_TOOL_USE_BETA));
        assert!(!beta.contains("fine-grained-tool-streaming"));
        assert!(!beta.contains("computer-use"));
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

    #[test]
    fn cch_matches_current_claude_code_golden_vector() {
        let profile: Value = serde_json::from_str(WIRE_PROFILE_JSON).unwrap();
        let vector = &profile["cch"]["goldenVectors"][0];
        assert_eq!(vector["signature"], "8d393");
        assert_eq!(
            vector["syntheticBody"]["messages"][0]["content"][0]["text"],
            "ping"
        );
        let body = vector["syntheticBody"].clone();
        let expected_signature = vector["signature"].as_str().unwrap();
        let unsigned_billing = body["system"][0]["text"].as_str().unwrap().to_string();
        let signed = sign_claude_oauth_messages_body(body);
        let signed_billing = signed["system"][0]["text"].as_str().unwrap();
        assert!(
            signed_billing.contains(&format!("cch={expected_signature};")),
            "unexpected signed billing block: {signed_billing}"
        );

        let mut model_changed = signed.clone();
        model_changed["model"] = json!("model-b");
        model_changed["system"][0]["text"] = json!(unsigned_billing);
        assert!(
            sign_claude_oauth_messages_body(model_changed)["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("cch={expected_signature};"))
        );
    }
}
