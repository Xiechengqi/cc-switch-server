//! Nonstream callers still use the CLI's streaming upstream transport. The
//! enclosing ZenStream validates framing, terminals and clean EOF first.
use super::ZenWire;
use crate::proxy::{request_memory::retained_json_bytes, ProxyError};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) struct ZenAggregation {
    wire: ZenWire,
    document: Value,
    items: BTreeMap<u64, Value>,
    partial_inputs: BTreeMap<u64, String>,
}

fn invalid() -> ProxyError {
    ProxyError::bad_gateway("OpenCode Zen invalid stream aggregation")
}

fn append(target: &mut Value, fragment: &Value) -> Result<(), ProxyError> {
    if fragment.is_null() {
        return Ok(());
    }
    append_text(target, fragment.as_str().ok_or_else(invalid)?)
}

fn append_text(target: &mut Value, fragment: &str) -> Result<(), ProxyError> {
    if target.is_null() {
        *target = Value::String(String::new());
    }
    target.as_str().ok_or_else(invalid)?;
    if let Value::String(text) = target {
        text.push_str(fragment);
    }
    Ok(())
}

pub(super) fn collect_chat_tool_deltas(
    items: &mut BTreeMap<u64, Value>,
    calls: &Value,
) -> Result<(), ProxyError> {
    for call in calls.as_array().ok_or_else(invalid)? {
        let index = call["index"].as_u64().ok_or_else(invalid)?;
        if items.len() >= 128 && !items.contains_key(&index) {
            return Err(invalid());
        }
        let tool = items
            .entry(index)
            .or_insert_with(|| json!({"type":"function","function":{"name":"","arguments":""}}));
        if let Some(id) = call.get("id") {
            if !id.is_string() || tool.get("id").is_some_and(|old| old != id) {
                return Err(invalid());
            }
            tool["id"] = id.clone();
        }
        if call.get("type").is_some_and(|v| v != "function") {
            return Err(invalid());
        }
        if let Some(function) = call.get("function") {
            if !function.is_object() {
                return Err(invalid());
            }
            for key in ["name", "arguments"] {
                if let Some(fragment) = function.get(key) {
                    append(&mut tool["function"][key], fragment)?;
                }
            }
        }
    }
    Ok(())
}

