use std::collections::HashMap;

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ToolCompatibilityMode {
    #[default]
    Raw,
    ClaudeCode,
}

pub(super) fn builtin_name(client_name: &str, mode: ToolCompatibilityMode) -> Option<&'static str> {
    if mode != ToolCompatibilityMode::ClaudeCode {
        return None;
    }
    match client_name {
        "Write" => Some("fs_write"),
        "Edit" => Some("str_replace"),
        "Bash" => Some("execute_bash"),
        "Read" => Some("read_file"),
        "Glob" => Some("file_search"),
        "Grep" => Some("grep_search"),
        "LS" => Some("list_directory"),
        "WebSearch" => Some("web_search"),
        _ => None,
    }
}

pub(super) fn map_name(
    client_name: &str,
    mode: ToolCompatibilityMode,
    tool_name_map: &mut HashMap<String, String>,
) -> Option<String> {
    let kiro_name = builtin_name(client_name, mode)?;
    tool_name_map
        .entry(kiro_name.to_string())
        .or_insert_with(|| client_name.to_string());
    Some(kiro_name.to_string())
}

pub(super) fn schema_to_kiro(client_name: &str, schema: Value) -> Value {
    let Some(kiro_name) = builtin_name(client_name, ToolCompatibilityMode::ClaudeCode) else {
        return schema;
    };
    rewrite_schema_keys(schema, |key| outbound_key(kiro_name, key))
}

pub(super) fn input_to_kiro(client_name: &str, input: Value) -> Value {
    let Some(kiro_name) = builtin_name(client_name, ToolCompatibilityMode::ClaudeCode) else {
        return input;
    };
    let Value::Object(source) = input else {
        return input;
    };
    let mut output = Map::new();
    for (key, value) in source {
        let mapped = outbound_key(kiro_name, &key).unwrap_or(key.as_str());
        output.insert(mapped.to_string(), value);
    }
    if matches!(kiro_name, "read_file" | "file_search" | "list_directory") {
        output
            .entry("explanation".to_string())
            .or_insert_with(|| json!(format!("Mapped from Claude Code {client_name}.")));
    }
    if kiro_name == "read_file" {
        let offset = number(output.remove("offset").as_ref());
        let limit = number(output.remove("limit").as_ref());
        if let Some(start) = offset {
            output.insert("start_line".to_string(), json!(start));
        }
        if let Some(limit) = limit {
            let end = offset
                .map(|start| start.saturating_add(limit).saturating_sub(1))
                .unwrap_or(limit);
            output.insert("end_line".to_string(), json!(end));
        }
    }
    Value::Object(output)
}

pub(super) fn input_from_kiro(kiro_name: &str, input: Value) -> Value {
    let Value::Object(source) = input else {
        return input;
    };
    let mut output = Map::new();
    let start = source.get("start_line").and_then(number_value);
    let end = source.get("end_line").and_then(number_value);
    for (key, value) in source {
        if key == "explanation"
            || (kiro_name == "read_file" && matches!(key.as_str(), "start_line" | "end_line"))
        {
            continue;
        }
        let mapped = inbound_key(kiro_name, &key).unwrap_or(key.as_str());
        output.insert(mapped.to_string(), value);
    }
    if kiro_name == "read_file" {
        if let Some(start) = start {
            output.insert("offset".to_string(), json!(start));
        }
        if let Some(end) = end {
            let limit = start
                .map(|start| end.saturating_sub(start).saturating_add(1))
                .unwrap_or(end);
            if limit > 0 {
                output.insert("limit".to_string(), json!(limit));
            }
        }
    }
    Value::Object(output)
}

fn outbound_key<'a>(kiro_name: &str, key: &'a str) -> Option<&'a str> {
    match (kiro_name, key) {
        ("fs_write", "file_path") => Some("path"),
        ("fs_write", "content") => Some("text"),
        ("str_replace", "file_path") => Some("path"),
        ("str_replace", "old_string") => Some("oldStr"),
        ("str_replace", "new_string") => Some("newStr"),
        ("read_file", "file_path") => Some("path"),
        ("file_search", "pattern") => Some("query"),
        ("grep_search", "pattern") => Some("query"),
        ("grep_search", "glob") => Some("includePattern"),
        ("list_directory", "path") => Some("path"),
        _ => None,
    }
}

fn inbound_key<'a>(kiro_name: &str, key: &'a str) -> Option<&'a str> {
    match (kiro_name, key) {
        ("fs_write", "path") => Some("file_path"),
        ("fs_write", "text") => Some("content"),
        ("str_replace", "path") => Some("file_path"),
        ("str_replace", "oldStr") => Some("old_string"),
        ("str_replace", "newStr") => Some("new_string"),
        ("read_file", "path") => Some("file_path"),
        ("file_search", "query") => Some("pattern"),
        ("grep_search", "query") => Some("pattern"),
        ("grep_search", "includePattern") => Some("glob"),
        _ => None,
    }
}

fn rewrite_schema_keys(mut schema: Value, map: impl Fn(&str) -> Option<&str> + Copy) -> Value {
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    if let Some(Value::Object(properties)) = object.remove("properties") {
        let properties = properties
            .into_iter()
            .map(|(key, value)| {
                let mapped = map(&key).unwrap_or(&key).to_string();
                (mapped, rewrite_schema_keys(value, map))
            })
            .collect();
        object.insert("properties".to_string(), Value::Object(properties));
    }
    if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
        for key in required {
            if let Some(value) = key.as_str() {
                if let Some(mapped) = map(value) {
                    *key = Value::String(mapped.to_string());
                }
            }
        }
    }
    if let Some(items) = object.remove("items") {
        object.insert("items".to_string(), rewrite_schema_keys(items, map));
    }
    schema
}

fn number(value: Option<&Value>) -> Option<i64> {
    value.and_then(number_value)
}

fn number_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_read_round_trip_preserves_line_range() {
        let outbound = input_to_kiro(
            "Read",
            json!({"file_path":"src/main.rs","offset":10,"limit":5}),
        );
        assert_eq!(outbound["path"], "src/main.rs");
        assert_eq!(outbound["start_line"], 10);
        assert_eq!(outbound["end_line"], 14);
        let inbound = input_from_kiro("read_file", outbound);
        assert_eq!(
            inbound,
            json!({"file_path":"src/main.rs","offset":10,"limit":5})
        );
    }

    #[test]
    fn raw_mode_does_not_capture_same_named_custom_tool() {
        let mut map = HashMap::new();
        assert!(map_name("Read", ToolCompatibilityMode::Raw, &mut map).is_none());
        assert!(map.is_empty());
    }
}
