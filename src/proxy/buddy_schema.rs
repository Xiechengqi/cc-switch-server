//! Constraint-preserving normalization for Buddy's existing function tools.
//! Visit only JSON Schema positions; property names and enum values are data.

use serde_json::{json, Map, Value};

const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 16_384;

/// Conservative extra logical bytes reserved before normalization allocates.
/// Counting data positions too is intentional: it cannot under-reserve for a
/// schema vocabulary this normalizer does not recognize yet.
pub(super) fn growth_bound(request: &Value) -> usize {
    fn extra(value: &Value) -> usize {
        match value {
            Value::Bool(false) => 128,
            Value::Array(values) => values
                .iter()
                .fold(0usize, |sum, value| sum.saturating_add(extra(value))),
            Value::Object(object) => object.values().fold(
                if !object.contains_key("type") && object.get("enum").is_some_and(Value::is_array) {
                    384usize
                } else {
                    0
                },
                |sum, value| sum.saturating_add(extra(value)),
            ),
            _ => 0,
        }
    }
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.pointer("/function/parameters"))
                .fold(0usize, |sum, schema| sum.saturating_add(extra(schema)))
        })
        .unwrap_or(0)
}

pub(super) fn normalize_tools(request: &mut Map<String, Value>) -> Result<(), String> {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut remaining = MAX_SCHEMA_NODES;
    for tool in tools {
        if let Some(schema) = tool.pointer_mut("/function/parameters") {
            normalize_schema(schema, 0, &mut remaining)?;
        }
    }
    Ok(())
}

fn normalize_schema(schema: &mut Value, depth: usize, remaining: &mut usize) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH || *remaining == 0 {
        return Err("Buddy tool schema exceeds normalization limits".to_string());
    }
    *remaining -= 1;
    if let Value::Bool(allowed) = schema {
        *schema = if *allowed {
            json!({})
        } else {
            json!({"not": {}})
        };
        return Ok(());
    }
    let object = schema
        .as_object_mut()
        .ok_or_else(|| "Buddy tool schema must be an object or boolean".to_string())?;
    if !object.contains_key("type") {
        if let Some(values) = object.get("enum").and_then(Value::as_array) {
            // An empty enum remains unsatisfiable. Never remove it or infer a
            // default string type that would change the accepted values.
            let mut types = std::collections::BTreeSet::new();
            for value in values {
                types.insert(match value {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                });
            }
            if !types.is_empty() {
                let inferred = if types.len() == 1 {
                    Value::String(types.first().expect("nonempty types").to_string())
                } else {
                    Value::Array(
                        types
                            .into_iter()
                            .map(|value| Value::String(value.to_string()))
                            .collect(),
                    )
                };
                object.insert("type".to_string(), inferred);
            }
        }
    }
    for (key, value) in object.iter_mut() {
        match key.as_str() {
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                if let Some(children) = value.as_object_mut() {
                    for child in children.values_mut() {
                        normalize_schema(child, depth + 1, remaining)?;
                    }
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(children) = value.as_array_mut() {
                    for child in children {
                        normalize_schema(child, depth + 1, remaining)?;
                    }
                }
            }
            "items" if value.is_array() => {
                for child in value.as_array_mut().expect("tuple items") {
                    normalize_schema(child, depth + 1, remaining)?;
                }
            }
            "not"
            | "if"
            | "then"
            | "else"
            | "items"
            | "additionalItems"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "contains"
            | "propertyNames"
            | "contentSchema" => {
                normalize_schema(value, depth + 1, remaining)?;
            }
            "dependencies" => {
                if let Some(children) = value.as_object_mut() {
                    for child in children.values_mut().filter(|child| !child.is_array()) {
                        normalize_schema(child, depth + 1, remaining)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_schema_constraints_and_data_positions() {
        let mut request = json!({"tools":[{"type":"function", "function":{
            "name":"sample", "parameters":{
                "type":"object", "required":["deny"], "additionalProperties":false,
                "properties":{
                    "deny": false, "allow":true,
                    "enum": {"enum":[true, null, "x", 2]},
                    "empty":{"enum":[]},
                    "explicit":{"type":"integer", "enum":[1,2]},
                    "tuple":{"type":"array", "items":[false, {"enum":["x"]}], "additionalItems":false},
                    "literal":{"enum":[{"properties":false}]},
                    "union":{"oneOf":[{"enum":["x"]}, false], "$ref":"#/$defs/x"}
                }, "$defs":{"x":{"enum":["x"]}}
            }
        }}]});
        normalize_tools(request.as_object_mut().unwrap()).unwrap();
        let schema = &request["tools"][0]["function"]["parameters"];
        assert_eq!(schema["additionalProperties"], json!({"not":{}}));
        assert_eq!(schema["properties"]["deny"], json!({"not":{}}));
        assert_eq!(schema["properties"]["allow"], json!({}));
        assert_eq!(
            schema["properties"]["enum"]["type"],
            json!(["boolean", "null", "number", "string"])
        );
        assert_eq!(schema["properties"]["empty"], json!({"enum":[]}));
        assert_eq!(schema["properties"]["explicit"]["type"], "integer");
        assert!(schema["properties"]["tuple"]["items"].is_array());
        assert!(schema["properties"]["tuple"].get("anyOf").is_none());
        assert_eq!(
            schema["properties"]["literal"]["enum"],
            json!([{"properties":false}])
        );
        assert_eq!(schema["properties"]["union"]["$ref"], "#/$defs/x");
        assert!(schema["properties"]["union"].get("oneOf").is_some());
        assert_eq!(schema["required"], json!(["deny"]));
    }

    #[test]
    fn bounds_schema_recursion_and_total_nodes() {
        let mut schema = json!({});
        for _ in 0..MAX_SCHEMA_DEPTH + 1 {
            schema = json!({"items":schema});
        }
        let mut remaining = MAX_SCHEMA_NODES;
        assert!(normalize_schema(&mut schema, 0, &mut remaining).is_err());
        let mut schema = json!({"allOf":vec![json!({}); MAX_SCHEMA_NODES]});
        let mut remaining = MAX_SCHEMA_NODES;
        assert!(normalize_schema(&mut schema, 0, &mut remaining).is_err());
    }
}
