//! Pull the structured fields cursor's AgentService needs out of the three
//! request body shapes cc-switch accepts (Anthropic Messages, OpenAI Chat
//! Completions, OpenAI Responses).
//!
//! Tool-steering directives (`TOOL_COMMIT_DIRECTIVE`, `tool_choice` hints) and
//! output constraints (`max_tokens`, `stop`, `response_format`) are injected
//! into `user_text` because Cursor's AgentService has no native equivalents
//! (ported from OmniRoute / composer-api).

use super::cursor_agent_proto::{
    anthropic_tools_to_mcp_defs, openai_tools_to_mcp_defs, McpToolDef,
};
use super::cursor_image::ImageRef;
use base64::Engine;
use bytes::Bytes;
use serde_json::{json, Value};

/// Prepended when the client declares tools — measurably raises tool-call rate
/// on Cursor's agent endpoint (OmniRoute A/B: ~53% → ~88%).
const TOOL_COMMIT_DIRECTIVE: &str = "\
You are serving an OpenAI-compatible API request and the client has provided executable tools.\n\
When a tool is needed to answer (real-time data, web/search lookups, file or project operations), you MUST issue the actual tool call. Do NOT describe what you are about to do as prose and then stop — call the tool.\n\
Answer directly only when no tool is needed.\n\
Do not emit duplicate tool calls: call each operation once, then continue after the tool result is returned.\n\
Never claim that tools are unavailable.";

const DEFAULT_WORKING_DIRECTORY: &str = ".";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GeminiNative,
}

