use super::{catalog::ZenAdmission, controls::parameter_error, ZenWire};
use crate::proxy::{
    transforms::{response_input_items, response_tools_from_request},
    ProxyError,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const EXEC_NAME: &str = "exec";
pub(super) const EXEC_NAMESPACE: &str = "functions";
// The pinned Codex 0.170.0 declaration admits any nonempty source. The CLI
// interprets the optional pragma and owns execution and permission checks.
const EXEC_GRAMMAR: &str = r#"
start: pragma_source | plain_source
pragma_source: PRAGMA_LINE NEWLINE SOURCE
plain_source: SOURCE

PRAGMA_LINE: /[ \t]*\/\/ @exec:[^\r\n]*/
NEWLINE: /\r?\n/
SOURCE: /[\s\S]+/
"#;

pub(super) fn prepare_bridge(
    request: &mut Value,
    admitted: &ZenAdmission,
) -> Result<bool, ProxyError> {
    scope_reasoning(request, admitted)?;
    let exec = normalize_exec_declaration(request, admitted)?;
    if exec {
        validate_exec_history(request)?;
    }
    Ok(exec)
}

fn reasoning_error(message: &str) -> ProxyError {
    parameter_error("reasoning.context", message)
}

fn is_user_turn(item: &Value) -> bool {
    item["role"] == "user" && (item["content"].is_string() || item["content"].is_array())
}

fn scope_reasoning(request: &mut Value, admitted: &ZenAdmission) -> Result<(), ProxyError> {
    let Some(value) = request.pointer("/reasoning/context") else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let scope = value
        .as_str()
        .filter(|scope| matches!(*scope, "auto" | "current_turn" | "all_turns"))
        .ok_or_else(|| reasoning_error("expects auto, current_turn or all_turns"))?;
    let current_turn = scope == "current_turn";
    let automatic = scope == "auto";
    let Some(input) = request.get_mut("input") else {
        return Ok(());
    };
    let items = match input {
        Value::Array(items) => items.as_mut_slice(),
        Value::Object(_) => std::slice::from_mut(input),
        _ => return Ok(()),
    };
    let boundary = items.iter().rposition(is_user_turn);
    let chat_replay = admitted.endpoint.wire() == ZenWire::Chat
        && !admitted.stale
        && admitted.capabilities.reasoning == Some(true)
        && admitted.capabilities.chat_reasoning_content;
    for (index, item) in items.iter().enumerate() {
        match item["type"].as_str() {
            Some("compaction" | "context_compaction" | "compaction_trigger" | "item_reference") => {
                return Err(reasoning_error(
                    "cannot retain opaque or referenced reasoning history on this model/protocol",
                ));
            }
            Some("reasoning") if current_turn && boundary.is_some_and(|last| index < last) => {}
            Some("reasoning") => {
                let has_text = plain_reasoning_item(item).ok_or_else(|| {
                    reasoning_error("cannot retain opaque or unsupported reasoning history on this model/protocol")
                })?;
                if has_text && (automatic || !chat_replay) {
                    return Err(reasoning_error("cannot preserve the requested reasoning scope on this model/protocol. Select the bound Zen model in Codex CLI"));
                }
            }
            _ if ["reasoning", "reasoning_content", "reasoning_text"]
                .iter()
                .any(|key| item.get(*key).is_some_and(|value| !value.is_null())) =>
            {
                return Err(reasoning_error(
                    "requires explicit Responses reasoning items for scoped history",
                ));
            }
            _ => {}
        }
    }
    if let (true, Some(last), Value::Array(items)) = (current_turn, boundary, input) {
        let mut index = 0;
        items.retain(|item| {
            let keep = index >= last || item["type"] != "reasoning";
            index += 1;
            keep
        });
    }
    Ok(())
}

fn plain_reasoning_item(item: &Value) -> Option<bool> {
    let mut has_text = false;
    for (key, value) in item.as_object()? {
        match key.as_str() {
            "id" | "type" | "status" if value.is_string() => {}
            "content" if value.is_null() => {}
            "encrypted_content" if value.is_null() || value.as_str() == Some("") => {}
            "summary" | "content" => {
                for part in value.as_array()? {
                    let map = part.as_object()?;
                    if map.len() != 2
                        || !matches!(
                            part["type"].as_str(),
                            Some("summary_text" | "reasoning_text" | "text")
                        )
                    {
                        return None;
                    }
                    has_text |= !part["text"].as_str()?.is_empty();
                }
            }
            _ => return None,
        }
    }
    Some(has_text)
}

pub(super) fn validate_chat_replay(request: &Value, chat: &Value) -> Result<(), ProxyError> {
    let mut messages = chat["messages"].as_array().into_iter().flatten();
    let mut source_turn = 0;
    let mut target_turn = 0;
    let mut rest = "";
    for item in response_input_items(request) {
        if is_user_turn(item) {
            source_turn += 1;
        }
        if item["type"] != "reasoning" {
            continue;
        }
        let text = crate::proxy::reasoning_bridge::reasoning_summary_text(item);
        if text.is_empty() {
            continue;
        }
        loop {
            if target_turn == source_turn {
                if let Some(offset) = rest.find(&text) {
                    rest = &rest[offset + text.len()..];
                    break;
                }
            }
            let Some(message) = messages.next().filter(|_| target_turn <= source_turn) else {
                return Err(reasoning_error(
                    "cannot attach all requested reasoning to the corresponding Chat turn",
                ));
            };
            if message["role"] == "user" {
                target_turn += 1;
            }
            rest = if message["role"] == "assistant" {
                message["reasoning_content"].as_str().unwrap_or("")
            } else {
                ""
            };
        }
    }
    Ok(())
}

fn exec_namespace(tool: &Value) -> bool {
    tool["type"] == "namespace" && tool["name"] == EXEC_NAMESPACE
}

fn namespace_execs<'a>(
    tools: impl IntoIterator<Item = &'a Value>,
) -> impl Iterator<Item = &'a Value> {
    tools
        .into_iter()
        .filter(|tool| exec_namespace(tool))
        .filter_map(|tool| {
            tool.get("tools")
                .or_else(|| tool.get("children"))?
                .as_array()
        })
        .flatten()
        .filter(|tool| tool["name"] == EXEC_NAME)
}

