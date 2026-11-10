//! Narrow Moonshot Chat compatibility: inline local JSON Pointer references.
//! Cycles and exhausted budgets are rejected instead of weakening the schema.
use serde_json::{json, Map, Value};

use crate::infra::resident_memory::serialized_json_bytes;

use super::ProxyError;

const MAX_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_REQUEST_SCHEMA_BYTES: usize = 1024 * 1024;
const MAX_SCHEMA_NODES: usize = 8192;
const MAX_SCHEMA_DEPTH: usize = 64;

fn invalid(reason: &'static str) -> ProxyError {
    ProxyError::bad_request(format!("Kimi Chat tool schema {reason}"))
}

#[derive(Debug)]
struct Budget {
    bytes: usize,
    nodes: usize,
}

impl Budget {
    fn consume(&mut self, bytes: usize, depth: usize) -> Result<(), ProxyError> {
        if depth > MAX_SCHEMA_DEPTH || self.nodes == 0 || bytes > self.bytes {
            return Err(invalid("exceeds the local expansion budget"));
        }
        self.bytes -= bytes;
        self.nodes -= 1;
        Ok(())
    }
    fn key(&mut self, key: &str, depth: usize) -> Result<(), ProxyError> {
        let bytes = serialized_json_bytes(&key, self.bytes)
            .ok_or_else(|| invalid("exceeds the local expansion budget"))?;
        self.consume(bytes.saturating_add(2), depth)
    }
}

// JSON embedded in default/const/enum/examples is instance data. Its keys must
// remain literal even when they are named "$ref" or "properties".
fn copy_literal(value: &Value, budget: &mut Budget, depth: usize) -> Result<Value, ProxyError> {
    match value {
        Value::Array(values) => {
            budget.consume(2_usize.saturating_add(values.len()), depth)?;
            values
                .iter()
                .map(|value| copy_literal(value, budget, depth + 1))
                .collect()
        }
        Value::Object(values) => {
            budget.consume(2, depth)?;
            let mut result = Map::new();
            for (key, value) in values {
                budget.key(key, depth)?;
                result.insert(key.clone(), copy_literal(value, budget, depth + 1)?);
            }
            Ok(Value::Object(result))
        }
        _ => {
            let bytes = serialized_json_bytes(value, budget.bytes)
                .ok_or_else(|| invalid("exceeds the local expansion budget"))?;
            budget.consume(bytes, depth)?;
            Ok(value.clone())
        }
    }
}

fn schema_map(key: &str) -> bool {
    matches!(key, "properties" | "patternProperties" | "dependentSchemas")
}

fn schema_array(key: &str) -> bool {
    matches!(key, "allOf" | "anyOf" | "oneOf" | "prefixItems")
}

fn schema_single(key: &str) -> bool {
    matches!(
        key,
        "items"
            | "additionalItems"
            | "additionalProperties"
            | "unevaluatedItems"
            | "unevaluatedProperties"
            | "contains"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else"
            | "contentSchema"
    )
}