#[derive(Debug, Clone)]
pub struct ToolResultBlock {
    /// Client-facing tool call id — what cc-switch emitted in the previous
    /// turn. Used to look up the pending exec_id in the session.
    pub tool_call_id: String,
    /// Result content as a plain string (cursor's McpResult expects text).
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub struct AgentRunPlan {
    pub system_prompt: Option<String>,
    pub user_text: String,
    pub tools: Vec<McpToolDef>,
    pub images: Vec<ImageRef>,
    pub tool_results: Vec<ToolResultBlock>,
    /// Cursor's `RequestedModel.model_id` — the value passed to
    /// `resolve_requested_model`. Comes from the upstream-mapped body.
    pub model_id: String,
    /// Optional Responses API `previous_response_id` — used to find a parked
    /// session.
    pub previous_response_id: Option<String>,
    /// Working directory surfaced in RequestContext ack (composer-api SDK).
    pub working_directory: String,
}

/// Validate tool-result context for AgentService routing. Returns an error
/// message if the request carries a `function_call_output` / `tool_result`
/// whose `call_id` is empty — Cursor's AgentService cannot match it to a
/// pending exec_id and the turn would silently fail. Mirrors sub2api's
/// `validateFunctionCallOutputRequest` guard.
pub fn validate_tool_result_context(plan: &AgentRunPlan) -> Result<(), String> {
    for tr in &plan.tool_results {
        if tr.tool_call_id.trim().is_empty() {
            return Err("function_call_output requires a non-empty call_id; \
                 continuation via previous_response_id without call_id is not supported"
                .to_string());
        }
    }
    Ok(())
}

/// Build a plan from a request body. The body is the **upstream-mapped**
/// version (after `apply_model_mapping`), so `model_id` here is what cursor
/// will see on the wire.
pub fn build_plan(protocol: InboundProtocol, body: &Value) -> AgentRunPlan {
    let model_id = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let previous_response_id = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let (system_prompt, user_text, images, tool_results) = match protocol {
        InboundProtocol::AnthropicMessages => decompose_anthropic(body),
        InboundProtocol::OpenAiChat => decompose_openai_chat(body),
        InboundProtocol::OpenAiResponses => decompose_openai_responses(body),
        InboundProtocol::GeminiNative => decompose_gemini_native(body),
    };
    let tools = match protocol {
        InboundProtocol::AnthropicMessages => body
            .get("tools")
            .map(anthropic_tools_to_mcp_defs)
            .unwrap_or_default(),
        InboundProtocol::OpenAiChat | InboundProtocol::OpenAiResponses => body
            .get("tools")
            .map(openai_tools_to_mcp_defs)
            .unwrap_or_default(),
        InboundProtocol::GeminiNative => gemini_tools_to_mcp_defs(body.get("tools")),
    };
    let tool_choice = extract_tool_choice(body, protocol);
    let mut tools = tools;
    if tool_choice_disables_tools(&tool_choice) {
        tools.clear();
    }
    let working_directory = extract_working_directory(body);
    // OmniRoute found that Cursor's AgentService does not reliably honor
    // system prompts delivered via the KV blob channel. Prepend system
    // content into the UserMessage text as a pragmatic workaround. The
    // KV-blob is still sent as a complementary channel.
    let user_text_with_system = if let Some(ref sys) = system_prompt {
        if !sys.trim().is_empty() {
            format!("{sys}\n\n{user_text}")
        } else {
            user_text.clone()
        }
    } else {
        user_text.clone()
    };
    let user_text =
        enhance_agent_user_text(&user_text_with_system, &tool_choice, &tools, body, protocol);

    AgentRunPlan {
        system_prompt,
        user_text,
        tools,
        images,
        tool_results,
        model_id,
        previous_response_id,
        working_directory,
    }
}

// ─── Anthropic Messages ────────────────────────────────────────────────────

fn decompose_anthropic(
    body: &Value,
) -> (Option<String>, String, Vec<ImageRef>, Vec<ToolResultBlock>) {
    let mut system_prompt: Option<String> = body
        .get("system")
        .and_then(stringify_anthropic_text_or_blocks);

    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    let mut conversation_lines: Vec<String> = Vec::new();
    let mut latest_user_text: Vec<String> = Vec::new();

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for (idx, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let is_last = idx == messages.len() - 1;
        let content = msg.get("content");
        let Some(content) = content else { continue };

        match content {
            Value::String(s) => match role {
                "user" if is_last => latest_user_text.push(s.clone()),
                _ => conversation_lines.push(format!("{}: {}", role_label(role), s)),
            },
            Value::Array(blocks) => {
                let mut text_acc = Vec::new();
                for block in blocks {
                    let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                text_acc.push(t.to_string());
                            }
                        }
                        "image" => {
                            if let Some(img) = anthropic_image_to_ref(block) {
                                images.push(img);
                            }
                        }
                        "tool_use" => {
                            // Assistant tool call from a prior turn. Render as
                            // a labeled line so the model has the context.
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                            let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                            let input = block
                                .get("input")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "{}".to_string());
                            conversation_lines.push(format!(
                                "Assistant called tool {name} ({id}) with arguments: {input}"
                            ));
                        }
                        "tool_result" => {
                            let id = block
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let is_error = block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let content_text = stringify_anthropic_text_or_blocks(
                                block.get("content").unwrap_or(&Value::Null),
                            )
                            .unwrap_or_default();
                            tool_results.push(ToolResultBlock {
                                tool_call_id: id.clone(),
                                content: content_text.clone(),
                                is_error,
                            });
                            // Also surface in conversation for cold-resume.
                            conversation_lines.push(format!("Tool result ({id}): {content_text}"));
                        }
                        _ => {}
                    }
                }
                let joined = text_acc.join("\n");
                if !joined.is_empty() {
                    if role == "user" && is_last {
                        latest_user_text.push(joined);
                    } else {
                        conversation_lines.push(format!("{}: {}", role_label(role), joined));
                    }
                }
            }
            _ => {}
        }
    }

    if system_prompt.is_none() {
        system_prompt = body
            .get("system")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    let user_text = if conversation_lines.is_empty() {
        latest_user_text.join("\n")
    } else {
        let mut all = conversation_lines;
        if !latest_user_text.is_empty() {
            all.push(format!("User: {}", latest_user_text.join("\n")));
        }
        all.join("\n\n")
    };
    (system_prompt, user_text, images, tool_results)
}

fn stringify_anthropic_text_or_blocks(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

fn anthropic_image_to_ref(block: &Value) -> Option<ImageRef> {
    let source = block.get("source")?;
    let kind = source.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "base64" => {
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            let data = source.get("data").and_then(Value::as_str)?;
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data.trim())
                    .ok()?;
            Some(ImageRef::Inline {
                mime: media_type,
                data: Bytes::from(decoded),
            })
        }
        "url" => {
            let url = source.get("url").and_then(Value::as_str)?;
            if url.starts_with("data:") {
                Some(ImageRef::DataUri(url.to_string()))
            } else {
                Some(ImageRef::HttpUrl(url.to_string()))
            }
        }
        _ => None,
    }
}

// ─── OpenAI Chat Completions ───────────────────────────────────────────────

