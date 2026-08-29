//! Pull the structured fields cursor's AgentService needs out of the four
//! request body shapes cc-switch accepts (Anthropic Messages, OpenAI Chat
//! Completions, OpenAI Responses, Gemini native).
//!
//! Tool-steering directives (`TOOL_COMMIT_DIRECTIVE`, `tool_choice` hints) and
//! output constraints (`max_tokens`, `stop`, `response_format`) are injected
//! into `user_text` because Cursor's AgentService has no native equivalents
//! (ported from OmniRoute / composer-api).

use super::agent_proto::{
    anthropic_tools_to_mcp_defs, openai_tools_to_mcp_defs, McpToolDef,
    CLIENT_MCP_PROVIDER_IDENTIFIER,
};
use super::image::ImageRef;
use super::tool_schema::{validate_tool_schema, ToolSchemaErrorKind};
use bytes::Bytes;
use serde_json::{json, Value};

/// Prepended when the client declares tools. Cursor exposes these through its
/// SDK MCP execution path; naming that path explicitly is required for custom
/// Codex tools such as `exec`, whose client-facing name is not an SDK builtin.
const TOOL_COMMIT_DIRECTIVE: &str = "\
You are serving an OpenAI-compatible API request through Cursor SDK and the outer client has provided executable tools.\n\
The declared client tool names are execution targets, not Cursor SDK builtin names. For local work, call the exact client target through SDK mcp with providerIdentifier \"client\", toolName set to the declared client tool name, and args matching its declared schema.\n\
When a tool is needed to answer (real-time data, web/search lookups, file or project operations), you MUST issue the actual SDK tool call. Do NOT describe what you are about to do as prose and then stop — call the tool.\n\
Answer directly only when no tool is needed.\n\
Do not emit duplicate tool calls: call each operation once, then continue after the tool result is returned.\n\
Never claim that tools are unavailable.";

const LOCAL_TOOL_REQUIRED_DIRECTIVE: &str = "\
\n\nLOCAL TOOL REQUIRED FOR THE LATEST USER REQUEST:\n\
The latest request requires local filesystem or shell execution. The next response is invalid unless it contains exactly one SDK mcp tool call before any prose or progress text.\n\
Use providerIdentifier \"client\" and an exact toolName from the SDK CLIENT TOOL ROUTING MAP, then wait for the outer client to return the tool result.";

const DEFAULT_WORKING_DIRECTORY: &str = ".";
const MAX_RESPONSE_TOOL_COUNT: usize = 128;
const MAX_RESPONSE_TOOL_NAMESPACE_DEPTH: usize = 4;
const MAX_RESPONSE_TOOL_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GeminiNative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultBlock {
    /// Client-facing tool call id — what cc-switch emitted in the previous
    /// turn. Used to look up the pending exec_id in the session.
    pub tool_call_id: String,
    /// Result content as a plain string (cursor's McpResult expects text).
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseToolNamespace {
    /// Flattened name exposed to Cursor's MCP inventory.
    pub internal_name: String,
    /// Original OpenAI Responses namespace and leaf name restored on output.
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolContinuationKind {
    #[default]
    None,
    PureToolResults,
    MixedToolResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTaskTurnSignal {
    Activate,
    Continue,
    Replace,
}

#[derive(Debug, Clone)]
pub struct AgentRunPlan {
    pub inbound_protocol: InboundProtocol,
    pub system_prompt: Option<String>,
    pub user_text: String,
    pub tools: Vec<McpToolDef>,
    /// OpenAI Responses tools that must be surfaced as `custom_tool_call`
    /// rather than ordinary `function_call` items. Cursor still sees these
    /// through a JSON MCP wrapper, but Codex receives its original wire kind.
    pub custom_tool_names: Vec<String>,
    /// Namespace metadata removed while adapting Responses tools to Cursor's
    /// flat MCP inventory and restored on downstream function-call items.
    pub response_tool_namespaces: Vec<ResponseToolNamespace>,
    pub images: Vec<ImageRef>,
    /// Tool results from completed earlier turns. They are retained in the
    /// flattened transcript for cold resume, but must never by themselves
    /// trigger a parked-session lookup.
    pub historical_tool_results: Vec<ToolResultBlock>,
    /// Results submitted by the current request delta. Only these may resume
    /// a live AgentService stream.
    pub tool_results: Vec<ToolResultBlock>,
    pub continuation_kind: ToolContinuationKind,
    /// The flattened request contains the assistant-side call metadata for
    /// every active result, so a fresh AgentService run can continue without
    /// asking the client to execute the same tool again.
    pub cold_resume_ready: bool,
    pub completed_tool_calls: Vec<CompletedToolCall>,
    /// Cursor's `RequestedModel.model_id` — the value passed to
    /// Cursor's model resolver. Comes from the upstream-mapped body.
    pub model_id: String,
    /// Optional Responses API `previous_response_id` — used to find a parked
    /// session.
    pub previous_response_id: Option<String>,
    /// Working directory surfaced in RequestContext ack (composer-api SDK).
    pub working_directory: String,
    pub tool_choice: ExtractedToolChoice,
    /// Credential-free Responses items retained across a parked tool turn and
    /// copied into completed-response state after the final terminal.
    pub response_input_items: Vec<Value>,
    /// A conservative semantic signal used to reject a promise-only response
    /// when the latest request plainly requires local project inspection.
    pub local_tool_required_by_intent: bool,
}

#[derive(Default)]
struct OpenAiResponseToolInventory {
    tools: Vec<McpToolDef>,
    custom_tool_names: Vec<String>,
    namespaces: Vec<ResponseToolNamespace>,
    registrations: std::collections::HashMap<String, ToolRegistrationSignature>,
}

#[derive(Debug, Clone, PartialEq)]
struct ToolRegistrationSignature {
    custom: bool,
    description: String,
    schema: super::agent_proto::McpToolSchema,
}

fn validate_mcp_tool_schemas(tools: &[McpToolDef]) -> Result<(), String> {
    for tool in tools {
        validate_tool_schema(tool.input_schema.as_json()).map_err(|error| {
            let reason = match error.kind {
                ToolSchemaErrorKind::InvalidSchema => "invalid_tool_schema",
                ToolSchemaErrorKind::ComplexityLimit => "schema_complexity_limit",
                ToolSchemaErrorKind::Validation => "invalid_tool_schema",
            };
            format!(
                "Cursor tool `{}` schema is invalid ({reason}): {} at {}",
                tool.name, error.message, error.path
            )
        })?;
    }
    Ok(())
}

/// Validate tool-result context for AgentService routing. Returns an error
/// message if the request carries a `function_call_output` / `tool_result`
/// whose `call_id` is empty — Cursor's AgentService cannot match it to a
/// pending exec_id and the turn would silently fail. Mirrors sub2api's
/// `validateFunctionCallOutputRequest` guard.
pub fn validate_tool_result_context(plan: &AgentRunPlan) -> Result<(), String> {
    let mut seen = std::collections::HashMap::<&str, (&str, bool)>::new();
    for tr in &plan.tool_results {
        let call_id = tr.tool_call_id.trim();
        if call_id.is_empty() {
            return Err("function_call_output requires a non-empty call_id; \
                 continuation via previous_response_id without call_id is not supported"
                .to_string());
        }
        if let Some((content, is_error)) = seen.insert(call_id, (&tr.content, tr.is_error)) {
            if content != tr.content || is_error != tr.is_error {
                return Err(format!(
                    "conflicting tool results for call_id `{call_id}` are not supported"
                ));
            }
        }
    }
    Ok(())
}

fn active_tool_results(
    protocol: InboundProtocol,
    body: &Value,
) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    match protocol {
        InboundProtocol::AnthropicMessages => active_anthropic_tool_results(body),
        InboundProtocol::OpenAiChat => active_openai_chat_tool_results(body),
        InboundProtocol::OpenAiResponses => active_openai_response_tool_results(body),
        InboundProtocol::GeminiNative => active_gemini_tool_results(body),
    }
}

fn active_anthropic_tool_results(body: &Value) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    let Some(message) = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return (Vec::new(), ToolContinuationKind::None);
    };
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return (Vec::new(), ToolContinuationKind::None);
    };
    let mut results = Vec::new();
    let mut mixed = false;
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "tool_result" => {
                let content = stringify_anthropic_text_or_blocks(
                    block.get("content").unwrap_or(&Value::Null),
                )
                .unwrap_or_default();
                results.push(ToolResultBlock {
                    tool_call_id: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    content,
                    is_error: block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            "text" => {
                mixed |= block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty());
            }
            _ => mixed = true,
        }
    }
    continuation_kind(results, mixed)
}

fn active_openai_chat_tool_results(body: &Value) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return (Vec::new(), ToolContinuationKind::None);
    };
    let mut results = messages
        .iter()
        .rev()
        .take_while(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .map(|message| ToolResultBlock {
            tool_call_id: message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            content: message
                .get("content")
                .and_then(openai_content_text)
                .unwrap_or_default(),
            is_error: false,
        })
        .collect::<Vec<_>>();
    results.reverse();
    continuation_kind(results, false)
}

