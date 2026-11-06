use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Caller-owned function identities and completion constraints. This is not a
/// claim that Trae implements strict JSON Schema structured generation.
#[derive(Debug, Clone, Default)]
pub(crate) struct TraeToolContract {
    schemas: BTreeMap<String, Value>,
    selected: Option<String>,
    required: bool,
    pub(super) parallel: bool,
}

impl TraeToolContract {
    pub(super) fn retained_bytes(&self) -> usize {
        self.schemas
            .iter()
            .map(|(name, schema)| {
                name.capacity() + super::super::request_memory::retained_json_bytes(schema) + 128
            })
            .sum::<usize>()
            + self.selected.as_ref().map_or(0, String::capacity)
    }

    pub(crate) fn from_payload(payload: &Value) -> Result<Self, String> {
        let mut contract = Self {
            parallel: true,
            ..Self::default()
        };
        if let Some(tools) = payload.get("tools").and_then(Value::as_array) {
            for tool in tools {
                let function = tool
                    .get("function")
                    .ok_or("Trae tool is missing function")?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| {
                        !name.is_empty()
                            && name.trim() == *name
                            && name.len() <= 256
                            && !name.chars().any(char::is_control)
                    })
                    .ok_or("Trae tool has an invalid exact function name")?;
                let schema = function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object","properties":{}}));
                let schema = if let Some(encoded) = schema.as_str() {
                    serde_json::from_str(encoded).map_err(|_| "Trae tool schema is invalid JSON")?
                } else {
                    schema
                };
                if contract.schemas.insert(name.to_string(), schema).is_some() {
                    return Err("Trae tools contain a duplicate function name".to_string());
                }
            }
        }
        if let Some(choice) = payload.get("tool_choice").and_then(Value::as_str) {
            match choice {
                "auto" | "none" => {}
                "required" => contract.required = true,
                name => {
                    contract.required = true;
                    contract.selected = Some(name.to_string());
                }
            }
            if contract.required && contract.schemas.is_empty() {
                return Err("Trae forced tool choice requires tools".to_string());
            }
            if contract
                .selected
                .as_ref()
                .is_some_and(|name| !contract.schemas.contains_key(name))
            {
                return Err("Trae selected function is not declared".to_string());
            }
        }
        if let Some(value) = payload
            .get("parallel_tool_calls")
            .filter(|value| !value.is_null())
        {
            contract.parallel = value
                .as_bool()
                .ok_or("Trae parallel_tool_calls must be a boolean")?;
        }
        Ok(contract)
    }

    pub(crate) fn allows_empty_arguments(&self, name: &str) -> bool {
        // Only the unambiguous zero-argument schema permits synthesizing {}.
        // Ref/composition/minProperties can make an otherwise empty object invalid.
        self.schemas.get(name).is_some_and(|schema| {
            schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|value| value.is_empty())
                && schema
                    .get("required")
                    .is_none_or(|value| value.as_array().is_some_and(|values| values.is_empty()))
                && schema.as_object().is_some_and(|object| {
                    object.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "type"
                                | "properties"
                                | "required"
                                | "additionalProperties"
                                | "description"
                                | "title"
                                | "$schema"
                        )
                    })
                })
        })
    }

    pub(crate) fn validate_call(&self, name: &str, arguments: &Value) -> Result<(), String> {
        let schema = self
            .schemas
            .get(name)
            .ok_or("Trae returned an undeclared function")?;
        if self
            .selected
            .as_deref()
            .is_some_and(|selected| selected != name)
        {
            return Err("Trae returned a function different from tool_choice".to_string());
        }
        let object = arguments
            .as_object()
            .ok_or("Trae tool arguments must be a JSON object")?;
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            if required
                .iter()
                .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
            {
                return Err("Trae tool arguments omit a required property".to_string());
            }
        }
        Ok(())
    }

    pub(crate) fn validate_count(&self, count: usize) -> Result<(), String> {
        if self.required && count == 0 {
            return Err("Trae did not satisfy required tool_choice".to_string());
        }
        if !self.parallel && count > 1 {
            return Err(
                "Trae returned parallel calls while parallel_tool_calls is false".to_string(),
            );
        }
        Ok(())
    }
}
