//! Request-owned tool validation. Never repair arguments or invent a name.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{
    append_fragment, append_stable_field, qoder_tool_call_index, ProxyError,
    MAX_QODER_CHAT_AGGREGATE_BYTES, MAX_QODER_CHAT_TOOL_CALLS,
};
use crate::proxy::request_memory::{
    json_parse_working_bytes, RequestMemoryBudget, RequestMemoryComponent,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QoderOutputContract {
    pub(crate) allowed_names: BTreeSet<String>,
    pub(crate) require_tool: bool,
    pub(crate) recover_text_tools: bool,
}

impl QoderOutputContract {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.allowed_names.iter().fold(0usize, |bytes, name| {
            bytes.saturating_add(name.capacity()).saturating_add(64)
        })
    }
}

#[derive(Debug, Default)]
pub(super) struct QoderToolState {
    calls: BTreeMap<usize, ObservedCall>,
    contract: Option<QoderOutputContract>,
}

#[derive(Debug, Default)]
struct ObservedCall {
    id: String,
    call_type: String,
    name: String,
    arguments: String,
    empty_object_input: bool,
    opened: bool,
    emitted_arguments: usize,
}

impl QoderToolState {
    pub(super) fn contract(&self) -> Option<&QoderOutputContract> {
        self.contract.as_ref()
    }
    pub(super) fn set_contract(&mut self, contract: QoderOutputContract) {
        self.contract = Some(contract);
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.calls.values().fold(
            self.contract
                .as_ref()
                .map_or(0, QoderOutputContract::retained_bytes),
            |bytes, call| {
                bytes
                    .saturating_add(std::mem::size_of::<(usize, ObservedCall)>() + 64)
                    .saturating_add(call.id.capacity())
                    .saturating_add(call.call_type.capacity())
                    .saturating_add(call.name.capacity())
                    .saturating_add(call.arguments.capacity())
            },
        )
    }

    /// Reserve before String/BTreeMap growth, including transient reallocations.
    pub(super) fn projected_bytes(&self, value: &Value) -> usize {
        // Delayed identities can release many complete JSON call objects on a
        // tiny finish event. Include their collection/key overhead separately
        // from the accumulated String storage, before building that output.
        let retained_projection = self
            .retained_bytes()
            .saturating_mul(3)
            .saturating_add(self.calls.len().saturating_mul(2048));
        let Some(calls) = value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
        else {
            // A finish-only event may release a complete, delayed identity.
            return retained_projection;
        };
        calls.iter().fold(retained_projection, |bytes, call| {
            bytes
                .saturating_add(super::retained_json_bytes(call).saturating_mul(2))
                .saturating_add(std::mem::size_of::<(usize, ObservedCall)>() + 2048)
        })
    }