fn resolve(
    value: &Value,
    root: &Value,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, ProxyError> {
    let Some(object) = value.as_object() else {
        return if value.is_boolean() {
            copy_literal(value, budget, depth)
        } else {
            Err(invalid("must contain object or boolean schemas"))
        };
    };
    budget.consume(2, depth)?;
    if [
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "$dynamicRef",
        "$recursiveAnchor",
        "$recursiveRef",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
    {
        return Err(invalid("uses unsupported reference scopes"));
    }
    let target = if let Some(reference) = object.get("$ref") {
        let reference = reference
            .as_str()
            .ok_or_else(|| invalid("has an invalid reference"))?;
        let pointer = reference
            .strip_prefix('#')
            .filter(|pointer| pointer.is_empty() || pointer.starts_with('/'))
            .ok_or_else(|| invalid("requires a local JSON Pointer reference"))?;
        if stack.iter().any(|current| current == reference) {
            return Err(invalid("contains a recursive reference"));
        }
        let target = root
            .pointer(pointer)
            .ok_or_else(|| invalid("has an unresolved reference"))?;
        stack.push(reference.to_string());
        let target = resolve(target, root, stack, budget, depth + 1)?;
        stack.pop();
        Some(target)
    } else {
        None
    };

    let mut result = Map::new();
    for (key, value) in object {
        if matches!(key.as_str(), "$ref" | "$defs" | "definitions") {
            continue;
        }
        budget.key(key, depth)?;
        let value = if key == "dependencies" {
            let values = value
                .as_object()
                .ok_or_else(|| invalid("has an invalid dependencies map"))?;
            budget.consume(2, depth + 1)?;
            let mut values_out = Map::new();
            for (name, schema) in values {
                budget.key(name, depth + 1)?;
                let value = if schema.is_array() {
                    copy_literal(schema, budget, depth + 2)?
                } else {
                    resolve(schema, root, stack, budget, depth + 2)?
                };
                values_out.insert(name.clone(), value);
            }
            Value::Object(values_out)
        } else if schema_map(key) {
            let values = value
                .as_object()
                .ok_or_else(|| invalid("has an invalid schema map"))?;
            budget.consume(2, depth + 1)?;
            let mut values_out = Map::new();
            for (name, schema) in values {
                budget.key(name, depth + 1)?;
                values_out.insert(
                    name.clone(),
                    resolve(schema, root, stack, budget, depth + 2)?,
                );
            }
            Value::Object(values_out)
        } else if schema_array(key) || (key == "items" && value.is_array()) {
            let values = value
                .as_array()
                .ok_or_else(|| invalid("has an invalid schema array"))?;
            budget.consume(2_usize.saturating_add(values.len()), depth + 1)?;
            Value::Array(
                values
                    .iter()
                    .map(|value| resolve(value, root, stack, budget, depth + 2))
                    .collect::<Result<_, _>>()?,
            )
        } else if schema_single(key) {
            resolve(value, root, stack, budget, depth + 1)?
        } else {
            copy_literal(value, budget, depth + 1)?
        };
        result.insert(key.clone(), value);
    }
    let Some(target) = target else {
        return Ok(Value::Object(result));
    };
    if result.is_empty() {
        return Ok(target);
    }
    let target = if let Value::Object(mut target_object) = target {
        if result
            .iter()
            .all(|(key, value)| target_object.get(key).is_none_or(|other| other == value))
        {
            target_object.extend(result);
            return Ok(Value::Object(target_object));
        }
        Value::Object(target_object)
    } else {
        target
    };
    // Both the referenced assertions and sibling assertions remain required.
    budget.consume(16, depth)?;
    Ok(json!({"allOf":[target, Value::Object(result)]}))
}

fn normalize(schema: &Value) -> Result<Value, ProxyError> {
    serialized_json_bytes(schema, MAX_SCHEMA_BYTES)
        .ok_or_else(|| invalid("exceeds the local input budget"))?;
    let mut budget = Budget {
        bytes: MAX_SCHEMA_BYTES,
        nodes: MAX_SCHEMA_NODES,
    };
    let result = resolve(schema, schema, &mut Vec::new(), &mut budget, 0)?;
    serialized_json_bytes(&result, MAX_SCHEMA_BYTES)
        .ok_or_else(|| invalid("exceeds the local output budget"))?;
    Ok(result)
}

pub(super) fn normalize_chat_tools(body: &mut Map<String, Value>) -> Result<(), ProxyError> {
    let mut remaining = MAX_REQUEST_SCHEMA_BYTES;
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if let Some(schema) = tool
                .get_mut("function")
                .and_then(|function| function.get_mut("parameters"))
            {
                let normalized = normalize(schema)?;
                let bytes = serialized_json_bytes(&normalized, remaining)
                    .ok_or_else(|| invalid("exceeds the local request budget"))?;
                remaining -= bytes;
                *schema = normalized;
            }
        }
    }
    if let Some(functions) = body.get_mut("functions").and_then(Value::as_array_mut) {
        for function in functions {
            if let Some(schema) = function.get_mut("parameters") {
                let normalized = normalize(schema)?;
                let bytes = serialized_json_bytes(&normalized, remaining)
                    .ok_or_else(|| invalid("exceeds the local request budget"))?;
                remaining -= bytes;
                *schema = normalized;
            }
        }
    }
    Ok(())
}
