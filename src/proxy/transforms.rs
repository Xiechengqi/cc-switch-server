#![allow(dead_code)]

use serde_json::{json, Map, Value};

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
        if role == "system" {
            collect_text_like(&message["content"], &mut system_parts);
            continue;
        }
        output_messages.extend(openai_chat_message_to_anthropic(message, role));
    }

    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    if !system_parts.is_empty() {
        output.insert("system".to_string(), Value::String(system_parts.join("\n")));
    }
    output.insert("messages".to_string(), Value::Array(output_messages));
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(thinking) = openai_reasoning_to_anthropic(input.get("reasoning")) {
        output.insert("thinking".to_string(), thinking);
    }
    if let Some(tools) = openai_tools_to_anthropic(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }

    Ok(Value::Object(output))
}

pub fn openai_responses_to_anthropic(input: &Value) -> Result<Value, TransformError> {
    let mut output = Map::new();
    copy_string(input, &mut output, "model");
    copy_bool(input, &mut output, "stream");
    copy_object(input, &mut output, "metadata");
    if let Some(thinking) = openai_reasoning_to_anthropic(input.get("reasoning")) {
        output.insert("thinking".to_string(), thinking);
    }
    if let Some(tools) = openai_tools_to_anthropic(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }

    let mut messages = Vec::new();
    match input.get("input") {
        Some(Value::String(text)) => messages.push(json!({
            "role": "user",
            "content": [{"type": "text", "text": text}]
        })),
        Some(Value::Array(items)) => {
            for item in items {
                messages.extend(openai_response_item_to_anthropic(item));
            }
        }
        _ => return Err(TransformError::new("openai responses input is required")),
    }
    output.insert("messages".to_string(), Value::Array(messages));

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
    let mut messages = Vec::new();
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
                append_response_input_item_to_chat_messages(item, &mut messages);
            }
        }
        Some(value @ Value::Object(_)) => {
            append_response_input_item_to_chat_messages(value, &mut messages)
        }
        _ => return Err(TransformError::new("openai responses input is required")),
    }

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
        output.insert("reasoning_effort".to_string(), effort.clone());
    }
    if let Some(tools) = openai_response_tools_to_chat(input.get("tools")) {
        output.insert("tools".to_string(), tools);
    }
    if let Some(tool_choice) = input.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            openai_response_tool_choice_to_chat(tool_choice),
        );
    }
    if let Some(format) = input.pointer("/text/format") {
        output.insert("response_format".to_string(), format.clone());
    }

    Ok(Value::Object(output))
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
        content.extend(openai_chat_response_content_to_anthropic(
            message.get("content"),
        ));
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            content.extend(tool_calls.iter().map(openai_tool_call_to_anthropic));
        }
    }

    if content.is_empty() {
        return Err(TransformError::new("openai chat response content is empty"));
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
                Some("function_call") => content.push(openai_function_call_to_anthropic(item)),
                Some("reasoning") => {
                    if let Some(text) = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .and_then(|items| items.first())
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str)
                    {
                        content.push(json!({"type": "thinking", "thinking": text}));
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
    if content.is_empty() {
        return Err(TransformError::new("openai responses output is empty"));
    }

    Ok(json!({
        "id": input.get("id").and_then(Value::as_str).unwrap_or("resp"),
        "type": "message",
        "role": "assistant",
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "content": content,
        "stop_reason": openai_status_to_anthropic_stop(input.get("status").and_then(Value::as_str)),
        "stop_sequence": Value::Null,
        "usage": anthropic_usage_from_openai_usage(input.get("usage"))
    }))
}

pub fn openai_responses_response_to_chat(input: &Value) -> Result<Value, TransformError> {
    let mut text = Vec::new();
    let mut tool_calls = Vec::new();
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
                Some("function_call") => {
                    if let Some(tool_call) = openai_response_function_call_to_chat(item) {
                        tool_calls.push(tool_call);
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
    if text.is_empty() && tool_calls.is_empty() {
        return Err(TransformError::new("openai responses output is empty"));
    }

    let has_tool_calls = !tool_calls.is_empty();
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text.join("")));
    if has_tool_calls {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
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

        let content = openai_chat_content_to_responses_content("assistant", message.get("content"));
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
                if let Some(item) = openai_chat_tool_call_to_response_item(tool_call, index) {
                    output.push(item);
                }
            }
        }
        if let Some(function_call) = message.get("function_call") {
            if let Some(item) = openai_chat_legacy_function_call_to_response_item(function_call) {
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
    if content.is_empty() {
        return Err(TransformError::new("gemini response content is empty"));
    }

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
    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_calls.push(anthropic_tool_use_to_openai(block)),
            Some("thinking") => {}
            _ => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push(value.to_string());
                }
            }
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(TransformError::new("anthropic response content is empty"));
    }

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), Value::String(text.join("")));
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
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
    let content_blocks = input
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| TransformError::new("anthropic response content must be an array"))?;
    let mut message_content = Vec::new();
    let mut output = Vec::new();
    let mut output_text = Vec::new();

    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => output.push(anthropic_tool_use_to_openai_response(block)),
            Some("thinking") => {}
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

    if !message_content.is_empty() {
        output.insert(
            0,
            json!({
                "type": "message",
                "role": "assistant",
                "content": message_content
            }),
        );
    }
    if output.is_empty() {
        return Err(TransformError::new("anthropic response content is empty"));
    }

    Ok(json!({
        "id": input.get("id").and_then(Value::as_str).unwrap_or("resp"),
        "object": "response",
        "status": "completed",
        "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
        "output": output,
        "output_text": output_text.join(""),
        "usage": openai_responses_usage_from_anthropic_usage(input.get("usage"))
    }))
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
        Some("response.completed") => {
            let mut frames = Vec::new();
            if let Some(usage) = input.pointer("/response/usage") {
                frames.push(StreamFrame::event(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn", "stop_sequence": Value::Null},
                        "usage": anthropic_usage_from_openai_usage(Some(usage))
                    }),
                ));
            }
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
        Some("response.completed") => {
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
            if let Some(arguments) = tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
            {
                frames.push(StreamFrame::json(json!({
                    "type": "response.function_call_arguments.delta",
                    "output_index": tool_call.get("index").cloned().unwrap_or_else(|| json!(0)),
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
        for part in parts {
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
        Some("content_block_delta") => input
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "type": "response.output_text.delta",
                    "delta": text
                }))]
            })
            .unwrap_or_default(),
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
        Some("content_block_delta") => input
            .pointer("/delta/text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![StreamFrame::json(json!({
                    "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": Value::Null}]
                }))]
            })
            .unwrap_or_default(),
        Some("message_delta") => input
            .get("usage")
            .map(|usage| {
                vec![StreamFrame::json(json!({
                    "choices": [],
                    "usage": openai_usage_from_anthropic_usage(Some(usage))
                }))]
            })
            .unwrap_or_default(),
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
    if let Some(reasoning) = anthropic_thinking_to_openai(input.get("thinking")) {
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
    if let Some(reasoning) = anthropic_thinking_to_openai(input.get("thinking")) {
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
                .map(anthropic_message_to_openai_response_item)
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
                .map(anthropic_message_to_gemini_content)
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

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
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
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": openai_chat_tool_output_to_string(message.get("content"))
                }));
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

    (instructions, input)
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
        "parameters": function.get("parameters").cloned().unwrap_or_else(|| json!({}))
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
    Some(json!({
        "id": format!("fc_{call_id}"),
        "type": "function_call",
        "status": "completed",
        "call_id": call_id,
        "name": name,
        "arguments": openai_tool_arguments_to_string(function.get("arguments"))
    }))
}

