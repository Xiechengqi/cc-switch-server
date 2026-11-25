use serde_json::{json, Value};

use crate::proxy::{cursor::tool_schema, ProxyError};

pub(super) const MAX_TEXT_BYTES: usize = 1024 * 1024;

pub(crate) fn schema_from_request(request: &Value) -> Result<Option<Value>, ProxyError> {
    let format = request
        .pointer("/output_config/format")
        .or_else(|| request.pointer("/text/format"))
        .or_else(|| request.get("response_format"));
    let Some(format) =
        format.filter(|format| format.get("type").and_then(Value::as_str) == Some("json_schema"))
    else {
        return Ok(None);
    };
    let schema = format
        .pointer("/json_schema/schema")
        .or_else(|| format.get("schema"))
        .ok_or_else(|| ProxyError::bad_request("Kiro structured output requires a JSON schema"))?;
    tool_schema::validate_tool_schema(schema).map_err(|_| {
        ProxyError::bad_request(
            "Kiro structured output schema is invalid or exceeds validation limits",
        )
    })?;
    Ok(Some(schema.clone()))
}

pub(crate) fn prepare_request(request: &mut Value, schema: Option<Value>, inject: bool) {
    let Some(schema) = schema else { return };
    if inject {
        let instruction = format!("Return only JSON that conforms to this schema. Omit Markdown and explanatory text.\n{schema}");
        let system = request
            .as_object_mut()
            .expect("canonical Messages request")
            .entry("system")
            .or_insert_with(|| json!([]));
        if system.is_null() {
            *system = json!([]);
        }
        if let Some(text) = system.as_str() {
            *system = json!([{"type":"text","text":text}]);
        }
        if let Some(blocks) = system.as_array_mut() {
            blocks.push(json!({"type":"text","text":instruction}));
        }
    }
    if !request.get("output_config").is_some_and(Value::is_object) {
        request["output_config"] = json!({});
    }
    request["output_config"]["format"] = json!({"type":"json_schema","schema":schema});
}

pub(super) fn extract(text: &str, schema: &Value) -> Option<String> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let mut remaining = text;
    let mut selected = None;
    let mut attempts = 0;
    let mut examined = 0_usize;
    let container_root = matches!(
        schema.get("type").and_then(Value::as_str),
        Some("object" | "array")
    );
    while let Some(index) = remaining.find(|ch: char| {
        matches!(ch, '{' | '[')
            || (!container_root && matches!(ch, '"' | '-' | '0'..='9' | 't' | 'f' | 'n'))
    }) {
        remaining = &remaining[index..];
        attempts += 1;
        examined = examined.saturating_add(remaining.len());
        if attempts > 128 || examined > MAX_TEXT_BYTES * 8 {
            break;
        }
        let mut values = serde_json::Deserializer::from_str(remaining).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            let consumed = values.byte_offset();
            if tool_schema::validate_tool_arguments(schema, &value).is_ok() {
                selected = serde_json::to_string(&value).ok();
            }
            remaining = &remaining[consumed..];
        } else {
            remaining = &remaining[remaining.chars().next()?.len_utf8()..];
        }
    }
    selected
}

pub(super) fn rewrite_message(message: &mut Value, schema: &Value) {
    let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    if blocks.iter().any(|block| block["type"] == "tool_use") {
        return;
    }
    let text = blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    let Some(value) = extract(&text, schema) else {
        return;
    };
    let mut replacement = Some(json!({"type":"text","text":value}));
    blocks.retain_mut(|block| {
        if block["type"] != "text" {
            return true;
        }
        match replacement.take() {
            Some(value) => {
                *block = value;
                true
            }
            None => false,
        }
    });
}
