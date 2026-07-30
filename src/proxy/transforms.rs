#![allow(dead_code)]

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::reasoning_bridge::{
    anthropic_block_from_openai_reasoning_item, anthropic_block_from_responses_reasoning_item,
    openai_reasoning_item_from_anthropic_block, reasoning_summary_text,
    responses_reasoning_item_from_anthropic_block, unsigned_responses_reasoning_item,
};
use super::tool_media::{
    extract_tool_media, flush_chat_media, queue_chat_media, sanitized_tool_text, ToolMediaPart,
    ToolMediaScope,
};
use super::tool_schema::normalize_function_parameters;

const DEFAULT_OPENAI_TO_ANTHROPIC_MAX_TOKENS: u64 = 8192;
const TOOL_SEARCH_PROXY_NAME: &str = "tool_search";
const CHAT_TOOL_NAME_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesToolKind {
    Function,
    Namespace,
    Custom,
    ToolSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResponsesToolSpec {
    kind: ResponsesToolKind,
    name: String,
    namespace: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ResponsesToolContext {
    chat_name_to_spec: BTreeMap<String, ResponsesToolSpec>,
    namespace_name_to_chat_name: BTreeMap<(String, String), String>,
}

impl ResponsesToolContext {
    fn from_custom_tool_names(names: &BTreeSet<String>) -> Self {
        let mut context = Self::default();
        for name in names {
            context.add_spec(
                name.clone(),
                ResponsesToolSpec {
                    kind: ResponsesToolKind::Custom,
                    name: name.clone(),
                    namespace: None,
                },
            );
        }
        context
    }

    pub(crate) fn custom_tool_names(&self) -> BTreeSet<String> {
        self.chat_name_to_spec
            .iter()
            .filter(|(_, spec)| spec.kind == ResponsesToolKind::Custom)
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) fn is_custom_tool_chat_name(&self, name: &str) -> bool {
        self.chat_name_to_spec
            .get(name)
            .is_some_and(|spec| spec.kind == ResponsesToolKind::Custom)
    }

    fn chat_name_for_response_function(&self, name: &str, namespace: Option<&str>) -> String {
        let Some(namespace) = namespace.filter(|value| !value.is_empty()) else {
            return name.to_string();
        };
        self.namespace_name_to_chat_name
            .get(&(namespace.to_string(), name.to_string()))
            .cloned()
            .unwrap_or_else(|| flatten_namespace_tool_name(namespace, name))
    }

    pub(crate) fn response_item_id(&self, call_id: &str, chat_name: &str) -> String {
        let prefix = if self.is_custom_tool_chat_name(chat_name) {
            "ctc"
        } else {
            "fc"
        };
        format!("{prefix}_{call_id}")
    }

    pub(crate) fn response_item(
        &self,
        item_id: &str,
        status: &str,
        call_id: &str,
        chat_name: &str,
        arguments: &str,
    ) -> Value {
        match self.chat_name_to_spec.get(chat_name) {
            Some(spec) if spec.kind == ResponsesToolKind::ToolSearch => json!({
                "type": "tool_search_call",
                "status": status,
                "call_id": call_id,
                "execution": "client",
                "arguments": parse_tool_search_arguments(arguments)
            }),
            Some(spec) if spec.kind == ResponsesToolKind::Custom => json!({
                "id": item_id,
                "type": "custom_tool_call",
                "status": status,
                "call_id": call_id,
                "name": spec.name,
                "input": unwrap_custom_tool_input(arguments)
            }),
            Some(spec) => {
                let mut item = json!({
                    "id": item_id,
                    "type": "function_call",
                    "status": status,
                    "call_id": call_id,
                    "name": spec.name,
                    "arguments": arguments
                });
                if let Some(namespace) = spec.namespace.as_deref() {
                    item["namespace"] = json!(namespace);
                }
                item
            }
            None => json!({
                "id": item_id,
                "type": "function_call",
                "status": status,
                "call_id": call_id,
                "name": chat_name,
                "arguments": arguments
            }),
        }
    }

    fn add_spec(&mut self, chat_name: String, spec: ResponsesToolSpec) {
        if chat_name.is_empty() || self.chat_name_to_spec.contains_key(&chat_name) {
            return;
        }
        if let Some(namespace) = spec.namespace.as_ref() {
            self.namespace_name_to_chat_name
                .insert((namespace.clone(), spec.name.clone()), chat_name.clone());
        }
        self.chat_name_to_spec.insert(chat_name, spec);
    }

    fn add_response_tool(&mut self, tool: &Value) {
        if let Some(name) = tool.as_str().map(str::trim).filter(|name| !name.is_empty()) {
            self.add_spec(
                name.to_string(),
                ResponsesToolSpec {
                    kind: ResponsesToolKind::Custom,
                    name: name.to_string(),
                    namespace: None,
                },
            );
            return;
        }
        match tool
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
        {
            "custom" => {
                if let Some(name) = response_tool_name(tool) {
                    self.add_spec(
                        name.clone(),
                        ResponsesToolSpec {
                            kind: ResponsesToolKind::Custom,
                            name,
                            namespace: None,
                        },
                    );
                }
            }
            "tool_search" => self.add_spec(
                TOOL_SEARCH_PROXY_NAME.to_string(),
                ResponsesToolSpec {
                    kind: ResponsesToolKind::ToolSearch,
                    name: TOOL_SEARCH_PROXY_NAME.to_string(),
                    namespace: None,
                },
            ),
            "namespace" => {
                let namespace = tool.get("name").and_then(Value::as_str).unwrap_or_default();
                for child in tool
                    .get("tools")
                    .or_else(|| tool.get("children"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(name) = response_tool_name(child) else {
                        continue;
                    };
                    let chat_name = flatten_namespace_tool_name(namespace, &name);
                    self.add_spec(
                        chat_name,
                        ResponsesToolSpec {
                            kind: ResponsesToolKind::Namespace,
                            name,
                            namespace: Some(namespace.to_string()),
                        },
                    );
                }
            }
            "function" | "" => {
                if let Some(name) = response_tool_name(tool) {
                    self.add_spec(
                        name.clone(),
                        ResponsesToolSpec {
                            kind: ResponsesToolKind::Function,
                            name,
                            namespace: None,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

impl From<BTreeSet<String>> for ResponsesToolContext {
    fn from(names: BTreeSet<String>) -> Self {
        Self::from_custom_tool_names(&names)
    }
}

pub(crate) fn responses_tool_context(input: &Value) -> ResponsesToolContext {
    let mut context = ResponsesToolContext::default();
    for tool in response_tools_from_request(input) {
        context.add_response_tool(tool);
    }
    if let Some(history) = input.get("input") {
        collect_tool_search_output_tools(history, &mut context);
    }
    context
}

pub(crate) fn responses_tool_context_from_bytes(input: &[u8]) -> ResponsesToolContext {
    serde_json::from_slice::<Value>(input)
        .ok()
        .map(|value| responses_tool_context(&value))
        .unwrap_or_default()
}

fn collect_tool_search_output_tools(value: &Value, context: &mut ResponsesToolContext) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_tool_search_output_tools(item, context);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("tool_search_output") {
                for tool in object
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    context.add_response_tool(tool);
                }
            }
            for value in object.values() {
                collect_tool_search_output_tools(value, context);
            }
        }
        _ => {}
    }
}

fn flatten_namespace_tool_name(namespace: &str, name: &str) -> String {
    let full_name = format!("{namespace}__{name}");
    if full_name.len() <= CHAT_TOOL_NAME_MAX_LEN {
        return full_name;
    }

    let digest = Sha256::digest(full_name.as_bytes());
    let hash = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let suffix = format!("__{hash}");
    let prefix_len = CHAT_TOOL_NAME_MAX_LEN.saturating_sub(suffix.len());
    let mut prefix = String::new();
    for character in full_name.chars() {
        if prefix.len() + character.len_utf8() > prefix_len {
            break;
        }
        prefix.push(character);
    }
    format!("{prefix}{suffix}")
}

fn parse_tool_search_arguments(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"query": arguments}))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformError {
    message: String,
}

impl TransformError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransformError {}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamPayload {
    Json(Value),
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamFrame {
    pub event: Option<&'static str>,
    pub payload: StreamPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasoningEffortMode {
    Passthrough,
    Ollama,
}

impl StreamFrame {
    pub fn json(payload: Value) -> Self {
        Self {
            event: None,
            payload: StreamPayload::Json(payload),
        }
    }

    pub fn event(event: &'static str, payload: Value) -> Self {
        Self {
            event: Some(event),
            payload: StreamPayload::Json(payload),
        }
    }

    pub fn done() -> Self {
        Self {
            event: None,
            payload: StreamPayload::Done,
        }
    }
}

pub fn openai_chat_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("openai chat messages must be an array"))?;
    let mut output_messages = Vec::new();
    let mut system_parts = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if matches!(role, "system" | "developer") {
            collect_text_like(&message["content"], &mut system_parts);
            continue;
        }
        output_messages.extend(openai_chat_message_to_anthropic(message, role)?);
    }

    merge_adjacent_anthropic_messages(&mut output_messages);
    drop_empty_anthropic_messages(&mut output_messages);
    drop_incomplete_anthropic_tool_turns(&mut output_messages);
    drop_empty_anthropic_messages(&mut output_messages);
    merge_adjacent_anthropic_messages(&mut output_messages);
    order_anthropic_tool_results_first(&mut output_messages);
    if output_messages.is_empty() {
        return Err(TransformError::new(
            "openai chat messages contain no valid anthropic messages",
        ));
    }
    ensure_leading_anthropic_user_message(&mut output_messages);
    let thinking_history_is_valid = trailing_turn_supports_thinking(&output_messages);

    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    if !system_parts.is_empty() {
        output.insert("system".to_string(), Value::String(system_parts.join("\n")));
    }
    output.insert("messages".to_string(), Value::Array(output_messages));
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(tools) = openai_tools_to_anthropic(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }
    apply_openai_request_controls_to_anthropic(
        input,
        &mut output,
        &["max_completion_tokens", "max_tokens", "max_output_tokens"],
    );
    apply_openai_reasoning_to_anthropic(input, &mut output, thinking_history_is_valid)?;
    apply_openai_tool_controls_to_anthropic(input, &mut output, None)?;

    Ok(Value::Object(output))
}

pub fn openai_responses_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    validate_response_tool_type_collisions(input)?;
    let tool_context = responses_tool_context(input);
    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(tools) = openai_response_tools_to_anthropic(input, &tool_context)? {
        output.insert("tools".to_string(), tools);
    }
    apply_openai_request_controls_to_anthropic(
        input,
        &mut output,
        &["max_output_tokens", "max_completion_tokens", "max_tokens"],
    );
    let mut system_parts = Vec::new();
    append_response_system_text(input.get("instructions"), &mut system_parts);
    if let Some(items) = input.get("input").and_then(Value::as_array) {
        for item in items {
            if matches!(
                item.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            ) {
                append_response_system_text(item.get("content"), &mut system_parts);
            }
        }
    }
    if !system_parts.is_empty() {
        output.insert(
            "system".to_string(),
            Value::String(system_parts.join("\n\n")),
        );
    }

    let mut messages = Vec::new();
    match input.get("input") {
        Some(Value::String(text)) => messages.push(json!({
            "role": "user",
            "content": [{"type": "text", "text": text}]
        })),
        Some(Value::Array(items)) => {
            for item in items {
                messages.extend(openai_response_item_to_anthropic(item, &tool_context)?);
            }
        }
        _ => return Err(TransformError::new("openai responses input is required")),
    }
    merge_adjacent_anthropic_messages(&mut messages);
    drop_empty_anthropic_messages(&mut messages);
    drop_incomplete_anthropic_tool_turns(&mut messages);
    drop_empty_anthropic_messages(&mut messages);
    merge_adjacent_anthropic_messages(&mut messages);
    order_anthropic_tool_results_first(&mut messages);
    if messages.is_empty() {
        return Err(TransformError::new(
            "openai responses input contains no valid anthropic messages",
        ));
    }
    ensure_leading_anthropic_user_message(&mut messages);
    let thinking_history_is_valid = trailing_turn_supports_thinking(&messages);
    output.insert("messages".to_string(), Value::Array(messages));
    apply_openai_reasoning_to_anthropic(input, &mut output, thinking_history_is_valid)?;
    apply_openai_tool_controls_to_anthropic(input, &mut output, Some(&tool_context))?;

    Ok(Value::Object(output))
}

pub fn openai_chat_to_responses(input: &Value) -> Result<Value, TransformError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("openai chat messages must be an array"))?;
    let (instructions, response_input) = openai_chat_messages_to_response_input(messages);

    let mut output = Map::new();
    copy_value(input, &mut output, "model");
    if !instructions.is_empty() {
        output.insert(
            "instructions".to_string(),
            Value::String(instructions.join("\n\n")),
        );
    }
    output.insert("input".to_string(), Value::Array(response_input));

    if let Some(value) = input
        .get("max_completion_tokens")
        .or_else(|| input.get("max_tokens"))
    {
        output.insert("max_output_tokens".to_string(), value.clone());
    }
    for key in [
        "temperature",
        "top_p",
        "stream",
        "store",
        "metadata",
        "parallel_tool_calls",
        "include",
        "service_tier",
        "prompt_cache_key",
        "truncation",
        "stop",
        "previous_response_id",
        "user",
        "safety_identifier",
    ] {
        copy_value(input, &mut output, key);
    }
    if let Some(reasoning) = input.get("reasoning") {
        output.insert("reasoning".to_string(), reasoning.clone());
    } else if let Some(effort) = input.get("reasoning_effort") {
        output.insert("reasoning".to_string(), json!({"effort": effort.clone()}));
    }
    if let Some(tools) = openai_chat_tools_to_responses(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }
    if let Some(tool_choice) = input.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            openai_chat_tool_choice_to_responses(tool_choice),
        );
    }
    if let Some(response_format) = input.get("response_format") {
        output.insert(
            "text".to_string(),
            json!({"format": response_format.clone()}),
        );
    } else {
        copy_value(input, &mut output, "text");
    }

    Ok(Value::Object(output))
}

pub fn openai_responses_to_chat(input: &Value) -> Result<Value, TransformError> {
    openai_responses_to_chat_with_reasoning_effort(input, ReasoningEffortMode::Passthrough)
}

pub(crate) fn openai_responses_to_chat_with_reasoning_effort(
    input: &Value,
    effort_mode: ReasoningEffortMode,
) -> Result<Value, TransformError> {
    validate_response_tool_type_collisions(input)?;
    let tool_context = responses_tool_context(input);
    let mut messages = Vec::new();
    let mut pending_media = Vec::new();
    let mut pending_reasoning = Vec::new();
    let mut last_assistant_index = None;
    if let Some(instructions) = input.get("instructions") {
        if let Some(text) = response_instruction_text(instructions) {
            messages.push(json!({"role": "system", "content": text}));
        }
    }
    match input.get("input") {
        Some(Value::String(text)) => {
            messages.push(json!({"role": "user", "content": text}));
        }
        Some(Value::Array(items)) => {
            for item in items {
                append_response_input_item_to_chat_messages(
                    item,
                    &tool_context,
                    &mut messages,
                    &mut pending_media,
                    &mut pending_reasoning,
                    &mut last_assistant_index,
                );
            }
        }
        Some(value @ Value::Object(_)) => append_response_input_item_to_chat_messages(
            value,
            &tool_context,
            &mut messages,
            &mut pending_media,
            &mut pending_reasoning,
            &mut last_assistant_index,
        ),
        _ => return Err(TransformError::new("openai responses input is required")),
    }
    flush_chat_media(&mut messages, &mut pending_media);
    attach_pending_reasoning_to_previous_assistant(
        &mut messages,
        last_assistant_index,
        &mut pending_reasoning,
    );

    let mut output = Map::new();
    copy_value(input, &mut output, "model");
    output.insert("messages".to_string(), Value::Array(messages));
    if let Some(max_tokens) = input.get("max_output_tokens") {
        output.insert("max_completion_tokens".to_string(), max_tokens.clone());
    }
    for key in [
        "temperature",
        "top_p",
        "stream",
        "frequency_penalty",
        "logit_bias",
        "logprobs",
        "metadata",
        "n",
        "parallel_tool_calls",
        "presence_penalty",
        "seed",
        "service_tier",
        "stop",
        "stream_options",
        "top_logprobs",
        "user",
    ] {
        copy_value(input, &mut output, key);
    }
    if let Some(effort) = input.pointer("/reasoning/effort") {
        if let Some(effort) = map_chat_reasoning_effort(effort, effort_mode) {
            output.insert("reasoning_effort".to_string(), effort);
        }
    }
    if let Some(tools) = openai_response_tools_to_chat(input, &tool_context)? {
        output.insert("tools".to_string(), tools);
    }
    if let Some(tool_choice) = input.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            openai_response_tool_choice_to_chat(tool_choice, &tool_context),
        );
    }
    if let Some(format) = input.pointer("/text/format") {
        output.insert("response_format".to_string(), format.clone());
    }

    Ok(Value::Object(output))
}

fn map_chat_reasoning_effort(effort: &Value, effort_mode: ReasoningEffortMode) -> Option<Value> {
    let ReasoningEffortMode::Ollama = effort_mode else {
        return Some(effort.clone());
    };

    let effort = effort.as_str()?.trim().to_ascii_lowercase();
    let mapped = match effort.as_str() {
        "max" | "xhigh" => "max",
        "high" => "high",
        "medium" => "medium",
        "low" | "minimal" => "low",
        "none" | "off" | "disabled" => "none",
        _ => return None,
    };
    Some(Value::String(mapped.to_string()))
}

pub fn gemini_native_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let contents = input
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("gemini contents must be an array"))?;
    let mut output = Map::new();
    copy_string(input, &mut output, "model");

    if let Some(system) = gemini_system_text(input.get("systemInstruction")) {
        output.insert("system".to_string(), Value::String(system));
    }

    output.insert(
        "messages".to_string(),
        Value::Array(
            contents
                .iter()
                .map(gemini_content_to_anthropic)
                .collect::<Vec<_>>(),
        ),
    );
    if let Some(tools) = gemini_tools_to_anthropic(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }

    let mut metadata = Map::new();
    if let Some(value) = input.get("generationConfig") {
        metadata.insert("geminiGenerationConfig".to_string(), value.clone());
    }
    if let Some(value) = input.get("safetySettings") {
        metadata.insert("geminiSafetySettings".to_string(), value.clone());
    }
    if !metadata.is_empty() {
        output.insert("metadata".to_string(), Value::Object(metadata));
    }

    Ok(Value::Object(output))
}

pub fn openai_chat_response_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let choices = input
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("openai chat choices must be an array"))?;
    let mut content = Vec::new();
    let mut stop_reason = None;

    for choice in choices {
        if stop_reason.is_none() {
            stop_reason = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(openai_finish_reason_to_anthropic);
        }
        let message = choice.get("message").or_else(|| choice.get("delta"));
        let Some(message) = message else {
            continue;
        };
        if let Some(reasoning) = openai_chat_message_reasoning_text(message) {
            content.push(json!({"type": "thinking", "thinking": reasoning}));
        }
        content.extend(openai_chat_response_content_to_anthropic(
            message.get("content"),
        ));
        if let Some(refusal) = message
            .get("refusal")
            .and_then(Value::as_str)
            .filter(|refusal| !refusal.is_empty())
        {
            content.push(json!({"type": "text", "text": refusal}));
        }
        let mut has_modern_tool_calls = false;
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            has_modern_tool_calls = !tool_calls.is_empty();
            content.extend(tool_calls.iter().map(openai_tool_call_to_anthropic));
        }
        if !has_modern_tool_calls {
            if let Some(item) = message
                .get("function_call")
                .and_then(openai_chat_legacy_function_call_to_response_item)
            {
                content.push(openai_function_call_to_anthropic(&item));
            }
        }
    }

    Ok(json!({
        "id": input.get("id").and_then(Value::as_str).unwrap_or("chatcmpl"),
        "type": "message",
        "role": "assistant",
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "content": content,
        "stop_reason": stop_reason.unwrap_or("end_turn"),
        "stop_sequence": Value::Null,
        "usage": anthropic_usage_from_openai_usage(input.get("usage"))
    }))
}

pub fn openai_responses_response_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let mut content = Vec::new();
    if let Some(output) = input.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(items) = item.get("content").and_then(Value::as_array) {
                        content
                            .extend(items.iter().filter_map(openai_response_output_to_anthropic));
                    }
                }
                Some("function_call") | Some("custom_tool_call") | Some("tool_search_call") => {
                    content.push(openai_function_call_to_anthropic(item))
                }
                Some("reasoning") => {
                    if let Some(block) = anthropic_block_from_responses_reasoning_item(item)
                        .or_else(|| anthropic_block_from_openai_reasoning_item(item))
                    {
                        content.push(block);
                    }
                }
                _ => {}
            }
        }
    }
    if content.is_empty() {
        if let Some(text) = input.get("output_text").and_then(Value::as_str) {
            content.push(json!({"type": "text", "text": text}));
        }
    }
    Ok(json!({
        "id": input.get("id").and_then(Value::as_str).unwrap_or("resp"),
        "type": "message",
        "role": "assistant",
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "content": content,
        "stop_reason": openai_response_to_anthropic_stop(input),
        "stop_sequence": Value::Null,
        "usage": anthropic_usage_from_openai_usage(input.get("usage"))
    }))
}

pub fn openai_responses_response_to_chat(input: &Value) -> Result<Value, TransformError> {
    let mut text = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning = Vec::new();
    if let Some(output) = input.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for part in content {
                            match part.get("type").and_then(Value::as_str) {
                                Some("output_text") | Some("text") => {
                                    if let Some(value) = part.get("text").and_then(Value::as_str) {
                                        text.push(value.to_string());
                                    }
                                }
                                Some("refusal") => {
                                    if let Some(value) = part
                                        .get("refusal")
                                        .or_else(|| part.get("text"))
                                        .and_then(Value::as_str)
                                    {
                                        text.push(value.to_string());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Some("function_call") | Some("custom_tool_call") | Some("tool_search_call") => {
                    if let Some(tool_call) = openai_response_function_call_to_chat(
                        item,
                        &ResponsesToolContext::default(),
                    ) {
                        tool_calls.push(tool_call);
                    }
                }
                Some("reasoning") => {
                    let summary = reasoning_summary_text(item);
                    if !summary.is_empty() {
                        reasoning.push(summary);
                    }
                }
                _ => {}
            }
        }
    }
    if text.is_empty() {
        if let Some(output_text) = input.get("output_text").and_then(Value::as_str) {
            text.push(output_text.to_string());
        }
    }
    if text.is_empty() && tool_calls.is_empty() && reasoning.is_empty() {
        return Err(TransformError::new("openai responses output is empty"));
    }

    let has_tool_calls = !tool_calls.is_empty();
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text.join("")));
    if has_tool_calls {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if !reasoning.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.join("\n\n")),
        );
    }

    Ok(json!({
        "id": chat_id_from_response_id(input.get("id").and_then(Value::as_str)),
        "object": "chat.completion",
        "created": input.get("created_at").or_else(|| input.get("created")).cloned().unwrap_or(Value::Null),
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": openai_response_finish_reason_to_chat(input, has_tool_calls)
        }],
        "usage": openai_chat_usage_from_responses_usage(input.get("usage"))
    }))
}

pub fn openai_chat_response_to_responses(input: &Value) -> Result<Value, TransformError> {
    openai_chat_response_to_responses_with_tool_context(input, &ResponsesToolContext::default())
}

pub(crate) fn openai_chat_response_to_responses_with_custom_tools(
    input: &Value,
    custom_tool_names: &BTreeSet<String>,
) -> Result<Value, TransformError> {
    let tool_context = ResponsesToolContext::from_custom_tool_names(custom_tool_names);
    openai_chat_response_to_responses_with_tool_context(input, &tool_context)
}