fn openai_chat_legacy_function_call_to_response_item(function_call: &Value) -> Option<Value> {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    let call_id = function_call
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(name);
    Some(json!({
        "id": format!("fc_{call_id}"),
        "type": "function_call",
        "status": "completed",
        "call_id": call_id,
        "name": name,
        "arguments": openai_tool_arguments_to_string(function_call.get("arguments"))
    }))
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

fn append_response_input_item_to_chat_messages(item: &Value, messages: &mut Vec<Value>) {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            if let Some(tool_call) = openai_response_function_call_to_chat(item) {
                messages.push(json!({
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": [tool_call]
                }));
            }
        }
        Some("function_call_output") => messages.push(json!({
            "role": "tool",
            "tool_call_id": item.get("call_id").and_then(Value::as_str).unwrap_or_default(),
            "content": item.get("output").cloned().unwrap_or_else(|| json!(""))
        })),
        Some("message") => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            messages.push(json!({
                "role": response_role_to_chat(role),
                "content": response_content_to_chat_content(role, item.get("content"))
            }));
        }
        Some("reasoning") => {}
        _ => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            if item.get("content").is_some() {
                messages.push(json!({
                    "role": response_role_to_chat(role),
                    "content": response_content_to_chat_content(role, item.get("content"))
                }));
            }
        }
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

fn openai_response_tools_to_chat(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    let chat_tools = tools
        .iter()
        .filter_map(openai_response_tool_to_chat_tool)
        .collect::<Vec<_>>();
    (!chat_tools.is_empty()).then_some(Value::Array(chat_tools))
}