fn reviewed_exec(tool: &Value) -> bool {
    tool["type"] == "custom"
        && tool["format"] == json!({"type":"grammar","syntax":"lark","definition":EXEC_GRAMMAR})
        && tool.as_object().is_some_and(|map| {
            map.keys()
                .all(|key| matches!(key.as_str(), "name" | "type" | "description" | "format"))
        })
}

fn normalize_exec_declaration(
    request: &mut Value,
    admitted: &ZenAdmission,
) -> Result<bool, ProxyError> {
    let candidates = response_input_items(request)
        .iter()
        .filter(|item| item["type"] == "additional_tools" && item["role"] == "developer")
        .filter_map(|item| item["tools"].as_array())
        .flat_map(namespace_execs)
        .filter(|tool| reviewed_exec(tool))
        .count();
    if candidates == 0 {
        return Ok(false);
    }
    if admitted.endpoint.wire() != ZenWire::Chat {
        return Err(parameter_error(
            "tools.format",
            "has no reviewed Code Mode grammar mapping on this model/protocol",
        ));
    }
    let count = namespace_execs(response_tools_from_request(request)).count();
    if candidates != 1 || count != 1 {
        return Err(parameter_error(
            "input.additional_tools",
            "has an ambiguous or inactive Code Mode exec declaration",
        ));
    }
    let items = match &mut request["input"] {
        Value::Array(items) => items.as_mut_slice(),
        item => std::slice::from_mut(item),
    };
    for item in items {
        if item["type"] != "additional_tools" || item["role"] != "developer" {
            continue;
        }
        let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) else {
            continue;
        };
        for namespace in tools.iter_mut().filter(|tool| exec_namespace(tool)) {
            let key = if namespace.get("tools").is_some() {
                "tools"
            } else {
                "children"
            };
            if let Some(exec) = namespace[key].as_array_mut().and_then(|tools| {
                tools
                    .iter_mut()
                    .find(|tool| tool["name"] == EXEC_NAME && reviewed_exec(tool))
            }) {
                exec["format"] = json!({"type":"text"});
                return Ok(true);
            }
        }
    }
    Err(parameter_error(
        "input.additional_tools",
        "could not resolve the Code Mode declaration",
    ))
}

