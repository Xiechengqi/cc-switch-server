//! Claude Messages semantics that must be resolved before a Zen wire is built.
use super::{catalog::ZenAdmission, ZenWire};
use crate::proxy::{transforms::plain_anthropic_thinking, ProxyError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SEARCH: &str = "ToolSearch";
const PLACEHOLDER: &str = "DeferredToolPlaceholder";
const PLACEHOLDER_DESCRIPTION: &str =
    "Reserved placeholder that keeps deferred tool loading active; never call this tool.";

pub(super) fn retain_thinking(request: &Value) -> Result<bool, ProxyError> {
    let Some(context) = request.get("context_management") else {
        return Ok(false);
    };
    if context == &json!({"edits":[{"type":"clear_thinking_20251015","keep":"all"}]}) {
        return Ok(true);
    }
    Err(ProxyError::bad_request(
        "OpenCode Zen context_management supports only clear_thinking_20251015 with keep=all",
    ))
}

pub(super) fn prepare_bridge(
    request: &mut Value,
    admitted: &ZenAdmission,
) -> Result<(), ProxyError> {
    if retain_thinking(request)? {
        let wire = admitted.endpoint.wire();
        let keep_chat_thinking = replay_chat_thinking(admitted);
        for (role, block) in message_blocks(request) {
            match block["type"].as_str() {
                Some("thinking" | "redacted_thinking")
                    if wire == ZenWire::Messages && role == "assistant" => {}
                Some("thinking")
                    if keep_chat_thinking
                        && role == "assistant"
                        && plain_anthropic_thinking(block).is_some() => {}
                Some("thinking" | "redacted_thinking") => {
                    return Err(ProxyError::bad_request(
                        "OpenCode Zen cannot retain this thinking history on the selected model/protocol; start a new Claude Code session with CLAUDE_CODE_DISABLE_THINKING=1",
                    ));
                }
                _ => {}
            }
        }
        // A retain-all edit leaves every block intact on the admitted wire.
        request
            .as_object_mut()
            .expect("validated object")
            .remove("context_management");
    }
    resolve_tool_search(request)
}

pub(super) fn replay_chat_thinking(admitted: &ZenAdmission) -> bool {
    admitted.endpoint.wire() == ZenWire::Chat
        && !admitted.stale
        && admitted.capabilities.reasoning == Some(true)
        && admitted.capabilities.chat_reasoning_content
}

fn message_blocks(request: &Value) -> impl Iterator<Item = (&str, &Value)> {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|message| {
            let role = message["role"].as_str().unwrap_or_default();
            message["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |block| (role, block))
        })
}

fn search_error(message: &'static str) -> ProxyError {
    ProxyError::bad_request(format!(
        "OpenCode Zen Claude ToolSearch {message}; set ENABLE_TOOL_SEARCH=false to use ordinary tools without deferred loading"
    ))
}

fn is_placeholder(tool: &Value) -> bool {
    tool["name"] == PLACEHOLDER
        && tool["description"] == PLACEHOLDER_DESCRIPTION
        && tool["defer_loading"] == true
        && tool["input_schema"] == json!({"type":"object","properties":{}})
        && tool.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "name" | "description" | "input_schema" | "defer_loading" | "cache_control"
                )
            })
        })
}

fn search_tool(tool: &Value) -> bool {
    tool["name"] == SEARCH
        && tool.get("type").is_none()
        && tool["defer_loading"] != true
        && tool
            .pointer("/input_schema/properties/query/type")
            .and_then(Value::as_str)
            == Some("string")
}

fn has_tool_reference(block: &Value) -> bool {
    block["type"] == "tool_reference"
        || (block["type"] == "tool_result"
            && block["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_reference")))
}