fn openai_response_tool_to_chat_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    let name = tool.get("name").and_then(Value::as_str)?;
    let mut function = Map::new();
    function.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".to_string(), description.clone());
    }
    function.insert(
        "parameters".to_string(),
        tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
    );
    if let Some(strict) = tool.get("strict") {
        function.insert("strict".to_string(), strict.clone());
    }
    Some(json!({"type": "function", "function": function}))
}

fn openai_response_tool_choice_to_chat(tool_choice: &Value) -> Value {
    match tool_choice {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("function") => {
            json!({
                "type": "function",
                "function": {
                    "name": object.get("name").and_then(Value::as_str).unwrap_or_default()
                }
            })
        }
        _ => tool_choice.clone(),
    }
}

fn openai_response_function_call_to_chat(item: &Value) -> Option<Value> {
    let name = item.get("name").and_then(Value::as_str)?;
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("call_0");
    Some(json!({
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}")
        }
    }))
}

fn openai_response_finish_reason_to_chat(response: &Value, has_tool_calls: bool) -> Value {
    if has_tool_calls {
        return json!("tool_calls");
    }
    match response.get("status").and_then(Value::as_str) {
        Some("incomplete") => json!("length"),
        Some("failed") | Some("cancelled") => json!("stop"),
        _ => json!("stop"),
    }
}

fn response_output_has_tool_calls(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
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

fn openai_chat_message_to_anthropic(message: &Value, role: &str) -> Vec<Value> {
    if role == "tool" {
        return vec![json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": message.get("tool_call_id").and_then(Value::as_str).unwrap_or("tool"),
                "content": text_from_value(message.get("content")).unwrap_or_default()
            }]
        })];
    }

    let mut content = openai_content_to_anthropic(message.get("content"));
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            content.push(openai_tool_call_to_anthropic(tool_call));
        }
    }
    vec![json!({
        "role": if role == "assistant" { "assistant" } else { "user" },
        "content": content
    })]
}

fn openai_response_item_to_anthropic(item: &Value) -> Vec<Value> {
    if item.get("type").and_then(Value::as_str) == Some("function_call") {
        return vec![json!({
            "role": "assistant",
            "content": [openai_function_call_to_anthropic(item)]
        })];
    }
    let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    vec![json!({
        "role": if role == "assistant" { "assistant" } else { "user" },
        "content": openai_content_to_anthropic(item.get("content"))
    })]
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

    for block in anthropic_content_blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => tool_calls.push(anthropic_tool_use_to_openai(block)),
            Some("tool_result") => messages.push(json!({
                "role": "tool",
                "tool_call_id": block.get("tool_use_id").and_then(Value::as_str).unwrap_or("tool"),
                "content": block.get("content").cloned().unwrap_or(Value::String(String::new()))
            })),
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
    messages
}

fn anthropic_message_to_openai_response_item(message: &Value) -> Value {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    json!({
        "role": if role == "assistant" { "assistant" } else { "user" },
        "content": anthropic_content_blocks(message)
            .into_iter()
            .map(anthropic_block_to_openai_response_content)
            .collect::<Vec<_>>()
    })
}

fn anthropic_message_to_gemini_content(message: &Value) -> Value {
    let role = match message.get("role").and_then(Value::as_str) {
        Some("assistant") => "model",
        _ => "user",
    };
    json!({
        "role": role,
        "parts": anthropic_content_blocks(message)
            .into_iter()
            .map(anthropic_block_to_gemini_part)
            .collect::<Vec<_>>()
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
    let input = item
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .unwrap_or_else(|| item.get("arguments").cloned().unwrap_or_else(|| json!({})));
    json!({
        "type": "tool_use",
        "id": item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("tool"),
        "name": item.get("name").and_then(Value::as_str).unwrap_or("tool"),
        "input": input
    })
}

fn gemini_part_to_anthropic(part: &Value) -> Value {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return json!({"type": "text", "text": text});
    }
    if let Some(inline_data) = part.get("inlineData").or_else(|| part.get("inline_data")) {
        return json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": inline_data.get("mimeType").or_else(|| inline_data.get("mime_type")).and_then(Value::as_str).unwrap_or("application/octet-stream"),
                "data": inline_data.get("data").and_then(Value::as_str).unwrap_or_default()
            }
        });
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
        return json!({
            "type": "tool_result",
            "tool_use_id": function_response.get("id").and_then(Value::as_str).unwrap_or_else(|| function_response.get("name").and_then(Value::as_str).unwrap_or("tool")),
            "content": function_response.get("response").cloned().unwrap_or(Value::Null)
        });
    }
    part.clone()
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
        tools
            .iter()
            .filter_map(|tool| {
                let function = tool.get("function")?;
                Some(json!({
                    "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "description": function.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": function.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
                }))
            })
            .collect(),
    ))
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
                    "input_schema": declaration.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
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
                        "parameters": tool.get("input_schema").cloned().unwrap_or_else(|| json!({"type": "object"}))
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
                "parameters": tool.get("input_schema").cloned().unwrap_or_else(|| json!({"type": "object"}))
            })
        }).collect::<Vec<_>>()
    }]))
}