pub(crate) fn openai_chat_response_to_responses_with_tool_context(
    input: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Value, TransformError> {
    let choices = input
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("openai chat choices must be an array"))?;
    let mut output = Vec::new();
    let mut output_text = Vec::new();
    let mut finish_reason = None;

    for choice in choices {
        if finish_reason.is_none() {
            finish_reason = choice.get("finish_reason").and_then(Value::as_str);
        }
        let message = choice.get("message").or_else(|| choice.get("delta"));
        let Some(message) = message else {
            continue;
        };

        if let Some(reasoning) = openai_chat_message_reasoning_text(message) {
            output.push(json!({
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": reasoning}]
            }));
        }

        let mut content =
            openai_chat_content_to_responses_content("assistant", message.get("content"));
        if let Some(refusal) = message
            .get("refusal")
            .and_then(Value::as_str)
            .filter(|refusal| !refusal.is_empty())
        {
            content.push(json!({"type": "refusal", "refusal": refusal}));
        }
        if !content.is_empty() {
            for part in &content {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    output_text.push(text.to_string());
                }
            }
            output.push(json!({
                "type": "message",
                "role": "assistant",
                "content": content
            }));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (index, tool_call) in tool_calls.iter().enumerate() {
                if let Some(item) = openai_chat_tool_call_to_response_item_with_tool_context(
                    tool_call,
                    index,
                    tool_context,
                ) {
                    output.push(item);
                }
            }
        }
        if let Some(function_call) = message.get("function_call") {
            if let Some(item) = openai_chat_legacy_function_call_to_response_item_with_context(
                function_call,
                tool_context,
            ) {
                output.push(item);
            }
        }
    }

    if output.is_empty() {
        return Err(TransformError::new("openai chat response content is empty"));
    }

    let status = openai_chat_finish_reason_to_response_status(finish_reason);
    let mut response = json!({
        "id": response_id_from_chat_id(input.get("id").and_then(Value::as_str)),
        "object": "response",
        "status": status,
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "output": output,
        "output_text": output_text.join(""),
        "usage": openai_responses_usage_from_chat_usage(input.get("usage"))
    });
    if status == "incomplete" {
        response["incomplete_details"] = json!({"reason": "max_output_tokens"});
    }
    Ok(response)
}

pub fn gemini_response_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let candidates = input
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("gemini candidates must be an array"))?;
    let first = candidates
        .first()
        .ok_or_else(|| TransformError::new("gemini candidates must not be empty"))?;
    let parts = first
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("gemini candidate parts must be an array"))?;
    let content = parts
        .iter()
        .map(gemini_part_to_anthropic)
        .collect::<Vec<_>>();
    Ok(json!({
        "id": input.get("responseId").and_then(Value::as_str).unwrap_or("gemini"),
        "type": "message",
        "role": "assistant",
        "model": input.get("modelVersion").and_then(Value::as_str).unwrap_or_default(),
        "content": content,
        "stop_reason": gemini_finish_reason_to_anthropic(first.get("finishReason").and_then(Value::as_str)),
        "stop_sequence": Value::Null,
        "usage": anthropic_usage_from_gemini_usage(input.get("usageMetadata"))
    }))
}

pub fn anthropic_response_to_openai_chat(input: &Value) -> Result<Value, TransformError> {
    let content_blocks = input
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic response content must be an array"))?;
    let mut text = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning = Vec::new();
    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_calls.push(anthropic_tool_use_to_openai(block)),
            Some("thinking") => {
                if let Some(value) = block.get("thinking").and_then(Value::as_str) {
                    reasoning.push(value.to_string());
                }
            }
            Some("redacted_thinking") => {}
            _ => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push(value.to_string());
                }
            }
        }
    }
    if text.is_empty() && tool_calls.is_empty() && reasoning.is_empty() {
        return Err(TransformError::new("anthropic response content is empty"));
    }

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text.join("")));
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if !reasoning.is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.join("\n\n")),
        );
    }

    Ok(json!({
        "id": input.get("id").and_then(Value::as_str).unwrap_or("chatcmpl"),
        "object": "chat.completion",
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": anthropic_stop_reason_to_openai(input.get("stop_reason").and_then(Value::as_str))
        }],
        "usage": openai_usage_from_anthropic_usage(input.get("usage"))
    }))
}

pub fn anthropic_response_to_openai_responses(input: &Value) -> Result<Value, TransformError> {
    anthropic_response_to_openai_responses_with_tool_context(
        input,
        &ResponsesToolContext::default(),
    )
}

pub(crate) fn anthropic_response_to_openai_responses_with_custom_tools(
    input: &Value,
    custom_tool_names: &BTreeSet<String>,
) -> Result<Value, TransformError> {
    let tool_context = ResponsesToolContext::from_custom_tool_names(custom_tool_names);
    anthropic_response_to_openai_responses_with_tool_context(input, &tool_context)
}

pub(crate) fn anthropic_response_to_openai_responses_with_tool_context(
    input: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Value, TransformError> {
    let content_blocks = input
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic response content must be an array"))?;
    let mut message_content = Vec::new();
    let mut output = Vec::new();
    let mut output_text = Vec::new();

    let response_id = input.get("id").and_then(Value::as_str).unwrap_or("resp");
    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                flush_response_output_message(&mut output, &mut message_content);
                output.push(anthropic_tool_use_to_openai_response_with_tool_context(
                    block,
                    tool_context,
                ));
            }
            Some("thinking" | "redacted_thinking") => {
                flush_response_output_message(&mut output, &mut message_content);
                let item_id = format!("rs_{response_id}_{}", output.len());
                if let Some(item) = openai_reasoning_item_from_anthropic_block(block) {
                    output.push(item);
                } else if let Some(item) =
                    responses_reasoning_item_from_anthropic_block(&item_id, block)
                {
                    output.push(item);
                } else if let Some(item) = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .and_then(|text| unsigned_responses_reasoning_item(&item_id, text))
                {
                    output.push(item);
                }
            }
            _ => {
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !text.is_empty() {
                    output_text.push(text.to_string());
                }
                message_content.push(json!({"type": "output_text", "text": text}));
            }
        }
    }
    flush_response_output_message(&mut output, &mut message_content);
    if output.is_empty() {
        return Err(TransformError::new("anthropic response content is empty"));
    }

    let incomplete_reason =
        anthropic_responses_incomplete_reason(input.get("stop_reason").and_then(Value::as_str));
    let mut response = json!({
        "id": response_id,
        "object": "response",
        "status": if incomplete_reason.is_some() {"incomplete"} else {"completed"},
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "output": output,
        "output_text": output_text.join(""),
        "usage": openai_responses_usage_from_anthropic_usage(input.get("usage"))
    });
    if let Some(reason) = incomplete_reason {
        response["incomplete_details"] = json!({"reason": reason});
    }
    Ok(response)
}

fn flush_response_output_message(output: &mut Vec<Value>, content: &mut Vec<Value>) {
    if !content.is_empty() {
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "content": std::mem::take(content)
        }));
    }
}

pub fn anthropic_response_to_gemini(input: &Value) -> Result<Value, TransformError> {
    let content_blocks = input
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic response content must be an array"))?;
    if content_blocks.is_empty() {
        return Err(TransformError::new("anthropic response content is empty"));
    }
    Ok(json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": content_blocks.iter().map(anthropic_block_to_gemini_part).collect::<Vec<_>>()
            },
            "finishReason": anthropic_stop_reason_to_gemini(input.get("stop_reason").and_then(Value::as_str))
        }],
        "usageMetadata": gemini_usage_from_anthropic_usage(input.get("usage")),
        "modelVersion": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "responseId": input.get("id").and_then(Value::as_str).unwrap_or("gemini")
    }))
}

pub fn openai_responses_stream_to_anthropic(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("response.created") => vec![StreamFrame::event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": input.pointer("/response/id").and_then(Value::as_str).unwrap_or("resp"),
                    "type": "message",
                    "role": "assistant",
                    "model": input.pointer("/response/model").and_then(Value::as_str).unwrap_or_default(),
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        )],
        Some("response.output_text.delta") => input
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                )]
            })
            .unwrap_or_default(),
        Some("response.output_item.added") => {
            let Some(item) = input.get("item") else {
                return Vec::new();
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Vec::new();
            }
            let index = input
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            vec![StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("tool"),
                        "name": item.get("name").and_then(Value::as_str).unwrap_or("tool"),
                        "input": {}
                    }
                }),
            )]
        }
        Some("response.function_call_arguments.delta") => {
            let index = input
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            input
                .get("delta")
                .and_then(Value::as_str)
                .map(|partial_json| {
                    vec![StreamFrame::event(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "input_json_delta", "partial_json": partial_json}
                        }),
                    )]
                })
                .unwrap_or_default()
        }
        Some("response.output_item.done") => {
            let index = input
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            vec![StreamFrame::event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            )]
        }
        Some("response.completed" | "response.incomplete") => {
            let mut frames = Vec::new();
            let response = input.get("response").unwrap_or(input);
            let mut message_delta = json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": openai_response_to_anthropic_stream_stop(response),
                    "stop_sequence": Value::Null
                }
            });
            if let Some(usage) = response.get("usage") {
                message_delta["usage"] = anthropic_usage_from_openai_usage(Some(usage));
            }
            frames.push(StreamFrame::event("message_delta", message_delta));
            frames.push(StreamFrame::event(
                "message_stop",
                json!({"type": "message_stop"}),
            ));
            frames
        }
        _ => Vec::new(),
    }
}

pub fn openai_responses_stream_to_chat(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("response.created") => vec![openai_chat_stream_chunk(
            input.pointer("/response/id").and_then(Value::as_str),
            input.pointer("/response/model").and_then(Value::as_str),
            json!({"role": "assistant"}),
            Value::Null,
            None,
        )],
        Some("response.in_progress") => Vec::new(),
        Some("response.output_item.added") => {
            let Some(item) = input.get("item") else {
                return Vec::new();
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Vec::new();
            }
            let index = input
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            vec![openai_chat_stream_chunk(
                None,
                None,
                json!({
                    "tool_calls": [{
                        "index": index,
                        "id": item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or_else(|| json!("call_0")),
                        "type": "function",
                        "function": {
                            "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                            "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or_default()
                        }
                    }]
                }),
                Value::Null,
                None,
            )]
        }
        Some("response.output_text.delta") => input
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| {
                vec![openai_chat_stream_chunk(
                    None,
                    None,
                    json!({"content": text}),
                    Value::Null,
                    None,
                )]
            })
            .unwrap_or_default(),
        Some("response.function_call_arguments.delta") => {
            let index = input
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            input
                .get("delta")
                .and_then(Value::as_str)
                .map(|arguments| {
                    vec![openai_chat_stream_chunk(
                        None,
                        None,
                        json!({
                            "tool_calls": [{
                                "index": index,
                                "function": {"arguments": arguments}
                            }]
                        }),
                        Value::Null,
                        None,
                    )]
                })
                .unwrap_or_default()
        }
        Some("response.completed" | "response.incomplete") => {
            let response = input.get("response").unwrap_or(input);
            let finish_reason = openai_response_finish_reason_to_chat(
                response,
                response_output_has_tool_calls(response),
            );
            let usage = response
                .get("usage")
                .map(|usage| openai_chat_usage_from_responses_usage(Some(usage)));
            vec![
                openai_chat_stream_chunk(
                    response.get("id").and_then(Value::as_str),
                    response.get("model").and_then(Value::as_str),
                    json!({}),
                    finish_reason,
                    usage,
                ),
                StreamFrame::done(),
            ]
        }
        Some("response.failed") => {
            let error = input
                .pointer("/response/error")
                .or_else(|| input.get("error"));
            vec![StreamFrame::json(json!({
                "error": error.cloned().unwrap_or_else(|| json!({
                    "message": "upstream response failed",
                    "type": "upstream_error"
                }))
            }))]
        }
        Some("error") => vec![StreamFrame::json(input.clone())],
        _ => Vec::new(),
    }
}

pub fn openai_chat_stream_to_responses(input: &Value) -> Vec<StreamFrame> {
    openai_chat_stream_to_responses_with_tool_context(input, &ResponsesToolContext::default())
}

pub(crate) fn openai_chat_stream_to_responses_with_custom_tools(
    input: &Value,
    custom_tool_names: &BTreeSet<String>,
) -> Vec<StreamFrame> {
    let tool_context = ResponsesToolContext::from_custom_tool_names(custom_tool_names);
    openai_chat_stream_to_responses_with_tool_context(input, &tool_context)
}

pub(crate) fn openai_chat_stream_to_responses_with_tool_context(
    input: &Value,
    tool_context: &ResponsesToolContext,
) -> Vec<StreamFrame> {
    if let Some(error) = input.get("error") {
        return vec![StreamFrame::json(json!({
            "type": "error",
            "error": error
        }))];
    }
    let Some(choice) = input
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        if let Some(usage) = input.get("usage") {
            return vec![StreamFrame::json(json!({
                "type": "response.completed",
                "response": {"usage": openai_responses_usage_from_chat_usage(Some(usage))}
            }))];
        }
        return Vec::new();
    };

    let mut frames = Vec::new();
    if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
        frames.push(StreamFrame::json(json!({
            "type": "response.output_text.delta",
            "delta": text
        })));
    }
    if let Some(tool_calls) = choice
        .pointer("/delta/tool_calls")
        .and_then(Value::as_array)
    {
        for tool_call in tool_calls {
            let index = tool_call.get("index").cloned().unwrap_or_else(|| json!(0));
            let name = tool_call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("call_0");
            if tool_call.get("id").is_some() || !name.is_empty() {
                let is_custom = tool_context.is_custom_tool_chat_name(name);
                let item_id = tool_context.response_item_id(call_id, name);
                let mut item =
                    tool_context.response_item(&item_id, "in_progress", call_id, name, "");
                if is_custom {
                    item["cc_switch_custom_bridge"] = Value::Bool(true);
                }
                frames.push(StreamFrame::json(json!({
                    "type": "response.output_item.added",
                    "output_index": index,
                    "item": item
                })));
            }
            if let Some(arguments) = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
            {
                frames.push(StreamFrame::json(json!({
                    "type": "response.function_call_arguments.delta",
                    "output_index": index,
                    "delta": arguments
                })));
            }
        }
    }
    if choice.get("finish_reason").is_some() || input.get("usage").is_some() {
        frames.push(StreamFrame::json(json!({
            "type": "response.completed",
            "response": {
                "id": response_id_from_chat_id(input.get("id").and_then(Value::as_str)),
                "status": openai_chat_finish_reason_to_response_status(choice.get("finish_reason").and_then(Value::as_str)),
                "model": input.get("model").cloned().unwrap_or_else(|| json!("")),
                "usage": openai_responses_usage_from_chat_usage(input.get("usage"))
            }
        })));
        frames.push(StreamFrame::done());
    }
    frames
}

pub fn openai_chat_stream_to_anthropic(input: &Value) -> Vec<StreamFrame> {
    let Some(choice) = input
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Vec::new();
    };
    if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
        return vec![StreamFrame::event(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": text}
            }),
        )];
    }
    if let Some(tool_calls) = choice
        .pointer("/delta/tool_calls")
        .and_then(Value::as_array)
    {
        let mut frames = Vec::new();
        for tool_call in tool_calls {
            if tool_call.get("id").is_some() || tool_call.pointer("/function/name").is_some() {
                frames.push(openai_chat_tool_delta_to_anthropic_start(tool_call));
            }
            if let Some(arguments) = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .filter(|arguments| !arguments.is_empty())
            {
                let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                frames.push(StreamFrame::event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "input_json_delta", "partial_json": arguments}
                    }),
                ));
            }
        }
        if !frames.is_empty() {
            return frames;
        }
    }
    if choice.get("finish_reason").is_some() {
        return vec![StreamFrame::event(
            "message_stop",
            json!({"type": "message_stop"}),
        )];
    }
    Vec::new()
}

pub fn gemini_stream_to_anthropic(input: &Value) -> Vec<StreamFrame> {
    let mut frames = Vec::new();
    if let Some(parts) = input
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                frames.push(StreamFrame::event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                ));
            }
            if let Some(function_call) = part
                .get("functionCall")
                .or_else(|| part.get("function_call"))
            {
                frames.extend(gemini_function_call_to_anthropic_frames(
                    function_call,
                    index as u64,
                ));
            }
        }
    }
    if let Some(usage) = input.get("usageMetadata") {
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": Value::Null},
                "usage": anthropic_usage_from_gemini_usage(Some(usage))
            }),
        ));
    }
    frames
}

pub fn anthropic_stream_to_openai_responses(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let Some(block) = input.get("content_block") else {
                return Vec::new();
            };
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                return Vec::new();
            }
            vec![StreamFrame::json(json!({
                "type": "response.output_item.added",
                "output_index": input.get("index").cloned().unwrap_or_else(|| json!(0)),
                "item": {
                    "type": "function_call",
                    "call_id": block.get("id").and_then(Value::as_str).unwrap_or("tool"),
                    "name": block.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "arguments": ""
                }
            }))]
        }
        Some("content_block_delta") => input
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "type": "response.output_text.delta",
                    "delta": text
                }))]
            })
            .or_else(|| {
                input
                    .pointer("/delta/partial_json")
                    .and_then(Value::as_str)
                    .map(|partial_json| {
                        vec![StreamFrame::json(json!({
                            "type": "response.function_call_arguments.delta",
                            "output_index": input.get("index").cloned().unwrap_or_else(|| json!(0)),
                            "delta": partial_json
                        }))]
                    })
            })
            .unwrap_or_default(),
        Some("content_block_stop") => vec![StreamFrame::json(json!({
            "type": "response.output_item.done",
            "output_index": input.get("index").cloned().unwrap_or_else(|| json!(0))
        }))],
        Some("message_delta") => {
            let usage = input
                .get("usage")
                .map(|usage| openai_responses_usage_from_anthropic_usage(Some(usage)));
            vec![StreamFrame::json(json!({
                "type": "response.completed",
                "response": {"usage": usage.unwrap_or_else(|| json!({}))}
            }))]
        }
        Some("message_stop") => vec![StreamFrame::done()],
        _ => Vec::new(),
    }
}

pub fn anthropic_stream_to_openai_chat(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let Some(block) = input.get("content_block") else {
                return Vec::new();
            };
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                return Vec::new();
            }
            vec![StreamFrame::json(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": input.get("index").cloned().unwrap_or_else(|| json!(0)),
                            "id": block.get("id").and_then(Value::as_str).unwrap_or("tool"),
                            "type": "function",
                            "function": {
                                "name": block.get("name").and_then(Value::as_str).unwrap_or("tool"),
                                "arguments": ""
                            }
                        }]
                    },
                    "finish_reason": Value::Null
                }]
            }))]
        }
        Some("content_block_delta") => input
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": Value::Null}]
                }))]
            })
            .or_else(|| {
                input
                    .pointer("/delta/partial_json")
                    .and_then(Value::as_str)
                    .map(|partial_json| {
                        vec![StreamFrame::json(json!({
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": input.get("index").cloned().unwrap_or_else(|| json!(0)),
                                        "function": {"arguments": partial_json}
                                    }]
                                },
                                "finish_reason": Value::Null
                            }]
                        }))]
                    })
            })
            .unwrap_or_default(),
        Some("message_delta") => {
            let mut frames = Vec::new();
            if let Some(reason) = input.pointer("/delta/stop_reason").and_then(Value::as_str) {
                frames.push(StreamFrame::json(json!({
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": anthropic_stop_reason_to_openai(Some(reason))
                    }]
                })));
            }
            if let Some(usage) = input.get("usage") {
                frames.push(StreamFrame::json(json!({
                    "choices": [],
                    "usage": openai_usage_from_anthropic_usage(Some(usage))
                })));
            }
            frames
        }
        Some("message_stop") => vec![
            StreamFrame::json(json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            })),
            StreamFrame::done(),
        ],
        _ => Vec::new(),
    }
}

pub fn anthropic_stream_to_gemini(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("content_block_delta") => input
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "candidates": [{
                        "content": {"role": "model", "parts": [{"text": text}]}
                    }]
                }))]
            })
            .unwrap_or_default(),
        Some("message_delta") => input
            .get("usage")
            .map(|usage| {
                vec![StreamFrame::json(json!({
                    "usageMetadata": gemini_usage_from_anthropic_usage(Some(usage))
                }))]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn openai_responses_stream_to_gemini(input: &Value) -> Vec<StreamFrame> {
    match input.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => input
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "candidates": [{
                        "content": {"role": "model", "parts": [{"text": text}]}
                    }]
                }))]
            })
            .unwrap_or_default(),
        Some("response.completed") => input
            .pointer("/response/usage")
            .map(|usage| {
                vec![StreamFrame::json(json!({
                    "usageMetadata": gemini_usage_from_openai_usage(Some(usage))
                }))]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn openai_chat_stream_to_gemini(input: &Value) -> Vec<StreamFrame> {
    let mut frames = Vec::new();
    if let Some(choice) = input
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    {
        if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
            frames.push(StreamFrame::json(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": text}]}
                }]
            })));
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            frames.push(StreamFrame::json(json!({
                "candidates": [{
                    "finishReason": openai_finish_reason_to_gemini(reason)
                }]
            })));
        }
    }
    if let Some(usage) = input.get("usage") {
        frames.push(StreamFrame::json(json!({
            "usageMetadata": gemini_usage_from_openai_usage(Some(usage))
        })));
    }
    frames
}

pub fn gemini_stream_to_openai_responses(input: &Value) -> Vec<StreamFrame> {
    let mut frames = Vec::new();
    if let Some(parts) = input
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                frames.push(StreamFrame::json(json!({
                    "type": "response.output_text.delta",
                    "delta": text
                })));
            }
        }
    }
    if let Some(usage) = input.get("usageMetadata") {
        frames.push(StreamFrame::json(json!({
            "type": "response.completed",
            "response": {"usage": openai_usage_from_gemini_usage(Some(usage))}
        })));
    }
    frames
}

pub fn gemini_stream_to_openai_chat(input: &Value) -> Vec<StreamFrame> {
    let mut frames = Vec::new();
    if let Some(parts) = input
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                frames.push(StreamFrame::json(json!({
                    "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": Value::Null}]
                })));
            }
        }
    }
    if let Some(usage) = input.get("usageMetadata") {
        frames.push(StreamFrame::json(json!({
            "choices": [],
            "usage": openai_usage_from_gemini_usage(Some(usage))
        })));
    }
    frames
}

pub fn anthropic_to_openai_chat(input: &Value) -> Result<Value, TransformError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic messages must be an array"))?;
    let mut output_messages = Vec::new();
    if let Some(system) = input.get("system").and_then(Value::as_str) {
        output_messages.push(json!({"role": "system", "content": system}));
    }
    for message in messages {
        output_messages.extend(anthropic_message_to_openai_chat(message));
    }

    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    output.insert("messages".to_string(), Value::Array(output_messages));
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(reasoning) = anthropic_reasoning_to_openai(input) {
        output.insert("reasoning".to_string(), reasoning);
    }
    if let Some(tools) = anthropic_tools_to_openai(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }

    Ok(Value::Object(output))
}

pub fn anthropic_to_openai_responses(input: &Value) -> Result<Value, TransformError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic messages must be an array"))?;
    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(reasoning) = anthropic_reasoning_to_openai(input) {
        output.insert("reasoning".to_string(), reasoning);
    }
    if let Some(tools) = anthropic_tools_to_openai(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }
    output.insert(
        "input".to_string(),
        Value::Array(
            messages
                .iter()
                .flat_map(anthropic_message_to_openai_response_items)
                .collect(),
        ),
    );

    Ok(Value::Object(output))
}

