use serde_json::{json, Value};

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
}
