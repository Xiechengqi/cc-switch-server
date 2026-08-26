use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

pub(crate) fn normalize_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = match parameters {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        _ => json!({"type": "object", "properties": {}}),
    };
    if let Some(object) = parameters.as_object_mut() {
        if object.get("type").and_then(Value::as_str) != Some("object") {
            object.insert("type".to_string(), json!("object"));
        }
    }
    parameters
}

/// Repairs the JSON Schema subset rejected by the official Codex Responses
/// transport without weakening otherwise valid constraints.
pub(crate) fn normalize_codex_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = normalize_function_parameters(parameters);
    normalize_codex_schema_node(&mut parameters);
    if let Some(object) = parameters.as_object_mut() {
        object.insert("type".to_string(), Value::String("object".to_string()));
    }
    parameters
}

pub(crate) fn normalize_codex_tool_schemas(request: &mut Value) {
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            normalize_codex_tool_schema(tool);
        }
    }
    if let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            if !item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("additional_tools"))
            {
                continue;
            }
            if let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in tools {
                    normalize_codex_tool_schema(tool);
                }
            }
        }
    }
}

fn normalize_codex_tool_schema(tool: &mut Value) {
    let Some(object) = tool.as_object_mut() else {
        return;
    };
    if is_reserved_codex_tool_object(object) {
        return;
    }
    let is_function = object.get("type").and_then(Value::as_str) == Some("function");
    let is_namespace = object.get("type").and_then(Value::as_str) == Some("namespace");
    if is_function {
        if let Some(nested) = object.get_mut("function").and_then(Value::as_object_mut) {
            let normalized = normalize_codex_function_parameters(nested.get("parameters"));
            nested.insert("parameters".to_string(), normalized);
        } else {
            let normalized = normalize_codex_function_parameters(object.get("parameters"));
            object.insert("parameters".to_string(), normalized);
        }
    } else if let Some(parameters) = object.get_mut("parameters") {
        normalize_codex_schema_node(parameters);
    }
    if is_namespace {
        if let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut) {
            for child in children {
                normalize_codex_tool_schema(child);
            }
        }
    }
}

pub(crate) fn is_reserved_codex_tool(tool: &Value) -> bool {
    tool.as_object().is_some_and(is_reserved_codex_tool_object)
}

fn is_reserved_codex_tool_object(object: &Map<String, Value>) -> bool {
    object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("collaboration."))
}

fn normalize_codex_schema_node(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if object.get("type").is_some_and(Value::is_null) {
        object.remove("type");
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_codex_schema_node(child);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "contains",
    ] {
        if let Some(child) = object.get_mut(key) {
            if let Some(children) = child.as_array_mut() {
                for child in children {
                    normalize_codex_schema_node(child);
                }
            } else {
                normalize_codex_schema_node(child);
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_codex_schema_node(child);
            }
        }
    }
}

/// Normalizes the subset of JSON Schema accepted by Gemini/Code Assist tools.
///
/// Keep this separate from `normalize_function_parameters`: other upstreams
/// accept a wider schema dialect and should not lose constraints just because
/// Gemini rejects them.
pub(crate) fn normalize_gemini_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = normalize_function_parameters(parameters);
    normalize_gemini_schema_node(&mut parameters);
    parameters
}

pub(crate) fn normalize_gemini_tool_schemas(request: &mut Value) {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    for tool in tools {
        let Some(tool) = tool.as_object_mut() else {
            continue;
        };
        let declarations = if tool.contains_key("functionDeclarations") {
            tool.get_mut("functionDeclarations")
        } else {
            tool.get_mut("function_declarations")
        };
        let Some(declarations) = declarations.and_then(Value::as_array_mut) else {
            continue;
        };
        for declaration in declarations {
            let Some(declaration) = declaration.as_object_mut() else {
                continue;
            };
            for key in [
                "parameters",
                "parametersJsonSchema",
                "parameters_json_schema",
            ] {
                if let Some(parameters) = declaration.get_mut(key) {
                    *parameters = normalize_gemini_function_parameters(Some(parameters));
                    break;
                }
            }
        }
    }
}