fn active_openai_response_tool_results(
    body: &Value,
) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    let Some(items) = body.get("input").and_then(Value::as_array) else {
        return (Vec::new(), ToolContinuationKind::None);
    };
    let mut results = items
        .iter()
        .rev()
        .take_while(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            )
        })
        .map(|item| ToolResultBlock {
            tool_call_id: item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            content: item
                .get("output")
                .map(response_tool_output_text)
                .unwrap_or_default(),
            is_error: false,
        })
        .collect::<Vec<_>>();
    results.reverse();
    continuation_kind(results, false)
}

fn active_gemini_tool_results(body: &Value) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    let Some(content) = body
        .get("contents")
        .and_then(Value::as_array)
        .and_then(|contents| contents.last())
        .filter(|content| {
            content
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
                == "user"
        })
    else {
        return (Vec::new(), ToolContinuationKind::None);
    };
    let parts = content.get("parts").unwrap_or(&Value::Null);
    let results = gemini_function_responses(parts)
        .into_iter()
        .map(|response| {
            let name = response
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("gemini_function_response");
            ToolResultBlock {
                tool_call_id: response
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .unwrap_or(name)
                    .to_string(),
                content: response
                    .get("response")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".to_string()),
                is_error: false,
            }
        })
        .collect::<Vec<_>>();
    let part_iter = match parts {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![parts],
        _ => Vec::new(),
    };
    let mixed = part_iter.iter().any(|part| {
        part.get("functionResponse").is_none()
            && part.get("function_response").is_none()
            && part
                .get("text")
                .and_then(Value::as_str)
                .is_none_or(|text| !text.trim().is_empty())
    });
    continuation_kind(results, mixed)
}

fn continuation_kind(
    results: Vec<ToolResultBlock>,
    mixed: bool,
) -> (Vec<ToolResultBlock>, ToolContinuationKind) {
    let kind = if results.is_empty() {
        ToolContinuationKind::None
    } else if mixed {
        ToolContinuationKind::MixedToolResults
    } else {
        ToolContinuationKind::PureToolResults
    };
    (results, kind)
}

fn stringify_json_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn response_tool_output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let rendered = items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.clone()),
                    Value::Object(object) => object
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some(Value::Object(object.clone()).to_string())),
                    other if !other.is_null() => Some(other.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if rendered.is_empty() {
                value.to_string()
            } else {
                rendered.join("\n")
            }
        }
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string()),
        _ => stringify_json_text(value),
    }
}

fn tool_call_context_complete(
    protocol: InboundProtocol,
    body: &Value,
    active_results: &[ToolResultBlock],
) -> bool {
    if active_results.is_empty() {
        return false;
    }
    let mut call_ids = std::collections::HashSet::<String>::new();
    match protocol {
        InboundProtocol::AnthropicMessages => {
            for message in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            {
                for block in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        call_ids.insert(id.to_string());
                    }
                }
            }
        }
        InboundProtocol::OpenAiChat => {
            for message in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            {
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        call_ids.insert(id.to_string());
                    }
                }
            }
        }
        InboundProtocol::OpenAiResponses => {
            for item in body
                .get("input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|item| {
                    matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call" | "custom_tool_call")
                    )
                })
            {
                if let Some(id) = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                {
                    call_ids.insert(id.to_string());
                }
            }
        }
        InboundProtocol::GeminiNative => {
            for content in body
                .get("contents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|content| {
                    matches!(
                        content.get("role").and_then(Value::as_str),
                        Some("model" | "assistant")
                    )
                })
            {
                for call in gemini_function_calls(content.get("parts").unwrap_or(&Value::Null)) {
                    if let Some(id) = call
                        .get("id")
                        .or_else(|| call.get("name"))
                        .and_then(Value::as_str)
                    {
                        call_ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    active_results
        .iter()
        .all(|result| call_ids.contains(result.tool_call_id.trim()))
}

fn completed_tool_calls(
    protocol: InboundProtocol,
    body: &Value,
    active_results: &[ToolResultBlock],
) -> Vec<CompletedToolCall> {
    let active = active_results
        .iter()
        .map(|result| result.tool_call_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if active.is_empty() {
        return Vec::new();
    }
    let mut calls = Vec::new();
    match protocol {
        InboundProtocol::AnthropicMessages => {
            for block in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                .filter_map(|message| message.get("content").and_then(Value::as_array))
                .flatten()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            {
                let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                if active.contains(id) {
                    calls.push(CompletedToolCall {
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
            }
        }
        InboundProtocol::OpenAiChat => {
            for call in body
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
                .flatten()
            {
                let id = call.get("id").and_then(Value::as_str).unwrap_or("");
                if active.contains(id) {
                    let function = call.get("function").unwrap_or(&Value::Null);
                    calls.push(CompletedToolCall {
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .and_then(|arguments| serde_json::from_str(arguments).ok())
                            .unwrap_or_else(|| json!({})),
                    });
                }
            }
        }
        InboundProtocol::OpenAiResponses => {
            for call in body
                .get("input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|item| {
                    matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("function_call" | "custom_tool_call")
                    )
                })
            {
                let custom = call.get("type").and_then(Value::as_str) == Some("custom_tool_call");
                let id = call
                    .get("call_id")
                    .or_else(|| call.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if active.contains(id) {
                    calls.push(CompletedToolCall {
                        name: qualify_response_tool_name(
                            call.get("namespace").and_then(Value::as_str),
                            call.get("name").and_then(Value::as_str).unwrap_or(""),
                        ),
                        arguments: if custom {
                            json!({
                                "input": call
                                    .get("input")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                            })
                        } else {
                            call.get("arguments")
                                .and_then(Value::as_str)
                                .and_then(|arguments| serde_json::from_str(arguments).ok())
                                .unwrap_or_else(|| json!({}))
                        },
                    });
                }
            }
        }
        InboundProtocol::GeminiNative => {
            for call in body
                .get("contents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|content| {
                    gemini_function_calls(content.get("parts").unwrap_or(&Value::Null))
                })
            {
                let id = call
                    .get("id")
                    .or_else(|| call.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if active.contains(id) {
                    calls.push(CompletedToolCall {
                        name: call
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
                    });
                }
            }
        }
    }
    calls
}

fn openai_response_tool_inventory(body: &Value) -> Result<OpenAiResponseToolInventory, String> {
    let mut inventory = OpenAiResponseToolInventory::default();
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for tool in tools {
            collect_openai_response_tool(tool, None, 0, &mut inventory)?;
        }
    }
    if let Some(items) = body.get("input").and_then(Value::as_array) {
        for item in items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        {
            let tools = item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                "Responses additional_tools item requires a tools array".to_string()
            })?;
            for tool in tools {
                collect_openai_response_tool(tool, None, 0, &mut inventory)?;
            }
        }
    }
    Ok(inventory)
}

fn collect_openai_response_tool(
    tool: &Value,
    namespace: Option<&str>,
    depth: usize,
    inventory: &mut OpenAiResponseToolInventory,
) -> Result<(), String> {
    if inventory.tools.len() >= MAX_RESPONSE_TOOL_COUNT {
        return Err(format!(
            "Responses tool inventory exceeds {MAX_RESPONSE_TOOL_COUNT} tools"
        ));
    }
    let kind = tool
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function");
    if kind == "namespace" {
        if depth >= MAX_RESPONSE_TOOL_NAMESPACE_DEPTH {
            return Err(format!(
                "Responses tool namespace exceeds depth {MAX_RESPONSE_TOOL_NAMESPACE_DEPTH}"
            ));
        }
        let name = response_tool_name(tool)
            .ok_or_else(|| "Responses namespace tool requires a non-empty name".to_string())?;
        let full_namespace = qualify_response_tool_name(namespace, name);
        let children = tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
            format!("Responses namespace `{full_namespace}` requires a tools array")
        })?;
        for child in children {
            collect_openai_response_tool(child, Some(&full_namespace), depth + 1, inventory)?;
        }
        return Ok(());
    }

    let name = response_tool_name(tool)
        .ok_or_else(|| format!("Responses {kind} tool requires a non-empty name"))?;
    let full_name = qualify_response_tool_name(namespace, name);
    if full_name.len() > 256 {
        return Err("Responses tool name exceeds 256 bytes".to_string());
    }
    let description = response_tool_description(tool);
    if description.len() > MAX_RESPONSE_TOOL_TEXT_BYTES {
        return Err(format!(
            "Responses tool `{full_name}` description exceeds {MAX_RESPONSE_TOOL_TEXT_BYTES} bytes"
        ));
    }

    let (definition, custom) = match kind {
        "function" => {
            let parameters = response_function_parameters(tool)
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            (
                McpToolDef::new(
                    full_name.clone(),
                    description,
                    parameters,
                    CLIENT_MCP_PROVIDER_IDENTIFIER.to_string(),
                    full_name.clone(),
                ),
                false,
            )
        }
        "custom" => {
            let format = tool
                .get("format")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("Responses custom tool `{full_name}` requires format"))?;
            if format.get("type").and_then(Value::as_str) != Some("grammar")
                || format.get("syntax").and_then(Value::as_str) != Some("lark")
            {
                return Err(format!(
                    "Responses custom tool `{full_name}` only supports Lark grammar format"
                ));
            }
            let grammar = format
                .get("definition")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("Responses custom tool `{full_name}` requires grammar definition")
                })?;
            if grammar.is_empty() || grammar.len() > MAX_RESPONSE_TOOL_TEXT_BYTES {
                return Err(format!(
                    "Responses custom tool `{full_name}` grammar must be 1..={MAX_RESPONSE_TOOL_TEXT_BYTES} bytes"
                ));
            }
            let description = format!(
                "{description}\n\nCUSTOM TOOL INPUT CONTRACT:\nCall this tool with a JSON object containing exactly one string field named `input`. The value is the raw custom-tool source and must satisfy this Lark grammar:\n{grammar}"
            );
            let schema = json!({
                "type":"object",
                "properties":{
                    "input":{
                        "type":"string",
                        "description":"Raw custom tool input satisfying the declared Lark grammar"
                    }
                },
                "required":["input"],
                "additionalProperties":false
            });
            (
                McpToolDef::new(
                    full_name.clone(),
                    description,
                    schema,
                    CLIENT_MCP_PROVIDER_IDENTIFIER.to_string(),
                    full_name.clone(),
                ),
                true,
            )
        }
        other => {
            return Err(format!(
                "unsupported Responses tool type `{other}` in Cursor AgentService inventory"
            ))
        }
    };

    let normalized = normalize_response_tool_identity(&full_name);
    let signature = ToolRegistrationSignature {
        custom,
        description: definition.description.clone(),
        schema: definition.input_schema.clone(),
    };
    if let Some(existing) = inventory.registrations.get(&normalized) {
        if existing == &signature {
            return Ok(());
        }
        return Err(format!(
            "Responses tool `{full_name}` conflicts with another normalized tool name"
        ));
    }
    inventory.registrations.insert(normalized, signature);
    if custom {
        inventory.custom_tool_names.push(full_name.clone());
    }
    if let Some(namespace) = namespace {
        inventory.namespaces.push(ResponseToolNamespace {
            internal_name: full_name,
            namespace: namespace.to_string(),
            name: name.to_string(),
        });
    }
    inventory.tools.push(definition);
    Ok(())
}