fn decompose_openai_chat(
    body: &Value,
) -> (Option<String>, String, Vec<ImageRef>, Vec<ToolResultBlock>) {
    let mut system_chunks: Vec<String> = Vec::new();
    let mut conversation_lines: Vec<String> = Vec::new();
    let mut latest_user_text: Vec<String> = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for (idx, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let is_last = idx == messages.len() - 1;
        let content = msg.get("content");
        match role {
            "system" => {
                if let Some(text) = content.and_then(openai_content_text) {
                    system_chunks.push(text);
                }
            }
            "user" => {
                if let Some(c) = content {
                    let (text, mut imgs) = openai_content_parts(c);
                    images.append(&mut imgs);
                    if !text.is_empty() {
                        if is_last {
                            latest_user_text.push(text);
                        } else {
                            conversation_lines.push(format!("User: {text}"));
                        }
                    }
                }
            }
            "assistant" => {
                let text = content.and_then(openai_content_text).unwrap_or_default();
                if !text.is_empty() {
                    conversation_lines.push(format!("Assistant: {text}"));
                }
                if let Some(tool_calls) = msg.get("tool_calls").and_then(Value::as_array) {
                    for tc in tool_calls {
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        conversation_lines.push(format!(
                            "Assistant called tool {name} ({id}) with arguments: {args}"
                        ));
                    }
                }
            }
            "tool" => {
                let id = msg
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let text = content.and_then(openai_content_text).unwrap_or_default();
                tool_results.push(ToolResultBlock {
                    tool_call_id: id.clone(),
                    content: text.clone(),
                    is_error: false,
                });
                conversation_lines.push(format!("Tool result ({id}): {text}"));
            }
            other => {
                let text = content.and_then(openai_content_text).unwrap_or_default();
                if !text.is_empty() {
                    conversation_lines.push(format!("{other}: {text}"));
                }
            }
        }
    }

    let user_text = if conversation_lines.is_empty() {
        latest_user_text.join("\n")
    } else {
        let mut all = conversation_lines;
        if !latest_user_text.is_empty() {
            all.push(format!("User: {}", latest_user_text.join("\n")));
        }
        all.join("\n\n")
    };
    let system_prompt = if system_chunks.is_empty() {
        None
    } else {
        Some(system_chunks.join("\n\n"))
    };
    (system_prompt, user_text, images, tool_results)
}

fn openai_content_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str).map(str::to_string))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