impl ZenAggregation {
    pub(super) fn new(wire: ZenWire) -> Self {
        Self {
            wire,
            document: match wire {
                ZenWire::Chat => {
                    json!({"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null},"finish_reason":null}]})
                }
                _ => Value::Null,
            },
            items: BTreeMap::new(),
            partial_inputs: BTreeMap::new(),
        }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        retained_json_bytes(&self.document)
            .saturating_add(
                self.items
                    .values()
                    .map(|v| retained_json_bytes(v).saturating_add(64))
                    .sum::<usize>(),
            )
            .saturating_add(
                self.partial_inputs
                    .values()
                    .map(|v| v.capacity().saturating_add(64))
                    .sum::<usize>(),
            )
    }

    pub(super) fn working_bytes(&self) -> usize {
        self.partial_inputs
            .values()
            .map(|v| crate::proxy::request_memory::json_parse_working_bytes(v.as_bytes()))
            .sum()
    }

    pub(super) fn observe(&mut self, value: &Value, event: Option<&str>) -> Result<(), ProxyError> {
        match self.wire {
            ZenWire::Chat => self.chat(value),
            ZenWire::Messages => self.messages(value),
            ZenWire::Responses => {
                match value["type"].as_str().or(event) {
                    Some("response.output_item.done") => {
                        let index = value["output_index"].as_u64().ok_or_else(invalid)?;
                        if self.items.len() >= 256 && !self.items.contains_key(&index) {
                            return Err(invalid());
                        }
                        self.items.insert(index, value["item"].clone());
                    }
                    Some("response.completed" | "response.incomplete") => {
                        self.document = value.get("response").unwrap_or(value).clone();
                    }
                    None if matches!(
                        value["status"].as_str(),
                        Some("completed" | "incomplete")
                    ) =>
                    {
                        self.document = value.clone()
                    }
                    _ => {}
                }
                Ok(())
            }
        }
    }

    fn chat(&mut self, value: &Value) -> Result<(), ProxyError> {
        for key in ["id", "model"] {
            if let Some(field) = value.get(key) {
                if !field.is_string() || self.document.get(key).is_some_and(|old| old != field) {
                    return Err(invalid());
                }
                self.document[key] = field.clone();
            }
        }
        for key in ["created", "system_fingerprint", "service_tier"] {
            if let Some(field) = value.get(key) {
                self.document[key] = field.clone();
            }
        }
        if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
            self.document["usage"] = usage.clone();
        }
        let choices = value["choices"].as_array().ok_or_else(invalid)?;
        if choices.is_empty() {
            return Ok(());
        }
        if choices.len() != 1
            || choices[0]
                .get("index")
                .is_some_and(|v| v.as_u64() != Some(0))
        {
            return Err(invalid());
        }
        let choice = &choices[0];
        let delta = choice["delta"].as_object().ok_or_else(invalid)?;
        let message = &mut self.document["choices"][0]["message"];
        if delta.get("role").is_some_and(|v| v != "assistant") {
            return Err(invalid());
        }
        if let Some((_, text)) = crate::proxy::chat_reasoning::select_fields(delta) {
            append_text(&mut message["reasoning_content"], text)?;
        }
        for key in ["content", "refusal"] {
            if let Some(fragment) = delta.get(key) {
                append(&mut message[key], fragment)?;
            }
        }
        if let Some(calls) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            collect_chat_tool_deltas(&mut self.items, calls)?;
        }
        if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            self.document["choices"][0]["finish_reason"] = reason.clone();
        }
        Ok(())
    }

    fn messages(&mut self, value: &Value) -> Result<(), ProxyError> {
        match value["type"].as_str() {
            Some("message_start") => self.document = value["message"].clone(),
            Some("content_block_start") => {
                let index = value["index"].as_u64().ok_or_else(invalid)?;
                if self.items.len() >= 256 || self.items.contains_key(&index) {
                    return Err(invalid());
                }
                self.items.insert(index, value["content_block"].clone());
            }
            Some("content_block_delta") => {
                let index = value["index"].as_u64().ok_or_else(invalid)?;
                let block = self.items.get_mut(&index).ok_or_else(invalid)?;
                let delta = &value["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => append(&mut block["text"], &delta["text"])?,
                    Some("thinking_delta") => append(&mut block["thinking"], &delta["thinking"])?,
                    Some("signature_delta") => {
                        append(&mut block["signature"], &delta["signature"])?
                    }
                    Some("input_json_delta") if block["type"] == "tool_use" => {
                        self.partial_inputs
                            .entry(index)
                            .or_default()
                            .push_str(delta["partial_json"].as_str().ok_or_else(invalid)?);
                    }
                    _ => return Err(invalid()),
                }
            }
            Some("content_block_stop") => {
                let index = value["index"].as_u64().ok_or_else(invalid)?;
                if let Some(input) = self.partial_inputs.remove(&index) {
                    self.items.get_mut(&index).ok_or_else(invalid)?["input"] =
                        serde_json::from_str(&input).map_err(|_| invalid())?;
                }
            }
            Some("message_delta") => {
                for key in ["stop_reason", "stop_sequence"] {
                    if let Some(field) = value["delta"].get(key) {
                        self.document[key] = field.clone();
                    }
                }
                if let Some(usage) = value.get("usage") {
                    let target = self
                        .document
                        .get_mut("usage")
                        .and_then(Value::as_object_mut)
                        .ok_or_else(invalid)?;
                    target.extend(usage.as_object().ok_or_else(invalid)?.clone());
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<Value, ProxyError> {
        if !self.partial_inputs.is_empty() || !self.document.is_object() {
            return Err(invalid());
        }
        if !self.items.is_empty() {
            let items = Value::Array(self.items.into_values().collect());
            match self.wire {
                ZenWire::Chat => self.document["choices"][0]["message"]["tool_calls"] = items,
                ZenWire::Messages => self.document["content"] = items,
                ZenWire::Responses
                    if self.document["output"]
                        .as_array()
                        .is_some_and(|v| v.is_empty()) =>
                {
                    self.document["output"] = items
                }
                _ => {}
            }
        }
        Ok(self.document)
    }
}