pub fn anthropic_to_gemini_native(input: &Value) -> Result<Value, TransformError> {
    let messages = input
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic messages must be an array"))?;
    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    let gemini_three = input
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.to_ascii_lowercase().contains("gemini-3"));

    if let Some(system) = input.get("system").and_then(Value::as_str) {
        output.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system}]}),
        );
    }
    output.insert(
        "contents".to_string(),
        Value::Array(
            messages
                .iter()
                .map(|message| anthropic_message_to_gemini_content(message, gemini_three))
                .collect(),
        ),
    );
    if let Some(tools) = anthropic_tools_to_gemini(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }
    if let Some(metadata) = input.get("metadata") {
        if let Some(config) = metadata.get("geminiGenerationConfig") {
            output.insert("generationConfig".to_string(), config.clone());
        }
        if let Some(safety) = metadata.get("geminiSafetySettings") {
            output.insert("safetySettings".to_string(), safety.clone());
        }
    }

    Ok(Value::Object(output))
}

fn openai_chat_messages_to_response_input(messages: &[Value]) -> (Vec<String>, Vec<Value>) {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    let mut pending_media = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if !matches!(role, "tool" | "function") {
            flush_responses_tool_media(&mut input, &mut pending_media);
        }
        match role {
            "system" | "developer" => {
                if let Some(text) = openai_chat_content_to_plain_text(message.get("content")) {
                    let text = text.trim();
                    if !text.is_empty() {
                        instructions.push(text.to_string());
                    }
                }
            }
            "tool" | "function" => {
                let call_id = message
                    .get("tool_call_id")
                    .or_else(|| message.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let output = message.get("content").cloned().unwrap_or(Value::Null);
                let extraction = extract_tool_media(&output, ToolMediaScope::ResponsesNative);
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": extraction
                        .as_ref()
                        .map(|value| sanitized_tool_text(&value.sanitized))
                        .unwrap_or_else(|| openai_chat_tool_output_to_string(message.get("content")))
                }));
                if let Some(extraction) = extraction {
                    queue_responses_tool_media(&mut pending_media, call_id, &extraction.media);
                }
            }
            "assistant" => {
                if let Some(reasoning) = openai_chat_message_reasoning_text(message) {
                    input.push(json!({
                        "type": "reasoning",
                        "summary": [{
                            "type": "summary_text",
                            "text": reasoning
                        }]
                    }));
                }
                let content =
                    openai_chat_content_to_responses_content("assistant", message.get("content"));
                if !content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for (index, tool_call) in tool_calls.iter().enumerate() {
                        if let Some(item) = openai_chat_tool_call_to_response_item(tool_call, index)
                        {
                            input.push(item);
                        }
                    }
                }
                if let Some(function_call) = message.get("function_call") {
                    if let Some(item) =
                        openai_chat_legacy_function_call_to_response_item(function_call)
                    {
                        input.push(item);
                    }
                }
            }
            _ => {
                let content =
                    openai_chat_content_to_responses_content("user", message.get("content"));
                input.push(json!({
                    "role": "user",
                    "content": content
                }));
            }
        }
    }
    flush_responses_tool_media(&mut input, &mut pending_media);

    (instructions, input)
}

fn queue_responses_tool_media(pending: &mut Vec<Value>, call_id: &str, media: &[ToolMediaPart]) {
    let parts = media
        .iter()
        .filter_map(ToolMediaPart::to_responses_part)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return;
    }
    pending.push(json!({
        "type": "input_text",
        "text": format!("[cc-switch-server: media output of tool call {call_id}]")
    }));
    pending.extend(parts);
}

fn flush_responses_tool_media(input: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if !pending.is_empty() {
        input.push(json!({
            "role": "user",
            "content": std::mem::take(pending)
        }));
    }
}

fn openai_chat_content_to_responses_content(role: &str, content: Option<&Value>) -> Vec<Value> {
    let text_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    let Some(content) = content else {
        return Vec::new();
    };
    match content {
        Value::String(text) if !text.is_empty() => vec![json!({
            "type": text_type,
            "text": text
        })],
        Value::String(_) | Value::Null => Vec::new(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| openai_chat_content_part_to_response_part(role, text_type, part))
            .collect(),
        other => vec![json!({
            "type": text_type,
            "text": other.to_string()
        })],
    }
}

fn openai_chat_content_part_to_response_part(
    role: &str,
    text_type: &str,
    part: &Value,
) -> Option<Value> {
    match part.get("type").and_then(Value::as_str) {
        Some("text") | Some("input_text") | Some("output_text") => {
            let text = part.get("text").and_then(Value::as_str)?;
            if text.is_empty() {
                return None;
            }
            let mut output = json!({"type": text_type, "text": text});
            copy_cache_control(part, &mut output);
            Some(output)
        }
        Some("refusal") if role == "assistant" => part
            .get("refusal")
            .or_else(|| part.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| json!({"type": "refusal", "refusal": text})),
        Some("image_url") if role != "assistant" => {
            let url = part
                .pointer("/image_url/url")
                .or_else(|| part.get("image_url"))
                .and_then(Value::as_str)?;
            (!url.is_empty()).then(|| json!({"type": "input_image", "image_url": url}))
        }
        Some("input_image") if role != "assistant" => {
            let url = part
                .get("image_url")
                .or_else(|| part.get("url"))
                .and_then(Value::as_str)?;
            (!url.is_empty()).then(|| json!({"type": "input_image", "image_url": url}))
        }
        Some("file") if role != "assistant" => {
            let file = part.get("file")?;
            let mut output = Map::new();
            output.insert("type".to_string(), json!("input_file"));
            for key in ["file_id", "file_data", "filename"] {
                if let Some(value) = file.get(key) {
                    output.insert(key.to_string(), value.clone());
                }
            }
            (output.len() > 1).then_some(Value::Object(output))
        }
        Some("input_audio") if role != "assistant" => part.get("input_audio").map(|audio| {
            json!({
                "type": "input_audio",
                "input_audio": audio
            })
        }),
        _ => part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| json!({"type": text_type, "text": text})),
    }
}

fn openai_chat_content_to_plain_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.as_str())
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn openai_chat_tool_output_to_string(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(_)) => openai_chat_content_to_plain_text(content).unwrap_or_default(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn openai_chat_message_reasoning_text(message: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning_text"] {
        if let Some(value) = message.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return Some(value.to_string());
            }
        }
    }
    if let Some(value) = message.get("reasoning").and_then(Value::as_str) {
        if !value.trim().is_empty() {
            return Some(value.to_string());
        }
    }
    message
        .get("reasoning")
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

pub(super) fn openai_chat_choice_payload(choice: &Value) -> &Value {
    let delta = choice.get("delta");
    if delta
        .and_then(Value::as_object)
        .is_some_and(|delta| !delta.is_empty())
    {
        return delta.expect("non-empty delta exists");
    }
    choice.get("message").or(delta).unwrap_or(choice)
}

pub(super) fn openai_chat_visible_text_fragments(payload: &Value) -> Vec<&str> {
    let mut fragments = Vec::new();
    match payload.get("content") {
        Some(Value::String(text)) if !text.is_empty() => fragments.push(text.as_str()),
        Some(Value::Array(parts)) => fragments.extend(parts.iter().filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("refusal"))
                .and_then(Value::as_str)
                .or_else(|| part.as_str())
                .filter(|text| !text.is_empty())
        })),
        _ => {}
    }
    if let Some(refusal) = payload
        .get("refusal")
        .and_then(Value::as_str)
        .filter(|refusal| !refusal.is_empty())
    {
        fragments.push(refusal);
    }
    fragments
}

pub(super) fn openai_chat_legacy_tool_delta(payload: &Value) -> Option<Value> {
    let function_call = payload
        .get("function_call")
        .filter(|function_call| function_call.is_object() && !function_call.is_null())?;
    if function_call.get("name").is_none() && function_call.get("arguments").is_none() {
        return None;
    }
    Some(json!({
        "index": 0,
        "id": function_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .unwrap_or("call_0"),
        "type": "function",
        "function": function_call
    }))
}

fn openai_chat_tools_to_responses(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    let response_tools = tools
        .iter()
        .filter_map(openai_chat_tool_to_response_tool)
        .collect::<Vec<_>>();
    (!response_tools.is_empty()).then_some(Value::Array(response_tools))
}

fn openai_chat_tool_to_response_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    let function = tool.get("function").unwrap_or(tool);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())?;
    let mut output = json!({
        "type": "function",
        "name": name,
        "description": function.get("description").cloned().unwrap_or(Value::Null),
        "parameters": normalize_function_parameters(function.get("parameters"))
    });
    if let Some(strict) = function.get("strict").or_else(|| tool.get("strict")) {
        output["strict"] = strict.clone();
    }
    Some(output)
}

fn openai_chat_tool_choice_to_responses(tool_choice: &Value) -> Value {
    match tool_choice {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("function") => {
            let name = object
                .get("function")
                .and_then(|function| function.get("name"))
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({"type": "function", "name": name})
        }
        _ => tool_choice.clone(),
    }
}

fn openai_chat_tool_call_to_response_item(tool_call: &Value, index: usize) -> Option<Value> {
    openai_chat_tool_call_to_response_item_with_tool_context(
        tool_call,
        index,
        &ResponsesToolContext::default(),
    )
}

fn openai_chat_tool_call_to_response_item_with_custom_tools(
    tool_call: &Value,
    index: usize,
    custom_tool_names: &BTreeSet<String>,
) -> Option<Value> {
    let tool_context = ResponsesToolContext::from_custom_tool_names(custom_tool_names);
    openai_chat_tool_call_to_response_item_with_tool_context(tool_call, index, &tool_context)
}

fn openai_chat_tool_call_to_response_item_with_tool_context(
    tool_call: &Value,
    index: usize,
    tool_context: &ResponsesToolContext,
) -> Option<Value> {
    let function = tool_call.get("function")?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("call_{index}"));
    let arguments = openai_tool_arguments_to_string(function.get("arguments"));
    let item_id = tool_context.response_item_id(&call_id, name);
    Some(tool_context.response_item(&item_id, "completed", &call_id, name, &arguments))
}

pub(super) fn unwrap_custom_tool_input(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

fn openai_chat_legacy_function_call_to_response_item(function_call: &Value) -> Option<Value> {
    openai_chat_legacy_function_call_to_response_item_with_context(
        function_call,
        &ResponsesToolContext::default(),
    )
}

fn openai_chat_legacy_function_call_to_response_item_with_context(
    function_call: &Value,
    tool_context: &ResponsesToolContext,
) -> Option<Value> {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    let call_id = function_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(name);
    let arguments = openai_tool_arguments_to_string(function_call.get("arguments"));
    let item_id = tool_context.response_item_id(call_id, name);
    Some(tool_context.response_item(&item_id, "completed", call_id, name, &arguments))
}

fn openai_tool_arguments_to_string(arguments: Option<&Value>) -> String {
    match arguments {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| text.clone()),
        Some(value) if !value.is_null() => value.to_string(),
        _ => "{}".to_string(),
    }
}

fn response_instruction_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.is_empty()).then_some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.as_str())
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn append_response_input_item_to_chat_messages(
    item: &Value,
    tool_context: &ResponsesToolContext,
    messages: &mut Vec<Value>,
    pending_media: &mut Vec<Value>,
    pending_reasoning: &mut Vec<String>,
    last_assistant_index: &mut Option<usize>,
) {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => append_response_tool_call_to_chat(
            item,
            tool_context,
            messages,
            pending_media,
            pending_reasoning,
            last_assistant_index,
        ),
        Some("function_call_output") => {
            attach_pending_reasoning_to_previous_assistant(
                messages,
                *last_assistant_index,
                pending_reasoning,
            );
            append_response_tool_output_to_chat(item, messages, pending_media, false)
        }
        Some("custom_tool_call") => append_response_tool_call_to_chat(
            item,
            tool_context,
            messages,
            pending_media,
            pending_reasoning,
            last_assistant_index,
        ),
        Some("custom_tool_call_output") => {
            attach_pending_reasoning_to_previous_assistant(
                messages,
                *last_assistant_index,
                pending_reasoning,
            );
            append_response_tool_output_to_chat(item, messages, pending_media, true)
        }
        Some("tool_search_call") => append_response_tool_call_to_chat(
            item,
            tool_context,
            messages,
            pending_media,
            pending_reasoning,
            last_assistant_index,
        ),
        Some("tool_search_output") => {
            attach_pending_reasoning_to_previous_assistant(
                messages,
                *last_assistant_index,
                pending_reasoning,
            );
            append_response_tool_output_to_chat(item, messages, pending_media, true)
        }
        Some("additional_tools") => {}
        Some("message") => {
            flush_chat_media(messages, pending_media);
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let chat_role = response_role_to_chat(role);
            let mut message = json!({
                "role": chat_role,
                "content": response_content_to_chat_content(role, item.get("content"))
            });
            if chat_role == "assistant" {
                append_pending_reasoning(
                    pending_reasoning,
                    openai_chat_message_reasoning_text(item),
                );
                attach_pending_reasoning_to_assistant(&mut message, pending_reasoning);
                *last_assistant_index = Some(messages.len());
            } else {
                attach_pending_reasoning_to_previous_assistant(
                    messages,
                    *last_assistant_index,
                    pending_reasoning,
                );
                *last_assistant_index = None;
            }
            messages.push(message);
        }
        Some("reasoning") => {
            flush_chat_media(messages, pending_media);
            append_pending_reasoning(pending_reasoning, Some(reasoning_summary_text(item)));
        }
        _ => {
            flush_chat_media(messages, pending_media);
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            if item.get("content").is_some() {
                let chat_role = response_role_to_chat(role);
                let mut message = json!({
                    "role": chat_role,
                    "content": response_content_to_chat_content(role, item.get("content"))
                });
                if chat_role == "assistant" {
                    attach_pending_reasoning_to_assistant(&mut message, pending_reasoning);
                    *last_assistant_index = Some(messages.len());
                } else {
                    attach_pending_reasoning_to_previous_assistant(
                        messages,
                        *last_assistant_index,
                        pending_reasoning,
                    );
                    *last_assistant_index = None;
                }
                messages.push(message);
            }
        }
    }
}

fn append_response_tool_call_to_chat(
    item: &Value,
    tool_context: &ResponsesToolContext,
    messages: &mut Vec<Value>,
    pending_media: &mut Vec<Value>,
    pending_reasoning: &mut Vec<String>,
    last_assistant_index: &mut Option<usize>,
) {
    flush_chat_media(messages, pending_media);
    append_pending_reasoning(pending_reasoning, openai_chat_message_reasoning_text(item));
    let Some(tool_call) = openai_response_function_call_to_chat(item, tool_context) else {
        return;
    };
    if let Some(message) = messages.last_mut().filter(|message| {
        message.get("role").and_then(Value::as_str) == Some("assistant")
            && message.get("content").is_some_and(Value::is_null)
            && message.get("tool_calls").is_some_and(Value::is_array)
    }) {
        if let Some(tool_calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            tool_calls.push(tool_call);
        }
        attach_pending_reasoning_to_assistant(message, pending_reasoning);
        *last_assistant_index = messages.len().checked_sub(1);
        return;
    }
    let mut message = json!({
        "role": "assistant",
        "content": Value::Null,
        "tool_calls": [tool_call]
    });
    attach_pending_reasoning_to_assistant(&mut message, pending_reasoning);
    *last_assistant_index = Some(messages.len());
    messages.push(message);
}

fn append_pending_reasoning(pending: &mut Vec<String>, reasoning: Option<String>) {
    let Some(reasoning) = reasoning else {
        return;
    };
    let reasoning = reasoning.trim();
    if reasoning.is_empty() {
        return;
    }
    if !pending.iter().any(|existing| existing == reasoning) {
        pending.push(reasoning.to_string());
    }
}

fn take_pending_reasoning(pending: &mut Vec<String>) -> Option<String> {
    if pending.is_empty() {
        None
    } else {
        Some(std::mem::take(pending).join("\n\n"))
    }
}

fn attach_pending_reasoning_to_assistant(message: &mut Value, pending: &mut Vec<String>) {
    let Some(reasoning) = take_pending_reasoning(pending) else {
        return;
    };
    append_reasoning_to_chat_message(message, &reasoning);
}

fn attach_pending_reasoning_to_previous_assistant(
    messages: &mut [Value],
    last_assistant_index: Option<usize>,
    pending: &mut Vec<String>,
) {
    let Some(reasoning) = take_pending_reasoning(pending) else {
        return;
    };
    let Some(message) = last_assistant_index.and_then(|index| messages.get_mut(index)) else {
        return;
    };
    if message.get("role").and_then(Value::as_str) == Some("assistant") {
        append_reasoning_to_chat_message(message, &reasoning);
    }
}

fn append_reasoning_to_chat_message(message: &mut Value, reasoning: &str) {
    let reasoning = reasoning.trim();
    if reasoning.is_empty() {
        return;
    }
    let Some(message) = message.as_object_mut() else {
        return;
    };
    match message
        .get_mut("reasoning_content")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    {
        Some(existing) if !existing.trim().is_empty() => {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(format!("{existing}\n\n{reasoning}")),
            );
        }
        _ => {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning.to_string()),
            );
        }
    }
}

fn append_response_tool_output_to_chat(
    item: &Value,
    messages: &mut Vec<Value>,
    pending_media: &mut Vec<Value>,
    legacy_text_output: bool,
) {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let output = item.get("output").cloned().unwrap_or_else(|| json!(""));
    let extraction = extract_tool_media(&output, ToolMediaScope::ChatNative);
    let content = extraction
        .as_ref()
        .map(|value| json!(sanitized_tool_text(&value.sanitized)))
        .unwrap_or_else(|| {
            if legacy_text_output {
                json!(response_tool_output_text(Some(&output)))
            } else {
                output.clone()
            }
        });
    messages.push(json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": content
    }));
    if let Some(extraction) = extraction {
        queue_chat_media(pending_media, call_id, &extraction.media);
    }
}

fn response_content_to_chat_content(role: &str, content: Option<&Value>) -> Value {
    let Some(content) = content else {
        return Value::Null;
    };
    match content {
        Value::String(text) => json!(text),
        Value::Array(parts) => {
            let chat_parts = parts
                .iter()
                .filter_map(|part| response_content_part_to_chat_part(role, part))
                .collect::<Vec<_>>();
            if chat_parts.len() == 1 {
                if let Some(text) = chat_parts[0].get("text").and_then(Value::as_str) {
                    return json!(text);
                }
            }
            Value::Array(chat_parts)
        }
        Value::Null => Value::Null,
        other => json!(other.to_string()),
    }
}

fn response_content_part_to_chat_part(role: &str, part: &Value) -> Option<Value> {
    match part.get("type").and_then(Value::as_str) {
        Some("input_text") | Some("output_text") | Some("text") => part
            .get("text")
            .and_then(Value::as_str)
            .map(|text| json!({"type": "text", "text": text})),
        Some("refusal") if role == "assistant" => part
            .get("refusal")
            .or_else(|| part.get("text"))
            .and_then(Value::as_str)
            .map(|text| json!({"type": "refusal", "refusal": text})),
        Some("input_image") => part
            .get("image_url")
            .or_else(|| part.get("url"))
            .and_then(Value::as_str)
            .map(|url| json!({"type": "image_url", "image_url": {"url": url}})),
        Some("input_file") => {
            let mut file = Map::new();
            for key in ["file_id", "file_data", "filename"] {
                if let Some(value) = part.get(key) {
                    file.insert(key.to_string(), value.clone());
                }
            }
            (!file.is_empty()).then_some(json!({"type": "file", "file": file}))
        }
        _ => None,
    }
}

fn response_role_to_chat(role: &str) -> &'static str {
    match role {
        "assistant" => "assistant",
        "system" | "developer" => "system",
        "tool" => "tool",
        _ => "user",
    }
}

fn response_tool_output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

pub(crate) fn responses_custom_tool_names(input: &Value) -> BTreeSet<String> {
    response_tools_from_request(input)
        .into_iter()
        .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("custom"))
        .filter_map(response_tool_name)
        .collect()
}

pub(crate) fn responses_custom_tool_names_from_bytes(input: &[u8]) -> BTreeSet<String> {
    serde_json::from_slice::<Value>(input)
        .ok()
        .map(|value| responses_custom_tool_names(&value))
        .unwrap_or_default()
}

fn response_tools_from_request(input: &Value) -> Vec<&Value> {
    let mut tools = input
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if let Some(history) = input.get("input") {
        collect_response_tools_from_history(history, &mut tools);
    }
    tools
}

fn collect_response_tools_from_history<'a>(value: &'a Value, tools: &mut Vec<&'a Value>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_response_tools_from_history(item, tools);
            }
        }
        Value::Object(object) => {
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("additional_tools" | "tool_search_output")
            ) {
                tools.extend(
                    object
                        .get("tools")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten(),
                );
            }
            for nested in object.values() {
                collect_response_tools_from_history(nested, tools);
            }
        }
        _ => {}
    }
}

fn validate_response_tool_type_collisions(input: &Value) -> Result<(), TransformError> {
    let mut custom_names = BTreeSet::new();
    let mut function_names = BTreeSet::new();
    for tool in response_tools_from_request(input) {
        let Some(name) = response_tool_name(tool) else {
            continue;
        };
        match tool.get("type").and_then(Value::as_str) {
            Some("custom") => {
                custom_names.insert(name);
            }
            None | Some("") | Some("function") => {
                function_names.insert(name);
            }
            _ => {}
        }
    }
    if let Some(name) = custom_names.intersection(&function_names).next() {
        return Err(TransformError::new(format!(
            "Responses custom tool '{name}' conflicts with a function tool of the same name; rename one of the tools"
        )));
    }
    Ok(())
}

fn openai_response_tools_to_chat(
    input: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Option<Value>, TransformError> {
    let tools = response_tools_from_request(input);
    let named_tools = tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.get("type").and_then(Value::as_str),
                None | Some("") | Some("function") | Some("custom")
            )
        })
        .filter_map(|tool| response_tool_name(tool))
        .collect::<BTreeSet<_>>();
    let has_tool_search = tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some("tool_search"));
    if has_tool_search && named_tools.contains("tool_search") {
        return Err(TransformError::new(
            "built-in tool_search conflicts with a declared tool named tool_search; rename the custom tool",
        ));
    }

    let mut chat_tools = Vec::new();
    let mut seen = BTreeSet::new();
    for tool in tools {
        for chat_tool in openai_response_tool_to_chat_tools(tool, tool_context) {
            let name = chat_tool
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if seen.insert(name) {
                chat_tools.push(chat_tool);
            }
        }
    }
    Ok((!chat_tools.is_empty()).then_some(Value::Array(chat_tools)))
}

fn openai_response_tool_to_chat_tools(
    tool: &Value,
    tool_context: &ResponsesToolContext,
) -> Vec<Value> {
    if let Some(name) = tool.as_str().map(str::trim).filter(|name| !name.is_empty()) {
        return vec![openai_response_custom_tool_to_chat(name, &Value::Null)];
    }
    match tool
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function")
    {
        "namespace" => {
            let namespace = tool.get("name").and_then(Value::as_str).unwrap_or_default();
            return tool
                .get("tools")
                .or_else(|| tool.get("children"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|child| {
                    let child_name = response_tool_name(child)?;
                    let name =
                        tool_context.chat_name_for_response_function(&child_name, Some(namespace));
                    openai_response_function_tool_to_chat(child, &name)
                })
                .collect();
        }
        "custom" => {
            let Some(name) = response_tool_name(tool) else {
                return Vec::new();
            };
            return vec![openai_response_custom_tool_to_chat(
                &name,
                tool.get("description").unwrap_or(&Value::Null),
            )];
        }
        "tool_search" => {
            return vec![json!({
                "type": "function",
                "function": {
                    "name": "tool_search",
                    "description": "Search and load Codex tools or connectors for the current task.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "limit": {"type": "integer"}
                        },
                        "required": ["query"]
                    }
                }
            })];
        }
        "function" | "" => {}
        _ => return Vec::new(),
    }
    response_tool_name(tool)
        .and_then(|name| openai_response_function_tool_to_chat(tool, &name))
        .into_iter()
        .collect()
}

