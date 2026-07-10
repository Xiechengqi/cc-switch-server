use serde_json::Value;

pub(crate) fn encode_sse_event(value: &Value) -> Option<String> {
    let event = value.get("type").and_then(Value::as_str)?;
    let data = encode_json_event(value)?;
    Some(format!("event: {event}\ndata: {data}\n\n"))
}

pub(crate) fn encode_named_sse_event(event: &str, value: &Value) -> Option<String> {
    let data = encode_json_event(value)?;
    Some(format!("event: {event}\ndata: {data}\n\n"))
}

fn encode_json_event(value: &Value) -> Option<String> {
    match value.get("type").and_then(Value::as_str)? {
        "response.output_item.added" => encode_output_item_event(value),
        "response.output_item.done" => encode_output_item_event(value),
        "response.output_text.delta" => encode_output_text_delta(value),
        "response.function_call_arguments.delta" => encode_function_call_delta(value),
        "response.completed" => encode_response_completed(value),
        _ => None,
    }
}

fn encode_output_item_event(value: &Value) -> Option<String> {
    let event_type = json_string(value.get("type")?)?;
    let output_index = integer_or_zero(value.get("output_index"));
    let mut fields = vec![
        format!("\"type\":{event_type}"),
        format!("\"output_index\":{output_index}"),
    ];
    if let Some(item) = value.get("item") {
        fields.push(format!("\"item\":{}", json_string(item)?));
    }
    Some(format!("{{{}}}", fields.join(",")))
}

fn encode_output_text_delta(value: &Value) -> Option<String> {
    let event_type = json_string(value.get("type")?)?;
    let output_index = integer_or_zero(value.get("output_index"));
    let content_index = integer_or_zero(value.get("content_index"));
    let delta = json_string(value.get("delta").unwrap_or(&Value::String(String::new())))?;
    let mut fields = vec![format!("\"type\":{event_type}")];
    if let Some(item_id) = value.get("item_id") {
        fields.push(format!("\"item_id\":{}", json_string(item_id)?));
    }
    fields.push(format!("\"output_index\":{output_index}"));
    fields.push(format!("\"content_index\":{content_index}"));
    fields.push(format!("\"delta\":{delta}"));
    Some(format!("{{{}}}", fields.join(",")))
}

fn encode_function_call_delta(value: &Value) -> Option<String> {
    let event_type = json_string(value.get("type")?)?;
    let output_index = integer_or_zero(value.get("output_index"));
    let delta = json_string(value.get("delta").unwrap_or(&Value::String(String::new())))?;
    let mut fields = vec![format!("\"type\":{event_type}")];
    if let Some(item_id) = value.get("item_id") {
        fields.push(format!("\"item_id\":{}", json_string(item_id)?));
    }
    fields.push(format!("\"output_index\":{output_index}"));
    fields.push(format!("\"delta\":{delta}"));
    Some(format!("{{{}}}", fields.join(",")))
}

fn encode_response_completed(value: &Value) -> Option<String> {
    let event_type = json_string(value.get("type")?)?;
    let response = json_string(
        value
            .get("response")
            .unwrap_or(&Value::Object(Default::default())),
    )?;
    Some(format!("{{\"type\":{event_type},\"response\":{response}}}"))
}

fn integer_or_zero(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .and_then(Value::as_u64)
                .and_then(|value| i64::try_from(value).ok())
        })
        .unwrap_or(0)
}

fn json_string(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn output_item_added_keeps_wire_order_and_zero_index() {
        let data = encode_json_event(&json!({
            "type": "response.output_item.added",
            "item": {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": ""}
        }))
        .unwrap();
        assert_eq!(
            data,
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":""}}"#
        );
    }

    #[test]
    fn deltas_keep_required_zero_fields() {
        let data = encode_json_event(&json!({
            "type": "response.output_text.delta",
            "delta": "hello"
        }))
        .unwrap();
        assert_eq!(
            data,
            r#"{"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"}"#
        );
    }

    #[test]
    fn sse_event_uses_response_type_as_event_name() {
        let data = encode_sse_event(&json!({
            "type": "response.completed",
            "response": {"id": "resp_1"}
        }))
        .unwrap();
        assert_eq!(
            data,
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\"}}\n\n"
        );
    }
}
