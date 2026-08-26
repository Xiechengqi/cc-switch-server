use serde_json::{json, Value};

use super::ProxyError;

const REASONING_ENCRYPTED_CONTENT: &str = "reasoning.encrypted_content";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionSignalMode {
    PreserveIfAbsent,
    AddIfMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompactionNormalization {
    pub had_trigger: bool,
    pub added_trigger: bool,
    pub removed_duplicates: usize,
}

pub(crate) fn normalize_compaction_trigger(
    body: &mut Value,
    mode: CompactionSignalMode,
) -> Result<CompactionNormalization, ProxyError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("Codex Responses body must be a JSON object"))?;

    let original_input = object.remove("input").unwrap_or(Value::Array(Vec::new()));
    let mut input = match original_input {
        Value::String(text) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        item => vec![item],
    };

    let original_len = input.len();
    input.retain(|item| !is_compaction_trigger(item));
    let trigger_count = original_len.saturating_sub(input.len());
    let had_trigger = trigger_count > 0;
    let add_trigger = had_trigger || mode == CompactionSignalMode::AddIfMissing;
    if add_trigger {
        input.push(json!({"type": "compaction_trigger"}));
    }
    object.insert("input".to_string(), Value::Array(input));

    if add_trigger {
        object.insert("stream".to_string(), Value::Bool(true));
        object.insert("store".to_string(), Value::Bool(false));
        match object.get_mut("include") {
            Some(Value::Array(include)) => {
                if !include.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|value| value == REASONING_ENCRYPTED_CONTENT)
                }) {
                    include.push(Value::String(REASONING_ENCRYPTED_CONTENT.to_string()));
                }
            }
            _ => {
                object.insert(
                    "include".to_string(),
                    Value::Array(vec![Value::String(REASONING_ENCRYPTED_CONTENT.to_string())]),
                );
            }
        }
    }

    Ok(CompactionNormalization {
        had_trigger,
        added_trigger: !had_trigger && add_trigger,
        removed_duplicates: trigger_count.saturating_sub(1),
    })
}

pub(crate) fn body_has_compaction_trigger(body: &Value) -> bool {
    match body.get("input") {
        Some(Value::Array(items)) => items.iter().any(is_compaction_trigger),
        Some(item) => is_compaction_trigger(item),
        None => false,
    }
}

pub(crate) fn responses_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .strip_suffix("/responses/compact")
        .map(|prefix| format!("{prefix}/responses"))
        .unwrap_or_else(|| trimmed.to_string())
}

fn is_compaction_trigger(item: &Value) -> bool {
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("compaction_trigger"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_trigger_order_duplicates_and_required_transport_fields() {
        let mut body = json!({
            "input": [
                {"type": " Compaction_Trigger "},
                {"type": "message", "role": "user", "content": "keep"},
                {"type": "compaction_trigger"}
            ],
            "stream": false,
            "store": true,
            "include": ["file_search_call.results", "reasoning.encrypted_content"]
        });

        let result =
            normalize_compaction_trigger(&mut body, CompactionSignalMode::PreserveIfAbsent)
                .unwrap();

        assert!(result.had_trigger);
        assert!(!result.added_trigger);
        assert_eq!(result.removed_duplicates, 1);
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["input"].as_array().unwrap().len(), 2);
        assert_eq!(
            body.pointer("/input/1/type"),
            Some(&json!("compaction_trigger"))
        );
        assert_eq!(body["include"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn explicit_compact_promotes_string_input_and_is_idempotent() {
        let mut body = json!({"input": "summarize this"});
        let first =
            normalize_compaction_trigger(&mut body, CompactionSignalMode::AddIfMissing).unwrap();
        let once = body.clone();
        let second =
            normalize_compaction_trigger(&mut body, CompactionSignalMode::AddIfMissing).unwrap();

        assert!(first.added_trigger);
        assert!(second.had_trigger);
        assert_eq!(body, once);
        assert_eq!(body.pointer("/input/0/type"), Some(&json!("message")));
        assert_eq!(
            body.pointer("/input/1/type"),
            Some(&json!("compaction_trigger"))
        );
    }

    #[test]
    fn preserve_mode_does_not_add_a_missing_trigger() {
        let mut body =
            json!({"input": [{"type": "message", "content": {"type": "compaction_trigger"}}]});
        let result =
            normalize_compaction_trigger(&mut body, CompactionSignalMode::PreserveIfAbsent)
                .unwrap();
        assert!(!result.had_trigger);
        assert!(body.get("stream").is_none());
        assert_eq!(
            body.pointer("/input/0/content/type"),
            Some(&json!("compaction_trigger"))
        );
    }

    #[test]
    fn converts_compact_endpoint_back_to_responses() {
        assert_eq!(
            responses_url("https://chatgpt.com/backend-api/codex/responses/compact/"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            responses_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }
}