fn resolve_tool_search(request: &mut Value) -> Result<(), ProxyError> {
    let declared = request["tools"].as_array().map_or(&[][..], Vec::as_slice);
    for tool in declared {
        if tool
            .get("defer_loading")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(search_error("requires boolean defer_loading"));
        }
    }
    let active = declared.iter().any(|tool| tool["defer_loading"] == true)
        || message_blocks(request).any(|(_, block)| has_tool_reference(block));
    if !active {
        if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
            for tool in tools {
                if let Some(tool) = tool.as_object_mut() {
                    tool.remove("defer_loading");
                }
            }
        }
        return Ok(());
    }
    if !declared.iter().any(search_tool) {
        return Err(search_error("requires the client ToolSearch declaration"));
    }
    let mut definitions = BTreeMap::new();
    for tool in declared {
        let name = tool["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| search_error("requires named tool definitions"))?;
        if definitions.insert(name, tool).is_some() {
            return Err(search_error("has duplicate tool definitions"));
        }
        if name == PLACEHOLDER && !is_placeholder(tool) {
            return Err(search_error("has an unrecognized placeholder definition"));
        }
    }

    let mut calls = BTreeSet::new();
    let mut used_ids = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for (role, block) in message_blocks(request) {
        if role == "assistant" && block["type"] == "tool_use" {
            if let Some(id) = block["id"].as_str().filter(|id| !id.is_empty()) {
                if !used_ids.insert(id) {
                    return Err(search_error("has a duplicate call ID"));
                }
                if block["name"] == SEARCH {
                    calls.insert(id);
                }
            } else if block["name"] == SEARCH {
                return Err(search_error("has a missing call ID"));
            }
        }
        let matching_search = role == "user"
            && block["type"] == "tool_result"
            && block["tool_use_id"]
                .as_str()
                .is_some_and(|id| calls.remove(id));
        if !has_tool_reference(block) {
            continue;
        }
        if !matching_search
            || block
                .get("is_error")
                .is_some_and(|value| value.as_bool() != Some(false))
        {
            return Err(search_error(
                "references must belong to a successful preceding client ToolSearch call",
            ));
        }
        for part in block["content"].as_array().into_iter().flatten() {
            if part["type"] == "text" && part["text"].is_string() {
                continue;
            }
            let name = part["tool_name"]
                .as_str()
                .filter(|name| {
                    part["type"] == "tool_reference" && *name != SEARCH && *name != PLACEHOLDER
                })
                .ok_or_else(|| search_error("has an invalid tool reference"))?;
            if !definitions
                .get(name)
                .is_some_and(|tool| tool.get("type").is_none() && tool["input_schema"].is_object())
            {
                return Err(search_error("reference has no current client tool schema"));
            }
            selected.insert(name);
        }
    }

    if request["tool_choice"]["type"] == "tool"
        && request["tool_choice"]["name"].as_str().is_some_and(|name| {
            name == PLACEHOLDER
                || definitions
                    .get(name)
                    .is_some_and(|tool| tool["defer_loading"] == true && !selected.contains(name))
        })
    {
        return Err(search_error("cannot force an unavailable deferred tool"));
    }

    let tools: Vec<_> = declared
        .iter()
        .filter(|tool| {
            let name = tool["name"].as_str().expect("validated tool name");
            name != PLACEHOLDER && (tool["defer_loading"] != true || selected.contains(name))
        })
        .cloned()
        .map(|mut tool| {
            tool.as_object_mut()
                .expect("validated tool")
                .remove("defer_loading");
            tool
        })
        .collect();
    // Pair references with their preceding call first, then render their names
    // as text. The resolved declarations carry the authoritative schemas.
    if let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) {
        for block in messages
            .iter_mut()
            .filter_map(|message| message.get_mut("content").and_then(Value::as_array_mut))
            .flatten()
            .filter(|block| has_tool_reference(block))
        {
            let parts = block["content"].as_array().expect("validated tool result");
            let text = parts
                .iter()
                .map(|part| {
                    if part["type"] == "tool_reference" {
                        format!("Available tool: {}", part["tool_name"])
                    } else {
                        part["text"].as_str().expect("validated text").to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            block["content"] = Value::String(text);
        }
    }
    request["tools"] = Value::Array(tools);
    Ok(())
}
