use serde_json::Value;

use super::super::transforms::{self, TransformError};

const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const BUDDY_CODING_IDENTITY: &str = "You are a helpful coding assistant.";
const CLAUDE_BILLING_HEADER: &str = "x-anthropic-billing-header:";

pub(crate) fn transform_buddy_claude_request(input: &Value) -> Result<Value, TransformError> {
    let mut chat = transforms::anthropic_to_openai_chat(input)?;
    normalize_claude_cli_preamble(&mut chat);
    Ok(chat)
}

/// Only the leading CLI billing metadata and fixed identity are transport data.
/// User messages, tool output, assistant history and subsequent system text
/// remain caller data. No regex, brand-wide replacement or recursive scan.
fn normalize_claude_cli_preamble(chat: &mut Value) {
    let Some(system) = chat.pointer_mut("/messages/0") else {
        return;
    };
    if system.get("role").and_then(Value::as_str) != Some("system") {
        return;
    }
    match system.get_mut("content") {
        Some(Value::String(text)) => {
            strip_leading_billing_metadata(text);
            replace_identity_prefix(text);
        }
        Some(Value::Array(blocks)) => {
            let mut metadata_blocks = 0;
            for block in blocks.iter_mut() {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    break;
                }
                let Some(Value::String(text)) = block.get_mut("text") else {
                    break;
                };
                if strip_leading_billing_metadata(text) && text.is_empty() {
                    metadata_blocks += 1;
                    continue;
                }
                replace_identity_prefix(text);
                break;
            }
            blocks.drain(..metadata_blocks);
        }
        _ => {}
    }
}

fn is_billing_metadata_line(line: &str) -> bool {
    let Some(fields) = line.strip_prefix(CLAUDE_BILLING_HEADER) else {
        return false;
    };
    let mut version = false;
    let mut entrypoint = false;
    for field in fields
        .split(';')
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        let Some((name, value)) = field.split_once('=') else {
            return false;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return false;
        }
        match name {
            "cc_version" => version = true,
            "cc_entrypoint" => entrypoint = true,
            _ => {}
        }
    }
    version && entrypoint
}

fn strip_leading_billing_metadata(text: &mut String) -> bool {
    let mut offset = 0;
    loop {
        let rest = &text[offset..];
        let end = rest.find('\n').unwrap_or(rest.len());
        if !is_billing_metadata_line(rest[..end].trim_end_matches('\r')) {
            break;
        }
        offset += end;
        offset += text[offset..].len() - text[offset..].trim_start_matches(['\r', '\n']).len();
    }
    if offset == 0 {
        return false;
    }
    text.drain(..offset);
    true
}

fn replace_identity_prefix(text: &mut String) {
    let Some(suffix) = text.strip_prefix(CLAUDE_CODE_IDENTITY) else {
        return;
    };
    if suffix.is_empty() || suffix.starts_with('\n') || suffix.starts_with("\r\n") {
        text.replace_range(..CLAUDE_CODE_IDENTITY.len(), BUDDY_CODING_IDENTITY);
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
            assert!(!buddy["messages"][0]["content"]
                .to_string()
                .contains(CLAUDE_BILLING_HEADER));
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

    #[test]
    fn buddy_claude_billing_metadata_is_removed_without_changing_other_system_content() {
        let billing =
            "x-anthropic-billing-header: cc_version=2.1.305; cc_entrypoint=cli; cch=fixture;";
        let custom = "Keep every project instruction.\nQuoted x-anthropic-billing-header: stays.";
        for (system, expected) in [
            (json!(format!("{billing}\r\n\r\n{custom}")), json!(custom)),
            (
                json!([{"type":"text","text":billing},
                    {"type":"text","text":custom,"cache_control":{"type":"ephemeral"}}]),
                json!([{"type":"text","text":custom,"cache_control":{"type":"ephemeral"}}]),
            ),
            (
                json!([{"type":"text","text":format!("{billing}\n{billing}\n")},
                {"type":"text","text":format!("{billing}\n{custom}")}]),
                json!([{"type":"text","text":custom}]),
            ),
            (json!(billing), json!("")),
            (json!([{"type":"text","text":billing}]), json!([])),
        ] {
            let input = source(json!({"system":system}));
            let mut generic = transforms::anthropic_to_openai_chat(&input).unwrap();
            generic["messages"][0]["content"] = expected;
            assert_eq!(transform_buddy_claude_request(&input).unwrap(), generic);
        }
    }

    #[test]
    fn buddy_claude_billing_lookalikes_and_nonleading_metadata_remain_caller_data() {
        let billing = "x-anthropic-billing-header: cc_version=fixture; cc_entrypoint=cli;";
        for system in [
            json!(format!("Policy first.\n{billing}")),
            json!(format!("Quoted: {billing}")),
            json!("x-anthropic-billing-header: this is a caller instruction"),
            json!("x-anthropic-billing-header: cc_version=fixture; keep this instruction"),
            json!("x-anthropic-billing-header: cc_version=fixture; cc_entrypoint=cli; trailing instruction"),
            json!([{"type":"text","text":"Policy first."},{"type":"text","text":billing}]),
        ] {
            let input = source(json!({"system":system,"messages":[
                {"role":"user","content":billing},
                {"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"lookup","input":{"query":billing}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":billing}]}
            ]}));
            assert_eq!(transform_buddy_claude_request(&input).unwrap(),
                transforms::anthropic_to_openai_chat(&input).unwrap());
        }
    }
}