fn openai_response_custom_tool_to_chat(name: &str, description: &Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"]
            }
        }
    })
}

fn response_tool_name(tool: &Value) -> Option<String> {
    tool.get("name")
        .or_else(|| tool.pointer("/function/name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn openai_response_function_tool_to_chat(tool: &Value, name: &str) -> Option<Value> {
    if name.is_empty() {
        return None;
    }
    let mut function = Map::new();
    function.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    function.insert(
        "parameters".to_string(),
        normalize_function_parameters(
            tool.get("parameters")
                .or_else(|| tool.get("parametersJsonSchema"))
                .or_else(|| tool.get("input_schema"))
                .or_else(|| tool.pointer("/function/parameters")),
        ),
    );
    if let Some(strict) = tool.get("strict") {
        function.insert("strict".to_string(), strict.clone());
    }
    Some(json!({"type": "function", "function": function}))
}

fn openai_response_tool_choice_to_chat(
    tool_choice: &Value,
    tool_context: &ResponsesToolContext,
) -> Value {
    match tool_choice {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("function") => {
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let namespace = object.get("namespace").and_then(Value::as_str);
            json!({
                "type": "function",
                "function": {
                    "name": tool_context.chat_name_for_response_function(name, namespace)
                }
            })
        }
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("custom") => {
            json!({
                "type": "function",
                "function": {
                    "name": object.get("name").and_then(Value::as_str).unwrap_or_default()
                }
            })
        }
        Value::Object(object)
            if object.get("type").and_then(Value::as_str) == Some("tool_search") =>
        {
            json!({
                "type": "function",
                "function": {"name": "tool_search"}
            })
        }
        _ => tool_choice.clone(),
    }
}

fn openai_response_function_call_to_chat(
    item: &Value,
    tool_context: &ResponsesToolContext,
) -> Option<Value> {
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("call_0");
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    let (name, arguments) = match item_type {
        "custom_tool_call" => (
            item.get("name").and_then(Value::as_str)?.to_string(),
            json!({
                "input": item.get("input").cloned().unwrap_or_else(|| json!(""))
            })
            .to_string(),
        ),
        "tool_search_call" => (
            TOOL_SEARCH_PROXY_NAME.to_string(),
            item.get("arguments")
                .cloned()
                .or_else(|| item.get("query").map(|query| json!({"query": query})))
                .map(|value| match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "{}".to_string()),
        ),
        _ => {
            let name = item.get("name").and_then(Value::as_str)?;
            let namespace = item.get("namespace").and_then(Value::as_str);
            (
                tool_context.chat_name_for_response_function(name, namespace),
                openai_tool_arguments_to_string(item.get("arguments")),
            )
        }
    };
    Some(json!({
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    }))
}

pub(super) fn openai_response_finish_reason_to_chat(
    response: &Value,
    has_tool_calls: bool,
) -> Value {
    match response.get("status").and_then(Value::as_str) {
        Some("incomplete")
            if response
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
                == Some("content_filter") =>
        {
            json!("content_filter")
        }
        Some("incomplete") => json!("length"),
        Some("failed") | Some("cancelled") => json!("stop"),
        _ if has_tool_calls => json!("tool_calls"),
        _ => json!("stop"),
    }
}

fn response_output_has_tool_calls(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call") | Some("custom_tool_call") | Some("tool_search_call")
                )
            })
        })
}

fn openai_chat_finish_reason_to_response_status(finish_reason: Option<&str>) -> &'static str {
    match finish_reason {
        Some("length") => "incomplete",
        Some("content_filter") => "failed",
        _ => "completed",
    }
}

fn chat_id_from_response_id(id: Option<&str>) -> String {
    match id {
        Some(value) if value.starts_with("chatcmpl_") => value.to_string(),
        Some(value) if value.starts_with("resp_") => {
            format!("chatcmpl_{}", value.trim_start_matches("resp_"))
        }
        Some(value) if !value.is_empty() => value.to_string(),
        _ => "chatcmpl_ccswitch".to_string(),
    }
}

fn response_id_from_chat_id(id: Option<&str>) -> String {
    match id {
        Some(value) if value.starts_with("resp_") => value.to_string(),
        Some(value) if value.starts_with("chatcmpl_") => {
            format!("resp_{}", value.trim_start_matches("chatcmpl_"))
        }
        Some(value) if !value.is_empty() => format!("resp_{value}"),
        _ => "resp_ccswitch".to_string(),
    }
}

fn openai_chat_stream_chunk(
    id: Option<&str>,
    model: Option<&str>,
    delta: Value,
    finish_reason: Value,
    usage: Option<Value>,
) -> StreamFrame {
    let mut chunk = json!({
        "id": chat_id_from_response_id(id),
        "object": "chat.completion.chunk",
        "model": model.unwrap_or_default(),
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish_reason
        }]
    });
    if let Some(usage) = usage {
        chunk["usage"] = usage;
    }
    StreamFrame::json(chunk)
}

fn openai_chat_message_to_anthropic(
    message: &Value,
    role: &str,
) -> Result<Vec<Value>, TransformError> {
    if role == "tool" {
        let output = message.get("content").cloned().unwrap_or(Value::Null);
        return Ok(vec![json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default(),
                "content": anthropic_tool_result_content(&output)
            }]
        })]);
    }

    let mut content = openai_content_to_anthropic(message.get("content"));
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            content.push(openai_chat_request_tool_call_to_anthropic(tool_call)?);
        }
    }
    Ok(vec![json!({
        "role": if role == "assistant" { "assistant" } else { "user" },
        "content": content
    })])
}

fn openai_chat_request_tool_call_to_anthropic(tool_call: &Value) -> Result<Value, TransformError> {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| TransformError::new("openai chat tool call name is required"))?;
    let input = response_function_arguments_object(function, name)?;
    Ok(json!({
        "type": "tool_use",
        "id": tool_call.get("id").and_then(Value::as_str).unwrap_or_default(),
        "name": name,
        "input": input
    }))
}

fn openai_response_item_to_anthropic(
    item: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Vec<Value>, TransformError> {
    if matches!(
        item.get("role").and_then(Value::as_str),
        Some("system" | "developer")
    ) {
        return Ok(Vec::new());
    }
    match item.get("type").and_then(Value::as_str) {
        Some("additional_tools") => return Ok(Vec::new()),
        Some("reasoning") => {
            return Ok(anthropic_block_from_responses_reasoning_item(item)
                .map(|block| vec![json!({"role": "assistant", "content": [block]})])
                .unwrap_or_default());
        }
        Some("function_call" | "custom_tool_call" | "tool_search_call") => {
            if item.get("status").and_then(Value::as_str) == Some("incomplete") {
                return Ok(Vec::new());
            }
            return Ok(vec![json!({
                "role": "assistant",
                "content": [openai_response_request_tool_call_to_anthropic(item, tool_context)?]
            })]);
        }
        Some("function_call_output" | "custom_tool_call_output" | "tool_search_output") => {
            let output = item.get("output").cloned().unwrap_or(Value::Null);
            let mut block = json!({
                "type": "tool_result",
                "tool_use_id": item.get("call_id").and_then(Value::as_str).unwrap_or_default(),
                "content": anthropic_tool_result_content(&output)
            });
            if let Some(is_error) = item.get("is_error") {
                block["is_error"] = is_error.clone();
            }
            return Ok(vec![json!({"role": "user", "content": [block]})]);
        }
        Some("input_text" | "output_text" | "text") => {
            let role = if item.get("type").and_then(Value::as_str) == Some("output_text") {
                "assistant"
            } else {
                "user"
            };
            return Ok(vec![json!({
                "role": role,
                "content": [openai_content_item_to_anthropic(item)]
            })]);
        }
        Some("input_image") => {
            return Ok(vec![json!({
                "role": "user",
                "content": [openai_content_item_to_anthropic(item)]
            })]);
        }
        _ => {}
    }
    let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    Ok(vec![json!({
        "role": if role == "assistant" { "assistant" } else { "user" },
        "content": openai_content_to_anthropic(item.get("content"))
    })])
}

fn openai_response_request_tool_call_to_anthropic(
    item: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Value, TransformError> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    let original_name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
    let name = match item_type {
        "tool_search_call" => TOOL_SEARCH_PROXY_NAME.to_string(),
        "function_call" => tool_context.chat_name_for_response_function(
            original_name,
            item.get("namespace").and_then(Value::as_str),
        ),
        _ => original_name.to_string(),
    };
    let input = match item_type {
        "custom_tool_call" => {
            json!({"input": item.get("input").cloned().unwrap_or_else(|| json!(""))})
        }
        "tool_search_call" => item
            .get("arguments")
            .filter(|value| value.is_object())
            .cloned()
            .or_else(|| item.get("query").map(|query| json!({"query": query})))
            .unwrap_or_else(|| json!({})),
        _ => response_function_arguments_object(item, original_name)?,
    };
    Ok(json!({
        "type": "tool_use",
        "id": item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "name": name,
        "input": input
    }))
}

fn response_function_arguments_object(item: &Value, name: &str) -> Result<Value, TransformError> {
    let input = match item.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(Value::String(arguments)) if arguments.trim().is_empty() => json!({}),
        Some(Value::String(arguments)) => {
            serde_json::from_str::<Value>(arguments).map_err(|error| {
                TransformError::new(format!(
                    "invalid function_call arguments for '{name}': {error}"
                ))
            })?
        }
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => {
            return Err(TransformError::new(format!(
                "function_call arguments for '{name}' must be a json object"
            )))
        }
    };
    if !input.is_object() {
        return Err(TransformError::new(format!(
            "function_call arguments for '{name}' must be a json object"
        )));
    }
    Ok(input)
}

fn merge_adjacent_anthropic_messages(messages: &mut Vec<Value>) {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for message in std::mem::take(messages) {
        let role = message.get("role").and_then(Value::as_str);
        let can_merge = merged
            .last()
            .and_then(|previous| previous.get("role").and_then(Value::as_str))
            == role;
        if can_merge {
            let next = message
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(content) = merged
                .last_mut()
                .and_then(|previous| previous.get_mut("content"))
                .and_then(Value::as_array_mut)
            {
                content.extend(next);
                continue;
            }
        }
        merged.push(message);
    }
    *messages = merged;
}

fn append_response_system_text(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(text) = value.and_then(response_instruction_text) else {
        return;
    };
    let text = text.trim();
    if !text.is_empty() {
        output.push(text.to_string());
    }
}

fn drop_empty_anthropic_messages(messages: &mut Vec<Value>) {
    for message in messages.iter_mut() {
        if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
            content.retain(|block| {
                block.get("type").and_then(Value::as_str) != Some("text")
                    || block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
            });
        }
    }
    messages.retain(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_none_or(|content| !content.is_empty())
    });
}

fn drop_incomplete_anthropic_tool_turns(messages: &mut Vec<Value>) {
    let original = std::mem::take(messages);
    let mut sanitized = Vec::with_capacity(original.len());
    let mut index = 0;

    while index < original.len() {
        let message = &original[index];
        let tool_use_ids = if message.get("role").and_then(Value::as_str) == Some("assistant") {
            anthropic_message_block_ids(message, "tool_use", "id")
        } else {
            Vec::new()
        };
        if !tool_use_ids.is_empty() {
            let paired_user = original
                .get(index + 1)
                .filter(|next| next.get("role").and_then(Value::as_str) == Some("user"));
            let tool_result_ids = paired_user
                .map(|user| anthropic_message_block_ids(user, "tool_result", "tool_use_id"))
                .unwrap_or_default();
            let unique_tool_uses: HashSet<&str> = tool_use_ids.iter().copied().collect();
            let unique_tool_results: HashSet<&str> = tool_result_ids.iter().copied().collect();
            let complete = tool_use_ids.iter().all(|id| !id.is_empty())
                && tool_result_ids.iter().all(|id| !id.is_empty())
                && unique_tool_uses.len() == tool_use_ids.len()
                && unique_tool_results.len() == tool_result_ids.len()
                && unique_tool_uses == unique_tool_results;

            if complete {
                sanitized.push(message.clone());
                sanitized.push(
                    paired_user
                        .expect("complete tool turn has a user message")
                        .clone(),
                );
            } else if let Some(user) = paired_user {
                let mut user = user.clone();
                drop_anthropic_tool_result_blocks(&mut user);
                if anthropic_message_has_content(&user) {
                    sanitized.push(user);
                }
            }
            index += if paired_user.is_some() { 2 } else { 1 };
            continue;
        }

        let mut message = message.clone();
        if message.get("role").and_then(Value::as_str) == Some("user") {
            drop_anthropic_tool_result_blocks(&mut message);
        }
        if anthropic_message_has_content(&message) {
            sanitized.push(message);
        }
        index += 1;
    }

    *messages = sanitized;
}

fn anthropic_message_block_ids<'a>(
    message: &'a Value,
    block_type: &str,
    id_field: &str,
) -> Vec<&'a str> {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
        .map(|block| {
            block
                .get(id_field)
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect()
}

fn drop_anthropic_tool_result_blocks(message: &mut Value) {
    if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
        content.retain(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"));
    }
}

fn anthropic_message_has_content(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_none_or(|content| !content.is_empty())
}

fn trailing_turn_supports_thinking(messages: &[Value]) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let tool_result_ids = anthropic_message_block_ids(last, "tool_result", "tool_use_id");
    if tool_result_ids.is_empty() {
        return true;
    }
    if tool_result_ids.iter().any(|id| id.is_empty()) {
        return false;
    }

    let Some(paired_assistant) = messages.get(messages.len().saturating_sub(2)) else {
        return false;
    };
    if paired_assistant.get("role").and_then(Value::as_str) != Some("assistant") {
        return false;
    }
    let Some(blocks) = paired_assistant.get("content").and_then(Value::as_array) else {
        return false;
    };
    if !blocks.iter().any(anthropic_history_thinking_is_signed) {
        return false;
    }

    let paired_tool_use_ids = anthropic_message_block_ids(paired_assistant, "tool_use", "id");
    let unique_tool_uses: HashSet<&str> = paired_tool_use_ids.iter().copied().collect();
    let unique_tool_results: HashSet<&str> = tool_result_ids.iter().copied().collect();
    paired_tool_use_ids.iter().all(|id| !id.is_empty())
        && unique_tool_uses.len() == paired_tool_use_ids.len()
        && unique_tool_results.len() == tool_result_ids.len()
        && unique_tool_uses == unique_tool_results
}

fn anthropic_history_thinking_is_signed(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => block
            .get("signature")
            .and_then(Value::as_str)
            .is_some_and(|signature| !signature.is_empty()),
        Some("redacted_thinking") => block
            .get("data")
            .and_then(Value::as_str)
            .is_some_and(|data| !data.is_empty()),
        _ => false,
    }
}

fn order_anthropic_tool_results_first(messages: &mut [Value]) {
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        content
            .sort_by_key(|block| block.get("type").and_then(Value::as_str) != Some("tool_result"));
    }
}

fn ensure_leading_anthropic_user_message(messages: &mut Vec<Value>) {
    if messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        != Some("user")
    {
        messages.insert(
            0,
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "(continuing the conversation)"}]
            }),
        );
    }
}

fn anthropic_tool_result_content(output: &Value) -> Value {
    let Some(extraction) = extract_tool_media(output, ToolMediaScope::AnthropicNative) else {
        return match output {
            Value::String(_) | Value::Array(_) => output.clone(),
            Value::Null => json!(""),
            other => json!(other.to_string()),
        };
    };
    let mut content = vec![json!({
        "type": "text",
        "text": sanitized_tool_text(&extraction.sanitized)
    })];
    content.extend(
        extraction
            .media
            .iter()
            .filter_map(ToolMediaPart::to_anthropic_block),
    );
    Value::Array(content)
}

fn gemini_content_to_anthropic(content: &Value) -> Value {
    let role = match content.get("role").and_then(Value::as_str) {
        Some("model") => "assistant",
        _ => "user",
    };
    let parts: Vec<Value> = content
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(gemini_part_to_anthropic).collect())
        .unwrap_or_default();
    json!({"role": role, "content": parts})
}

fn anthropic_message_to_openai_chat(message: &Value) -> Vec<Value> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let mut messages = Vec::new();
    let mut content_items = Vec::new();
    let mut tool_calls = Vec::new();
    let mut pending_media = Vec::new();

    for block in anthropic_content_blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_calls.push(anthropic_tool_use_to_openai(block)),
            Some("tool_result") => {
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let output = block
                    .get("content")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                let extraction = extract_tool_media(&output, ToolMediaScope::ChatNative);
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": extraction
                        .as_ref()
                        .map(|value| json!(sanitized_tool_text(&value.sanitized)))
                        .unwrap_or(output)
                }));
                if let Some(extraction) = extraction {
                    queue_chat_media(&mut pending_media, call_id, &extraction.media);
                }
            }
            Some("thinking" | "redacted_thinking") => {}
            _ => content_items.push(anthropic_block_to_openai_chat_content(block)),
        }
    }

    if !content_items.is_empty() || !tool_calls.is_empty() {
        let mut base = Map::new();
        base.insert(
            "role".to_string(),
            Value::String(if role == "assistant" {
                "assistant".to_string()
            } else {
                "user".to_string()
            }),
        );
        base.insert("content".to_string(), Value::Array(content_items));
        if !tool_calls.is_empty() {
            base.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }
        messages.insert(0, Value::Object(base));
    }
    flush_chat_media(&mut messages, &mut pending_media);
    messages
}

fn anthropic_message_to_openai_response_items(message: &Value) -> Vec<Value> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let response_role = if role == "assistant" {
        "assistant"
    } else {
        "user"
    };
    let mut items = Vec::new();
    let mut content = Vec::new();
    let mut pending_media = Vec::new();
    let message_start = items.len();
    for block in anthropic_content_blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                flush_response_message(&mut items, response_role, &mut content);
                flush_responses_tool_media(&mut items, &mut pending_media);
                items.push(anthropic_tool_use_to_openai_response(block));
            }
            Some("tool_result") => {
                flush_response_message(&mut items, response_role, &mut content);
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let output = block.get("content").cloned().unwrap_or(Value::Null);
                let extraction = extract_tool_media(&output, ToolMediaScope::ResponsesNative);
                let mut item = json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": extraction
                        .as_ref()
                        .map(|value| sanitized_tool_text(&value.sanitized))
                        .unwrap_or_else(|| sanitized_tool_text(&output))
                });
                if let Some(is_error) = block.get("is_error") {
                    item["is_error"] = is_error.clone();
                }
                items.push(item);
                if let Some(extraction) = extraction {
                    queue_responses_tool_media(&mut pending_media, call_id, &extraction.media);
                }
            }
            Some("thinking" | "redacted_thinking") => {
                flush_response_message(&mut items, response_role, &mut content);
                flush_responses_tool_media(&mut items, &mut pending_media);
                if let Some(item) = openai_reasoning_item_from_anthropic_block(block) {
                    items.push(item);
                } else {
                    let item_id = format!("rs_history_{}", items.len());
                    if let Some(item) =
                        responses_reasoning_item_from_anthropic_block(&item_id, block)
                    {
                        items.push(item);
                    }
                }
            }
            _ => {
                flush_responses_tool_media(&mut items, &mut pending_media);
                content.push(anthropic_block_to_openai_response_content(block));
            }
        }
    }
    flush_response_message(&mut items, response_role, &mut content);
    flush_responses_tool_media(&mut items, &mut pending_media);
    if response_role == "assistant" {
        remove_reasoning_without_generated_follower(&mut items, message_start);
    }
    items
}

fn remove_reasoning_without_generated_follower(items: &mut Vec<Value>, start: usize) {
    let mut has_generated_follower = false;
    for index in (start..items.len()).rev() {
        let item_type = items[index].get("type").and_then(Value::as_str);
        let assistant_message =
            items[index].get("role").and_then(Value::as_str) == Some("assistant");
        if item_type == Some("reasoning") {
            if !has_generated_follower {
                items.remove(index);
            }
        } else if matches!(
            item_type,
            Some("function_call" | "custom_tool_call" | "tool_search_call")
        ) || assistant_message
        {
            has_generated_follower = true;
        }
    }
}

fn flush_response_message(items: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if !content.is_empty() {
        items.push(json!({"role": role, "content": std::mem::take(content)}));
    }
}

fn anthropic_message_to_gemini_content(message: &Value, gemini_three: bool) -> Value {
    let role = match message.get("role").and_then(Value::as_str) {
        Some("assistant") => "model",
        _ => "user",
    };
    let mut parts = Vec::new();
    for block in anthropic_content_blocks(message) {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            parts.push(anthropic_block_to_gemini_part(block));
            continue;
        }
        let call_id = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let output = block.get("content").cloned().unwrap_or(Value::Null);
        let extraction = extract_tool_media(&output, ToolMediaScope::GeminiNative);
        let mut function_response = json!({
            "name": call_id,
            "id": call_id,
            "response": extraction
                .as_ref()
                .map(|value| value.sanitized.clone())
                .unwrap_or(output)
        });
        if let Some(is_error) = block.get("is_error") {
            function_response["isError"] = is_error.clone();
        }
        let media_parts = extraction
            .as_ref()
            .map(|value| {
                value
                    .media
                    .iter()
                    .filter_map(ToolMediaPart::to_gemini_part)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if gemini_three && !media_parts.is_empty() {
            function_response["parts"] = Value::Array(media_parts);
            parts.push(json!({"functionResponse": function_response}));
        } else {
            parts.push(json!({"functionResponse": function_response}));
            if !media_parts.is_empty() {
                parts.push(json!({
                    "text": format!("[cc-switch-server: media output of tool call {call_id}]")
                }));
                parts.extend(media_parts);
            }
        }
    }
    json!({
        "role": role,
        "parts": parts
    })
}

fn openai_content_to_anthropic(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
        Some(Value::Array(items)) => items.iter().map(openai_content_item_to_anthropic).collect(),
        Some(value) if !value.is_null() => vec![json!({"type": "text", "text": value.to_string()})],
        _ => Vec::new(),
    }
}

fn openai_content_item_to_anthropic(item: &Value) -> Value {
    match item.get("type").and_then(Value::as_str) {
        Some("text") | Some("input_text") | Some("output_text") => json!({
            "type": "text",
            "text": item.get("text").and_then(Value::as_str).unwrap_or_default()
        }),
        Some("refusal") => json!({
            "type": "text",
            "text": item.get("refusal").or_else(|| item.get("text")).and_then(Value::as_str).unwrap_or_default()
        }),
        Some("image_url") => image_url_to_anthropic(item.pointer("/image_url/url")),
        Some("input_image") => {
            image_url_to_anthropic(item.get("image_url").or_else(|| item.get("url")))
        }
        _ => item.clone(),
    }
}

fn openai_chat_response_content_to_anthropic(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            vec![json!({"type": "text", "text": text})]
        }
        Some(Value::Array(items)) => items.iter().map(openai_content_item_to_anthropic).collect(),
        _ => Vec::new(),
    }
}

fn openai_response_output_to_anthropic(item: &Value) -> Option<Value> {
    match item.get("type").and_then(Value::as_str) {
        Some("output_text") | Some("text") => Some(json!({
            "type": "text",
            "text": item.get("text").and_then(Value::as_str).unwrap_or_default()
        })),
        Some("refusal") => Some(json!({
            "type": "text",
            "text": item.get("refusal").or_else(|| item.get("text")).and_then(Value::as_str).unwrap_or_default()
        })),
        _ => None,
    }
}

fn image_url_to_anthropic(url: Option<&Value>) -> Value {
    let url = url.and_then(Value::as_str).unwrap_or_default();
    if let Some((media_type, data)) = parse_data_url(url) {
        json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": data
            }
        })
    } else {
        json!({"type": "image", "source": {"type": "url", "url": url}})
    }
}

