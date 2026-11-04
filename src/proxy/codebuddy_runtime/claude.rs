use serde_json::Value;

use super::super::transforms::{self, TransformError};

const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const BUDDY_CODING_IDENTITY: &str = "You are a helpful coding assistant.";
const CLAUDE_BILLING_HEADER: &str = "x-anthropic-billing-header:";

pub(crate) fn transform_buddy_claude_request(input: &Value) -> Result<Value, TransformError> {
    let mut chat = transforms::anthropic_to_openai_chat(input)?;
    normalize_default_claude_code_identity(&mut chat);
    Ok(chat)
}

/// Only the fixed, leading CLI identity is a transport compatibility concern.
/// User messages, tool output, assistant history and subsequent system text
/// remain caller data. No regex, brand-wide replacement or recursive scan.
fn normalize_default_claude_code_identity(chat: &mut Value) {
    let Some(system) = chat.pointer_mut("/messages/0") else {
        return;
    };
    if system.get("role").and_then(Value::as_str) != Some("system") {
        return;
    }
    match system.get_mut("content") {
        Some(Value::String(text)) => replace_identity_prefix(text),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(text) = block.get("text").and_then(Value::as_str) else {
                    break;
                };
                if text.starts_with(CLAUDE_BILLING_HEADER)
                    && !text.trim_end_matches(['\r', '\n']).contains(['\r', '\n'])
                {
                    continue;
                }
                if let Some(Value::String(text)) = block.get_mut("text") {
                    replace_identity_prefix(text);
                }
                break;
            }
        }
        _ => {}
    }
}

fn replace_identity_prefix(text: &mut String) {
    let offset = if text.starts_with(CLAUDE_BILLING_HEADER) {
        let Some(end) = text.find('\n') else {
            return;
        };
        let rest = text[end + 1..].trim_start_matches(['\r', '\n']);
        text.len() - rest.len()
    } else {
        0
    };
    let Some(suffix) = text[offset..].strip_prefix(CLAUDE_CODE_IDENTITY) else {
        return;
    };
    if suffix.is_empty() || suffix.starts_with('\n') || suffix.starts_with("\r\n") {
        text.replace_range(
            offset..offset + CLAUDE_CODE_IDENTITY.len(),
            BUDDY_CODING_IDENTITY,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(extra: Value) -> Value {
        let mut input = json!({"model":"claude-opus-5-5",
            "messages":[{"role":"user","content":"ping"}]});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        input
    }

    #[test]
    fn buddy_claude_default_identity_normalization_preserves_caller_instructions_and_data() {
        let billing = "x-anthropic-billing-header: cc_version=fixture; cc_entrypoint=cli;";
        let custom = format!("Custom policy.\nExample: {CLAUDE_CODE_IDENTITY}");
        for system in [
            json!(CLAUDE_CODE_IDENTITY),
            json!(format!("{CLAUDE_CODE_IDENTITY}\r\n{custom}")),
            json!(format!("{billing}\n\n{CLAUDE_CODE_IDENTITY}\n{custom}")),
            json!([{"type":"text","text":billing},
                {"type":"text","text":CLAUDE_CODE_IDENTITY},
                {"type":"text","text":custom,"cache_control":{"type":"ephemeral"}}]),
        ] {
            let input = source(json!({"system":system,"messages":[
                {"role":"user","content":CLAUDE_CODE_IDENTITY},
                {"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"lookup","input":{"query":CLAUDE_CODE_IDENTITY}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":CLAUDE_CODE_IDENTITY}]}
            ]}));
            let generic = transforms::anthropic_to_openai_chat(&input).unwrap();
            let buddy = transform_buddy_claude_request(&input).unwrap();
            assert!(buddy["messages"][0]["content"]
                .to_string()
                .contains(BUDDY_CODING_IDENTITY));
            assert_eq!(
                buddy["messages"].as_array().unwrap()[1..],
                generic["messages"].as_array().unwrap()[1..]
            );
            if generic["messages"][0]["content"]
                .to_string()
                .contains("Custom policy.")
            {
                assert!(buddy["messages"][0]["content"]
                    .to_string()
                    .contains(&custom.replace('\n', "\\n")));
            }
        }
        for system in [
            json!(format!("Example:\n{CLAUDE_CODE_IDENTITY}")),
            json!(format!("{CLAUDE_CODE_IDENTITY} This is quoted text.")),
            json!([{"type":"text","text":"Custom policy."},{"type":"text","text":CLAUDE_CODE_IDENTITY}]),
        ] {
            let input = source(json!({"system":system}));
            assert_eq!(
                transform_buddy_claude_request(&input).unwrap(),
                transforms::anthropic_to_openai_chat(&input).unwrap()
            );
        }
    }
}