fn response_tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")
        .and_then(|function| function.get("name"))
        .or_else(|| tool.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn response_tool_description(tool: &Value) -> String {
    tool.get("function")
        .and_then(|function| function.get("description"))
        .or_else(|| tool.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn response_function_parameters(tool: &Value) -> Option<Value> {
    tool.get("function")
        .and_then(|function| function.get("parameters"))
        .or_else(|| tool.get("parameters"))
        .or_else(|| tool.get("input_schema"))
        .cloned()
}

fn qualify_response_tool_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) => format!("{namespace}.{name}"),
        None => name.to_string(),
    }
}

fn normalize_response_tool_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn validate_tool_choice_contract(plan: &AgentRunPlan) -> Result<(), String> {
    if !plan.tool_results.is_empty() {
        return Ok(());
    }
    let required_name = match &plan.tool_choice {
        ExtractedToolChoice::Auto | ExtractedToolChoice::None => return Ok(()),
        ExtractedToolChoice::Required => None,
        ExtractedToolChoice::Named(name) => Some(name.as_str()),
    };
    if plan.tools.is_empty() {
        return Err("tool_choice requires at least one declared client tool".to_string());
    }
    if let Some(required_name) = required_name {
        let normalize = |value: &str| {
            value
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let required = normalize(required_name);
        let matches = plan
            .tools
            .iter()
            .filter(|tool| normalize(&tool.name) == required)
            .count();
        if matches != 1 {
            return Err(format!(
                "named tool_choice `{required_name}` must match exactly one declared client tool"
            ));
        }
    }
    Ok(())
}

/// Build a plan from a request body. The body is the **upstream-mapped**
/// version (after `apply_model_mapping`), so `model_id` here is what cursor
/// will see on the wire.
pub fn try_build_plan(protocol: InboundProtocol, body: &Value) -> Result<AgentRunPlan, String> {
    let model_id = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let previous_response_id = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let (system_prompt, user_text, images, all_tool_results) = match protocol {
        InboundProtocol::AnthropicMessages => decompose_anthropic(body),
        InboundProtocol::OpenAiChat => decompose_openai_chat(body),
        InboundProtocol::OpenAiResponses => decompose_openai_responses(body),
        InboundProtocol::GeminiNative => decompose_gemini_native(body),
    };
    let (tools, custom_tool_names, response_tool_namespaces) = match protocol {
        InboundProtocol::AnthropicMessages => (
            body.get("tools")
                .map(anthropic_tools_to_mcp_defs)
                .unwrap_or_default(),
            Vec::new(),
            Vec::new(),
        ),
        InboundProtocol::OpenAiChat => (
            body.get("tools")
                .map(openai_tools_to_mcp_defs)
                .unwrap_or_default(),
            Vec::new(),
            Vec::new(),
        ),
        InboundProtocol::OpenAiResponses => {
            let inventory = openai_response_tool_inventory(body)?;
            (
                inventory.tools,
                inventory.custom_tool_names,
                inventory.namespaces,
            )
        }
        InboundProtocol::GeminiNative => (
            gemini_tools_to_mcp_defs(body.get("tools")),
            Vec::new(),
            Vec::new(),
        ),
    };
    validate_mcp_tool_schemas(&tools)?;
    let tool_choice = extract_tool_choice(body, protocol);
    let mut tools = tools;
    if tool_choice_disables_tools(&tool_choice) {
        tools.clear();
    }
    let custom_tool_names = if tools.is_empty() {
        Vec::new()
    } else {
        custom_tool_names
    };
    let response_tool_namespaces = if tools.is_empty() {
        Vec::new()
    } else {
        response_tool_namespaces
    };
    let working_directory = extract_working_directory(body);
    let (tool_results, continuation_kind) = active_tool_results(protocol, body);
    let historical_count = all_tool_results.len().saturating_sub(tool_results.len());
    let historical_tool_results = all_tool_results[..historical_count].to_vec();
    let local_tool_required_by_intent =
        request_requires_local_tool(protocol, body, &tools, &tool_results);
    let cold_resume_ready = tool_call_context_complete(protocol, body, &tool_results);
    let completed_tool_calls = completed_tool_calls(protocol, body, &tool_results);
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
    let mut user_text =
        enhance_agent_user_text(&user_text_with_system, &tool_choice, &tools, body, protocol);
    if local_tool_required_by_intent && tool_commit_enabled() {
        user_text.push_str(LOCAL_TOOL_REQUIRED_DIRECTIVE);
    }
    if !tool_results.is_empty() && cold_resume_ready {
        user_text.push_str(
            "\n\nTOOL CONTINUATION SAFETY:\nThe tool results in this request have already been executed by the client. Continue from those results. Do not repeat an identical tool call or repeat its side effects.",
        );
    }

    Ok(AgentRunPlan {
        inbound_protocol: protocol,
        system_prompt,
        user_text,
        tools,
        custom_tool_names,
        response_tool_namespaces,
        images,
        historical_tool_results,
        tool_results,
        continuation_kind,
        cold_resume_ready,
        completed_tool_calls,
        model_id,
        previous_response_id,
        working_directory,
        tool_choice,
        response_input_items: if protocol == InboundProtocol::OpenAiResponses {
            normalized_response_input_items(body)
        } else {
            Vec::new()
        },
        local_tool_required_by_intent,
    })
}

/// Test helper for request bodies whose contract has already been validated.
/// Production paths use [`try_build_plan`] so an invalid tool inventory can
/// never be silently replaced with an empty one.
#[cfg(test)]
pub fn build_plan(protocol: InboundProtocol, body: &Value) -> AgentRunPlan {
    try_build_plan(protocol, body).expect("validated Cursor request plan")
}

/// Preserve the continuation classification from the original inbound delta
/// after Responses `previous_response_id` state has been prepended. Cached
/// historical function outputs are context, not a new tool continuation.
pub fn preserve_current_continuation(plan: &mut AgentRunPlan, original: &AgentRunPlan) {
    plan.tool_results = original.tool_results.clone();
    plan.continuation_kind = original.continuation_kind;
}

pub fn estimate_responses_input_tokens(body: &Value) -> Result<u32, String> {
    let plan = try_build_plan(InboundProtocol::OpenAiResponses, body)?;
    Ok(estimate_agent_plan_input_tokens(&plan))
}

pub fn estimate_agent_plan_input_tokens(plan: &AgentRunPlan) -> u32 {
    let mut characters = plan.user_text.chars().count() as u64;
    for tool in &plan.tools {
        characters = characters
            .saturating_add(tool.name.chars().count() as u64)
            .saturating_add(tool.description.chars().count() as u64)
            .saturating_add(tool.input_schema.encoded_json_len() as u64);
    }
    let estimated = characters.saturating_add(3) / 4;
    estimated.min(u64::from(u32::MAX)) as u32
}

pub fn validate_request_contract(
    protocol: InboundProtocol,
    body: &Value,
    compact: bool,
) -> Result<(), String> {
    if body
        .get("n")
        .filter(|value| !value.is_null())
        .is_some_and(|value| value.as_u64() != Some(1))
    {
        return Err("unsupported parameter `n`: Cursor text responses only support n=1".into());
    }
    if body.get("logprobs").and_then(Value::as_bool) == Some(true)
        || body.get("top_logprobs").is_some()
    {
        return Err("unsupported parameter `logprobs`: Cursor text responses do not expose log probabilities".into());
    }
    if body
        .get("modalities")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() != Some("text")))
    {
        return Err("unsupported parameter `modalities`: only text output is supported".into());
    }
    if body.get("audio").is_some() {
        return Err("unsupported parameter `audio`: audio output is not supported".into());
    }
    if protocol == InboundProtocol::OpenAiChat
        && (body.get("functions").is_some() || body.get("function_call").is_some())
    {
        return Err("unsupported parameter `functions`: use tools/tool_choice instead".into());
    }
    if protocol == InboundProtocol::OpenAiResponses
        && body.get("background").and_then(Value::as_bool) == Some(true)
    {
        return Err(
            "unsupported parameter `background`: Cursor responses run synchronously".into(),
        );
    }
    if protocol == InboundProtocol::OpenAiResponses {
        const HOSTED_TOOL_TYPES: &[&str] = &[
            "web_search",
            "web_search_preview",
            "file_search",
            "computer_use_preview",
            "code_interpreter",
            "image_generation",
        ];
        if let Some(tool_type) = body
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("type").and_then(Value::as_str))
            .find(|tool_type| HOSTED_TOOL_TYPES.contains(tool_type))
        {
            return Err(format!(
                "unsupported parameter `tools`: hosted tool type `{tool_type}` cannot run on the Cursor text rail"
            ));
        }
        let inventory = openai_response_tool_inventory(body)?;
        validate_mcp_tool_schemas(&inventory.tools)?;
        if compact && !inventory.tools.is_empty() {
            return Err(
                "unsupported parameter `tools`: response compaction cannot execute tools".into(),
            );
        }
    }
    if protocol != InboundProtocol::OpenAiResponses {
        let tools = match protocol {
            InboundProtocol::AnthropicMessages => body
                .get("tools")
                .map(anthropic_tools_to_mcp_defs)
                .unwrap_or_default(),
            InboundProtocol::OpenAiChat => body
                .get("tools")
                .map(openai_tools_to_mcp_defs)
                .unwrap_or_default(),
            InboundProtocol::GeminiNative => gemini_tools_to_mcp_defs(body.get("tools")),
            InboundProtocol::OpenAiResponses => unreachable!(),
        };
        validate_mcp_tool_schemas(&tools)?;
    }
    if protocol == InboundProtocol::GeminiNative
        && body
            .pointer("/generationConfig/candidateCount")
            .or_else(|| body.pointer("/generation_config/candidate_count"))
            .filter(|value| !value.is_null())
            .is_some_and(|value| value.as_u64() != Some(1))
    {
        return Err(
            "unsupported parameter `candidateCount`: Cursor only returns one candidate".into(),
        );
    }
    if compact {
        if body.get("stream").and_then(Value::as_bool) == Some(true) {
            return Err(
                "unsupported parameter `stream`: response compaction is non-streaming".into(),
            );
        }
        if protocol != InboundProtocol::OpenAiResponses
            && body
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty())
        {
            return Err(
                "unsupported parameter `tools`: response compaction cannot execute tools".into(),
            );
        }
    }
    Ok(())
}