fn openai_tool_call_to_anthropic(tool_call: &Value) -> Value {
    let function = tool_call.get("function").unwrap_or(&Value::Null);
    let input = function
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .unwrap_or_else(|| {
            function
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}))
        });
    json!({
        "type": "tool_use",
        "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("tool"),
        "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
        "input": input
    })
}

fn openai_function_call_to_anthropic(item: &Value) -> Value {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    let input = if item_type == "custom_tool_call" {
        json!({"input": item.get("input").cloned().unwrap_or_else(|| json!(""))})
    } else if item_type == "tool_search_call" {
        item.get("arguments")
            .cloned()
            .or_else(|| item.get("query").map(|query| json!({"query": query})))
            .unwrap_or_else(|| json!({}))
    } else {
        item.get("arguments")
            .and_then(Value::as_str)
            .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
            .unwrap_or_else(|| item.get("arguments").cloned().unwrap_or_else(|| json!({})))
    };
    let name = if item_type == "tool_search_call" {
        "tool_search"
    } else {
        item.get("name").and_then(Value::as_str).unwrap_or("tool")
    };
    json!({
        "type": "tool_use",
        "id": item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("tool"),
        "name": name,
        "input": input
    })
}

fn gemini_part_to_anthropic(part: &Value) -> Value {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return json!({"type": "text", "text": text});
    }
    if part.get("inlineData").is_some()
        || part.get("inline_data").is_some()
        || part.get("fileData").is_some()
        || part.get("file_data").is_some()
    {
        if let Some(block) =
            extract_tool_media(part, ToolMediaScope::AnthropicNative).and_then(|value| {
                value
                    .media
                    .first()
                    .and_then(ToolMediaPart::to_anthropic_block)
            })
        {
            return block;
        }
    }
    if let Some(function_call) = part
        .get("functionCall")
        .or_else(|| part.get("function_call"))
    {
        return json!({
            "type": "tool_use",
            "id": function_call.get("id").and_then(Value::as_str).unwrap_or_else(|| function_call.get("name").and_then(Value::as_str).unwrap_or("tool")),
            "name": function_call.get("name").and_then(Value::as_str).unwrap_or("tool"),
            "input": function_call.get("args").cloned().unwrap_or_else(|| json!({}))
        });
    }
    if let Some(function_response) = part
        .get("functionResponse")
        .or_else(|| part.get("function_response"))
    {
        return gemini_function_response_to_anthropic(function_response);
    }
    part.clone()
}

fn gemini_function_response_to_anthropic(function_response: &Value) -> Value {
    let output = function_response
        .get("response")
        .cloned()
        .unwrap_or(Value::Null);
    let output_extraction = extract_tool_media(&output, ToolMediaScope::AnthropicNative);
    let parts = function_response
        .get("parts")
        .cloned()
        .unwrap_or(Value::Null);
    let parts_extraction = extract_tool_media(&parts, ToolMediaScope::AnthropicNative);
    let mut media = Vec::new();
    let mut content = Vec::new();
    if let Some(extraction) = output_extraction {
        content.push(json!({
            "type": "text",
            "text": sanitized_tool_text(&extraction.sanitized)
        }));
        media.extend(extraction.media);
    } else if !output.is_null() {
        content.push(json!({
            "type": "text",
            "text": sanitized_tool_text(&output)
        }));
    }
    if let Some(extraction) = parts_extraction {
        if extraction.sanitized != Value::Null {
            content.push(json!({
                "type": "text",
                "text": sanitized_tool_text(&extraction.sanitized)
            }));
        }
        media.extend(extraction.media);
    } else if let Some(parts) = parts.as_array().filter(|parts| !parts.is_empty()) {
        content.push(json!({
            "type": "text",
            "text": serde_json::to_string(parts).unwrap_or_default()
        }));
    }
    content.extend(media.iter().filter_map(ToolMediaPart::to_anthropic_block));
    let mut block = json!({
        "type": "tool_result",
        "tool_use_id": function_response
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| function_response.get("name").and_then(Value::as_str).unwrap_or("tool")),
        "content": content
    });
    if let Some(is_error) = function_response
        .get("isError")
        .or_else(|| function_response.get("is_error"))
    {
        block["is_error"] = is_error.clone();
    }
    block
}

fn anthropic_block_to_openai_chat_content(block: &Value) -> Value {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => anthropic_image_to_openai_chat(block),
        _ => {
            let mut output = json!({
                "type": "text",
                "text": block.get("text").and_then(Value::as_str).unwrap_or_default()
            });
            copy_cache_control(block, &mut output);
            output
        }
    }
}

fn anthropic_block_to_openai_response_content(block: &Value) -> Value {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => anthropic_image_to_openai_response(block),
        _ => {
            let mut output = json!({
                "type": "input_text",
                "text": block.get("text").and_then(Value::as_str).unwrap_or_default()
            });
            copy_cache_control(block, &mut output);
            output
        }
    }
}

fn anthropic_block_to_gemini_part(block: &Value) -> Value {
    match block.get("type").and_then(Value::as_str) {
        Some("image") => json!({
            "inlineData": {
                "mimeType": block.pointer("/source/media_type").and_then(Value::as_str).unwrap_or("application/octet-stream"),
                "data": block.pointer("/source/data").and_then(Value::as_str).unwrap_or_default()
            }
        }),
        Some("tool_use") => json!({
            "functionCall": {
                "name": block.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "args": block.get("input").cloned().unwrap_or_else(|| json!({}))
            }
        }),
        Some("tool_result") => json!({
            "functionResponse": {
                "name": block.get("tool_use_id").and_then(Value::as_str).unwrap_or("tool"),
                "response": block.get("content").cloned().unwrap_or(Value::Null)
            }
        }),
        _ => {
            let mut output =
                json!({"text": block.get("text").and_then(Value::as_str).unwrap_or_default()});
            copy_cache_control(block, &mut output);
            output
        }
    }
}

fn anthropic_tool_use_to_openai(block: &Value) -> Value {
    json!({
        "id": block.get("id").and_then(Value::as_str).unwrap_or("tool"),
        "type": "function",
        "function": {
            "name": block.get("name").and_then(Value::as_str).unwrap_or("tool"),
            "arguments": block.get("input").cloned().unwrap_or_else(|| json!({})).to_string()
        }
    })
}

fn anthropic_tool_use_to_openai_response(block: &Value) -> Value {
    json!({
        "type": "function_call",
        "call_id": block.get("id").and_then(Value::as_str).unwrap_or("tool"),
        "name": block.get("name").and_then(Value::as_str).unwrap_or("tool"),
        "arguments": block.get("input").cloned().unwrap_or_else(|| json!({})).to_string()
    })
}

fn anthropic_tool_use_to_openai_response_with_custom_tools(
    block: &Value,
    custom_tool_names: &BTreeSet<String>,
) -> Value {
    let tool_context = ResponsesToolContext::from_custom_tool_names(custom_tool_names);
    anthropic_tool_use_to_openai_response_with_tool_context(block, &tool_context)
}

fn anthropic_tool_use_to_openai_response_with_tool_context(
    block: &Value,
    tool_context: &ResponsesToolContext,
) -> Value {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let call_id = block.get("id").and_then(Value::as_str).unwrap_or("tool");
    let arguments = block
        .get("input")
        .cloned()
        .unwrap_or_else(|| json!({}))
        .to_string();
    let item_id = tool_context.response_item_id(call_id, name);
    tool_context.response_item(&item_id, "completed", call_id, name, &arguments)
}

fn anthropic_image_to_openai_chat(block: &Value) -> Value {
    let media_type = block
        .pointer("/source/media_type")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let data = block
        .pointer("/source/data")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}})
}

fn anthropic_image_to_openai_response(block: &Value) -> Value {
    let media_type = block
        .pointer("/source/media_type")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let data = block
        .pointer("/source/data")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({"type": "input_image", "image_url": format!("data:{media_type};base64,{data}")})
}

fn openai_tools_to_anthropic(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    Some(Value::Array(
        tools.iter().filter_map(openai_tool_to_anthropic).collect(),
    ))
}

fn openai_response_tools_to_anthropic(
    input: &Value,
    tool_context: &ResponsesToolContext,
) -> Result<Option<Value>, TransformError> {
    let Some(chat_tools) = openai_response_tools_to_chat(input, tool_context)? else {
        return Ok(None);
    };
    let tools = chat_tools
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(openai_tool_to_anthropic)
        .collect::<Vec<_>>();
    Ok((!tools.is_empty()).then_some(Value::Array(tools)))
}

fn apply_openai_request_controls_to_anthropic(
    input: &Value,
    output: &mut Map<String, Value>,
    max_token_keys: &[&str],
) {
    let max_tokens = max_token_keys
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_u64))
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_OPENAI_TO_ANTHROPIC_MAX_TOKENS);
    output.insert("max_tokens".to_string(), json!(max_tokens));

    copy_value(input, output, "temperature");
    copy_value(input, output, "top_p");
    if let Some(stop_sequences) = openai_stop_sequences(input.get("stop")) {
        output.insert("stop_sequences".to_string(), stop_sequences);
    }
}

fn openai_stop_sequences(stop: Option<&Value>) -> Option<Value> {
    match stop? {
        Value::String(stop) if !stop.is_empty() => Some(json!([stop])),
        Value::Array(stops) => {
            let stops = stops
                .iter()
                .filter_map(Value::as_str)
                .filter(|stop| !stop.is_empty())
                .map(|stop| Value::String(stop.to_string()))
                .collect::<Vec<_>>();
            (!stops.is_empty()).then_some(Value::Array(stops))
        }
        _ => None,
    }
}

fn apply_openai_tool_controls_to_anthropic(
    input: &Value,
    output: &mut Map<String, Value>,
    tool_context: Option<&ResponsesToolContext>,
) -> Result<(), TransformError> {
    let has_tools = output
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    if !has_tools {
        return Ok(());
    }

    if let Some(tool_choice) = input
        .get("tool_choice")
        .and_then(|choice| openai_tool_choice_to_anthropic(choice, tool_context))
    {
        let forced = matches!(
            tool_choice.get("type").and_then(Value::as_str),
            Some("any" | "tool")
        );
        let thinking_enabled = matches!(
            output
                .get("thinking")
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str),
            Some("enabled" | "adaptive")
        );
        if forced && thinking_enabled {
            let model = output
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if super::thinking::thinking_cannot_be_disabled(model) {
                return Err(TransformError::new(
                    "anthropic model requires adaptive thinking and cannot honor a forced tool_choice",
                ));
            }
            output.insert("thinking".to_string(), json!({"type": "disabled"}));
            output.remove("output_config");
            copy_value(input, output, "temperature");
            copy_value(input, output, "top_p");
        }
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if input.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false) {
        let tool_choice = output
            .entry("tool_choice".to_string())
            .or_insert_with(|| json!({"type": "auto"}));
        if let Some(tool_choice) = tool_choice.as_object_mut() {
            tool_choice.insert("disable_parallel_tool_use".to_string(), Value::Bool(true));
        }
    }
    Ok(())
}

fn openai_tool_choice_to_anthropic(
    tool_choice: &Value,
    tool_context: Option<&ResponsesToolContext>,
) -> Option<Value> {
    match tool_choice {
        Value::String(choice) => match choice.as_str() {
            "auto" => Some(json!({"type": "auto"})),
            "required" => Some(json!({"type": "any"})),
            "none" => Some(json!({"type": "none"})),
            _ => None,
        },
        Value::Object(choice) => match choice.get("type").and_then(Value::as_str) {
            Some("auto" | "any" | "none") => Some(tool_choice.clone()),
            Some("required") => Some(json!({"type": "any"})),
            Some(choice_type @ ("function" | "custom")) => choice
                .get("name")
                .or_else(|| {
                    choice
                        .get("function")
                        .and_then(|function| function.get("name"))
                })
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(|name| {
                    let name = if choice_type == "function" {
                        tool_context
                            .map(|context| {
                                context.chat_name_for_response_function(
                                    name,
                                    choice.get("namespace").and_then(Value::as_str),
                                )
                            })
                            .unwrap_or_else(|| name.to_string())
                    } else {
                        name.to_string()
                    };
                    json!({"type": "tool", "name": name})
                }),
            Some("tool_search") => Some(json!({"type": "tool", "name": "tool_search"})),
            Some("tool") => choice
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(|name| json!({"type": "tool", "name": name})),
            _ => None,
        },
        _ => None,
    }
}

fn openai_tool_to_anthropic(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) == Some("custom") {
        let name = response_tool_name(tool)?;
        return Some(json!({
            "name": name,
            "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
            "input_schema": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"],
                "additionalProperties": false
            }
        }));
    }

    let function = match tool.get("function") {
        Some(function) => function,
        None if tool.get("type").and_then(Value::as_str) == Some("function") => tool,
        None => return None,
    };
    Some(json!({
        "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
        "description": function.get("description").cloned().unwrap_or(Value::String(String::new())),
        "input_schema": normalize_function_parameters(function.get("parameters"))
    }))
}

fn gemini_tools_to_anthropic(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    let mut output = Vec::new();
    for tool in tools {
        if let Some(declarations) = tool
            .get("functionDeclarations")
            .or_else(|| tool.get("function_declarations"))
            .and_then(Value::as_array)
        {
            for declaration in declarations {
                output.push(json!({
                    "name": declaration.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "description": declaration.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": normalize_function_parameters(declaration.get("parameters"))
                }));
            }
        }
    }
    Some(Value::Array(output))
}

fn anthropic_tools_to_openai(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    Some(Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.get("name").and_then(Value::as_str).unwrap_or("tool"),
                        "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                        "parameters": normalize_function_parameters(tool.get("input_schema"))
                    }
                })
            })
            .collect(),
    ))
}

fn anthropic_tools_to_gemini(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    Some(json!([{
        "functionDeclarations": tools.iter().map(|tool| {
            json!({
                "name": tool.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "description": tool.get("description").cloned().unwrap_or(Value::String(String::new())),
                "parameters": normalize_function_parameters(tool.get("input_schema"))
            })
        }).collect::<Vec<_>>()
    }]))
}

fn apply_openai_reasoning_to_anthropic(
    input: &Value,
    output: &mut Map<String, Value>,
    thinking_history_is_valid: bool,
) -> Result<(), TransformError> {
    let reasoning_effort = input
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .or_else(|| input.get("reasoning_effort").and_then(Value::as_str));
    let model = output
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let max_tokens = output
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_OPENAI_TO_ANTHROPIC_MAX_TOKENS);
    let adaptive_model = super::thinking::uses_adaptive_thinking(model);
    let adaptive_by_default = super::thinking::adaptive_thinking_is_default(model);
    let cannot_disable_thinking = super::thinking::thinking_cannot_be_disabled(model);
    let explicitly_disabled = reasoning_effort.is_some_and(reasoning_explicitly_disabled);
    let mapped_effort = reasoning_effort.and_then(openai_effort_to_anthropic);
    let adaptive_should_think = adaptive_model && (adaptive_by_default || mapped_effort.is_some());
    let mut thinking_enabled = false;

    if !thinking_history_is_valid {
        if cannot_disable_thinking {
            return Err(TransformError::new(
                "anthropic model requires thinking, but the tool history has no signed thinking block to replay",
            ));
        }
        if adaptive_should_think {
            output.insert("thinking".to_string(), json!({"type": "disabled"}));
        }
    } else if adaptive_should_think && (!explicitly_disabled || cannot_disable_thinking) {
        thinking_enabled = true;
        output.insert("thinking".to_string(), json!({"type": "adaptive"}));
        if let Some(effort) = mapped_effort {
            output.insert("output_config".to_string(), json!({"effort": effort}));
        } else if explicitly_disabled && cannot_disable_thinking {
            output.insert("output_config".to_string(), json!({"effort": "low"}));
        }
    } else if explicitly_disabled {
        output.insert("thinking".to_string(), json!({"type": "disabled"}));
    } else if let Some(mut budget_tokens) =
        reasoning_effort.and_then(openai_effort_to_thinking_budget)
    {
        budget_tokens = budget_tokens.min(max_tokens / 2);
        if budget_tokens >= 1024 {
            thinking_enabled = true;
            output.insert(
                "thinking".to_string(),
                json!({"type": "enabled", "budget_tokens": budget_tokens}),
            );
        }
    }

    if thinking_enabled {
        output.remove("temperature");
        output.remove("top_p");
    }
    Ok(())
}

fn openai_effort_to_thinking_budget(effort: &str) -> Option<u64> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "minimal" | "low" => Some(2048),
        "medium" => Some(8192),
        "high" => Some(16384),
        "xhigh" | "max" => Some(24576),
        _ => None,
    }
}

fn openai_effort_to_anthropic(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" => Some("max"),
        _ => None,
    }
}

fn reasoning_explicitly_disabled(effort: &str) -> bool {
    matches!(
        effort.trim().to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    )
}

fn anthropic_reasoning_to_openai(input: &Value) -> Option<Value> {
    let explicit_effort = input
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
        .or_else(|| {
            input
                .pointer("/metadata/geminiGenerationConfig/thinkingConfig/thinkingLevel")
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_ascii_lowercase);
    if let Some(effort) = explicit_effort {
        return Some(json!({"effort": effort}));
    }
    anthropic_thinking_to_openai(input.get("thinking"))
}

fn anthropic_thinking_to_openai(thinking: Option<&Value>) -> Option<Value> {
    let thinking = thinking?;
    let mut output = Map::new();
    if let Some(effort) = thinking.get("effort").and_then(Value::as_str) {
        output.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    if let Some(summary) = thinking.get("summary").and_then(Value::as_str) {
        output.insert("summary".to_string(), Value::String(summary.to_string()));
    }
    if output.is_empty() {
        output.insert("effort".to_string(), Value::String("medium".to_string()));
    }
    Some(Value::Object(output))
}

fn anthropic_content_blocks(message: &Value) -> Vec<&Value> {
    match message.get("content") {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}

fn gemini_system_text(system: Option<&Value>) -> Option<String> {
    let parts = system?.get("parts")?.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn collect_text_like(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(text) => output.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    output.push(text.to_string());
                }
            }
        }
        _ => {}
    }
}

fn text_from_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        value => Some(value.to_string()),
    }
}

fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some((media_type, data))
}

fn copy_string(input: &Value, output: &mut Map<String, Value>, key: &str) {
    if let Some(value) = input.get(key).and_then(Value::as_str) {
        output.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn copy_bool(input: &Value, output: &mut Map<String, Value>, key: &str) {
    if let Some(value) = input.get(key).and_then(Value::as_bool) {
        output.insert(key.to_string(), Value::Bool(value));
    }
}

fn copy_object(input: &Value, output: &mut Map<String, Value>, key: &str) {
    if let Some(value) = input.get(key).filter(|value| value.is_object()) {
        output.insert(key.to_string(), value.clone());
    }
}

fn copy_value(input: &Value, output: &mut Map<String, Value>, key: &str) {
    if let Some(value) = input.get(key) {
        output.insert(key.to_string(), value.clone());
    }
}

fn copy_cache_control(input: &Value, output: &mut Value) {
    if let Some(cache_control) = input.get("cache_control") {
        if let Some(object) = output.as_object_mut() {
            object.insert("cache_control".to_string(), cache_control.clone());
        }
    }
}

pub(super) fn openai_finish_reason_to_anthropic(reason: &str) -> &'static str {
    match reason {
        "tool_calls" | "function_call" => "tool_use",
        "length" => "max_tokens",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

fn openai_response_to_anthropic_stop(response: &Value) -> &'static str {
    openai_response_to_anthropic_stop_with_tools(response, response_output_has_tool_calls(response))
}

pub(super) fn openai_response_to_anthropic_stop_with_tools(
    response: &Value,
    has_tool_calls: bool,
) -> &'static str {
    match (
        response.get("status").and_then(Value::as_str),
        response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str),
    ) {
        (Some("incomplete"), Some("content_filter")) => "refusal",
        (Some("incomplete"), _) => "max_tokens",
        _ if has_tool_calls => "tool_use",
        _ => "end_turn",
    }
}

fn openai_response_to_anthropic_stream_stop(response: &Value) -> &'static str {
    openai_response_to_anthropic_stop(response)
}

fn openai_finish_reason_to_gemini(reason: &str) -> &'static str {
    match reason {
        "length" => "MAX_TOKENS",
        "content_filter" => "SAFETY",
        _ => "STOP",
    }
}

fn gemini_finish_reason_to_anthropic(reason: Option<&str>) -> &'static str {
    match reason {
        Some("MAX_TOKENS") => "max_tokens",
        Some("STOP") | None => "end_turn",
        _ => "stop_sequence",
    }
}

pub(super) fn anthropic_stop_reason_to_openai(reason: Option<&str>) -> &'static str {
    match reason {
        Some("tool_use") => "tool_calls",
        Some("max_tokens" | "model_context_window_exceeded") => "length",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

fn anthropic_responses_incomplete_reason(reason: Option<&str>) -> Option<&'static str> {
    match reason {
        Some("refusal") => Some("content_filter"),
        Some("max_tokens" | "model_context_window_exceeded") => Some("max_output_tokens"),
        _ => None,
    }
}

fn anthropic_stop_reason_to_gemini(reason: Option<&str>) -> &'static str {
    match reason {
        Some("max_tokens") => "MAX_TOKENS",
        _ => "STOP",
    }
}

fn openai_chat_tool_delta_to_anthropic_start(tool_call: &Value) -> StreamFrame {
    let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
    StreamFrame::event(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {
                "type": "tool_use",
                "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                "name": tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool"),
                "input": {}
            }
        }),
    )
}

fn gemini_function_call_to_anthropic_frames(function_call: &Value, index: u64) -> Vec<StreamFrame> {
    let mut frames = vec![StreamFrame::event(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {
                "type": "tool_use",
                "id": function_call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                "name": function_call.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "input": {}
            }
        }),
    )];
    if let Some(args) = function_call
        .get("args")
        .or_else(|| function_call.get("arguments"))
    {
        if !args.is_null() {
            frames.push(StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
                    }
                }),
            ));
        }
    }
    frames
}

pub(super) fn anthropic_usage_from_openai_usage(usage: Option<&Value>) -> Value {
    let inclusive_input_tokens = usage_number(
        usage,
        &[
            &["prompt_tokens"],
            &["input_tokens"],
            &["total_prompt_tokens"],
        ],
    )
    .unwrap_or(0);
    let output_tokens =
        usage_number(usage, &[&["completion_tokens"], &["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(
        usage,
        &[
            &["prompt_tokens_details", "cached_tokens"],
            &["input_tokens_details", "cached_tokens"],
        ],
    );
    let cache_creation = usage_number(
        usage,
        &[
            &["cache_creation_input_tokens"],
            &["input_tokens_details", "cache_creation_tokens"],
            &["input_tokens_details", "cache_write_tokens"],
            &["prompt_tokens_details", "cache_creation_tokens"],
            &["prompt_tokens_details", "cache_write_tokens"],
        ],
    );
    let input_tokens = inclusive_input_tokens
        .saturating_sub(cache_read.unwrap_or(0))
        .saturating_sub(cache_creation.unwrap_or(0));
    let mut output = Map::new();
    output.insert("input_tokens".to_string(), json!(input_tokens));
    output.insert("output_tokens".to_string(), json!(output_tokens));
    if let Some(cache_read) = cache_read {
        output.insert("cache_read_input_tokens".to_string(), json!(cache_read));
    }
    if let Some(cache_creation) = cache_creation {
        output.insert(
            "cache_creation_input_tokens".to_string(),
            json!(cache_creation),
        );
    }
    Value::Object(output)
}

fn anthropic_usage_from_gemini_usage(usage: Option<&Value>) -> Value {
    let inclusive_input_tokens =
        usage_number(usage, &[&["promptTokenCount"], &["prompt_token_count"]]).unwrap_or(0);
    let output_tokens = usage_number(
        usage,
        &[&["candidatesTokenCount"], &["candidates_token_count"]],
    )
    .unwrap_or(0);
    let cache_read = usage_number(
        usage,
        &[
            &["cachedContentTokenCount"],
            &["cached_content_token_count"],
        ],
    );
    let input_tokens = inclusive_input_tokens.saturating_sub(cache_read.unwrap_or(0));
    let mut output = Map::new();
    output.insert("input_tokens".to_string(), json!(input_tokens));
    output.insert("output_tokens".to_string(), json!(output_tokens));
    if let Some(cache_read) = cache_read {
        output.insert("cache_read_input_tokens".to_string(), json!(cache_read));
    }
    Value::Object(output)
}

fn openai_usage_from_anthropic_usage(usage: Option<&Value>) -> Value {
    let fresh_input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
    let cache_creation = usage_number(usage, &[&["cache_creation_input_tokens"]]);
    let input_tokens = fresh_input_tokens
        .saturating_add(cache_read.unwrap_or(0))
        .saturating_add(cache_creation.unwrap_or(0));
    let mut output = json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "prompt_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)}
    });
    if let Some(cache_creation) = cache_creation {
        output["cache_creation_input_tokens"] = json!(cache_creation);
        output["prompt_tokens_details"]["cached_creation_tokens"] = json!(cache_creation);
    }
    output
}