fn openai_content_parts(v: &Value) -> (String, Vec<ImageRef>) {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    match v {
        Value::String(s) => texts.push(s.clone()),
        Value::Array(arr) => {
            for part in arr {
                let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "text" | "input_text" => {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            texts.push(t.to_string());
                        }
                    }
                    "image_url" => {
                        let url = part
                            .get("image_url")
                            .and_then(|iu| iu.get("url"))
                            .or_else(|| part.get("image_url").filter(|v| v.is_string()))
                            .and_then(Value::as_str);
                        if let Some(url) = url {
                            push_image_ref(url, &mut images);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (texts.join("\n"), images)
}

fn push_image_ref(url: &str, out: &mut Vec<ImageRef>) {
    if url.starts_with("data:") {
        out.push(ImageRef::DataUri(url.to_string()));
    } else if url.starts_with("http://") || url.starts_with("https://") {
        out.push(ImageRef::HttpUrl(url.to_string()));
    }
}

// ─── OpenAI Responses ──────────────────────────────────────────────────────

fn decompose_openai_responses(
    body: &Value,
) -> (Option<String>, String, Vec<ImageRef>, Vec<ToolResultBlock>) {
    let mut system_chunks: Vec<String> = Vec::new();
    let mut conversation_lines: Vec<String> = Vec::new();
    let mut latest_user_text: Vec<String> = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();

    if let Some(instructions) = body.get("instructions").and_then(Value::as_str) {
        system_chunks.push(instructions.to_string());
    }

    // Responses `input` can be:
    //   * a string (single user turn)
    //   * an array of typed input items (messages, function_call,
    //     function_call_output, etc.)
    let input = body.get("input");
    if let Some(input) = input {
        match input {
            Value::String(s) => latest_user_text.push(s.clone()),
            Value::Array(items) => {
                let len = items.len();
                for (idx, item) in items.iter().enumerate() {
                    let kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("message");
                    let is_last = idx == len - 1;
                    match kind {
                        "message" | "" => {
                            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                            let (text, mut imgs) = openai_responses_content_parts(
                                item.get("content").unwrap_or(&Value::Null),
                            );
                            images.append(&mut imgs);
                            match role {
                                "system" => {
                                    if !text.is_empty() {
                                        system_chunks.push(text);
                                    }
                                }
                                "user" => {
                                    if !text.is_empty() {
                                        if is_last {
                                            latest_user_text.push(text);
                                        } else {
                                            conversation_lines.push(format!("User: {text}"));
                                        }
                                    }
                                }
                                "assistant" => {
                                    if !text.is_empty() {
                                        conversation_lines.push(format!("Assistant: {text}"));
                                    }
                                }
                                other => {
                                    if !text.is_empty() {
                                        conversation_lines.push(format!("{other}: {text}"));
                                    }
                                }
                            }
                        }
                        "function_call" => {
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                            let call_id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let args = item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}");
                            conversation_lines.push(format!(
                                "Assistant called tool {name} ({call_id}) with arguments: {args}"
                            ));
                        }
                        "function_call_output" => {
                            let call_id = item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let output = item
                                .get("output")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            tool_results.push(ToolResultBlock {
                                tool_call_id: call_id.clone(),
                                content: output.clone(),
                                is_error: false,
                            });
                            conversation_lines.push(format!("Tool result ({call_id}): {output}"));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let user_text = if conversation_lines.is_empty() {
        latest_user_text.join("\n")
    } else {
        let mut all = conversation_lines;
        if !latest_user_text.is_empty() {
            all.push(format!("User: {}", latest_user_text.join("\n")));
        }
        all.join("\n\n")
    };
    let system_prompt = if system_chunks.is_empty() {
        None
    } else {
        Some(system_chunks.join("\n\n"))
    };
    (system_prompt, user_text, images, tool_results)
}

fn openai_responses_content_parts(v: &Value) -> (String, Vec<ImageRef>) {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    match v {
        Value::String(s) => texts.push(s.clone()),
        Value::Array(arr) => {
            for part in arr {
                let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "input_text" | "text" | "output_text" => {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            texts.push(t.to_string());
                        }
                    }
                    "input_image" => {
                        let url = part
                            .get("image_url")
                            .and_then(Value::as_str)
                            .or_else(|| part.get("url").and_then(Value::as_str));
                        if let Some(url) = url {
                            push_image_ref(url, &mut images);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (texts.join("\n"), images)
}

// ─── Gemini Native ─────────────────────────────────────────────────────────

fn decompose_gemini_native(
    body: &Value,
) -> (Option<String>, String, Vec<ImageRef>, Vec<ToolResultBlock>) {
    let mut system_chunks = Vec::new();
    if let Some(system) = body
        .get("systemInstruction")
        .or_else(|| body.get("system_instruction"))
    {
        let (text, _) = gemini_parts_text_images(system.get("parts").unwrap_or(system));
        if !text.is_empty() {
            system_chunks.push(text);
        }
    }

    let mut conversation_lines = Vec::new();
    let mut latest_user_text = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    let contents = body
        .get("contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for (idx, content) in contents.iter().enumerate() {
        let role = content
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let is_last = idx == contents.len() - 1;
        let parts = content.get("parts").unwrap_or(&Value::Null);
        let (text, mut part_images) = gemini_parts_text_images(parts);
        images.append(&mut part_images);

        for function_call in gemini_function_calls(parts) {
            let name = function_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = function_call
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({}))
                .to_string();
            if !name.is_empty() {
                conversation_lines.push(format!(
                    "Assistant called tool {name} ({name}) with arguments: {args}"
                ));
            }
        }
        for function_response in gemini_function_responses(parts) {
            let name = function_response
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("gemini_function_response")
                .to_string();
            let response = function_response
                .get("response")
                .map(Value::to_string)
                .unwrap_or_else(|| "{}".to_string());
            tool_results.push(ToolResultBlock {
                tool_call_id: name.clone(),
                content: response.clone(),
                is_error: false,
            });
            conversation_lines.push(format!("Tool result ({name}): {response}"));
        }

        if text.is_empty() {
            continue;
        }
        match role {
            "user" if is_last => latest_user_text.push(text),
            "user" => conversation_lines.push(format!("User: {text}")),
            "model" | "assistant" => conversation_lines.push(format!("Assistant: {text}")),
            "system" => system_chunks.push(text),
            other => conversation_lines.push(format!("{other}: {text}")),
        }
    }

    let user_text = if conversation_lines.is_empty() {
        latest_user_text.join("\n")
    } else {
        let mut all = conversation_lines;
        if !latest_user_text.is_empty() {
            all.push(format!("User: {}", latest_user_text.join("\n")));
        }
        all.join("\n\n")
    };
    let system_prompt = if system_chunks.is_empty() {
        None
    } else {
        Some(system_chunks.join("\n\n"))
    };
    (system_prompt, user_text, images, tool_results)
}

fn gemini_parts_text_images(parts: &Value) -> (String, Vec<ImageRef>) {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    let part_iter: Vec<&Value> = match parts {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![parts],
        _ => Vec::new(),
    };
    for part in part_iter {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            texts.push(text.to_string());
        }
        if let Some(image) = gemini_inline_image(part) {
            images.push(image);
        }
        if let Some(image) = gemini_file_image(part) {
            images.push(image);
        }
    }
    (texts.join("\n"), images)
}

fn gemini_inline_image(part: &Value) -> Option<ImageRef> {
    let data = part.get("inlineData").or_else(|| part.get("inline_data"))?;
    let mime = data
        .get("mimeType")
        .or_else(|| data.get("mime_type"))
        .and_then(Value::as_str)
        .unwrap_or("image/png")
        .to_string();
    let raw = data.get("data").and_then(Value::as_str)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .ok()?;
    Some(ImageRef::Inline {
        mime,
        data: Bytes::from(decoded),
    })
}

fn gemini_file_image(part: &Value) -> Option<ImageRef> {
    let data = part.get("fileData").or_else(|| part.get("file_data"))?;
    let uri = data
        .get("fileUri")
        .or_else(|| data.get("file_uri"))
        .and_then(Value::as_str)?;
    if uri.starts_with("data:") {
        Some(ImageRef::DataUri(uri.to_string()))
    } else if uri.starts_with("http://") || uri.starts_with("https://") {
        Some(ImageRef::HttpUrl(uri.to_string()))
    } else {
        None
    }
}

fn gemini_function_calls(parts: &Value) -> Vec<Value> {
    gemini_part_objects(parts, "functionCall", "function_call")
}

fn gemini_function_responses(parts: &Value) -> Vec<Value> {
    gemini_part_objects(parts, "functionResponse", "function_response")
}

fn gemini_part_objects(parts: &Value, camel: &str, snake: &str) -> Vec<Value> {
    let part_iter: Vec<&Value> = match parts {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![parts],
        _ => Vec::new(),
    };
    part_iter
        .into_iter()
        .filter_map(|part| part.get(camel).or_else(|| part.get(snake)).cloned())
        .collect()
}

fn gemini_tools_to_mcp_defs(tools: Option<&Value>) -> Vec<McpToolDef> {
    let Some(Value::Array(items)) = tools else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tool in items {
        let declarations = tool
            .get("functionDeclarations")
            .or_else(|| tool.get("function_declarations"))
            .and_then(Value::as_array);
        let Some(declarations) = declarations else {
            continue;
        };
        for declaration in declarations {
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let schema = declaration
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            out.push(McpToolDef {
                name: name.clone(),
                description: declaration
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                input_schema: Bytes::from(schema.to_string()),
                provider_identifier: "cc-switch".to_string(),
                tool_name: name,
            });
        }
    }
    out
}

fn role_label(role: &str) -> &'static str {
    match role {
        "system" => "System",
        "assistant" => "Assistant",
        "tool" => "Tool",
        _ => "User",
    }
}

// ─── Tool directives & output constraints (OmniRoute / composer-api) ───────

#[derive(Debug, Clone, Default)]
pub enum ExtractedToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
}

pub fn extract_tool_choice(body: &Value, protocol: InboundProtocol) -> ExtractedToolChoice {
    let Some(raw) = body.get("tool_choice") else {
        return ExtractedToolChoice::Auto;
    };
    match protocol {
        InboundProtocol::AnthropicMessages => match raw.get("type").and_then(Value::as_str) {
            Some("none") => ExtractedToolChoice::None,
            Some("any") => ExtractedToolChoice::Required,
            Some("tool") => raw
                .get("name")
                .and_then(Value::as_str)
                .map(|n| ExtractedToolChoice::Named(n.to_string()))
                .unwrap_or(ExtractedToolChoice::Auto),
            Some("auto") | None => ExtractedToolChoice::Auto,
            _ => ExtractedToolChoice::Auto,
        },
        InboundProtocol::OpenAiChat
        | InboundProtocol::OpenAiResponses
        | InboundProtocol::GeminiNative => {
            if raw.as_str() == Some("none") {
                ExtractedToolChoice::None
            } else if raw.as_str() == Some("required") {
                ExtractedToolChoice::Required
            } else if raw.get("type").and_then(Value::as_str) == Some("function") {
                raw.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .map(|n| ExtractedToolChoice::Named(n.to_string()))
                    .unwrap_or(ExtractedToolChoice::Auto)
            } else {
                ExtractedToolChoice::Auto
            }
        }
    }
}

pub fn tool_choice_disables_tools(choice: &ExtractedToolChoice) -> bool {
    matches!(choice, ExtractedToolChoice::None)
}

fn tool_choice_directive_line(choice: &ExtractedToolChoice) -> &'static str {
    match choice {
        ExtractedToolChoice::Required => {
            "\nYou MUST call at least one of the available tools now; do not answer without calling a tool."
        }
        ExtractedToolChoice::Named(_) => {
            "\nYou MUST call the specified tool now and not any other tool."
        }
        _ => "",
    }
}

fn tool_choice_named_suffix(choice: &ExtractedToolChoice) -> String {
    if let ExtractedToolChoice::Named(name) = choice {
        format!("\nYou MUST call the `{name}` tool now and not any other tool.")
    } else {
        String::new()
    }
}

pub fn build_output_constraints(body: &Value, protocol: InboundProtocol) -> String {
    let mut constraints: Vec<String> = Vec::new();

    let max_tokens = match protocol {
        InboundProtocol::OpenAiResponses => body
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .or_else(|| body.get("max_tokens").and_then(Value::as_u64)),
        _ => body
            .get("max_completion_tokens")
            .and_then(Value::as_u64)
            .or_else(|| body.get("max_tokens").and_then(Value::as_u64)),
    };
    if let Some(n) = max_tokens {
        if n > 0 {
            constraints.push(format!("Keep the answer within about {n} output tokens."));
        }
    }

    if let Some(stop) = body.get("stop") {
        match stop {
            Value::String(s) if !s.is_empty() => {
                constraints.push(format!(
                    "Do not include any text at or after this stop sequence: {s}"
                ));
            }
            Value::Array(arr) => {
                let parts: Vec<&str> = arr
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .collect();
                if !parts.is_empty() {
                    constraints.push(format!(
                        "Stop before any of these sequences: {}",
                        parts.join(", ")
                    ));
                }
            }
            _ => {}
        }
    }

    let fmt = body.get("response_format").or_else(|| body.get("text"));
    if let Some(fmt) = fmt {
        let fmt_type = fmt.get("type").and_then(Value::as_str);
        if fmt_type == Some("json_object") {
            constraints.push(
                "Return a single valid JSON object and no surrounding prose or code fences."
                    .to_string(),
            );
        } else if fmt_type == Some("json_schema") {
            let schema = fmt
                .get("json_schema")
                .and_then(|js| js.get("schema"))
                .or_else(|| fmt.get("schema"));
            constraints.push(format!(
                "Return only valid JSON (no prose or code fences) matching this schema: {}",
                schema
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| fmt.to_string())
            ));
        }
    }

    if constraints.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nOUTPUT CONSTRAINTS:\n{}",
            constraints
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

pub fn tool_commit_enabled() -> bool {
    match std::env::var("CC_SWITCH_CURSOR_TOOL_DIRECTIVE")
        .or_else(|_| std::env::var("CURSOR_TOOL_DIRECTIVE"))
    {
        Ok(v) => !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off")),
        Err(_) => true,
    }
}

pub fn enhance_agent_user_text(
    user_text: &str,
    tool_choice: &ExtractedToolChoice,
    tools: &[McpToolDef],
    body: &Value,
    protocol: InboundProtocol,
) -> String {
    let mut prefix = String::new();
    if !tools.is_empty() && tool_commit_enabled() {
        prefix.push_str(TOOL_COMMIT_DIRECTIVE);
        if matches!(tool_choice, ExtractedToolChoice::Named(_)) {
            prefix.push_str(&tool_choice_named_suffix(tool_choice));
        } else {
            prefix.push_str(tool_choice_directive_line(tool_choice));
        }
        prefix.push_str("\n\n");
    }
    let constraints = build_output_constraints(body, protocol);
    if prefix.is_empty() && constraints.is_empty() {
        user_text.to_string()
    } else {
        format!("{prefix}{user_text}{constraints}")
    }
}

pub fn extract_working_directory(body: &Value) -> String {
    body.get("metadata")
        .and_then(|m| m.get("working_directory"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("metadata")
                .and_then(|m| m.get("cwd"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::env::var("CC_SWITCH_CURSOR_WORKING_DIR")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_WORKING_DIRECTORY.to_string())
        })
}

/// Retry prompt when a tool-using turn ended without surfacing a tool call.
pub fn retry_prompt_after_missing_tool(
    user_text: &str,
    attempt: usize,
    max_attempts: usize,
) -> String {
    format!(
        "{user_text}\n\n\
         [cc-switch retry {attempt}/{max_attempts}] \
         You declared tools but did not call any. You MUST call an appropriate tool now \
         instead of describing what you would do."
    )
}

/// Retry when the model invoked a tool outside the client inventory.
pub fn retry_prompt_after_unmapped_tool(
    user_text: &str,
    tool_name: &str,
    attempt: usize,
    max_attempts: usize,
) -> String {
    format!(
        "{user_text}\n\n\
         [cc-switch retry {attempt}/{max_attempts}] \
         Tool `{tool_name}` is not in the client tool inventory. \
         You MUST call one of the declared tools with valid arguments."
    )
}

/// Retry when Cursor invoked a declared tool with arguments that do not satisfy
/// the client's schema, or when the arguments clearly belong to another tool.
pub fn retry_prompt_after_invalid_tool(
    user_text: &str,
    reason: &str,
    allowed_tools: &[String],
    attempt: usize,
    max_attempts: usize,
) -> String {
    let allowed = if allowed_tools.is_empty() {
        "none".to_string()
    } else {
        allowed_tools.join(", ")
    };
    format!(
        "{user_text}\n\n\
         [cc-switch retry {attempt}/{max_attempts}] \
         The previous tool call was rejected before reaching the client because its \
         arguments did not match the declared tool schema: {reason}. \
         Allowed tool names: {allowed}. \
         You MUST call one of the declared tools with valid arguments."
    )
}

/// Rough input token estimate for usage events (chars / 4).
pub fn estimate_input_tokens(text: &str) -> u32 {
    let len = text.len();
    if len == 0 {
        return 0;
    }
    ((len / 4).max(1)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_single_user_string() {
        let body = json!({
            "model": "claude-sonnet-4-7",
            "messages": [{ "role": "user", "content": "hello" }]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert_eq!(plan.user_text, "hello");
        assert!(plan.tools.is_empty());
        assert!(plan.images.is_empty());
    }

    #[test]
    fn anthropic_system_and_tools() {
        let body = json!({
            "model": "claude-sonnet-4-7",
            "system": "be precise",
            "tools": [{ "name": "weather", "description": "wx",
                         "input_schema": {"type": "object"} }],
            "messages": [{ "role": "user", "content": "hello" }]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert_eq!(plan.system_prompt.as_deref(), Some("be precise"));
        assert_eq!(plan.tools.len(), 1);
        assert_eq!(plan.tools[0].name, "weather");
    }

    #[test]
    fn anthropic_tool_result_round_trip() {
        let body = json!({
            "model": "claude-sonnet-4-7",
            "messages": [
                { "role": "user", "content": "what is the weather?" },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "tc_1", "name": "weather", "input": {"city":"BJ"} }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "tc_1", "content": "sunny" }
                ]}
            ]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert_eq!(plan.tool_results.len(), 1);
        assert_eq!(plan.tool_results[0].tool_call_id, "tc_1");
        assert_eq!(plan.tool_results[0].content, "sunny");
    }

    #[test]
    fn openai_chat_image_url() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "look:" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/x.png" } }
                ]
            }]
        });
        let plan = build_plan(InboundProtocol::OpenAiChat, &body);
        assert_eq!(plan.images.len(), 1);
        match &plan.images[0] {
            ImageRef::HttpUrl(u) => assert_eq!(u, "https://example.com/x.png"),
            _ => panic!("expected HttpUrl"),
        }
    }

    #[test]
    fn openai_responses_function_call_output() {
        let body = json!({
            "model": "gpt-5",
            "input": [
                { "type": "message", "role": "user", "content": [
                    { "type": "input_text", "text": "weather?" }
                ]},
                { "type": "function_call", "name": "weather", "call_id": "fc_1",
                  "arguments": "{\"city\":\"BJ\"}" },
                { "type": "function_call_output", "call_id": "fc_1", "output": "sunny" }
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(plan.tool_results.len(), 1);
        assert_eq!(plan.tool_results[0].tool_call_id, "fc_1");
        assert_eq!(plan.tool_results[0].content, "sunny");
    }

    #[test]
    fn openai_responses_previous_response_id_extracted() {
        let body = json!({
            "model": "gpt-5",
            "previous_response_id": "resp_abc",
            "input": "again"
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(plan.previous_response_id.as_deref(), Some("resp_abc"));
    }

    #[test]
    fn validate_tool_result_context_rejects_empty_call_id() {
        let body = json!({
            "model": "gpt-5",
            "input": [
                { "type": "function_call_output", "call_id": "", "output": "bad" }
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(validate_tool_result_context(&plan).is_err());
    }

    #[test]
    fn validate_tool_result_context_accepts_non_empty_call_id() {
        let body = json!({
            "model": "gpt-5",
            "input": [
                { "type": "function_call_output", "call_id": "fc_1", "output": "ok" }
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(validate_tool_result_context(&plan).is_ok());
    }

    #[test]
    fn validate_tool_result_context_accepts_no_tool_results() {
        let body = json!({ "model": "gpt-5", "input": "hello" });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(validate_tool_result_context(&plan).is_ok());
    }

    #[test]
    fn tool_choice_none_strips_tools() {
        let body = json!({
            "model": "gpt-5",
            "tool_choice": "none",
            "tools": [{ "type": "function", "function": { "name": "Bash", "parameters": {} } }],
            "input": "hello"
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.tools.is_empty());
        assert!(!plan.user_text.contains("executable tools"));
    }

    #[test]
    fn tools_inject_commit_directive() {
        let body = json!({
            "model": "gpt-5",
            "tools": [{ "type": "function", "function": { "name": "Bash", "parameters": {} } }],
            "input": "run ls"
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.user_text.contains("MUST issue the actual tool call"));
        assert!(plan.user_text.contains("run ls"));
    }

    #[test]
    fn tool_choice_required_adds_directive() {
        let body = json!({
            "model": "gpt-5",
            "tool_choice": "required",
            "tools": [{ "type": "function", "function": { "name": "Bash", "parameters": {} } }],
            "input": "go"
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.user_text.contains("MUST call at least one"));
    }

    #[test]
    fn output_constraints_max_tokens() {
        let body = json!({
            "model": "gpt-5",
            "max_output_tokens": 512,
            "input": "hi"
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.user_text.contains("512 output tokens"));
    }

    #[test]
    fn gemini_native_extracts_text_image_and_tools() {
        let body = json!({
            "model": "gemini-2.5-pro",
            "systemInstruction": {"parts": [{"text": "be terse"}]},
            "contents": [{
                "role": "user",
                "parts": [
                    {"text": "describe"},
                    {"inlineData": {
                        "mimeType": "image/png",
                        "data": "aGVsbG8="
                    }}
                ]
            }],
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup",
                    "description": "lookup data",
                    "parameters": {"type": "object", "properties": {}}
                }]
            }]
        });
        let plan = build_plan(InboundProtocol::GeminiNative, &body);
        assert_eq!(plan.model_id, "gemini-2.5-pro");
        assert!(plan.user_text.contains("be terse"));
        assert!(plan.user_text.contains("describe"));
        assert_eq!(plan.images.len(), 1);
        assert_eq!(plan.tools.len(), 1);
        assert_eq!(plan.tools[0].name, "lookup");
    }

    #[test]
    fn estimate_input_tokens_nonzero_for_text() {
        assert!(estimate_input_tokens("hello world this is a test") > 0);
        assert_eq!(estimate_input_tokens(""), 0);
    }

    #[test]
    fn tool_commit_can_be_disabled_via_env() {
        std::env::set_var("CC_SWITCH_CURSOR_TOOL_DIRECTIVE", "0");
        assert!(!tool_commit_enabled());
        std::env::remove_var("CC_SWITCH_CURSOR_TOOL_DIRECTIVE");
        assert!(tool_commit_enabled());
    }

    #[test]
    fn composer_model_forces_working_directory_default() {
        let body = json!({ "model": "composer-2.5", "input": "hi" });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(plan.working_directory, ".");
    }
}