pub fn normalized_response_input_items(body: &Value) -> Vec<Value> {
    match body.get("input") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": text,
        })],
        Some(Value::Null) | None => Vec::new(),
        Some(value) => vec![value.clone()],
    }
}

pub fn prepend_response_context(body: &mut Value, previous: &[Value]) -> Result<(), String> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| "Responses request body must be an object".to_string())?;
    let current = match object.remove("input") {
        Some(Value::Array(items)) => items,
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": text,
        })],
        Some(Value::Null) | None => Vec::new(),
        Some(value) => vec![value],
    };
    let mut combined = Vec::with_capacity(previous.len().saturating_add(current.len()));
    let mut call_memory = std::collections::HashMap::<(String, String), Value>::new();
    for item in previous.iter().chain(current.iter()) {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("tool_call_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if matches!(
            kind,
            "function_call"
                | "function_call_output"
                | "custom_tool_call"
                | "custom_tool_call_output"
        ) {
            let call_id = call_id
                .ok_or_else(|| format!("Responses {kind} item requires a non-empty call_id"))?;
            let key = (kind.to_string(), call_id.to_string());
            let semantic_item = response_call_memory_semantic_item(item);
            if let Some(existing) = call_memory.get(&key) {
                if existing != &semantic_item {
                    return Err(format!(
                        "Responses continuation contains conflicting {kind} items for call_id `{call_id}`"
                    ));
                }
                continue;
            }
            call_memory.insert(key, semantic_item);
        }
        combined.push(item.clone());
    }
    if combined.len() > 400 {
        return Err("Responses expanded continuation exceeds 400 input items".to_string());
    }
    object.insert("input".to_string(), Value::Array(combined));
    object.remove("previous_response_id");
    Ok(())
}

/// Compare Responses call memory by the fields that describe the call or its
/// result. `id` identifies the surrounding Responses item, while `status` is
/// API return-state metadata; Codex may echo either field even though the
/// parked semantic snapshot intentionally stores neither. `call_id` remains
/// part of the map key, and every other field must still match exactly.
fn response_call_memory_semantic_item(item: &Value) -> Value {
    let mut semantic = item.clone();
    if let Some(object) = semantic.as_object_mut() {
        object.remove("id");
        object.remove("status");
    }
    semantic
}

pub fn prepare_response_compaction(body: &mut Value) -> Result<(), String> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| "response compaction body must be an object".to_string())?;
    let requested = object
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut instructions = [
        "You are compacting a long-running Responses API conversation.",
        "Return only a concise continuation summary preserving user goals, decisions, constraints, important file paths, pending tasks, tool results, and unresolved errors.",
        "Do not add new actions, execute tools, or answer the original request; summarize conversation state for a future model turn.",
    ]
    .join("\n");
    if let Some(requested) = requested {
        instructions.push_str("\n\nCOMPACTION INSTRUCTIONS:\n");
        instructions.push_str(requested);
    }
    object.insert("instructions".to_string(), Value::String(instructions));
    object.insert("stream".to_string(), Value::Bool(false));
    object.insert("tool_choice".to_string(), Value::String("none".to_string()));
    object.remove("tools");
    object.remove("background");
    Ok(())
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
                .unwrap_or("image/png");
            inline_base64_image_ref(media_type, source.get("data").and_then(Value::as_str)?)
        }
        "url" => {
            let url = source.get("url").and_then(Value::as_str)?;
            if crate::proxy::remote_image::is_data_image_uri(url) {
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
    if crate::proxy::remote_image::is_data_image_uri(url) {
        out.push(ImageRef::DataUri(url.to_string()));
    } else if crate::proxy::remote_image::is_http_image_url(url) {
        out.push(ImageRef::HttpUrl(url.to_string()));
    }
}

fn inline_base64_image_ref(mime: &str, data: &str) -> Option<ImageRef> {
    let data = data.trim();
    let max_encoded_bytes = (super::image::MAX_IMAGE_BYTES.saturating_add(2) / 3).saturating_mul(4);
    if data.len() > max_encoded_bytes {
        return None;
    }
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).ok()?;
    Some(ImageRef::Inline {
        mime: mime.to_string(),
        data: Bytes::from(decoded),
    })
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
                        "custom_tool_call" => {
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                            let call_id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let input = item.get("input").and_then(Value::as_str).unwrap_or("");
                            conversation_lines.push(format!(
                                "Assistant called custom tool {name} ({call_id}) with input: {input}"
                            ));
                        }
                        "function_call_output" | "custom_tool_call_output" => {
                            let call_id = item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let output = item
                                .get("output")
                                .map(response_tool_output_text)
                                .unwrap_or_default();
                            tool_results.push(ToolResultBlock {
                                tool_call_id: call_id.clone(),
                                content: output.clone(),
                                is_error: false,
                            });
                            conversation_lines.push(format!("Tool result ({call_id}): {output}"));
                        }
                        "compaction" => {
                            if let Some(summary) = item
                                .get("encrypted_content")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|summary| !summary.is_empty())
                            {
                                conversation_lines
                                    .push(format!("Prior conversation compaction: {summary}"));
                            }
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
            let call_id = function_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .unwrap_or(name);
            let args = function_call
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({}))
                .to_string();
            if !name.is_empty() {
                conversation_lines.push(format!(
                    "Assistant called tool {name} ({call_id}) with arguments: {args}"
                ));
            }
        }
        for function_response in gemini_function_responses(parts) {
            let name = function_response
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("gemini_function_response")
                .to_string();
            let tool_call_id = function_response
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| name.clone());
            let response = function_response
                .get("response")
                .map(Value::to_string)
                .unwrap_or_else(|| "{}".to_string());
            tool_results.push(ToolResultBlock {
                tool_call_id: tool_call_id.clone(),
                content: response.clone(),
                is_error: false,
            });
            conversation_lines.push(format!("Tool result ({tool_call_id}): {response}"));
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
        .unwrap_or("image/png");
    let raw = data.get("data").and_then(Value::as_str)?;
    inline_base64_image_ref(mime, raw)
}