fn validate_exec_history(request: &Value) -> Result<(), ProxyError> {
    let items = response_input_items(request);
    let mut custom_calls = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        if item["type"] != "custom_tool_call" {
            continue;
        }
        let exec = item["name"] == EXEC_NAME && item["namespace"] == EXEC_NAMESPACE;
        let id = item["call_id"].as_str().filter(|id| !id.trim().is_empty());
        if exec && (id.is_none() || item["input"].as_str().is_none_or(str::is_empty)) {
            return Err(parameter_error(
                "input.custom_tool_call",
                "requires a call ID and nonempty Code Mode source",
            ));
        }
        if id.is_some_and(|id| custom_calls.insert(id, (index, exec)).is_some()) {
            return Err(parameter_error(
                "input.custom_tool_call",
                "requires a unique custom call ID",
            ));
        }
    }
    let mut results = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let id = item["call_id"].as_str();
        let call = id.and_then(|id| custom_calls.get(id));
        if item["type"] == "custom_tool_call_output" {
            if let (Some(id), Some((at, _))) = (id, call) {
                if *at < index && results.insert(id) && !item["output"].is_null() {
                    continue;
                }
            }
            return Err(parameter_error(
                "input.custom_tool_call_output",
                "requires one result following a matching custom tool call",
            ));
        }
        if call.is_some_and(|(at, exec)| *exec && *at != index) {
            return Err(parameter_error(
                "input.custom_tool_call_output",
                "conflicts with a Code Mode call ID",
            ));
        }
    }
    if custom_calls
        .iter()
        .any(|(id, (_, exec))| *exec && !results.contains(id))
    {
        return Err(parameter_error(
            "input.custom_tool_call_output",
            "is required for each Code Mode call",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArguments {
    input: String,
}

pub(super) fn valid_exec_arguments(arguments: &str) -> bool {
    arguments.trim_start().starts_with('{')
        && serde_json::from_str::<ExecArguments>(arguments).is_ok_and(|args| !args.input.is_empty())
}

pub(super) fn constrain_exec_schema(converted: &mut Value, name: &str) -> Result<(), ProxyError> {
    let function = converted
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .find(|tool| tool["function"]["name"] == name)
        .and_then(|tool| tool.get_mut("function"))
        .ok_or_else(|| {
            parameter_error(
                "input.additional_tools",
                "could not map Code Mode exec to a Chat tool",
            )
        })?;
    function["parameters"] = json!({
        "type":"object",
        "properties":{
            "input":{
                "type":"string",
                "minLength":1,
                "description":"The complete JavaScript source including any leading @exec pragma."
            }
        },
        "required":["input"],
        "additionalProperties":false
    });
    Ok(())
}

#[derive(Default)]
pub(super) struct ChatToolCalls {
    pub(super) exec_name: Option<String>,
    calls: BTreeMap<u64, Value>,
    bytes: usize,
}

impl ChatToolCalls {
    pub(super) fn retained_bytes(&self) -> usize {
        self.exec_name
            .as_ref()
            .map_or(0, String::capacity)
            .saturating_add(
                self.calls
                    .values()
                    .map(|call| {
                        crate::proxy::request_memory::retained_json_bytes(call).saturating_add(64)
                    })
                    .sum::<usize>(),
            )
    }

    pub(super) fn observe(
        &mut self,
        value: &mut Value,
        allowed: &BTreeSet<String>,
        event_limit: usize,
    ) -> Result<(), ProxyError> {
        let Some(choice) = value
            .pointer_mut("/choices/0")
            .and_then(Value::as_object_mut)
        else {
            return Ok(());
        };
        if let Some(calls) = choice
            .get_mut("delta")
            .and_then(Value::as_object_mut)
            .and_then(|delta| delta.remove("tool_calls"))
            .filter(|calls| !calls.is_null())
        {
            let added = calls
                .as_array()
                .ok_or_else(invalid_tool_output)?
                .iter()
                .map(|call| {
                    [
                        call.get("id"),
                        call.pointer("/function/name"),
                        call.pointer("/function/arguments"),
                    ]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::len)
                    .sum::<usize>()
                })
                .sum::<usize>();
            self.bytes = self.bytes.saturating_add(added);
            if self.bytes > event_limit {
                return Err(ProxyError::bad_gateway(
                    "OpenCode Zen Chat tool output exceeds the SSE event limit",
                ));
            }
            super::aggregate::collect_chat_tool_deltas(&mut self.calls, &calls)?;
        }
        let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) else {
            return Ok(());
        };
        match reason {
            "tool_calls" if !self.calls.is_empty() => {}
            "length" | "content_filter" => {
                self.calls.clear();
                return Ok(());
            }
            "tool_calls" => return Err(invalid_tool_output()),
            _ if self.calls.is_empty() => return Ok(()),
            _ => return Err(invalid_tool_output()),
        }
        let mut ids = BTreeSet::new();
        let mut calls = Vec::with_capacity(self.calls.len());
        for (index, mut call) in std::mem::take(&mut self.calls) {
            let id = call["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(invalid_tool_output)?;
            let name = call["function"]["name"]
                .as_str()
                .filter(|name| allowed.contains(*name))
                .ok_or_else(invalid_tool_output)?;
            if !ids.insert(id.to_string())
                || (self.exec_name.as_deref() == Some(name)
                    && !call["function"]["arguments"]
                        .as_str()
                        .is_some_and(valid_exec_arguments))
            {
                return Err(invalid_tool_output());
            }
            call["index"] = index.into();
            calls.push(call);
        }
        let delta = choice
            .entry("delta")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(invalid_tool_output)?;
        delta.insert("tool_calls".to_string(), Value::Array(calls));
        Ok(())
    }
}

fn invalid_tool_output() -> ProxyError {
    ProxyError::bad_gateway("OpenCode Zen returned malformed or unbound Chat tool output")
}