pub(super) fn openai_responses_usage_from_anthropic_usage(usage: Option<&Value>) -> Value {
    let fresh_input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
    let cache_creation = usage_number(usage, &[&["cache_creation_input_tokens"]]);
    let input_tokens = fresh_input_tokens
        .saturating_add(cache_read.unwrap_or(0))
        .saturating_add(cache_creation.unwrap_or(0));
    let mut output = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "input_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)}
    });
    if let Some(cache_creation) = cache_creation {
        output["cache_creation_input_tokens"] = json!(cache_creation);
        output["input_tokens_details"]["cache_write_tokens"] = json!(cache_creation);
    }
    output
}

pub(super) fn openai_chat_usage_from_responses_usage(usage: Option<&Value>) -> Value {
    let prompt_tokens = usage_number(usage, &[&["input_tokens"], &["prompt_tokens"]]).unwrap_or(0);
    let completion_tokens =
        usage_number(usage, &[&["output_tokens"], &["completion_tokens"]]).unwrap_or(0);
    let total_tokens =
        usage_number(usage, &[&["total_tokens"]]).unwrap_or(prompt_tokens + completion_tokens);
    let cached_tokens = usage_number(
        usage,
        &[
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
        ],
    )
    .unwrap_or(0);

    let mut output = Map::new();
    output.insert("prompt_tokens".to_string(), json!(prompt_tokens));
    output.insert("completion_tokens".to_string(), json!(completion_tokens));
    output.insert("total_tokens".to_string(), json!(total_tokens));
    output.insert(
        "prompt_tokens_details".to_string(),
        json!({"cached_tokens": cached_tokens}),
    );
    if let Some(cache_creation) = usage_number(
        usage,
        &[
            &["cache_creation_input_tokens"],
            &["input_tokens_details", "cache_creation_tokens"],
            &["input_tokens_details", "cache_write_tokens"],
        ],
    ) {
        output.insert(
            "cache_creation_input_tokens".to_string(),
            json!(cache_creation),
        );
        output["prompt_tokens_details"]["cached_creation_tokens"] = json!(cache_creation);
    }
    if let Some(details) = usage
        .and_then(|usage| usage.get("output_tokens_details"))
        .or_else(|| usage.and_then(|usage| usage.get("completion_tokens_details")))
    {
        output.insert("completion_tokens_details".to_string(), details.clone());
    }
    Value::Object(output)
}

pub(super) fn openai_responses_usage_from_chat_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage_number(usage, &[&["prompt_tokens"], &["input_tokens"]]).unwrap_or(0);
    let output_tokens =
        usage_number(usage, &[&["completion_tokens"], &["output_tokens"]]).unwrap_or(0);
    let total_tokens =
        usage_number(usage, &[&["total_tokens"]]).unwrap_or(input_tokens + output_tokens);
    let cached_tokens = usage_number(
        usage,
        &[
            &["prompt_tokens_details", "cached_tokens"],
            &["input_tokens_details", "cached_tokens"],
        ],
    )
    .unwrap_or(0);

    let mut output = Map::new();
    output.insert("input_tokens".to_string(), json!(input_tokens));
    output.insert("output_tokens".to_string(), json!(output_tokens));
    output.insert("total_tokens".to_string(), json!(total_tokens));
    output.insert(
        "input_tokens_details".to_string(),
        json!({"cached_tokens": cached_tokens}),
    );
    if let Some(cache_creation) = usage_number(
        usage,
        &[
            &["cache_creation_input_tokens"],
            &["prompt_tokens_details", "cached_creation_tokens"],
            &["prompt_tokens_details", "cache_creation_tokens"],
            &["prompt_tokens_details", "cache_write_tokens"],
        ],
    ) {
        output.insert(
            "cache_creation_input_tokens".to_string(),
            json!(cache_creation),
        );
        output["input_tokens_details"]["cache_write_tokens"] = json!(cache_creation);
    }
    if let Some(details) = usage
        .and_then(|usage| usage.get("completion_tokens_details"))
        .or_else(|| usage.and_then(|usage| usage.get("output_tokens_details")))
    {
        output.insert("output_tokens_details".to_string(), details.clone());
    }
    Value::Object(output)
}

fn openai_usage_from_gemini_usage(usage: Option<&Value>) -> Value {
    let input_tokens =
        usage_number(usage, &[&["promptTokenCount"], &["prompt_token_count"]]).unwrap_or(0);
    let output_tokens = usage_number(
        usage,
        &[&["candidatesTokenCount"], &["candidates_token_count"]],
    )
    .unwrap_or(0);
    let cache_read = usage_number(
        usage,
        &[
            &["cachedContentTokenCount"],
            &["cached_content_token_count"],
        ],
    );
    json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "prompt_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)}
    })
}

fn gemini_usage_from_anthropic_usage(usage: Option<&Value>) -> Value {
    let fresh_input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
    let cache_creation = usage_number(usage, &[&["cache_creation_input_tokens"]]);
    let input_tokens = fresh_input_tokens
        .saturating_add(cache_read.unwrap_or(0))
        .saturating_add(cache_creation.unwrap_or(0));
    json!({
        "promptTokenCount": input_tokens,
        "candidatesTokenCount": output_tokens,
        "totalTokenCount": input_tokens + output_tokens,
        "cachedContentTokenCount": cache_read.unwrap_or(0)
    })
}