fn gemini_file_image(part: &Value) -> Option<ImageRef> {
    let data = part.get("fileData").or_else(|| part.get("file_data"))?;
    let uri = data
        .get("fileUri")
        .or_else(|| data.get("file_uri"))
        .and_then(Value::as_str)?;
    if crate::proxy::remote_image::is_data_image_uri(uri) {
        Some(ImageRef::DataUri(uri.to_string()))
    } else if crate::proxy::remote_image::is_http_image_url(uri) {
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
            out.push(McpToolDef::new(
                name.clone(),
                declaration
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                schema,
                CLIENT_MCP_PROVIDER_IDENTIFIER.to_string(),
                name,
            ));
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ExtractedToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
}

pub fn extract_tool_choice(body: &Value, protocol: InboundProtocol) -> ExtractedToolChoice {
    if protocol == InboundProtocol::GeminiNative {
        let config = body
            .pointer("/toolConfig/functionCallingConfig")
            .or_else(|| body.pointer("/tool_config/function_calling_config"));
        if let Some(config) = config {
            let mode = config
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("AUTO")
                .to_ascii_uppercase();
            if mode == "NONE" {
                return ExtractedToolChoice::None;
            }
            if mode == "ANY" {
                let allowed = config
                    .get("allowedFunctionNames")
                    .or_else(|| config.get("allowed_function_names"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .collect::<Vec<_>>();
                return match allowed.as_slice() {
                    [name] => ExtractedToolChoice::Named((*name).to_string()),
                    _ => ExtractedToolChoice::Required,
                };
            }
        }
    }
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
                let name = raw
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .or_else(|| raw.get("name"))
                    .and_then(Value::as_str);
                match name {
                    Some(name) if protocol == InboundProtocol::OpenAiResponses => {
                        let namespace = raw
                            .get("function")
                            .and_then(|function| function.get("namespace"))
                            .or_else(|| raw.get("namespace"))
                            .and_then(Value::as_str);
                        ExtractedToolChoice::Named(qualify_response_tool_name(namespace, name))
                    }
                    Some(name) => ExtractedToolChoice::Named(name.to_string()),
                    None => ExtractedToolChoice::Auto,
                }
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
    let configured = std::env::var("CC_SWITCH_CURSOR_TOOL_DIRECTIVE")
        .or_else(|_| std::env::var("CURSOR_TOOL_DIRECTIVE"))
        .ok();
    tool_commit_enabled_from(configured.as_deref())
}

fn tool_commit_enabled_from(configured: Option<&str>) -> bool {
    configured.is_none_or(|value| {
        !(value == "0" || value.eq_ignore_ascii_case("false") || value.eq_ignore_ascii_case("off"))
    })
}

fn cursor_sdk_tool_routing_directive(tools: &[McpToolDef]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let routes = tools
        .iter()
        .map(|tool| {
            let provider = serde_json::to_string(&tool.provider_identifier)
                .unwrap_or_else(|_| "\"client\"".to_string());
            let tool_name = serde_json::to_string(&tool.tool_name)
                .unwrap_or_else(|_| "\"unknown\"".to_string());
            let custom_input = tool
                .input_schema
                .as_json()
                .get("properties")
                .and_then(|properties| properties.get("input"))
                .and_then(|input| input.get("type"))
                .and_then(Value::as_str)
                == Some("string")
                && tool
                    .input_schema
                    .as_json()
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.iter().any(|field| field == "input"));
            format!(
                "- SDK mcp: providerIdentifier={provider}, toolName={tool_name}, args {}.",
                if custom_input {
                    "must be a JSON object with the raw custom-tool source in string field `input`"
                } else {
                    "must match the declared client tool schema"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let preferred_local_route = tools
        .iter()
        .find(|tool| tool.tool_name.eq_ignore_ascii_case("exec"))
        .map(|tool| {
            let provider = serde_json::to_string(&tool.provider_identifier)
                .unwrap_or_else(|_| "\"client\"".to_string());
            let tool_name = serde_json::to_string(&tool.tool_name)
                .unwrap_or_else(|_| "\"exec\"".to_string());
            format!(
                "\nFor local project inspection or changes, prefer SDK mcp with providerIdentifier={provider} and toolName={tool_name}; this is the outer client's unified local execution tool."
            )
        })
        .unwrap_or_default();
    format!(
        "\nSDK CLIENT TOOL ROUTING MAP:\n\
         Emit SDK `mcp` calls using these exact routes; the Server forwards them to the outer client:\n\
         {routes}{preferred_local_route}\n\
         Do not substitute an undeclared SDK builtin, a different tool name, or prose for these routes."
    )
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
        prefix.push_str(&cursor_sdk_tool_routing_directive(tools));
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

fn request_requires_local_tool(
    protocol: InboundProtocol,
    body: &Value,
    tools: &[McpToolDef],
    active_results: &[ToolResultBlock],
) -> bool {
    if tools.is_empty() || !active_results.is_empty() {
        return false;
    }
    let user_turns = current_user_turns(protocol, body);
    let Some(latest) = user_turns.last().map(String::as_str) else {
        return false;
    };
    if explicitly_requests_declared_tool(latest, tools) || local_project_intent(latest) {
        return true;
    }
    if !elliptical_local_followup(latest) {
        return false;
    }
    // Terse agent follow-ups often omit the repository noun entirely. Walk
    // past earlier terse turns ("details" -> "continue") and inherit only
    // the nearest substantive user task, rather than matching any stale local
    // task in the full conversation.
    user_turns[..user_turns.len().saturating_sub(1)]
        .iter()
        .rev()
        .find(|turn| !elliptical_local_followup(turn))
        .is_some_and(|turn| {
            explicitly_requests_declared_tool(turn, tools) || local_project_intent(turn)
        })
}

pub fn local_task_turn_signal(
    protocol: InboundProtocol,
    body: &Value,
    tools: &[McpToolDef],
) -> Option<LocalTaskTurnSignal> {
    let user_turns = current_user_turns(protocol, body);
    let latest = user_turns.last()?.as_str();
    Some(
        if explicitly_requests_declared_tool(latest, tools) || local_project_intent(latest) {
            LocalTaskTurnSignal::Activate
        } else if elliptical_local_followup(latest) {
            LocalTaskTurnSignal::Continue
        } else {
            LocalTaskTurnSignal::Replace
        },
    )
}

fn elliptical_local_followup(text: &str) -> bool {
    let normalized = text
        .trim()
        .trim_end_matches(['.', '。', '!', '！', '?', '？'])
        .trim()
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "continue"
            | "go on"
            | "keep going"
            | "details"
            | "more details"
            | "inspect"
            | "analyze"
            | "analyse"
            | "review"
            | "read"
            | "write"
            | "继续"
            | "接着"
            | "继续分析"
            | "继续解读"
            | "深入"
            | "深入分析"
            | "深入解读"
            | "细节"
            | "更多细节"
            | "细节解读"
            | "分析"
            | "解读"
            | "审查"
            | "读取"
            | "写入"
    )
}

fn explicitly_requests_declared_tool(text: &str, tools: &[McpToolDef]) -> bool {
    let normalized_text = normalize_tool_request_text(text);
    tools.iter().any(|tool| {
        let normalized_name = normalize_tool_request_text(&tool.name);
        if normalized_name.is_empty() {
            return false;
        }
        normalized_text == normalized_name
            || [
                format!("use{normalized_name}"),
                format!("call{normalized_name}"),
                format!("run{normalized_name}"),
                format!("使用{normalized_name}"),
                format!("调用{normalized_name}"),
                format!("运行{normalized_name}"),
            ]
            .contains(&normalized_text)
    })
}

fn normalize_tool_request_text(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(['.', '。', '!', '！', '?', '？'])
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '`')
        .flat_map(char::to_lowercase)
        .collect()
}

fn current_user_turns(protocol: InboundProtocol, body: &Value) -> Vec<String> {
    match protocol {
        InboundProtocol::AnthropicMessages => body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|message| {
                stringify_anthropic_text_or_blocks(message.get("content").unwrap_or(&Value::Null))
            })
            .filter(|text| !text.trim().is_empty())
            .collect(),
        InboundProtocol::OpenAiChat => body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|message| message.get("content").and_then(openai_content_text))
            .filter(|text| !text.trim().is_empty())
            .collect(),
        InboundProtocol::OpenAiResponses => match body.get("input") {
            Some(Value::String(text)) if !text.trim().is_empty() => vec![text.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter(|item| {
                    item.get("type")
                        .and_then(Value::as_str)
                        .is_none_or(|kind| kind == "message")
                        && item.get("role").and_then(Value::as_str).unwrap_or("user") == "user"
                })
                .filter_map(|item| {
                    let (text, _) =
                        openai_responses_content_parts(item.get("content").unwrap_or(&Value::Null));
                    (!text.trim().is_empty()).then_some(text)
                })
                .collect(),
            _ => Vec::new(),
        },
        InboundProtocol::GeminiNative => body
            .get("contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|content| {
                content
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    == "user"
            })
            .filter_map(|content| {
                let (text, _) =
                    gemini_parts_text_images(content.get("parts").unwrap_or(&Value::Null));
                (!text.trim().is_empty()).then_some(text)
            })
            .collect(),
    }
}

fn local_project_intent(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let local_signal = [
        "current project",
        "current directory",
        "this project",
        "repository",
        "repo",
        "codebase",
        "recent commit",
        "git history",
        "当前项目",
        "当前目录",
        "这个项目",
        "本项目",
        "仓库",
        "代码库",
        "项目结构",
        "最近提交",
        "提交记录",
    ]
    .iter()
    .any(|signal| lower.contains(signal));
    let action_signal = [
        "inspect",
        "analyze",
        "analyse",
        "review",
        "read",
        "summarize",
        "explain",
        "implement",
        "modify",
        "test",
        "查看",
        "读取",
        "分析",
        "解读",
        "审查",
        "总结",
        "实现",
        "修改",
        "测试",
    ]
    .iter()
    .any(|signal| lower.contains(signal));
    local_signal && action_signal
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
    allowed_tools: &[McpToolDef],
    attempt: usize,
    max_attempts: usize,
) -> String {
    let routes = cursor_sdk_tool_routing_directive(allowed_tools);
    format!(
        "{user_text}\n\n\
         TOOL CALL RETRY (attempt {attempt} of {max_attempts}):\n\
         Your previous Cursor SDK response did not emit a local tool call, but the latest user request requires local execution.\n\
         The next response is invalid unless it contains exactly one SDK mcp tool call.\n\
         Do not answer in prose. Use providerIdentifier \"client\", an exact declared client toolName, and schema-valid args, then wait for the local tool result.\
         {routes}"
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
         Allowed client tool targets: {allowed}. \
         You MUST call the required target through SDK mcp with providerIdentifier \
         \"client\", its exact client toolName, and schema-valid args. Do not answer in prose."
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
        assert_eq!(
            plan.continuation_kind,
            ToolContinuationKind::PureToolResults
        );
        assert!(plan.cold_resume_ready);
    }

    #[test]
    fn anthropic_historical_tool_result_is_not_an_active_continuation() {
        let body = json!({
            "model": "default",
            "messages": [
                {"role":"assistant","content":[{"type":"tool_use","id":"old","name":"read","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"old","content":"done"}]},
                {"role":"assistant","content":"finished"},
                {"role":"user","content":"继续"}
            ]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert!(plan.tool_results.is_empty());
        assert_eq!(plan.historical_tool_results.len(), 1);
        assert_eq!(plan.continuation_kind, ToolContinuationKind::None);
    }

    #[test]
    fn anthropic_mixed_tool_result_requires_cold_resume() {
        let body = json!({
            "messages": [
                {"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"read","input":{}}]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"call-1","content":"ok"},
                    {"type":"text","text":"also summarize it"}
                ]}
            ]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert_eq!(
            plan.continuation_kind,
            ToolContinuationKind::MixedToolResults
        );
        assert!(plan.cold_resume_ready);
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
    fn image_url_classification_accepts_case_insensitive_schemes() {
        let mut images = Vec::new();
        push_image_ref("HTTPS://example.com/x.png", &mut images);
        push_image_ref("DATA:image/png;base64,iVBORw0KGgo=", &mut images);
        assert!(matches!(images.first(), Some(ImageRef::HttpUrl(_))));
        assert!(matches!(images.get(1), Some(ImageRef::DataUri(_))));
    }

    #[test]
    fn inline_base64_image_rejects_oversized_payload_before_decoding() {
        let max_encoded_bytes =
            (crate::proxy::cursor::image::MAX_IMAGE_BYTES.saturating_add(2) / 3).saturating_mul(4);
        let payload = "A".repeat(max_encoded_bytes + 1);
        assert!(inline_base64_image_ref("image/png", &payload).is_none());
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
    fn responses_historical_function_output_before_new_user_is_not_active() {
        let body = json!({
            "input": [
                {"type":"function_call","name":"read","call_id":"old","arguments":"{}"},
                {"type":"function_call_output","call_id":"old","output":"done"},
                {"type":"message","role":"assistant","content":"finished"},
                {"type":"message","role":"user","content":"continue"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.tool_results.is_empty());
        assert_eq!(plan.historical_tool_results.len(), 1);
    }

    #[test]
    fn responses_prepended_cache_cannot_turn_history_into_current_continuation() {
        let original_body = json!({
            "previous_response_id":"resp_previous",
            "input": []
        });
        let original = build_plan(InboundProtocol::OpenAiResponses, &original_body);
        let expanded_body = json!({
            "input":[{"type":"function_call_output","call_id":"old","output":"done"}]
        });
        let mut expanded = build_plan(InboundProtocol::OpenAiResponses, &expanded_body);
        assert_eq!(expanded.tool_results.len(), 1);
        preserve_current_continuation(&mut expanded, &original);
        assert!(expanded.tool_results.is_empty());
        assert_eq!(expanded.continuation_kind, ToolContinuationKind::None);
    }

    #[test]
    fn responses_prepended_cache_allows_continue_to_inherit_local_intent() {
        let original_body = json!({
            "previous_response_id":"resp_previous",
            "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "input":[{"type":"message","role":"user","content":"continue"}]
        });
        let original = build_plan(InboundProtocol::OpenAiResponses, &original_body);
        assert!(!original.local_tool_required_by_intent);
        let expanded_body = json!({
            "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "input":[
                {"type":"message","role":"user","content":"Summarize recent commits"},
                {"type":"message","role":"assistant","content":"I will inspect them."},
                {"type":"message","role":"user","content":"continue"}
            ]
        });
        let mut expanded = build_plan(InboundProtocol::OpenAiResponses, &expanded_body);
        preserve_current_continuation(&mut expanded, &original);
        assert!(expanded.local_tool_required_by_intent);
    }

    #[test]
    fn chat_trailing_tool_messages_form_the_current_continuation() {
        let body = json!({
            "messages": [
                {"role":"assistant","tool_calls":[{"id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"README.md\"}"}}]},
                {"role":"tool","tool_call_id":"call-1","content":"contents"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiChat, &body);
        assert_eq!(plan.tool_results.len(), 1);
        assert!(plan.cold_resume_ready);
        assert_eq!(plan.completed_tool_calls[0].name, "read");
    }

    #[test]
    fn chat_only_uses_trailing_tool_message_suffix() {
        let body = json!({
            "messages": [
                {"role":"assistant","tool_calls":[{"id":"old","type":"function","function":{"name":"read","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"old","content":"done"},
                {"role":"assistant","content":"finished"},
                {"role":"user","content":"continue"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiChat, &body);
        assert!(plan.tool_results.is_empty());
        assert_eq!(plan.historical_tool_results.len(), 1);
    }

    #[test]
    fn local_project_request_requires_a_tool_even_with_auto_choice() {
        let body = json!({
            "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "input":[{"type":"message","role":"user","content":"深入解读当前项目"}]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.local_tool_required_by_intent);
    }

    #[test]
    fn responses_terse_followups_inherit_the_nearest_substantive_local_task() {
        let body = json!({
            "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "input":[
                {"type":"message","role":"user","content":"深入解读当前项目"},
                {"type":"message","role":"assistant","content":"我会先查看代码。"},
                {"type":"message","role":"user","content":"细节解读"},
                {"type":"message","role":"assistant","content":"继续梳理。"},
                {"type":"message","role":"user","content":"继续"},
                {"type":"message","role":"assistant","content":"继续读取。"},
                {"type":"message","role":"user","content":"Write"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.local_tool_required_by_intent);
    }

    #[test]
    fn responses_turn_signal_distinguishes_activation_continuation_and_replacement() {
        let tool = json!({
            "type":"custom",
            "name":"exec",
            "format":{"type":"grammar","syntax":"lark","definition":"start: SOURCE\nSOURCE: /[\\s\\S]+/"}
        });
        for (content, expected) in [
            ("深入解读当前项目", LocalTaskTurnSignal::Activate),
            ("继续", LocalTaskTurnSignal::Continue),
            (
                "Explain TCP congestion control",
                LocalTaskTurnSignal::Replace,
            ),
        ] {
            let body = json!({
                "input":[
                    {"type":"additional_tools","role":"developer","tools":[tool.clone()]},
                    {"type":"message","role":"user","content":content}
                ]
            });
            let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
            assert_eq!(
                local_task_turn_signal(InboundProtocol::OpenAiResponses, &body, &plan.tools),
                Some(expected),
                "content={content}"
            );
        }
    }

    #[test]
    fn fresh_terse_or_general_explanations_do_not_force_local_tools() {
        for content in ["细节解读", "Explain TCP congestion control"] {
            let body = json!({
                "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
                "input":[{"type":"message","role":"user","content":content}]
            });
            let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
            assert!(!plan.local_tool_required_by_intent, "content={content}");
        }

        let superseded_local_task = json!({
            "tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "input":[
                {"type":"message","role":"user","content":"深入解读当前项目"},
                {"type":"message","role":"assistant","content":"已完成。"},
                {"type":"message","role":"user","content":"Explain TCP congestion control"},
                {"type":"message","role":"assistant","content":"TCP uses congestion windows."},
                {"type":"message","role":"user","content":"continue"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &superseded_local_task);
        assert!(!plan.local_tool_required_by_intent);
    }

    #[test]
    fn an_explicit_declared_tool_request_requires_a_tool() {
        let body = json!({
            "input":[
                {"type":"additional_tools","role":"developer","tools":[{
                    "type":"custom",
                    "name":"exec",
                    "format":{"type":"grammar","syntax":"lark","definition":"start: SOURCE\nSOURCE: /[\\s\\S]+/"}
                }]},
                {"type":"message","role":"user","content":"Use `exec`."}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(plan.local_tool_required_by_intent);
    }

    #[test]
    fn codex_additional_tools_builds_custom_function_and_namespace_inventory() {
        let body = json!({
            "model":"gpt-5.6-sol",
            "tool_choice":{
                "type":"function",
                "name":"list_agents",
                "namespace":"collaboration"
            },
            "input":[
                {
                    "type":"additional_tools",
                    "role":"developer",
                    "tools":[
                        {
                            "type":"custom",
                            "name":"exec",
                            "description":"Run unified local tools",
                            "format":{
                                "type":"grammar",
                                "syntax":"lark",
                                "definition":"start: SOURCE\nSOURCE: /[\\s\\S]+/"
                            }
                        },
                        {
                            "type":"function",
                            "name":"wait",
                            "parameters":{
                                "type":"object",
                                "properties":{"cell_id":{"type":"string"}},
                                "required":["cell_id"]
                            }
                        },
                        {
                            "type":"namespace",
                            "name":"collaboration",
                            "tools":[{
                                "type":"function",
                                "name":"list_agents",
                                "parameters":{"type":"object","properties":{}}
                            }]
                        }
                    ]
                },
                {"type":"message","role":"user","content":"深入解读当前项目"}
            ]
        });
        validate_request_contract(InboundProtocol::OpenAiResponses, &body, false).unwrap();
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(
            plan.tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["exec", "wait", "collaboration.list_agents"]
        );
        assert_eq!(plan.custom_tool_names, vec!["exec"]);
        assert!(plan
            .tools
            .iter()
            .all(|tool| tool.provider_identifier == CLIENT_MCP_PROVIDER_IDENTIFIER));
        assert!(plan.user_text.contains("SDK CLIENT TOOL ROUTING MAP"));
        assert!(plan.user_text.contains("providerIdentifier=\"client\""));
        assert!(plan.user_text.contains("toolName=\"exec\""));
        assert!(plan.user_text.contains("raw custom-tool source"));
        assert!(plan.user_text.contains("prefer SDK mcp"));
        assert!(plan.user_text.contains("unified local execution tool"));
        assert!(plan
            .user_text
            .contains("LOCAL TOOL REQUIRED FOR THE LATEST USER REQUEST"));
        assert!(plan
            .user_text
            .contains("exactly one SDK mcp tool call before any prose"));
        assert_eq!(
            plan.tool_choice,
            ExtractedToolChoice::Named("collaboration.list_agents".to_string())
        );
        assert_eq!(
            plan.response_tool_namespaces,
            vec![ResponseToolNamespace {
                internal_name: "collaboration.list_agents".to_string(),
                namespace: "collaboration".to_string(),
                name: "list_agents".to_string(),
            }]
        );
        let exec_schema = plan.tools[0].input_schema.as_json();
        assert_eq!(exec_schema["required"], json!(["input"]));
        assert!(plan.tools[0].description.contains("Lark grammar"));
        assert!(plan.local_tool_required_by_intent);
    }

    #[test]
    fn codex_custom_tool_output_forms_a_cold_resumable_continuation() {
        let body = json!({
            "input":[
                {
                    "type":"additional_tools",
                    "role":"developer",
                    "tools":[{
                        "type":"custom",
                        "name":"exec",
                        "format":{
                            "type":"grammar",
                            "syntax":"lark",
                            "definition":"start: SOURCE\nSOURCE: /[\\s\\S]+/"
                        }
                    }]
                },
                {"type":"message","role":"user","content":"inspect this project"},
                {
                    "type":"custom_tool_call",
                    "name":"exec",
                    "call_id":"call_exec_1",
                    "input":"const r=await tools.exec_command({cmd:\"pwd\"}); text(r.output);"
                },
                {
                    "type":"custom_tool_call_output",
                    "call_id":"call_exec_1",
                    "output":[{"type":"input_text","text":"/workspace\n"}]
                }
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(
            plan.continuation_kind,
            ToolContinuationKind::PureToolResults
        );
        assert_eq!(plan.tool_results.len(), 1);
        assert_eq!(plan.tool_results[0].content, "/workspace\n");
        assert!(plan.cold_resume_ready);
        assert_eq!(plan.completed_tool_calls.len(), 1);
        assert_eq!(plan.completed_tool_calls[0].name, "exec");
        assert!(plan.completed_tool_calls[0].arguments["input"]
            .as_str()
            .unwrap()
            .contains("tools.exec_command"));
    }

    #[test]
    fn codex_0144_neutral_fixture_matches_inventory_and_continuation_contract() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/cursor/responses/codex_0144_additional_tools.json"
        )))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();

        let initial = &cases[0];
        let initial_plan = build_plan(InboundProtocol::OpenAiResponses, &initial["request"]);
        assert_eq!(
            initial_plan
                .tools
                .iter()
                .map(|tool| Value::String(tool.name.clone()))
                .collect::<Vec<_>>(),
            initial["expectedToolNames"].as_array().unwrap().clone()
        );
        assert_eq!(
            initial_plan
                .custom_tool_names
                .iter()
                .cloned()
                .map(Value::String)
                .collect::<Vec<_>>(),
            initial["expectedCustomToolNames"]
                .as_array()
                .unwrap()
                .clone()
        );

        let continuation = &cases[1];
        let continuation_plan =
            build_plan(InboundProtocol::OpenAiResponses, &continuation["request"]);
        assert_eq!(
            continuation_plan.inbound_protocol,
            InboundProtocol::OpenAiResponses
        );
        assert_eq!(
            continuation_plan.continuation_kind,
            ToolContinuationKind::PureToolResults
        );
        assert!(continuation_plan.cold_resume_ready);
        assert_eq!(
            continuation_plan.tool_results[0].content,
            continuation["expectedOutputText"].as_str().unwrap()
        );
    }

    #[test]
    fn responses_additional_tools_rejects_unsupported_custom_formats_and_conflicts() {
        let unsupported = json!({
            "input":[{
                "type":"additional_tools",
                "tools":[{
                    "type":"custom",
                    "name":"exec",
                    "format":{"type":"text"}
                }]
            }]
        });
        assert!(
            validate_request_contract(InboundProtocol::OpenAiResponses, &unsupported, false)
                .is_err()
        );

        let conflict = json!({
            "tools":[{
                "type":"function",
                "name":"wait",
                "parameters":{"type":"object","properties":{}}
            }],
            "input":[{
                "type":"additional_tools",
                "tools":[{
                    "type":"function",
                    "name":"wait",
                    "parameters":{"type":"object","required":["id"]}
                }]
            }]
        });
        assert!(
            validate_request_contract(InboundProtocol::OpenAiResponses, &conflict, false)
                .unwrap_err()
                .contains("conflicts")
        );
    }

    #[test]
    fn request_contract_rejects_invalid_tool_schemas_before_agentservice() {
        let invalid_branch = json!({
            "tools":[{
                "type":"function",
                "function":{
                    "name":"lookup",
                    "parameters":{
                        "type":"object",
                        "oneOf":[
                            {"properties":{"q":{"type":"string"}}},
                            {"properties":{"path":{"type":"string","pattern":"["}}}
                        ]
                    }
                }
            }]
        });
        let error = validate_request_contract(InboundProtocol::OpenAiChat, &invalid_branch, false)
            .unwrap_err();
        assert!(error.contains("invalid_tool_schema"));
        assert!(error.contains("lookup"));

        let invalid_root = json!({
            "tools":[{"name":"lookup","input_schema":true}]
        });
        let error =
            validate_request_contract(InboundProtocol::AnthropicMessages, &invalid_root, false)
                .unwrap_err();
        assert!(error.contains("root must be a JSON object"));
    }

    #[test]
    fn claude_continue_inherits_the_previous_local_project_intent() {
        let body = json!({
            "tools":[{"name":"Read","input_schema":{"type":"object"}}],
            "messages":[
                {"role":"user","content":"深入解读当前项目"},
                {"role":"assistant","content":"我先查看结构。"},
                {"role":"user","content":"继续"}
            ]
        });
        let plan = build_plan(InboundProtocol::AnthropicMessages, &body);
        assert!(plan.local_tool_required_by_intent);
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
    fn validate_tool_result_context_rejects_conflicting_duplicate_call_id() {
        let body = json!({
            "input": [
                {"type":"function_call_output","call_id":"fc_1","output":"first"},
                {"type":"function_call_output","call_id":"fc_1","output":"different"}
            ]
        });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert!(validate_tool_result_context(&plan).is_err());
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
        assert!(plan
            .user_text
            .contains("MUST issue the actual SDK tool call"));
        assert!(plan.user_text.contains("providerIdentifier=\"client\""));
        assert!(plan.user_text.contains("toolName=\"Bash\""));
        assert!(plan.user_text.contains("run ls"));
    }

    #[test]
    fn missing_tool_retry_repeats_the_exact_cursor_sdk_route() {
        let tools = vec![McpToolDef::new(
            "exec".to_string(),
            "Run unified local tools".to_string(),
            json!({
                "type":"object",
                "properties":{"input":{"type":"string"}},
                "required":["input"]
            }),
            CLIENT_MCP_PROVIDER_IDENTIFIER.to_string(),
            "exec".to_string(),
        )];
        let retry = retry_prompt_after_missing_tool("inspect the repo", &tools, 2, 3);
        assert!(retry.contains("TOOL CALL RETRY (attempt 2 of 3)"));
        assert!(retry.contains("exactly one SDK mcp tool call"));
        assert!(retry.contains("providerIdentifier=\"client\""));
        assert!(retry.contains("toolName=\"exec\""));
        assert!(retry.contains("raw custom-tool source"));
        assert!(retry.contains("unified local execution tool"));
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
    fn gemini_native_tool_result_prefers_call_id_with_name_fallback() {
        let body = json!({
            "model": "gemini-2.5-pro",
            "contents": [
                {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "id": "call_lookup_1",
                            "name": "lookup",
                            "args": {"query": "status"}
                        }
                    }]
                },
                {
                    "role": "user",
                    "parts": [
                        {
                            "functionResponse": {
                                "id": "call_lookup_1",
                                "name": "lookup",
                                "response": {"value": "ready"}
                            }
                        },
                        {
                            "functionResponse": {
                                "name": "legacy_lookup",
                                "response": {"value": "legacy"}
                            }
                        }
                    ]
                }
            ]
        });

        let plan = build_plan(InboundProtocol::GeminiNative, &body);
        assert_eq!(plan.tool_results.len(), 2);
        assert_eq!(plan.tool_results[0].tool_call_id, "call_lookup_1");
        assert_eq!(plan.tool_results[1].tool_call_id, "legacy_lookup");
        assert!(plan
            .user_text
            .contains("Assistant called tool lookup (call_lookup_1)"));
        assert!(plan.user_text.contains("Tool result (call_lookup_1)"));
    }

    #[test]
    fn estimate_input_tokens_nonzero_for_text() {
        assert!(estimate_input_tokens("hello world this is a test") > 0);
        assert_eq!(estimate_input_tokens(""), 0);
        let plain = estimate_responses_input_tokens(&json!({"input":"hello"})).unwrap();
        let with_tool = estimate_responses_input_tokens(&json!({
            "input":"hello",
            "tools":[{
                "type":"function",
                "name":"lookup",
                "description":"lookup a value",
                "parameters":{"type":"object","properties":{"key":{"type":"string"}}}
            }]
        }))
        .unwrap();
        assert!(with_tool > plain);
    }

    #[test]
    fn tool_commit_can_be_disabled_via_env() {
        assert!(!tool_commit_enabled_from(Some("0")));
        assert!(!tool_commit_enabled_from(Some("false")));
        assert!(!tool_commit_enabled_from(Some("OFF")));
        assert!(tool_commit_enabled_from(Some("1")));
        assert!(tool_commit_enabled_from(None));
    }

    #[test]
    fn composer_model_forces_working_directory_default() {
        let body = json!({ "model": "composer-2.5", "input": "hi" });
        let plan = build_plan(InboundProtocol::OpenAiResponses, &body);
        assert_eq!(plan.working_directory, ".");
    }

    #[test]
    fn completed_response_context_deduplicates_and_rejects_conflicting_call_memory() {
        let previous = vec![json!({
            "type":"function_call",
            "call_id":"call_1",
            "name":"lookup",
            "arguments":"{}"
        })];
        let mut identical = json!({
            "input":[
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"ok"}
            ],
            "previous_response_id":"resp_1"
        });
        prepend_response_context(&mut identical, &previous).unwrap();
        assert_eq!(identical["input"].as_array().unwrap().len(), 2);
        assert!(identical.get("previous_response_id").is_none());

        let mut conflicting = json!({
            "input":[{"type":"function_call","call_id":"call_1","name":"other","arguments":"{}"}]
        });
        assert!(prepend_response_context(&mut conflicting, &previous).is_err());
    }

    #[test]
    fn completed_response_context_ignores_only_returned_call_item_metadata() {
        let previous = vec![json!({
            "type":"custom_tool_call",
            "call_id":"tool_1",
            "name":"exec",
            "input":"text(await tools.exec_command({cmd: \"pwd\"}));"
        })];
        let mut echoed = json!({
            "input":[
                {
                    "id":"ctc_server_item_1",
                    "type":"custom_tool_call",
                    "call_id":"tool_1",
                    "name":"exec",
                    "input":"text(await tools.exec_command({cmd: \"pwd\"}));",
                    "status":"completed"
                },
                {
                    "type":"custom_tool_call_output",
                    "call_id":"tool_1",
                    "output":"/workspace\n"
                }
            ]
        });
        prepend_response_context(&mut echoed, &previous).unwrap();
        assert_eq!(echoed["input"].as_array().unwrap().len(), 2);

        for changed in [
            json!({
                "id":"ctc_other",
                "type":"custom_tool_call",
                "call_id":"tool_1",
                "name":"other",
                "input":"text(await tools.exec_command({cmd: \"pwd\"}));",
                "status":"completed"
            }),
            json!({
                "id":"ctc_other",
                "type":"custom_tool_call",
                "call_id":"tool_1",
                "name":"exec",
                "input":"text(await tools.exec_command({cmd: \"ls\"}));",
                "status":"completed"
            }),
            json!({
                "id":"ctc_other",
                "type":"custom_tool_call",
                "call_id":"tool_1",
                "name":"exec",
                "input":"text(await tools.exec_command({cmd: \"pwd\"}));",
                "status":"completed",
                "unexpected_semantic_field":true
            }),
        ] {
            let mut body = json!({"input":[changed]});
            assert!(prepend_response_context(&mut body, &previous).is_err());
        }
    }

    #[test]
    fn compaction_plan_is_non_streaming_and_side_effect_free() {
        let mut body = json!({
            "model":"gpt-5",
            "instructions":"Keep deployment constraints.",
            "input":[{"type":"message","role":"user","content":"context"}],
            "tools":[{"type":"function","name":"shell"}],
            "background":true
        });
        prepare_response_compaction(&mut body).unwrap();
        assert_eq!(body["stream"], false);
        assert_eq!(body["tool_choice"], "none");
        assert!(body.get("tools").is_none());
        assert!(body.get("background").is_none());
        assert!(body["instructions"]
            .as_str()
            .unwrap()
            .contains("Keep deployment constraints"));

        let continuation = build_plan(
            InboundProtocol::OpenAiResponses,
            &json!({
                "input":[
                    {"type":"compaction","encrypted_content":"Goal: finish review"},
                    {"type":"message","role":"user","content":"continue"}
                ]
            }),
        );
        assert!(continuation
            .user_text
            .contains("Prior conversation compaction: Goal: finish review"));
    }

    #[test]
    fn request_contract_rejects_silently_unimplementable_parameters() {
        assert!(
            validate_request_contract(InboundProtocol::OpenAiChat, &json!({"n":2}), false)
                .unwrap_err()
                .contains("`n`")
        );
        assert!(
            validate_request_contract(InboundProtocol::OpenAiChat, &json!({"n":-1}), false)
                .unwrap_err()
                .contains("`n`")
        );
        assert!(validate_request_contract(
            InboundProtocol::OpenAiResponses,
            &json!({"tools":[{"type":"web_search_preview"}]}),
            false
        )
        .unwrap_err()
        .contains("hosted tool"));
        assert!(validate_request_contract(
            InboundProtocol::GeminiNative,
            &json!({"generationConfig":{"candidateCount":2}}),
            false
        )
        .unwrap_err()
        .contains("candidateCount"));
        assert!(validate_request_contract(
            InboundProtocol::GeminiNative,
            &json!({"generationConfig":{"candidateCount":"many"}}),
            false
        )
        .unwrap_err()
        .contains("candidateCount"));
        assert_eq!(
            extract_tool_choice(
                &json!({
                    "toolConfig":{"functionCallingConfig":{
                        "mode":"ANY",
                        "allowedFunctionNames":["lookup"]
                    }}
                }),
                InboundProtocol::GeminiNative,
            ),
            ExtractedToolChoice::Named("lookup".to_string())
        );
        let required_without_tools = build_plan(
            InboundProtocol::OpenAiResponses,
            &json!({"input":"go","tool_choice":"required"}),
        );
        assert!(validate_tool_choice_contract(&required_without_tools).is_err());

        let continuation = build_plan(
            InboundProtocol::OpenAiResponses,
            &json!({
                "input":[{"type":"function_call_output","call_id":"call_1","output":"ok"}],
                "tool_choice":"required"
            }),
        );
        assert!(validate_tool_choice_contract(&continuation).is_ok());
    }
}
