use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::Value;

const DATELINE_GATE_ENV: &str = "CC_SWITCH_CLAUDE_DATELINE_NORMALIZATION";

pub(super) fn dateline_normalization_enabled() -> bool {
    std::env::var(DATELINE_GATE_ENV)
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
}

pub(super) fn normalize_anthropic_dateline(body: &mut Value) -> usize {
    let Some(object) = body.as_object_mut() else {
        return 0;
    };
    let mut hits = 0;
    if let Some(system) = object.get_mut("system") {
        match system {
            Value::String(text) => hits += normalize_text(text),
            Value::Array(blocks) => {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("text") {
                        continue;
                    }
                    if let Some(Value::String(text)) = block.get_mut("text") {
                        hits += normalize_text(text);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            let Some(content) = message.get_mut("content") else {
                continue;
            };
            match content {
                Value::String(text) => hits += normalize_reminder_blocks(text),
                Value::Array(blocks) => {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) != Some("text") {
                            continue;
                        }
                        if let Some(Value::String(text)) = block.get_mut("text") {
                            hits += normalize_reminder_blocks(text);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    hits
}

fn dateline_hyphen_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"Today['’ʼʹ]s date is (\d{4})-(\d{2})-(\d{2})\.")
            .expect("valid Anthropic hyphen dateline regex")
    })
}

fn dateline_slash_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"Today['’ʼʹ]s date is (\d{4})/(\d{2})/(\d{2})\.")
            .expect("valid Anthropic slash dateline regex")
    })
}

fn reminder_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?s)<system-reminder>.*?</system-reminder>")
            .expect("valid system-reminder regex")
    })
}

fn normalize_text(text: &mut String) -> usize {
    if !text.contains("date is ") {
        return 0;
    }
    let mut hits = 0;
    let mut normalized = text.clone();
    for regex in [dateline_hyphen_regex(), dateline_slash_regex()] {
        normalized = regex
            .replace_all(&normalized, |captures: &Captures<'_>| {
                let replacement = format!(
                    "Today's date is {}-{}-{}.",
                    &captures[1], &captures[2], &captures[3]
                );
                if captures.get(0).map(|value| value.as_str()) != Some(replacement.as_str()) {
                    hits += 1;
                }
                replacement
            })
            .into_owned();
    }
    if hits > 0 {
        *text = normalized;
    }
    hits
}

fn normalize_reminder_blocks(text: &mut String) -> usize {
    if !text.contains("<system-reminder>") {
        return 0;
    }
    let mut hits = 0;
    let normalized = reminder_regex()
        .replace_all(text, |captures: &Captures<'_>| {
            let mut block = captures[0].to_string();
            hits += normalize_text(&mut block);
            block
        })
        .into_owned();
    if hits > 0 {
        *text = normalized;
    }
    hits
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn normalizes_only_system_and_system_reminders() {
        let mut body = json!({
            "system": [{"type": "text", "text": "Today’s date is 2026/08/26."}],
            "messages": [
                {"role": "user", "content": "Todayʼs date is 2026/08/26. <system-reminder>Todayʹs date is 2026/08/26.</system-reminder>"},
                {"role": "user", "content": [
                    {"type": "text", "text": "Today’s date is 2026/08/26."},
                    {"type": "tool_result", "content": "<system-reminder>Today’s date is 2026/08/26.</system-reminder>"}
                ]}
            ]
        });
        assert_eq!(super::normalize_anthropic_dateline(&mut body), 2);
        assert_eq!(body["system"][0]["text"], "Today's date is 2026-08-26.");
        assert!(body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("Todayʼs date is 2026/08/26."));
        assert!(body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Today's date is 2026-08-26."));
        assert_eq!(
            body["messages"][1]["content"][0]["text"],
            "Today’s date is 2026/08/26."
        );
        assert!(body["messages"][1]["content"][1]["content"]
            .as_str()
            .unwrap()
            .contains("2026/08/26"));
    }

    #[test]
    fn mixed_separators_and_canonical_text_are_unchanged() {
        let mut body = json!({"system": "Today's date is 2026-08-26. Today’s date is 2026-08/26."});
        let original = body.clone();
        assert_eq!(super::normalize_anthropic_dateline(&mut body), 0);
        assert_eq!(body, original);
    }
}