fn gemini_usage_from_openai_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage_number(
        usage,
        &[
            &["prompt_tokens"],
            &["input_tokens"],
            &["total_prompt_tokens"],
        ],
    )
    .unwrap_or(0);
    let output_tokens =
        usage_number(usage, &[&["completion_tokens"], &["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(
        usage,
        &[
            &["prompt_tokens_details", "cached_tokens"],
            &["input_tokens_details", "cached_tokens"],
        ],
    );
    json!({
        "promptTokenCount": input_tokens,
        "candidatesTokenCount": output_tokens,
        "totalTokenCount": input_tokens + output_tokens,
        "cachedContentTokenCount": cache_read.unwrap_or(0)
    })
}

fn usage_number(usage: Option<&Value>, paths: &[&[&str]]) -> Option<i64> {
    let usage = usage?;
    for path in paths {
        let mut cursor = usage;
        let mut found = true;
        for key in *path {
            if let Some(next) = cursor.get(*key) {
                cursor = next;
            } else {
                found = false;
                break;
            }
        }
        if found {
            if let Some(value) = cursor.as_i64() {
                return Some(value);
            }
            if let Some(value) = cursor.as_u64() {
                return Some(value as i64);
            }
            if let Some(value) = cursor.as_f64() {
                return Some(value as i64);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::store::usage_from_json;

    fn frame_json(frame: &StreamFrame) -> &Value {
        match &frame.payload {
            StreamPayload::Json(value) => value,
            other => panic!("expected json payload: {other:?}"),
        }
    }

    #[test]
    fn openai_chat_to_anthropic_preserves_tools_thinking_cache_and_image() {
        let input = json!({
            "model": "gpt-5.5",
            "max_completion_tokens": 2048,
            "stream": true,
            "metadata": {"user_id": "u1"},
            "reasoning": {"effort": "medium", "summary": "auto"},
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Lookup data",
                    "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}
                }
            }],
            "messages": [
                {"role": "system", "content": "system text"},
                {"role": "user", "content": [
                    {"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}
                ]},
                {"role": "assistant", "content": "checking", "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{\"q\":\"x\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "result"},
                {"role": "assistant", "content": "done"},
                {"role": "user", "content": "continue"}
            ]
        });

        let output = openai_chat_to_anthropic(&input).unwrap();

        assert_eq!(output["system"], "system text");
        assert_eq!(
            output.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(
            output
                .pointer("/thinking/budget_tokens")
                .and_then(Value::as_u64),
            Some(1024)
        );
        assert!(output.pointer("/thinking/effort").is_none());
        assert!(output.pointer("/thinking/summary").is_none());
        assert_eq!(
            output.pointer("/tools/0/name").and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/messages/0/content/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            output
                .pointer("/messages/1/content/1/type")
                .and_then(Value::as_str),
            Some("tool_use")
        );
        assert_eq!(
            output
                .pointer("/messages/2/content/0/type")
                .and_then(Value::as_str),
            Some("tool_result")
        );
        assert_eq!(
            output.pointer("/metadata/user_id").and_then(Value::as_str),
            Some("u1")
        );
        assert_eq!(output["max_tokens"], 2048);
    }

    #[test]
    fn openai_chat_to_anthropic_validates_tool_arguments_and_drops_incomplete_turns() {
        for arguments in [json!("{broken"), json!("[]"), json!(1)] {
            let error = openai_chat_to_anthropic(&json!({
                "model": "claude-sonnet-4-5",
                "messages": [
                    {"role": "user", "content": "run"},
                    {"role": "assistant", "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "lookup", "arguments": arguments}
                    }]}
                ]
            }))
            .expect_err("invalid chat tool arguments must fail closed");
            assert!(error.to_string().contains("arguments"));
        }

        let output = openai_chat_to_anthropic(&json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {"role": "tool", "tool_call_id": "orphan", "content": "ignored"},
                {"role": "user", "content": "run"},
                {"role": "assistant", "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}
                }]},
                {"role": "user", "content": "continue"}
            ]
        }))
        .unwrap();

        let serialized = output["messages"].to_string();
        assert!(!serialized.contains("tool_use"));
        assert!(!serialized.contains("tool_result"));
        assert!(serialized.contains("run"));
        assert!(serialized.contains("continue"));
    }

    #[test]
    fn openai_responses_to_anthropic_preserves_input_image_reasoning_and_usage_shape() {
        let input = json!({
            "model": "gpt-5.5",
            "max_output_tokens": 4096,
            "reasoning": {"effort": "low"},
            "input": [{
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "describe"},
                    {"type": "input_image", "image_url": "data:image/jpeg;base64,BB=="}
                ]
            }]
        });

        let output = openai_responses_to_anthropic(&input).unwrap();
        assert_eq!(
            output
                .pointer("/messages/0/content/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/jpeg")
        );
        assert_eq!(
            output.pointer("/thinking/type").and_then(Value::as_str),
            Some("enabled")
        );
        assert_eq!(output["thinking"]["budget_tokens"], 2048);
        assert_eq!(output["max_tokens"], 4096);

        let usage = usage_from_json(&json!({
            "response": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 4,
                    "input_tokens_details": {"cached_tokens": 60}
                }
            }
        }));
        assert_eq!(usage.raw_input_tokens, Some(100));
        assert_eq!(usage.cache_read_tokens, Some(60));
        assert_eq!(usage.total_tokens, Some(104));
    }

    #[test]
    fn openai_reasoning_maps_to_valid_anthropic_legacy_and_adaptive_shapes() {
        let adaptive = openai_responses_to_anthropic(&json!({
            "model": "anthropic.claude-sonnet-4-6-20250514-v1:0",
            "max_output_tokens": 4096,
            "temperature": 0.2,
            "top_p": 0.9,
            "reasoning": {"effort": "high", "summary": "auto"},
            "input": "ping"
        }))
        .unwrap();

        assert_eq!(adaptive["thinking"], json!({"type": "adaptive"}));
        assert_eq!(adaptive["output_config"], json!({"effort": "high"}));
        assert!(adaptive.get("temperature").is_none());
        assert!(adaptive.get("top_p").is_none());
        assert!(adaptive.pointer("/thinking/summary").is_none());

        let legacy = openai_chat_to_anthropic(&json!({
            "model": "claude-sonnet-4-5",
            "max_completion_tokens": 8192,
            "reasoning_effort": "high",
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .unwrap();

        assert_eq!(
            legacy["thinking"],
            json!({"type": "enabled", "budget_tokens": 4096})
        );
        assert!(legacy.get("output_config").is_none());
    }

    #[test]
    fn openai_reasoning_disable_and_small_budget_preserve_sampling() {
        let disabled = openai_chat_to_anthropic(&json!({
            "model": "claude-sonnet-4-5",
            "max_completion_tokens": 4096,
            "reasoning_effort": "none",
            "temperature": 0.2,
            "top_p": 0.9,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .unwrap();

        assert_eq!(disabled["thinking"], json!({"type": "disabled"}));
        assert_eq!(disabled["temperature"], 0.2);
        assert_eq!(disabled["top_p"], 0.9);

        let too_small = openai_responses_to_anthropic(&json!({
            "model": "claude-sonnet-4-5",
            "max_output_tokens": 1024,
            "temperature": 0.3,
            "reasoning": {"effort": "high"},
            "input": "ping"
        }))
        .unwrap();

        assert!(too_small.get("thinking").is_none());
        assert_eq!(too_small["temperature"], 0.3);

        let required = openai_responses_to_anthropic(&json!({
            "model": "claude-fable-5",
            "reasoning": {"effort": "disabled"},
            "input": "ping"
        }))
        .unwrap();
        assert_eq!(required["thinking"], json!({"type": "adaptive"}));
        assert_eq!(required["output_config"], json!({"effort": "low"}));
    }

    #[test]
    fn anthropic_tool_continuation_requires_signed_thinking_history() {
        let unsigned = openai_responses_to_anthropic(&json!({
            "model": "claude-sonnet-4-6",
            "max_output_tokens": 4096,
            "temperature": 0.4,
            "reasoning": {"effort": "high"},
            "input": [
                {"role": "user", "content": "run"},
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "ok"}
            ]
        }))
        .unwrap();
        assert_eq!(unsigned["thinking"], json!({"type": "disabled"}));
        assert_eq!(unsigned["temperature"], 0.4);

        let signed_reasoning = responses_reasoning_item_from_anthropic_block(
            "rs_1",
            &json!({
                "type": "thinking",
                "thinking": "use the tool",
                "signature": "anthropic-signature"
            }),
        )
        .unwrap();
        let signed = openai_responses_to_anthropic(&json!({
            "model": "claude-sonnet-4-6",
            "max_output_tokens": 4096,
            "reasoning": {"effort": "high"},
            "input": [
                {"role": "user", "content": "run"},
                signed_reasoning,
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "ok"}
            ]
        }))
        .unwrap();
        assert_eq!(signed["thinking"], json!({"type": "adaptive"}));

        let error = openai_responses_to_anthropic(&json!({
            "model": "claude-fable-5",
            "input": [
                {"role": "user", "content": "run"},
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "ok"}
            ]
        }))
        .expect_err("required thinking cannot continue unsigned tool history");
        assert!(error.to_string().contains("no signed thinking block"));
    }

    #[test]
    fn forced_anthropic_tool_choice_disables_optional_thinking() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "claude-sonnet-4-6",
            "temperature": 0.3,
            "reasoning": {"effort": "high"},
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"}
            }],
            "tool_choice": {"type": "function", "name": "lookup"},
            "input": "ping"
        }))
        .unwrap();

        assert_eq!(output["thinking"], json!({"type": "disabled"}));
        assert!(output.get("output_config").is_none());
        assert_eq!(output["temperature"], 0.3);
        assert_eq!(
            output["tool_choice"],
            json!({"type": "tool", "name": "lookup"})
        );

        let error = openai_responses_to_anthropic(&json!({
            "model": "claude-fable-5",
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"}
            }],
            "tool_choice": "required",
            "input": "ping"
        }))
        .expect_err("required thinking cannot honor forced tools");
        assert!(error
            .to_string()
            .contains("cannot honor a forced tool_choice"));
    }

    #[test]
    fn openai_responses_to_anthropic_extracts_system_and_normalizes_leading_assistant() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "instructions": "top-level policy",
            "input": [
                {"role": "system", "content": "system policy"},
                {"role": "developer", "content": [{"type": "input_text", "text": "developer policy"}]},
                {"role": "assistant", "content": [{"type": "output_text", "text": "prior answer"}]}
            ]
        }))
        .unwrap();

        assert_eq!(
            output.get("system").and_then(Value::as_str),
            Some("top-level policy\n\nsystem policy\n\ndeveloper policy")
        );
        assert_eq!(output["messages"][0]["role"], "user");
        assert_eq!(
            output["messages"][0]["content"][0]["text"],
            "(continuing the conversation)"
        );
        assert_eq!(output["messages"][1]["role"], "assistant");
        assert_eq!(output["messages"][1]["content"][0]["text"], "prior answer");
        assert_eq!(output["max_tokens"], DEFAULT_OPENAI_TO_ANTHROPIC_MAX_TOKENS);
    }

    #[test]
    fn openai_responses_to_anthropic_maps_request_controls() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "max_output_tokens": 1024,
            "temperature": 0.2,
            "top_p": 0.9,
            "stop": ["END", "STOP"],
            "parallel_tool_calls": false,
            "tool_choice": {"type": "function", "name": "lookup"},
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"}
            }],
            "input": "ping"
        }))
        .unwrap();

        assert_eq!(output["max_tokens"], 1024);
        assert_eq!(output["temperature"], 0.2);
        assert_eq!(output["top_p"], 0.9);
        assert_eq!(output["stop_sequences"], json!(["END", "STOP"]));
        assert_eq!(output["tool_choice"]["type"], "tool");
        assert_eq!(output["tool_choice"]["name"], "lookup");
        assert_eq!(output["tool_choice"]["disable_parallel_tool_use"], true);

        let unsupported = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "tools": [{"type": "web_search_preview"}],
            "tool_choice": "required",
            "input": "ping"
        }))
        .unwrap();
        assert!(unsupported.get("tools").is_none());
        assert!(unsupported.get("tool_choice").is_none());
    }

    #[test]
    fn anthropic_request_normalization_matches_bridge_contract_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/request_normalization.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "anthropic-request-normalization");
        assert_eq!(fixture["category"], "request_normalization");

        let output = openai_responses_to_anthropic(&fixture["input"]).unwrap();
        let expected = &fixture["expected"];
        assert_eq!(output["system"], expected["system"]);
        assert_eq!(output["max_tokens"], expected["maxTokens"]);
        assert_eq!(
            output["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message["role"].clone())
                .collect::<Vec<_>>(),
            expected["roles"].as_array().unwrap().clone()
        );
        assert_eq!(
            output["messages"][1]["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block["id"].clone())
                .collect::<Vec<_>>(),
            expected["toolUseIds"].as_array().unwrap().clone()
        );
        assert_eq!(
            output["messages"][2]["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "tool_result")
                .map(|block| block["tool_use_id"].clone())
                .collect::<Vec<_>>(),
            expected["toolResultIds"].as_array().unwrap().clone()
        );
        assert_eq!(
            output["messages"][2]["content"][2]["text"],
            expected["trailingText"]
        );
        assert_eq!(output["tool_choice"]["type"], expected["toolChoiceType"]);
        assert_eq!(
            output["tool_choice"]["disable_parallel_tool_use"],
            expected["disableParallelToolUse"]
        );

        let error = openai_responses_to_anthropic(&fixture["invalidArgumentsInput"])
            .expect_err("invalid completed arguments must fail closed");
        assert!(error
            .to_string()
            .contains("invalid function_call arguments"));
    }

    #[test]
    fn openai_responses_to_anthropic_keeps_complete_parallel_tool_turn() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "input": [
                {"role": "user", "content": "run both"},
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": "{\"q\":1}"},
                {"type": "function_call", "call_id": "c2", "name": "lookup", "arguments": "{\"q\":2}"},
                {"role": "user", "content": [{"type": "input_text", "text": "then summarize"}]},
                {"type": "function_call_output", "call_id": "c1", "output": "one"},
                {"type": "function_call_output", "call_id": "c2", "output": "two"}
            ]
        }))
        .unwrap();

        assert_eq!(
            output["messages"][1]["content"].as_array().unwrap().len(),
            2
        );
        assert_eq!(output["messages"][2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(output["messages"][2]["content"][1]["tool_use_id"], "c2");
        assert_eq!(
            output["messages"][2]["content"][2]["text"],
            "then summarize"
        );
    }

    #[test]
    fn openai_responses_to_anthropic_drops_unverifiable_reasoning_history() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "claude-sonnet-4-5",
            "input": [
                {"role": "user", "content": "before"},
                {
                    "id": "rs_openai",
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "visible"}],
                    "encrypted_content": "openai-provider-opaque"
                },
                {"role": "user", "content": "after"}
            ]
        }))
        .unwrap();

        let serialized = output["messages"].to_string();
        assert!(!serialized.contains("thinking"));
        assert!(!serialized.contains("openai-provider-opaque"));
        assert!(!serialized.contains("ccswitch-server-reasoning-v1"));
    }

    #[test]
    fn openai_responses_to_anthropic_drops_partial_and_incomplete_tool_turns() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "input": [
                {"role": "user", "content": "run both"},
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": "{}"},
                {"type": "function_call", "call_id": "c2", "name": "lookup", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "one"},
                {"role": "user", "content": "continue"},
                {"type": "function_call", "call_id": "c3", "name": "lookup", "arguments": "{broken", "status": "incomplete"},
                {"type": "function_call_output", "call_id": "c3", "output": "never ran"}
            ]
        }))
        .unwrap();

        let serialized = output["messages"].to_string();
        assert!(!serialized.contains("tool_use"));
        assert!(!serialized.contains("tool_result"));
        assert!(serialized.contains("run both"));
        assert!(serialized.contains("continue"));
    }

    #[test]
    fn openai_responses_to_anthropic_requires_object_function_arguments() {
        let empty = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "input": [
                {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": ""},
                {"type": "function_call_output", "call_id": "c1", "output": "ok"}
            ]
        }))
        .unwrap();
        assert_eq!(empty["messages"][1]["content"][0]["input"], json!({}));

        for arguments in [json!("{broken"), json!("[1,2]"), json!([1, 2])] {
            let error = openai_responses_to_anthropic(&json!({
                "model": "gpt-5.5",
                "input": [
                    {"type": "function_call", "call_id": "c1", "name": "lookup", "arguments": arguments},
                    {"type": "function_call_output", "call_id": "c1", "output": "ok"}
                ]
            }))
            .unwrap_err();
            assert!(error.to_string().contains("function_call arguments"));
        }
    }

    #[test]
    fn openai_responses_to_anthropic_drops_orphan_results_and_fails_closed() {
        let output = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "input": [
                {"type": "function_call_output", "call_id": "ghost", "output": "ignored"},
                {"role": "user", "content": "keep this"}
            ]
        }))
        .unwrap();
        assert_eq!(output["messages"].as_array().unwrap().len(), 1);
        assert_eq!(output["messages"][0]["content"][0]["text"], "keep this");

        let error = openai_responses_to_anthropic(&json!({
            "model": "gpt-5.5",
            "input": [
                {"type": "function_call_output", "call_id": "ghost", "output": "ignored"}
            ]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("no valid anthropic messages"));
    }

    #[test]
    fn openai_chat_to_responses_preserves_codex_bridge_fields() {
        let input = json!({
            "model": "gpt-5.5",
            "max_completion_tokens": 16,
            "reasoning_effort": "low",
            "response_format": {"type": "json_object"},
            "stream": true,
            "store": false,
            "parallel_tool_calls": true,
            "metadata": {"trace": "t1"},
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Lookup data",
                    "parameters": {"type": "object"},
                    "strict": true
                }
            }],
            "tool_choice": {"type": "function", "function": {"name": "lookup"}},
            "messages": [
                {"role": "system", "content": "system text"},
                {"role": "developer", "content": [{"type": "text", "text": "developer text"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "describe"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}
                ]},
                {"role": "assistant", "content": "checking", "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{\"q\":\"x\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": {"ok": true}}
            ]
        });

        let output = openai_chat_to_responses(&input).unwrap();

        assert_eq!(
            output.get("instructions").and_then(Value::as_str),
            Some("system text\n\ndeveloper text")
        );
        assert_eq!(
            output.get("max_output_tokens").and_then(Value::as_i64),
            Some(16)
        );
        assert_eq!(
            output.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("low")
        );
        assert_eq!(
            output.pointer("/text/format/type").and_then(Value::as_str),
            Some("json_object")
        );
        assert_eq!(
            output.pointer("/tools/0/name").and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output.pointer("/tool_choice/name").and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/input/0/content/1/type")
                .and_then(Value::as_str),
            Some("input_image")
        );
        assert_eq!(
            output.pointer("/input/2/type").and_then(Value::as_str),
            Some("function_call")
        );
        assert_eq!(
            output.pointer("/input/3/output").and_then(Value::as_str),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn openai_responses_to_chat_preserves_multimodal_request_fields() {
        let input = json!({
            "model": "gpt-5.5",
            "instructions": ["system text", {"type": "message", "content": "developer text"}],
            "max_output_tokens": 32,
            "reasoning": {"effort": "high"},
            "stream": true,
            "parallel_tool_calls": true,
            "text": {"format": {"type": "json_object"}},
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "describe"},
                    {"type": "input_image", "image_url": "data:image/png;base64,AA=="},
                    {"type": "input_file", "file_id": "file_123", "filename": "notes.txt"}
                ]
            }]
        });

        let output = openai_responses_to_chat(&input).unwrap();

        assert_eq!(
            output.pointer("/messages/0/role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            output
                .pointer("/messages/1/content/1/type")
                .and_then(Value::as_str),
            Some("image_url")
        );
        assert_eq!(
            output
                .pointer("/messages/1/content/2/file/file_id")
                .and_then(Value::as_str),
            Some("file_123")
        );
        assert_eq!(
            output.get("max_completion_tokens").and_then(Value::as_i64),
            Some(32)
        );
        assert_eq!(
            output.get("reasoning_effort").and_then(Value::as_str),
            Some("high")
        );
        assert_eq!(
            output
                .pointer("/response_format/type")
                .and_then(Value::as_str),
            Some("json_object")
        );
        assert_eq!(
            output.get("parallel_tool_calls").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn openai_responses_to_chat_maps_function_call_and_output_items() {
        let input = json!({
            "model": "gpt-5.5",
            "input": [
                {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{\"q\":\"x\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": {"ok": true}}
            ]
        });

        let output = openai_responses_to_chat(&input).unwrap();

        assert_eq!(
            output
                .pointer("/messages/0/tool_calls/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/messages/1/tool_call_id")
                .and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            output
                .pointer("/messages/1/content/ok")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn openai_responses_to_chat_maps_function_tools_and_tool_choice() {
        let input = json!({
            "model": "gpt-5.5",
            "input": "hello",
            "tools": [
                {"type": "function", "name": "lookup", "description": "Lookup", "parameters": {"type": "object"}, "strict": true},
                {"type": "web_search_preview"}
            ],
            "tool_choice": {"type": "function", "name": "lookup"}
        });

        let output = openai_responses_to_chat(&input).unwrap();

        assert_eq!(
            output
                .pointer("/tools/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output.get("tools").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert_eq!(
            output
                .pointer("/tool_choice/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
    }

    #[test]
    fn openai_responses_response_to_chat_preserves_tools_and_usage() {
        let input = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "created_at": 123,
            "model": "gpt-5.5",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "hello"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"q\":\"x\"}"
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 4,
                "total_tokens": 14,
                "input_tokens_details": {"cached_tokens": 2},
                "output_tokens_details": {"reasoning_tokens": 1}
            }
        });

        let output = openai_responses_response_to_chat(&input).unwrap();

        assert_eq!(
            output
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            output
                .pointer("/choices/0/message/tool_calls/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("tool_calls")
        );
        assert_eq!(
            output
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_i64),
            Some(10)
        );
        assert_eq!(
            output
                .pointer("/usage/completion_tokens_details/reasoning_tokens")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn openai_responses_response_to_chat_maps_incomplete_status_to_length() {
        let input = json!({
            "id": "resp_1",
            "object": "response",
            "status": "incomplete",
            "model": "gpt-5.5",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "partial"}]
            }]
        });

        let output = openai_responses_response_to_chat(&input).unwrap();

        assert_eq!(
            output
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("length")
        );
    }

    #[test]
    fn response_finish_reason_matrix_maps_across_protocols() {
        for (anthropic, openai, gemini) in [
            ("end_turn", "stop", "STOP"),
            ("tool_use", "tool_calls", "STOP"),
            ("max_tokens", "length", "MAX_TOKENS"),
        ] {
            let input = json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4",
                "content": [{"type": "text", "text": "hello"}],
                "stop_reason": anthropic
            });

            let chat = anthropic_response_to_openai_chat(&input).unwrap();
            assert_eq!(
                chat.pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str),
                Some(openai)
            );

            let gemini_output = anthropic_response_to_gemini(&input).unwrap();
            assert_eq!(
                gemini_output
                    .pointer("/candidates/0/finishReason")
                    .and_then(Value::as_str),
                Some(gemini)
            );
        }
    }

    #[test]
    fn gemini_native_to_anthropic_preserves_schema_safety_tools_and_media() {
        let input = json!({
            "model": "gemini-2.5-pro",
            "systemInstruction": {"parts": [{"text": "system"}]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "hello"},
                    {"inlineData": {"mimeType": "image/png", "data": "AA=="}},
                    {"functionCall": {"name": "lookup", "args": {"q": "x"}}}
                ]
            }],
            "tools": [{"functionDeclarations": [{"name": "lookup", "parameters": {"type": "object"}}]}],
            "generationConfig": {"responseSchema": {"type": "object"}},
            "safetySettings": [{"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE"}]
        });

        let output = gemini_native_to_anthropic(&input).unwrap();

        assert_eq!(output["system"], "system");
        assert_eq!(
            output
                .pointer("/messages/0/content/1/source/media_type")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            output
                .pointer("/messages/0/content/2/type")
                .and_then(Value::as_str),
            Some("tool_use")
        );
        assert_eq!(
            output.pointer("/tools/0/name").and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/metadata/geminiGenerationConfig/responseSchema/type")
                .and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            output
                .pointer("/metadata/geminiSafetySettings/0/threshold")
                .and_then(Value::as_str),
            Some("BLOCK_NONE")
        );
    }

    #[test]
    fn anthropic_to_openai_chat_and_responses_preserve_tool_cache_and_image() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "system",
            "thinking": {"type": "enabled", "effort": "high"},
            "tools": [{"name": "lookup", "input_schema": {"type": "object"}}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"q": "x"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "result"}
                ]}
            ]
        });

        let chat = anthropic_to_openai_chat(&input).unwrap();
        assert_eq!(
            chat.pointer("/messages/0/role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            chat.pointer("/messages/1/content/0/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral")
        );
        assert_eq!(
            chat.pointer("/messages/2/tool_calls/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            chat.pointer("/messages/3/role").and_then(Value::as_str),
            Some("tool")
        );
        assert_eq!(
            chat.pointer("/reasoning/effort").and_then(Value::as_str),
            Some("high")
        );

        let responses = anthropic_to_openai_responses(&input).unwrap();
        assert_eq!(
            responses
                .pointer("/input/0/content/1/type")
                .and_then(Value::as_str),
            Some("input_image")
        );
        assert_eq!(
            responses
                .pointer("/tools/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
    }

    #[test]
    fn translated_explicit_reasoning_effort_survives_claude_and_gemini_inputs() {
        let anthropic = json!({
            "model": "gpt-5.6-sol",
            "output_config": {"effort": "MAX"},
            "messages": [{"role": "user", "content": "ping"}]
        });
        let responses = anthropic_to_openai_responses(&anthropic).unwrap();
        assert_eq!(
            responses
                .pointer("/reasoning/effort")
                .and_then(Value::as_str),
            Some("max")
        );

        let gemini = json!({
            "model": "gpt-5.6-sol",
            "contents": [{"role": "user", "parts": [{"text": "ping"}]}],
            "generationConfig": {
                "thinkingConfig": {"thinkingLevel": "HIGH"}
            }
        });
        let anthropic = gemini_native_to_anthropic(&gemini).unwrap();
        let responses = anthropic_to_openai_responses(&anthropic).unwrap();
        assert_eq!(
            responses
                .pointer("/reasoning/effort")
                .and_then(Value::as_str),
            Some("high")
        );
    }

    #[test]
    fn anthropic_to_gemini_preserves_system_tools_schema_safety_and_usage_metadata() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "system",
            "metadata": {
                "geminiGenerationConfig": {"responseSchema": {"type": "object"}},
                "geminiSafetySettings": [{"threshold": "BLOCK_NONE"}]
            },
            "tools": [{"name": "lookup", "input_schema": {"type": "object"}}],
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}}
                ]
            }]
        });

        let output = anthropic_to_gemini_native(&input).unwrap();
        assert_eq!(
            output
                .pointer("/systemInstruction/parts/0/text")
                .and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            output
                .pointer("/contents/0/parts/1/inlineData/mimeType")
                .and_then(Value::as_str),
            Some("image/png")
        );
        assert_eq!(
            output
                .pointer("/tools/0/functionDeclarations/0/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            output
                .pointer("/generationConfig/responseSchema/type")
                .and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            output
                .pointer("/safetySettings/0/threshold")
                .and_then(Value::as_str),
            Some("BLOCK_NONE")
        );

        let usage = usage_from_json(&json!({
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 3,
                "cachedContentTokenCount": 6,
                "totalTokenCount": 13
            }
        }));
        assert_eq!(usage.input_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(6));
        assert_eq!(usage.total_tokens, Some(13));
    }

    #[test]
    fn response_snapshots_convert_openai_responses_to_anthropic() {
        let input = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.5",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hello"}]
            }],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 8,
                "input_tokens_details": {"cached_tokens": 60}
            }
        });

        let output = openai_responses_response_to_anthropic(&input).unwrap();

        assert_eq!(
            output,
            json!({
                "id": "resp_1",
                "type": "message",
                "role": "assistant",
                "model": "gpt-5.5",
                "content": [{"type": "text", "text": "hello"}],
                "stop_reason": "end_turn",
                "stop_sequence": Value::Null,
                "usage": {
                    "input_tokens": 40,
                    "output_tokens": 8,
                    "cache_read_input_tokens": 60
                }
            })
        );
    }

    #[test]
    fn anthropic_response_bridges_preserve_empty_content_as_an_array() {
        let chat = openai_chat_response_to_anthropic(&json!({
            "id": "chatcmpl-empty",
            "model": "empty-model",
            "choices": [{
                "message": {"role": "assistant", "content": ""},
                "finish_reason": "stop"
            }]
        }))
        .unwrap();
        let responses = openai_responses_response_to_anthropic(&json!({
            "id": "resp-empty",
            "model": "empty-model",
            "status": "completed",
            "output": []
        }))
        .unwrap();
        let gemini = gemini_response_to_anthropic(&json!({
            "responseId": "gem-empty",
            "modelVersion": "empty-model",
            "candidates": [{
                "content": {"role": "model", "parts": []},
                "finishReason": "STOP"
            }]
        }))
        .unwrap();

        for output in [chat, responses, gemini] {
            assert_eq!(output["content"], json!([]));
            assert_eq!(output["stop_reason"], json!("end_turn"));
            assert_eq!(output["stop_sequence"], Value::Null);
        }
    }

    #[test]
    fn chat_response_maps_legacy_function_call_to_anthropic_tool_use() {
        let output = openai_chat_response_to_anthropic(&json!({
            "id": "chatcmpl-legacy",
            "model": "legacy-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "function_call": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                },
                "finish_reason": "function_call"
            }]
        }))
        .unwrap();

        assert_eq!(output["stop_reason"], json!("tool_use"));
        assert_eq!(output["content"][0]["type"], json!("tool_use"));
        assert_eq!(output["content"][0]["name"], json!("get_weather"));
        assert_eq!(output["content"][0]["input"]["city"], json!("Tokyo"));
    }

    #[test]
    fn chat_response_preserves_part_and_message_level_refusals() {
        let input = json!({
            "id": "chatcmpl-refusal",
            "model": "guarded-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [{"type": "refusal", "refusal": "part refusal"}],
                    "refusal": "message refusal"
                },
                "finish_reason": "content_filter"
            }]
        });

        let anthropic = openai_chat_response_to_anthropic(&input).unwrap();
        assert_eq!(
            anthropic["content"][0],
            json!({"type": "text", "text": "part refusal"})
        );
        assert_eq!(
            anthropic["content"][1],
            json!({"type": "text", "text": "message refusal"})
        );
        assert_eq!(anthropic["stop_reason"], json!("refusal"));

        let responses = openai_chat_response_to_responses(&input).unwrap();
        assert_eq!(
            responses["output"][0]["content"][0]["type"],
            json!("refusal")
        );
        assert_eq!(
            responses["output"][0]["content"][1]["refusal"],
            json!("message refusal")
        );
        assert_eq!(responses["status"], json!("failed"));
    }

    #[test]
    fn responses_anthropic_usage_round_trip_preserves_cache_creation() {
        let responses_usage = json!({
            "input_tokens": 100,
            "output_tokens": 8,
            "cache_creation_input_tokens": 20,
            "input_tokens_details": {"cached_tokens": 60}
        });
        let anthropic = anthropic_usage_from_openai_usage(Some(&responses_usage));
        assert_eq!(anthropic["input_tokens"], json!(20));
        assert_eq!(anthropic["cache_read_input_tokens"], json!(60));
        assert_eq!(anthropic["cache_creation_input_tokens"], json!(20));

        let round_trip = openai_responses_usage_from_anthropic_usage(Some(&anthropic));
        assert_eq!(round_trip["input_tokens"], json!(100));
        assert_eq!(round_trip["total_tokens"], json!(108));
        assert_eq!(
            round_trip["input_tokens_details"]["cached_tokens"],
            json!(60)
        );
        assert_eq!(round_trip["cache_creation_input_tokens"], json!(20));
        assert_eq!(
            round_trip["input_tokens_details"]["cache_write_tokens"],
            json!(20)
        );

        let chat = openai_chat_usage_from_responses_usage(Some(&json!({
            "input_tokens": 10,
            "output_tokens": 1,
            "input_tokens_details": {"cached_tokens": 3, "cache_write_tokens": 0}
        })));
        assert_eq!(
            chat["prompt_tokens_details"]["cached_creation_tokens"],
            json!(0)
        );
    }

    #[test]
    fn response_snapshots_convert_anthropic_to_openai_responses_and_chat() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "tool_use", "id": "tool_1", "name": "lookup", "input": {"q": "x"}}
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 40,
                "output_tokens": 5,
                "cache_read_input_tokens": 10
            }
        });

        let responses = anthropic_response_to_openai_responses(&input).unwrap();
        assert_eq!(
            responses
                .pointer("/output/0/content/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            responses.pointer("/output/1/type").and_then(Value::as_str),
            Some("function_call")
        );
        assert_eq!(
            responses
                .pointer("/usage/input_tokens_details/cached_tokens")
                .and_then(Value::as_i64),
            Some(10)
        );

        let chat = anthropic_response_to_openai_chat(&input).unwrap();
        assert_eq!(
            chat.pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("hello")
        );
        assert_eq!(
            chat.pointer("/choices/0/message/tool_calls/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
        assert_eq!(
            chat.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("tool_calls")
        );
    }

    #[test]
    fn response_snapshots_convert_gemini_to_anthropic_and_back() {
        let input = json!({
            "responseId": "gem_1",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "hi"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 9,
                "candidatesTokenCount": 3,
                "cachedContentTokenCount": 4,
                "totalTokenCount": 12
            }
        });

        let anthropic = gemini_response_to_anthropic(&input).unwrap();
        assert_eq!(
            anthropic.pointer("/content/0/text").and_then(Value::as_str),
            Some("hi")
        );
        assert_eq!(
            anthropic
                .pointer("/usage/cache_read_input_tokens")
                .and_then(Value::as_i64),
            Some(4)
        );

        let gemini = anthropic_response_to_gemini(&anthropic).unwrap();
        assert_eq!(
            gemini
                .pointer("/candidates/0/content/parts/0/text")
                .and_then(Value::as_str),
            Some("hi")
        );
        assert_eq!(
            gemini
                .pointer("/usageMetadata/cachedContentTokenCount")
                .and_then(Value::as_i64),
            Some(4)
        );
    }

    #[test]
    fn stream_snapshots_convert_between_sse_formats() {
        let openai_frames = openai_responses_stream_to_anthropic(&json!({
            "type": "response.output_text.delta",
            "delta": "hi"
        }));
        assert_eq!(
            openai_frames,
            vec![StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "hi"}
                })
            )]
        );

        let chat_frames = anthropic_stream_to_openai_chat(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "hi"}
        }));
        assert_eq!(
            chat_frames,
            vec![StreamFrame::json(json!({
                "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": Value::Null}]
            }))]
        );

        let direct_chat_frames = openai_responses_stream_to_chat(&json!({
            "type": "response.output_text.delta",
            "delta": "hi"
        }));
        assert_eq!(
            direct_chat_frames,
            vec![StreamFrame::json(json!({
                "id": "chatcmpl_ccswitch",
                "object": "chat.completion.chunk",
                "model": "",
                "choices": [{
                    "index": 0,
                    "delta": {"content": "hi"},
                    "finish_reason": Value::Null
                }]
            }))]
        );

        let direct_done_frames = openai_responses_stream_to_chat(&json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "model": "gpt-5.5",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 4, "output_tokens": 2, "total_tokens": 6}
            }
        }));
        assert_eq!(
            direct_done_frames,
            vec![
                StreamFrame::json(json!({
                    "id": "chatcmpl_1",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.5",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 4,
                        "completion_tokens": 2,
                        "total_tokens": 6,
                        "prompt_tokens_details": {"cached_tokens": 0}
                    }
                })),
                StreamFrame::done()
            ]
        );

        let gemini_frames = openai_responses_stream_to_gemini(&json!({
            "type": "response.output_text.delta",
            "delta": "hi"
        }));
        assert_eq!(
            gemini_frames,
            vec![StreamFrame::json(json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [{"text": "hi"}]}
                }]
            }))]
        );

        let gemini_chat_frames = openai_chat_stream_to_gemini(&json!({
            "choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": Value::Null}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}
        }));
        assert_eq!(
            gemini_chat_frames,
            vec![
                StreamFrame::json(json!({
                    "candidates": [{
                        "content": {"role": "model", "parts": [{"text": "hi"}]}
                    }]
                })),
                StreamFrame::json(json!({
                    "usageMetadata": {
                        "promptTokenCount": 7,
                        "candidatesTokenCount": 2,
                        "totalTokenCount": 9,
                        "cachedContentTokenCount": 0
                    }
                }))
            ]
        );
    }

    #[test]
    fn streaming_tool_call_deltas_map_to_anthropic_tool_use_events() {
        let start_frames = openai_chat_stream_to_anthropic(&json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "id": "call_weather",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": ""}
                    }]
                },
                "finish_reason": Value::Null
            }]
        }));
        assert_eq!(
            start_frames,
            vec![StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "call_weather",
                        "name": "get_weather",
                        "input": {}
                    }
                })
            )]
        );

        let argument_frames = openai_chat_stream_to_anthropic(&json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 1,
                        "function": {"arguments": "{\"city\":\"SF\"}"}
                    }]
                },
                "finish_reason": Value::Null
            }]
        }));
        assert_eq!(
            argument_frames,
            vec![StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 1,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"city\":\"SF\"}"
                    }
                })
            )]
        );
    }

    #[test]
    fn responses_streaming_function_call_maps_to_anthropic_tool_events() {
        let start = openai_responses_stream_to_anthropic(&json!({
            "type": "response.output_item.added",
            "output_index": 2,
            "item": {
                "type": "function_call",
                "call_id": "call_lookup",
                "name": "lookup"
            }
        }));
        assert_eq!(
            start,
            vec![StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 2,
                    "content_block": {
                        "type": "tool_use",
                        "id": "call_lookup",
                        "name": "lookup",
                        "input": {}
                    }
                })
            )]
        );

        let delta = openai_responses_stream_to_anthropic(&json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 2,
            "delta": "{\"query\":\"server\"}"
        }));
        assert_eq!(
            delta,
            vec![StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 2,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"query\":\"server\"}"
                    }
                })
            )]
        );

        let completed = openai_responses_stream_to_anthropic(&json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "call_id": "call_lookup",
                    "name": "lookup",
                    "arguments": "{\"query\":\"server\"}"
                }]
            }
        }));
        assert_eq!(
            completed,
            vec![
                StreamFrame::event(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "tool_use", "stop_sequence": Value::Null}
                    })
                ),
                StreamFrame::event("message_stop", json!({"type": "message_stop"}))
            ]
        );
    }

    #[test]
    fn gemini_streaming_function_call_maps_to_anthropic_tool_events() {
        let frames = gemini_stream_to_anthropic(&json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": "call_gemini",
                            "name": "lookup",
                            "args": {"query": "server"}
                        }
                    }]
                }
            }]
        }));
        assert_eq!(
            frames,
            vec![
                StreamFrame::event(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {
                            "type": "tool_use",
                            "id": "call_gemini",
                            "name": "lookup",
                            "input": {}
                        }
                    })
                ),
                StreamFrame::event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": "{\"query\":\"server\"}"
                        }
                    })
                )
            ]
        );
    }

    #[test]
    fn anthropic_streaming_tool_use_maps_to_openai_chat_tool_call_deltas() {
        let start = anthropic_stream_to_openai_chat(&json!({
            "type": "content_block_start",
            "index": 3,
            "content_block": {
                "type": "tool_use",
                "id": "call_anthropic",
                "name": "lookup",
                "input": {}
            }
        }));
        assert_eq!(
            start,
            vec![StreamFrame::json(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 3,
                            "id": "call_anthropic",
                            "type": "function",
                            "function": {"name": "lookup", "arguments": ""}
                        }]
                    },
                    "finish_reason": Value::Null
                }]
            }))]
        );

        let delta = anthropic_stream_to_openai_chat(&json!({
            "type": "content_block_delta",
            "index": 3,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"query\":\"server\"}"
            }
        }));
        assert_eq!(
            delta,
            vec![StreamFrame::json(json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 3,
                            "function": {"arguments": "{\"query\":\"server\"}"}
                        }]
                    },
                    "finish_reason": Value::Null
                }]
            }))]
        );
    }

    #[test]
    fn anthropic_streaming_tool_use_maps_to_openai_responses_events() {
        let start = anthropic_stream_to_openai_responses(&json!({
            "type": "content_block_start",
            "index": 4,
            "content_block": {
                "type": "tool_use",
                "id": "call_response",
                "name": "lookup",
                "input": {}
            }
        }));
        assert_eq!(
            start,
            vec![StreamFrame::json(json!({
                "type": "response.output_item.added",
                "output_index": 4,
                "item": {
                    "type": "function_call",
                    "call_id": "call_response",
                    "name": "lookup",
                    "arguments": ""
                }
            }))]
        );

        let delta = anthropic_stream_to_openai_responses(&json!({
            "type": "content_block_delta",
            "index": 4,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "{\"query\":\"server\"}"
            }
        }));
        assert_eq!(
            delta,
            vec![StreamFrame::json(json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 4,
                "delta": "{\"query\":\"server\"}"
            }))]
        );

        let stop = anthropic_stream_to_openai_responses(&json!({
            "type": "content_block_stop",
            "index": 4
        }));
        assert_eq!(
            stop,
            vec![StreamFrame::json(json!({
                "type": "response.output_item.done",
                "output_index": 4
            }))]
        );
    }

    #[test]
    fn anthropic_streaming_tool_stop_maps_to_openai_finish_reasons() {
        let chat = anthropic_stream_to_openai_chat(&json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": Value::Null}
        }));
        assert_eq!(
            chat,
            vec![StreamFrame::json(json!({
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "tool_calls"
                }]
            }))]
        );
    }

    #[test]
    fn anthropic_response_maps_max_tokens_stop_reason_to_openai_length() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "done"}],
            "model": "claude-sonnet-4",
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let output = anthropic_response_to_openai_chat(&input).unwrap();
        assert_eq!(
            output
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("length")
        );
        let responses = anthropic_response_to_openai_responses(&input).unwrap();
        assert_eq!(responses["status"], "incomplete");
        assert_eq!(
            responses["incomplete_details"]["reason"],
            "max_output_tokens"
        );
    }

    #[test]
    fn content_filter_and_refusal_map_consistently_across_snapshots() {
        let responses = json!({
            "id": "resp_filtered",
            "status": "incomplete",
            "incomplete_details": {"reason": "content_filter"},
            "model": "gpt",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "refusal", "refusal": "blocked"}]
            }, {
                "type": "function_call",
                "call_id": "call_partial",
                "name": "lookup",
                "arguments": "{"
            }]
        });
        assert_eq!(
            openai_responses_response_to_anthropic(&responses).unwrap()["stop_reason"],
            "refusal"
        );
        assert_eq!(
            openai_responses_response_to_chat(&responses).unwrap()["choices"][0]["finish_reason"],
            "content_filter"
        );

        let anthropic = json!({
            "id": "msg_refused",
            "model": "claude",
            "content": [{"type": "text", "text": "blocked"}],
            "stop_reason": "refusal"
        });
        let chat = anthropic_response_to_openai_chat(&anthropic).unwrap();
        let responses = anthropic_response_to_openai_responses(&anthropic).unwrap();
        assert_eq!(chat["choices"][0]["finish_reason"], "content_filter");
        assert_eq!(responses["status"], "incomplete");
        assert_eq!(responses["incomplete_details"]["reason"], "content_filter");
    }

    #[test]
    fn responses_max_tokens_wins_over_partial_tool_output() {
        let responses = json!({
            "id": "resp_truncated_tool",
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "model": "gpt",
            "output": [{
                "type": "function_call",
                "call_id": "call_partial",
                "name": "lookup",
                "arguments": "{"
            }]
        });

        assert_eq!(
            openai_responses_response_to_anthropic(&responses).unwrap()["stop_reason"],
            "max_tokens"
        );
        assert_eq!(
            openai_responses_response_to_chat(&responses).unwrap()["choices"][0]["finish_reason"],
            "length"
        );
    }

    #[test]
    fn anthropic_response_maps_tool_use_stop_reason_to_openai_tool_calls() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {}}],
            "model": "claude-sonnet-4",
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let output = anthropic_response_to_openai_chat(&input).unwrap();
        assert_eq!(
            output
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("tool_calls")
        );
    }

    #[test]
    fn anthropic_response_maps_end_turn_to_gemini_stop() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let output = anthropic_response_to_gemini(&input).unwrap();
        assert_eq!(
            output
                .pointer("/candidates/0/finishReason")
                .and_then(Value::as_str),
            Some("STOP")
        );
    }

    #[test]
    fn anthropic_response_maps_max_tokens_to_gemini_max_tokens() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "model": "claude-sonnet-4",
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let output = anthropic_response_to_gemini(&input).unwrap();
        assert_eq!(
            output
                .pointer("/candidates/0/finishReason")
                .and_then(Value::as_str),
            Some("MAX_TOKENS")
        );
    }

    #[test]
    fn anthropic_stream_stop_sequence_maps_to_openai_stop() {
        let frames = anthropic_stream_to_openai_chat(&json!({
            "type": "message_delta",
            "delta": {"stop_reason": "stop_sequence", "stop_sequence": "END"}
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("stop")
        );
    }

    #[test]
    fn anthropic_stream_max_tokens_maps_to_openai_length() {
        let frames = anthropic_stream_to_openai_chat(&json!({
            "type": "message_delta",
            "delta": {"stop_reason": "max_tokens", "stop_sequence": null}
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("length")
        );
    }

    #[test]
    fn anthropic_stream_text_delta_emits_openai_chat_chunk() {
        let frames = anthropic_stream_to_openai_chat(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"text": "hello"}
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn anthropic_stream_text_delta_emits_gemini_text_part() {
        let frames = anthropic_stream_to_gemini(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"text": "hello"}
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/candidates/0/content/parts/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn openai_chat_stream_length_maps_to_gemini_max_tokens() {
        let frames = openai_chat_stream_to_gemini(&json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "length"}]
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/candidates/0/finishReason")
                .and_then(Value::as_str),
            Some("MAX_TOKENS")
        );
    }

    #[test]
    fn openai_chat_stream_stop_maps_to_gemini_stop() {
        let frames = openai_chat_stream_to_gemini(&json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/candidates/0/finishReason")
                .and_then(Value::as_str),
            Some("STOP")
        );
    }

    #[test]
    fn openai_responses_completed_maps_tool_output_to_chat_tool_calls_finish() {
        let frames = openai_responses_stream_to_chat(&json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "model": "gpt-5",
                "status": "completed",
                "output": [{"type": "function_call", "call_id": "call_1", "name": "lookup"}]
            }
        }));
        assert_eq!(
            frame_json(&frames[0])
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("tool_calls")
        );
    }

    #[test]
    fn anthropic_to_openai_chat_maps_tools_array() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "name": "lookup",
                "description": "search",
                "input_schema": {"type": "object", "properties": {}}
            }]
        });
        let output = anthropic_to_openai_chat(&input).unwrap();
        assert_eq!(
            output
                .pointer("/tools/0/function/name")
                .and_then(Value::as_str),
            Some("lookup")
        );
    }

    #[test]
    fn anthropic_to_gemini_native_maps_system_instruction() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "be helpful",
            "messages": [{"role": "user", "content": "hi"}]
        });
        let output = anthropic_to_gemini_native(&input).unwrap();
        assert_eq!(
            output
                .pointer("/systemInstruction/parts/0/text")
                .and_then(Value::as_str),
            Some("be helpful")
        );
    }

    #[test]
    fn anthropic_to_openai_responses_maps_user_message() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "ping"}]}]
        });
        let output = anthropic_to_openai_responses(&input).unwrap();
        assert_eq!(
            output.pointer("/input/0/role").and_then(Value::as_str),
            Some("user")
        );
        assert_eq!(
            output
                .pointer("/input/0/content/0/text")
                .and_then(Value::as_str),
            Some("ping")
        );
    }

    #[test]
    fn anthropic_response_to_openai_responses_maps_text_blocks() {
        let input = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });
        let output = anthropic_response_to_openai_responses(&input).unwrap();
        assert_eq!(
            output
                .pointer("/output/0/content/0/text")
                .and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn stream_finish_reason_matrix_maps_to_downstream_protocols() {
        let chat_to_gemini = openai_chat_stream_to_gemini(&json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "length"}]
        }));
        assert_eq!(
            chat_to_gemini,
            vec![StreamFrame::json(json!({
                "candidates": [{"finishReason": "MAX_TOKENS"}]
            }))]
        );

        let responses_to_chat = openai_responses_stream_to_chat(&json!({
            "type": "response.completed",
            "response": {
                "id": "resp_tool",
                "model": "gpt-5.5",
                "status": "completed",
                "output": [{"type": "function_call", "call_id": "call_1", "name": "lookup"}]
            }
        }));
        assert_eq!(
            responses_to_chat[0],
            StreamFrame::json(json!({
                "id": "chatcmpl_tool",
                "object": "chat.completion.chunk",
                "model": "gpt-5.5",
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "tool_calls"
                }]
            }))
        );

        let incomplete = json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_partial_tool",
                "model": "gpt-5.5",
                "status": "incomplete",
                "incomplete_details": {"reason": "content_filter"},
                "output": [{"type": "function_call", "call_id": "call_1", "name": "lookup"}]
            }
        });
        let responses_to_chat = openai_responses_stream_to_chat(&incomplete);
        let StreamPayload::Json(chat_terminal) = &responses_to_chat[0].payload else {
            panic!("expected chat terminal JSON frame");
        };
        assert_eq!(
            chat_terminal.pointer("/choices/0/finish_reason"),
            Some(&json!("content_filter"))
        );
        let responses_to_anthropic = openai_responses_stream_to_anthropic(&incomplete);
        let StreamPayload::Json(anthropic_terminal) = &responses_to_anthropic[0].payload else {
            panic!("expected Anthropic terminal JSON frame");
        };
        assert_eq!(
            anthropic_terminal.pointer("/delta/stop_reason"),
            Some(&json!("refusal"))
        );
    }

    #[test]
    fn responses_lite_additional_custom_tools_and_history_convert_to_chat() {
        let input = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "exec", "description": "Run a command"},
                    {"type": "function", "name": "lookup", "parameters": {"type": "object"}}
                ]},
                {"type": "custom_tool_call", "call_id": "call_1", "name": "exec", "input": "pwd"},
                {"type": "custom_tool_call_output", "call_id": "call_1", "output": [
                    {"type": "input_text", "text": "/tmp"},
                    {"type": "input_text", "text": "\n"}
                ]}
            ]
        });
        let output = openai_responses_to_chat(&input).unwrap();
        assert_eq!(
            output.pointer("/tools/0/function/name"),
            Some(&json!("exec"))
        );
        assert_eq!(
            output.pointer("/tools/0/function/parameters/properties/input/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            output.pointer("/messages/0/tool_calls/0/function/arguments"),
            Some(&json!(r#"{"input":"pwd"}"#))
        );
        assert_eq!(
            output.pointer("/messages/1/content"),
            Some(&json!("/tmp\n"))
        );
        assert_eq!(
            responses_custom_tool_names(&input),
            BTreeSet::from(["exec".to_string()])
        );
    }

    #[test]
    fn responses_tool_search_name_collision_is_rejected() {
        let error = openai_responses_to_chat(&json!({
            "model": "gpt-5.6-sol",
            "input": "ping",
            "tools": [
                {"type": "tool_search"},
                {"type": "custom", "name": "tool_search"}
            ]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("tool_search conflicts"));

        let output = openai_responses_to_chat(&json!({
            "model": "gpt-5.6-sol",
            "input": "ping",
            "tools": [{"type": "tool_search"}],
            "tool_choice": {"type": "tool_search"}
        }))
        .unwrap();
        assert_eq!(
            output.pointer("/tool_choice/function/name"),
            Some(&json!("tool_search"))
        );
    }

    #[test]
    fn responses_custom_and_function_name_collision_is_rejected() {
        let top_level = json!({
            "model": "gpt-5.6-sol",
            "input": "ping",
            "tools": [
                {"type": "custom", "name": "exec"},
                {"type": "function", "name": "exec", "parameters": {"type": "object"}}
            ]
        });
        let chat_error = openai_responses_to_chat(&top_level).unwrap_err();
        assert!(chat_error
            .to_string()
            .contains("custom tool 'exec' conflicts with a function tool"));

        let additional_tools = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {"role": "user", "content": "ping"},
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "exec"},
                    {"type": "function", "name": "exec", "parameters": {"type": "object"}}
                ]}
            ]
        });
        let anthropic_error = openai_responses_to_anthropic(&additional_tools).unwrap_err();
        assert!(anthropic_error
            .to_string()
            .contains("custom tool 'exec' conflicts with a function tool"));
    }

    #[test]
    fn chat_custom_tool_response_is_restored_to_responses_item() {
        let custom_names = BTreeSet::from(["exec".to_string()]);
        let output = openai_chat_response_to_responses_with_custom_tools(
            &json!({
                "id": "chatcmpl_1",
                "model": "gpt-5.6-sol",
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {"role": "assistant", "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "exec", "arguments": "{\"input\":\"pwd\"}"}
                    }]}
                }]
            }),
            &custom_names,
        )
        .unwrap();
        assert_eq!(
            output.pointer("/output/0/type"),
            Some(&json!("custom_tool_call"))
        );
        assert_eq!(output.pointer("/output/0/input"), Some(&json!("pwd")));
    }

    #[test]
    fn responses_namespace_tools_round_trip_through_chat_and_anthropic() {
        let request = json!({
            "model": "gpt-5.6-sol",
            "tools": [{
                "type": "namespace",
                "name": "mcp_files",
                "tools": [{
                    "type": "function",
                    "name": "read",
                    "description": "Read a file",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
                }]
            }],
            "tool_choice": {"type": "function", "name": "read", "namespace": "mcp_files"},
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_read",
                    "name": "read",
                    "namespace": "mcp_files",
                    "arguments": "{\"path\":\"README.md\"}"
                },
                {"type": "function_call_output", "call_id": "call_read", "output": "contents"}
            ]
        });
        let flat_name = flatten_namespace_tool_name("mcp_files", "read");

        let chat_request = openai_responses_to_chat(&request).unwrap();
        assert_eq!(chat_request["tools"][0]["function"]["name"], flat_name);
        assert_eq!(
            chat_request["messages"][0]["tool_calls"][0]["function"]["name"],
            flat_name
        );
        assert_eq!(chat_request["tool_choice"]["function"]["name"], flat_name);

        let anthropic_request = openai_responses_to_anthropic(&request).unwrap();
        assert_eq!(anthropic_request["tools"][0]["name"], flat_name);
        assert_eq!(
            anthropic_request["messages"][1]["content"][0]["name"],
            flat_name
        );
        assert_eq!(anthropic_request["tool_choice"]["name"], flat_name);

        let context = responses_tool_context(&request);
        let chat_response = openai_chat_response_to_responses_with_tool_context(
            &json!({
                "id": "chatcmpl_namespace",
                "model": "chat",
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {"role": "assistant", "tool_calls": [{
                        "id": "call_read",
                        "type": "function",
                        "function": {"name": flat_name, "arguments": "{\"path\":\"README.md\"}"}
                    }]}
                }]
            }),
            &context,
        )
        .unwrap();
        assert_eq!(chat_response["output"][0]["type"], "function_call");
        assert_eq!(chat_response["output"][0]["name"], "read");
        assert_eq!(chat_response["output"][0]["namespace"], "mcp_files");

        let anthropic_response = anthropic_response_to_openai_responses_with_tool_context(
            &json!({
                "id": "msg_namespace",
                "model": "claude",
                "content": [{
                    "type": "tool_use",
                    "id": "call_read",
                    "name": flat_name,
                    "input": {"path": "README.md"}
                }],
                "stop_reason": "tool_use"
            }),
            &context,
        )
        .unwrap();
        assert_eq!(anthropic_response["output"][0]["name"], "read");
        assert_eq!(anthropic_response["output"][0]["namespace"], "mcp_files");
    }

    #[test]
    fn tool_search_responses_are_restored_as_native_calls() {
        let request = json!({
            "model": "gpt-5.6-sol",
            "input": "find email tools",
            "tools": [{"type": "tool_search"}]
        });
        let context = responses_tool_context(&request);
        let chat_response = openai_chat_response_to_responses_with_tool_context(
            &json!({
                "id": "chatcmpl_tool_search",
                "model": "chat",
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {"role": "assistant", "tool_calls": [{
                        "id": "call_search",
                        "type": "function",
                        "function": {"name": "tool_search", "arguments": "{\"query\":\"gmail\",\"limit\":5}"}
                    }]}
                }]
            }),
            &context,
        )
        .unwrap();
        assert_eq!(chat_response["output"][0]["type"], "tool_search_call");
        assert_eq!(chat_response["output"][0]["call_id"], "call_search");
        assert_eq!(chat_response["output"][0]["arguments"]["query"], "gmail");
        assert!(chat_response["output"][0].get("name").is_none());

        let fallback = context.response_item(
            "fc_call_fallback",
            "completed",
            "call_fallback",
            "tool_search",
            "not-json",
        );
        assert_eq!(fallback["type"], "tool_search_call");
        assert_eq!(fallback["arguments"], json!({"query": "not-json"}));

        let anthropic_response = anthropic_response_to_openai_responses_with_tool_context(
            &json!({
                "id": "msg_tool_search",
                "model": "claude",
                "content": [{
                    "type": "tool_use",
                    "id": "call_search",
                    "name": "tool_search",
                    "input": {"query": "gmail"}
                }],
                "stop_reason": "tool_use"
            }),
            &context,
        )
        .unwrap();
        assert_eq!(anthropic_response["output"][0]["type"], "tool_search_call");
        assert_eq!(
            anthropic_response["output"][0]["arguments"],
            json!({"query": "gmail"})
        );
    }

    #[test]
    fn tool_search_output_adds_dynamic_namespace_tools() {
        let request = json!({
            "model": "gpt-5.6-sol",
            "tools": [{"type": "tool_search"}],
            "tool_choice": {"type": "function", "name": "search", "namespace": "mcp_mail"},
            "input": [
                {
                    "type": "tool_search_output",
                    "call_id": "call_search",
                    "output": "loaded",
                    "tools": [{
                        "type": "namespace",
                        "name": "mcp_mail",
                        "tools": [{
                            "type": "function",
                            "name": "search",
                            "parameters": {"type": "object", "properties": {}}
                        }]
                    }]
                },
                {"role": "user", "content": "find mail"}
            ]
        });
        let flat_name = flatten_namespace_tool_name("mcp_mail", "search");

        let chat = openai_responses_to_chat(&request).unwrap();
        let chat_names = chat["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            chat_names,
            BTreeSet::from(["tool_search", flat_name.as_str()])
        );
        assert_eq!(chat["tool_choice"]["function"]["name"], flat_name);

        let anthropic = openai_responses_to_anthropic(&request).unwrap();
        let anthropic_names = anthropic["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            anthropic_names,
            BTreeSet::from(["tool_search", flat_name.as_str()])
        );
        assert_eq!(anthropic["tool_choice"]["name"], flat_name);
    }

    #[test]
    fn long_namespace_tool_names_are_stable_and_byte_bounded() {
        let namespace = format!("{}-connector", "\u{754c}".repeat(24));
        let first = flatten_namespace_tool_name(&namespace, "search_messages_with_filters");
        let second = flatten_namespace_tool_name(&namespace, "search_messages_with_filters");
        let different = flatten_namespace_tool_name(&namespace, "search_messages_with_labels");

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert!(first.len() <= CHAT_TOOL_NAME_MAX_LEN);
        assert!(different.len() <= CHAT_TOOL_NAME_MAX_LEN);
        assert!(first.is_char_boundary(first.len()));
    }

    #[test]
    fn responses_reasoning_attaches_forward_and_backfills_trailing_summary() {
        let output = openai_responses_to_chat(&json!({
            "model": "gpt-5.5",
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "before tool"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{}"
                },
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "after tool"}]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            output.pointer("/messages/0/reasoning_content"),
            Some(&json!("before tool\n\nafter tool"))
        );
        assert_eq!(
            output.pointer("/messages/0/tool_calls/0/function/name"),
            Some(&json!("lookup"))
        );
    }

    #[test]
    fn responses_reasoning_deduplicates_only_exact_fragments() {
        let output = openai_responses_to_chat(&json!({
            "model": "gpt-5.5",
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "prefix suffix"}]
                },
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "suffix"}]
                },
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "prefix suffix"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{}"
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            output.pointer("/messages/0/reasoning_content"),
            Some(&json!("prefix suffix\n\nsuffix"))
        );
    }

    #[test]
    fn anthropic_reasoning_only_assistant_history_is_removed() {
        let output = anthropic_to_openai_responses(&json!({
            "model": "claude-sonnet-4",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "thinking",
                        "thinking": "orphaned",
                        "signature": "provider-signature"
                    }]
                },
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "continue"}]
                }
            ]
        }))
        .unwrap();

        assert!(!output["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item.get("type").and_then(Value::as_str) == Some("reasoning") }));
        assert_eq!(output.pointer("/input/0/role"), Some(&json!("user")));
    }

    #[test]
    fn responses_snapshot_preserves_all_reasoning_summary_fragments() {
        let output = openai_responses_response_to_chat(&json!({
            "id": "resp_reasoning",
            "status": "completed",
            "model": "gpt-5.5",
            "output": [{
                "type": "reasoning",
                "summary": [
                    {"type": "summary_text", "text": "first"},
                    {"type": "reasoning_text", "text": " second"},
                    {"type": "summary_text", "text": " third"}
                ]
            }]
        }))
        .unwrap();

        assert_eq!(
            output.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("first second third"))
        );
    }

    #[test]
    fn openai_reasoning_opaque_payload_round_trips_through_anthropic() {
        let reasoning = json!({
            "id": "rs_opaque",
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "visible"}],
            "encrypted_content": "provider-opaque"
        });
        let anthropic = openai_responses_response_to_anthropic(&json!({
            "id": "resp_opaque",
            "status": "completed",
            "output": [reasoning.clone()]
        }))
        .unwrap();
        let restored = anthropic_response_to_openai_responses(&anthropic).unwrap();

        assert_eq!(restored.pointer("/output/0"), Some(&reasoning));
    }

    #[test]
    fn unsigned_anthropic_reasoning_stays_visible_without_opaque_content() {
        let output = anthropic_response_to_openai_responses(&json!({
            "id": "msg_unsigned",
            "model": "claude-sonnet-4",
            "content": [{"type": "thinking", "thinking": "visible only"}],
            "stop_reason": "end_turn"
        }))
        .unwrap();

        assert_eq!(output.pointer("/output/0/type"), Some(&json!("reasoning")));
        assert_eq!(
            output.pointer("/output/0/summary/0/text"),
            Some(&json!("visible only"))
        );
        assert!(output.pointer("/output/0/encrypted_content").is_none());
    }

    #[test]
    fn responses_custom_tools_round_trip_through_anthropic() {
        let request = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {"role": "user", "content": "run pwd"},
                {"type": "additional_tools", "tools": [
                    {"type": "custom", "name": "exec", "description": "Run a command"},
                    {"type": "function", "name": "lookup", "parameters": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}}
                    }}
                ]},
                {"type": "custom_tool_call", "call_id": "call_exec", "name": "exec", "input": "pwd"},
                {"type": "custom_tool_call_output", "call_id": "call_exec", "output": "done"}
            ]
        });

        let anthropic_request = openai_responses_to_anthropic(&request).unwrap();
        assert_eq!(anthropic_request["tools"][0]["name"], "exec");
        assert_eq!(
            anthropic_request["tools"][0]["input_schema"]["properties"]["input"]["type"],
            "string"
        );
        assert_eq!(anthropic_request["tools"][1]["name"], "lookup");
        assert_eq!(
            anthropic_request["messages"][1]["content"][0]["input"]["input"],
            "pwd"
        );
        assert!(anthropic_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["content"]
                .as_array()
                .is_some_and(|content| !content.is_empty())));

        let anthropic_response = json!({
            "id": "msg_custom",
            "model": "claude",
            "content": [{
                "type": "tool_use",
                "id": "call_exec",
                "name": "exec",
                "input": {"input": "pwd"}
            }],
            "stop_reason": "tool_use"
        });
        let restored = anthropic_response_to_openai_responses_with_custom_tools(
            &anthropic_response,
            &BTreeSet::from(["exec".to_string()]),
        )
        .unwrap();
        assert_eq!(restored["output"][0]["type"], "custom_tool_call");
        assert_eq!(restored["output"][0]["input"], "pwd");
    }
}