fn normalize_gemini_schema_node(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    promote_boolean_required_properties(object);

    if schema_type_is(object, "integer") {
        normalize_integral_exclusive_bound(object, "exclusiveMinimum", "minimum", 1);
        normalize_integral_exclusive_bound(object, "exclusiveMaximum", "maximum", -1);
    } else {
        object.remove("exclusiveMinimum");
        object.remove("exclusiveMaximum");
    }

    // Code Assist currently rejects this keyword even though it is valid JSON
    // Schema. Dropping it weakens validation but keeps the tool callable.
    object.remove("uniqueItems");

    for key in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_gemini_schema_node(child);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "contains",
        "if",
        "then",
        "else",
        "not",
        "propertyNames",
    ] {
        if let Some(child) = object.get_mut(key) {
            normalize_gemini_schema_node(child);
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_gemini_schema_node(child);
            }
        }
    }
}

fn promote_boolean_required_properties(object: &mut Map<String, Value>) {
    let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    let mut promoted = Vec::new();
    for (name, schema) in properties.iter_mut() {
        let Some(schema) = schema.as_object_mut() else {
            continue;
        };
        if let Some(required) = schema.get("required").and_then(Value::as_bool) {
            if required {
                promoted.push(name.clone());
            }
            schema.remove("required");
        }
    }
    if promoted.is_empty() {
        return;
    }

    let mut required = object
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    required.extend(promoted);
    object.insert(
        "required".to_string(),
        Value::Array(required.into_iter().map(Value::String).collect()),
    );
}

fn schema_type_is(object: &Map<String, Value>, expected: &str) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn normalize_integral_exclusive_bound(
    object: &mut Map<String, Value>,
    exclusive_key: &str,
    inclusive_key: &str,
    delta: i8,
) {
    let Some(exclusive) = object.remove(exclusive_key) else {
        return;
    };
    let candidate = match exclusive {
        Value::Bool(false) => return,
        Value::Bool(true) => object
            .get(inclusive_key)
            .and_then(|value| increment_integral_bound(value, delta)),
        value => increment_integral_bound(&value, delta),
    };
    let Some(candidate) = candidate else {
        return;
    };
    let replace = object
        .get(inclusive_key)
        .and_then(json_number_as_f64)
        .zip(json_number_as_f64(&candidate))
        .map(|(existing, candidate)| {
            if delta > 0 {
                existing < candidate
            } else {
                existing > candidate
            }
        })
        .unwrap_or(true);
    if replace {
        object.insert(inclusive_key.to_string(), candidate);
    }
}

fn increment_integral_bound(value: &Value, delta: i8) -> Option<Value> {
    if let Some(value) = value.as_i64() {
        return value
            .checked_add(i64::from(delta))
            .map(|value| Value::Number(value.into()));
    }
    let value = value.as_u64()?;
    if delta > 0 {
        value
            .checked_add(delta as u64)
            .map(|value| Value::Number(value.into()))
    } else {
        value
            .checked_sub(delta.unsigned_abs() as u64)
            .map(|value| Value::Number(value.into()))
    }
}