fn openai_reasoning_to_anthropic(reasoning: Option<&Value>) -> Option<Value> {
    let reasoning = reasoning?;
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String("enabled".to_string()));
    if let Some(effort) = reasoning.get("effort").and_then(Value::as_str) {
        output.insert("effort".to_string(), Value::String(effort.to_string()));
    }
    if let Some(summary) = reasoning.get("summary").and_then(Value::as_str) {
        output.insert("summary".to_string(), Value::String(summary.to_string()));
    }
    Some(Value::Object(output))
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

fn openai_finish_reason_to_anthropic(reason: &str) -> &'static str {
    match reason {
        "tool_calls" | "function_call" => "tool_use",
        "length" => "max_tokens",
        "content_filter" => "stop_sequence",
        _ => "end_turn",
    }
}

fn openai_status_to_anthropic_stop(status: Option<&str>) -> &'static str {
    match status {
        Some("incomplete") => "max_tokens",
        _ => "end_turn",
    }
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

fn anthropic_stop_reason_to_openai(reason: Option<&str>) -> &'static str {
    match reason {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        _ => "stop",
    }
}

fn anthropic_stop_reason_to_gemini(reason: Option<&str>) -> &'static str {
    match reason {
        Some("max_tokens") => "MAX_TOKENS",
        _ => "STOP",
    }
}

fn anthropic_usage_from_openai_usage(usage: Option<&Value>) -> Value {
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
    let mut output = Map::new();
    output.insert("input_tokens".to_string(), json!(input_tokens));
    output.insert("output_tokens".to_string(), json!(output_tokens));
    if let Some(cache_read) = cache_read {
        output.insert("cache_read_input_tokens".to_string(), json!(cache_read));
    }
    Value::Object(output)
}

fn anthropic_usage_from_gemini_usage(usage: Option<&Value>) -> Value {
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
    let mut output = Map::new();
    output.insert("input_tokens".to_string(), json!(input_tokens));
    output.insert("output_tokens".to_string(), json!(output_tokens));
    if let Some(cache_read) = cache_read {
        output.insert("cache_read_input_tokens".to_string(), json!(cache_read));
    }
    Value::Object(output)
}

fn openai_usage_from_anthropic_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
    json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "prompt_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)}
    })
}

fn openai_responses_usage_from_anthropic_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
        "input_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)}
    })
}

fn openai_chat_usage_from_responses_usage(usage: Option<&Value>) -> Value {
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
    if let Some(details) = usage
        .and_then(|usage| usage.get("output_tokens_details"))
        .or_else(|| usage.and_then(|usage| usage.get("completion_tokens_details")))
    {
        output.insert("completion_tokens_details".to_string(), details.clone());
    }
    Value::Object(output)
}

fn openai_responses_usage_from_chat_usage(usage: Option<&Value>) -> Value {
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
    let input_tokens = usage_number(usage, &[&["input_tokens"]]).unwrap_or(0);
    let output_tokens = usage_number(usage, &[&["output_tokens"]]).unwrap_or(0);
    let cache_read = usage_number(usage, &[&["cache_read_input_tokens"]]);
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

    #[test]
    fn openai_chat_to_anthropic_preserves_tools_thinking_cache_and_image() {
        let input = json!({
            "model": "gpt-5.5",
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
                {"role": "tool", "tool_call_id": "call_1", "content": "result"}
            ]
        });

        let output = openai_chat_to_anthropic(&input).unwrap();

        assert_eq!(output["system"], "system text");
        assert_eq!(
            output.pointer("/thinking/effort").and_then(Value::as_str),
            Some("medium")
        );
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
    }

    #[test]
    fn openai_responses_to_anthropic_preserves_input_image_reasoning_and_usage_shape() {
        let input = json!({
            "model": "gpt-5.5",
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
            output.pointer("/thinking/effort").and_then(Value::as_str),
            Some("low")
        );

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
        assert_eq!(usage.billed_input_tokens, Some(40));
        assert_eq!(usage.cache_read_tokens, Some(60));
        assert_eq!(usage.total_tokens, Some(104));
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
        assert_eq!(usage.input_tokens, Some(10));
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
                    "input_tokens": 100,
                    "output_tokens": 8,
                    "cache_read_input_tokens": 60
                }
            })
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
}