    pub(super) fn observe(&mut self, value: &mut Value) -> Result<(), ProxyError> {
        let Some(choice) = value.pointer_mut("/choices/0") else {
            return Ok(());
        };
        let finishing = choice
            .get("finish_reason")
            .is_some_and(|reason| !reason.is_null());
        let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) else {
            return Ok(());
        };
        let mut incoming = delta.remove("tool_calls").unwrap_or_else(|| json!([]));
        let calls = incoming
            .as_array_mut()
            .ok_or_else(|| ProxyError::bad_gateway("Qoder tool calls must be an array"))?;
        for call in calls.iter_mut() {
            let index = qoder_tool_call_index(call.get("index"), 0)?;
            if let Some(id) = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                if self
                    .calls
                    .iter()
                    .any(|(other, state)| *other != index && state.id == id)
                {
                    return Err(ProxyError::bad_gateway(
                        "Qoder tool call IDs are not unique",
                    ));
                }
            }
            let state = self.calls.entry(index).or_default();
            state.empty_object_input |= call
                .as_object_mut()
                .and_then(|call| call.remove("_qoder_empty_object_input"))
                == Some(Value::Bool(true));
            append_stable_field(&mut state.id, call.get("id"), "tool call id")?;
            append_stable_field(&mut state.call_type, call.get("type"), "tool call type")?;
            if !state.call_type.is_empty() && state.call_type != "function" {
                return Err(ProxyError::bad_gateway(
                    "Qoder returned an unsupported tool call type",
                ));
            }
            if let Some(function) = call.get("function") {
                let name = function.get("name");
                // A completed identity may be repeated, but may never be renamed.
                if self.contract.is_some() && state.opened {
                    if name
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.is_empty() && name != state.name)
                    {
                        return Err(ProxyError::bad_gateway("Qoder changed an opened tool name"));
                    }
                } else if name.and_then(Value::as_str) != Some(state.name.as_str())
                    || self
                        .contract
                        .as_ref()
                        .is_some_and(|contract| !contract.allowed_names.contains(&state.name))
                {
                    append_fragment(&mut state.name, name, "function name")?;
                }
                append_fragment(
                    &mut state.arguments,
                    function.get("arguments"),
                    "function arguments",
                )?;
            }
        }
        let mut empty_arguments = Vec::new();
        if finishing {
            for (index, state) in &mut self.calls {
                if state.arguments.is_empty() && state.empty_object_input {
                    // An explicit native input:{} is a valid zero-argument
                    // call only if no subsequent JSON fragments replaced it.
                    state.arguments.push_str("{}");
                    empty_arguments.push(json!({"index":index,"function":{"arguments":"{}"}}));
                }
            }
        }
        if self.calls.len() > MAX_QODER_CHAT_TOOL_CALLS
            || self.retained_bytes() > MAX_QODER_CHAT_AGGREGATE_BYTES
        {
            return Err(ProxyError::bad_gateway(
                "Qoder tool state exceeds the limit",
            ));
        }
        let Some(contract) = &self.contract else {
            calls.extend(empty_arguments);
            if !calls.is_empty() {
                delta.insert("tool_calls".to_string(), incoming);
            }
            return Ok(());
        };
        let mut output = Vec::new();
        for (index, state) in &mut self.calls {
            let exact = contract.allowed_names.contains(&state.name);
            let longer = contract
                .allowed_names
                .iter()
                .any(|name| name != &state.name && name.starts_with(&state.name));
            if !exact && !longer {
                return Err(ProxyError::bad_gateway(
                    "Qoder returned an undeclared tool name",
                ));
            }
            if !state.opened {
                if state.id.trim().is_empty() || !exact || (longer && !finishing) {
                    continue;
                }
                state.opened = true;
                output.push(json!({"index":index,"id":state.id,"type":"function","function":{"name":state.name,"arguments":state.arguments}}));
                state.emitted_arguments = state.arguments.len();
            } else if state.emitted_arguments < state.arguments.len() {
                output.push(json!({"index":index,"type":"function","function":{"name":"","arguments":&state.arguments[state.emitted_arguments..]}}));
                state.emitted_arguments = state.arguments.len();
            }
        }
        if !output.is_empty() {
            delta.insert("tool_calls".to_string(), Value::Array(output));
        }
        Ok(())
    }

    pub(super) fn validate(&self, budget: Option<&RequestMemoryBudget>) -> Result<(), ProxyError> {
        if self
            .contract
            .as_ref()
            .is_some_and(|contract| contract.require_tool)
            && self.calls.is_empty()
        {
            return Err(ProxyError::bad_gateway(
                "Qoder did not satisfy the required tool choice",
            ));
        }
        for state in self.calls.values() {
            if state.id.trim().is_empty() || state.name.trim().is_empty() {
                return Err(ProxyError::bad_gateway(
                    "Qoder tool call is missing id or function name",
                ));
            }
            if self
                .contract
                .as_ref()
                .is_some_and(|contract| !contract.allowed_names.contains(&state.name))
            {
                return Err(ProxyError::bad_gateway(
                    "Qoder returned an undeclared tool name",
                ));
            }
            let _parse_memory = budget
                .map(|budget| {
                    budget.reserve(
                        RequestMemoryComponent::ToolArguments,
                        json_parse_working_bytes(state.arguments.as_bytes()),
                    )
                })
                .transpose()
                .map_err(|error| error.into_proxy_error())?;
            serde_json::from_str::<Value>(&state.arguments).map_err(|_| {
                ProxyError::bad_gateway("Qoder tool call has invalid JSON arguments")
            })?;
        }
        Ok(())
    }
}