fn json_number_as_f64(value: &Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_missing_null_and_non_object_parameters() {
        for parameters in [None, Some(&Value::Null), Some(&json!(["invalid"]))] {
            assert_eq!(
                normalize_function_parameters(parameters),
                json!({"type": "object", "properties": {}})
            );
        }
    }

    #[test]
    fn forces_object_type_without_dropping_schema_keywords() {
        let normalized = normalize_function_parameters(Some(&json!({
            "type": null,
            "oneOf": [
                {"type": "object", "properties": {"id": {"type": "string"}}},
                {"type": "object", "properties": {"slug": {"type": "string"}}}
            ]
        })));
        assert_eq!(normalized["type"], "object");
        assert_eq!(normalized["oneOf"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn preserves_valid_object_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        });
        assert_eq!(normalize_function_parameters(Some(&schema)), schema);
    }

    #[test]
    fn proxy_bridge_contract_fixture_normalizes_tool_schema_roots() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/tool_schema.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "tool-schema-root-object");
        assert_eq!(fixture["category"], "tool_schema");

        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                normalize_function_parameters(case["input"].get("parameters")),
                case["expected"],
                "fixture case {}",
                case["name"]
            );
        }
    }

    #[test]
    fn gemini_normalizes_nested_exclusive_bounds_and_unique_items() {
        let normalized = normalize_gemini_function_parameters(Some(&json!({
            "type": "object",
            "properties": {
                "counts": {
                    "type": "array",
                    "uniqueItems": true,
                    "items": {"type": "integer", "exclusiveMinimum": 0}
                },
                "strict": {"type": "integer", "exclusiveMinimum": 0, "minimum": 5},
                "weak": {"type": "integer", "exclusiveMinimum": 2, "minimum": 1},
                "upper": {"type": "integer", "exclusiveMaximum": 10}
            }
        })));

        assert_eq!(
            normalized.pointer("/properties/counts/items/minimum"),
            Some(&json!(1))
        );
        assert!(normalized
            .pointer("/properties/counts/uniqueItems")
            .is_none());
        assert_eq!(
            normalized.pointer("/properties/strict/minimum"),
            Some(&json!(5))
        );
        assert_eq!(
            normalized.pointer("/properties/weak/minimum"),
            Some(&json!(3))
        );
        assert_eq!(
            normalized.pointer("/properties/upper/maximum"),
            Some(&json!(9))
        );
    }

    #[test]
    fn gemini_drops_ambiguous_exclusive_bounds_and_promotes_required_flags() {
        let normalized = normalize_gemini_function_parameters(Some(&json!({
            "type": "object",
            "properties": {
                "ratio": {"type": "number", "exclusiveMinimum": 0.5},
                "query": {"type": "string", "required": true},
                "optional": {"type": "string", "required": false}
            },
            "required": ["existing"]
        })));

        assert!(normalized
            .pointer("/properties/ratio/exclusiveMinimum")
            .is_none());
        assert!(normalized.pointer("/properties/query/required").is_none());
        assert!(normalized
            .pointer("/properties/optional/required")
            .is_none());
        assert_eq!(normalized["required"], json!(["existing", "query"]));
    }

    #[test]
    fn gemini_native_request_normalizes_each_function_declaration() {
        let mut request = json!({
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "limit": {"type": "integer", "exclusiveMinimum": 0}
                        }
                    }
                }]
            }]
        });
        normalize_gemini_tool_schemas(&mut request);
        assert_eq!(
            request.pointer("/tools/0/functionDeclarations/0/parameters/properties/limit/minimum"),
            Some(&json!(1))
        );
        assert!(request
            .pointer("/tools/0/functionDeclarations/0/parameters/properties/limit/exclusiveMinimum")
            .is_none());
    }

    #[test]
    fn codex_schema_removes_nested_null_types_without_weakening_constraints() {
        let mut request = json!({
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": null,
                    "properties": {"q": {"type": null, "pattern": "^[a-z]+$"}},
                    "$defs": {"item": {"type": null}},
                    "items": [{"type": null}],
                    "allOf": [{"type": null}]
                }
            }],
            "input": [{
                "type": "additional_tools",
                "tools": [{"type": "function", "name": "extra", "parameters": {"type": null}}]
            }]
        });
        normalize_codex_tool_schemas(&mut request);
        assert_eq!(
            request.pointer("/tools/0/parameters/type"),
            Some(&json!("object"))
        );
        assert!(request
            .pointer("/tools/0/parameters/properties/q/type")
            .is_none());
        assert_eq!(
            request.pointer("/tools/0/parameters/properties/q/pattern"),
            Some(&json!("^[a-z]+$"))
        );
        assert!(request
            .pointer("/tools/0/parameters/$defs/item/type")
            .is_none());
        assert!(request
            .pointer("/tools/0/parameters/items/0/type")
            .is_none());
        assert!(request
            .pointer("/tools/0/parameters/allOf/0/type")
            .is_none());
        assert_eq!(
            request.pointer("/input/0/tools/0/parameters/type"),
            Some(&json!("object"))
        );
    }

    #[test]
    fn codex_reserved_tool_is_value_identical() {
        let reserved = json!({
            "type": "function",
            "name": "collaboration.spawn_agent",
            "parameters": {"type": null}
        });
        let mut request = json!({"tools": [reserved.clone()]});
        normalize_codex_tool_schemas(&mut request);
        assert_eq!(request.pointer("/tools/0"), Some(&reserved));
    }
}
