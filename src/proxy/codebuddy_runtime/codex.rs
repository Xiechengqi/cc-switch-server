use serde_json::Value;

use crate::domain::codebuddy::BUDDY_CODEX_CLI_PREAMBLE;

const BUDDY_CODEX_PREAMBLE: &str =
    "You are a helpful coding assistant. You are expected to be precise, safe, and helpful.";

/// Explicitly authorized prompt-compatibility exception for the exact generated
/// Codex CLI 0.161.0 opening paragraph observed to trigger upstream code 11128.
/// Run on Responses instructions before role conversion: input system/developer
/// items, custom instructions, history and tools must never be scanned. This is
/// separate from protocol normalization and does not change outbound identity.
pub(crate) fn normalize_buddy_codex_instructions(input: &mut Value) {
    let Some(Value::String(text)) = input.get_mut("instructions") else {
        return;
    };
    let Some(suffix) = text.strip_prefix(BUDDY_CODEX_CLI_PREAMBLE) else {
        return;
    };
    if suffix.is_empty() || suffix.starts_with("\n\n") || suffix.starts_with("\r\n\r\n") {
        // Retain the behavioral expectation and every byte after this paragraph.
        text.replace_range(..BUDDY_CODEX_CLI_PREAMBLE.len(), BUDDY_CODEX_PREAMBLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn buddy_codex_preamble_preserves_context_history_tools_and_behavioral_expectation() {
        for suffix in [
            "",
            "\n\n# AGENTS.md\nPreserve project policy.",
            "\r\n\r\nCustom instructions.",
        ] {
            let mut input = json!({
                "instructions":format!("{BUDDY_CODEX_CLI_PREAMBLE}{suffix}"),
                "input":[
                    {"role":"developer","content":BUDDY_CODEX_CLI_PREAMBLE},
                    {"role":"user","content":BUDDY_CODEX_CLI_PREAMBLE},
                    {"type":"function_call","call_id":"call-1","name":"lookup","arguments":json!({"q":BUDDY_CODEX_CLI_PREAMBLE}).to_string()},
                    {"type":"function_call_output","call_id":"call-1","output":BUDDY_CODEX_CLI_PREAMBLE},
                    {"role":"assistant","content":BUDDY_CODEX_CLI_PREAMBLE}
                ],
                "tools":[{"type":"function","name":"lookup","description":BUDDY_CODEX_CLI_PREAMBLE,"parameters":{"type":"object"}}],
                "stream":true,"reasoning":{"effort":"low"},"max_output_tokens":256
            });
            let mut expected = input.clone();
            expected["instructions"] = json!(format!("{BUDDY_CODEX_PREAMBLE}{suffix}"));
            normalize_buddy_codex_instructions(&mut input);
            assert_eq!(input, expected);
            // Repeated preparation must not remove additional caller text.
            normalize_buddy_codex_instructions(&mut input);
            assert_eq!(input, expected);
        }
    }

    #[test]
    fn buddy_codex_nonleading_partial_or_nonstring_instructions_remain_unchanged() {
        for instructions in [
            Value::Null,
            json!([{"type":"input_text","text":BUDDY_CODEX_CLI_PREAMBLE}]),
            json!(format!(
                "Custom instructions.\n\n{BUDDY_CODEX_CLI_PREAMBLE}"
            )),
            json!(format!(" {BUDDY_CODEX_CLI_PREAMBLE}")),
            json!(format!("{BUDDY_CODEX_CLI_PREAMBLE} This is quoted text.")),
            json!(format!("{BUDDY_CODEX_CLI_PREAMBLE}\nKeep this exact line.")),
            json!(BUDDY_CODEX_CLI_PREAMBLE.replace("Codex CLI", "another CLI")),
            json!("You are Codex, an agent based on GPT-5."),
        ] {
            let mut input = json!({"instructions":instructions,"input":"ping"});
            let expected = input.clone();
            normalize_buddy_codex_instructions(&mut input);
            assert_eq!(input, expected);
        }
        let mut input = json!({"input":[{"role":"system","content":BUDDY_CODEX_CLI_PREAMBLE}]});
        let expected = input.clone();
        normalize_buddy_codex_instructions(&mut input);
        assert_eq!(input, expected);
    }
}
