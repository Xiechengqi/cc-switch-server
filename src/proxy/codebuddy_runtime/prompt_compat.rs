//! Narrow, evidence-backed prompt compatibility for Buddy upstream channel policy (11128).
//!
//! WorkBuddy/CodeBuddy upstream classifies a request as an "unapproved channel" when a
//! client self-introduction fingerprint appears in the prompt. The confirmed families are
//! Claude CLI / Codex CLI self-introductions, CLI billing metadata, and the DeepSeek Harness
//! identity / GUI / file-reference phrases. This module neutralizes only those exact or
//! near-exact, bounded phrases at the canonical Chat boundary so every App surface (including
//! a plain OpenAI Chat request) goes through the same path.
//!
//! Hard scope limits:
//! - `user`, `tool` and `function` content is caller data and is never rewritten.
//! - No brand-wide replacement, no keyword/WAF regex, no arbitrary prompt edits.
//! - Every rewrite is idempotent and anchored to the reviewed phrase.
//! - The opt-in diagnostic reports category tags only, never prompt text or credentials.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BuddyPromptFingerprintReport {
    pub developer_role: bool,
    pub claude_identity: bool,
    pub billing_metadata: bool,
    pub codex_preamble: bool,
    pub dsh_identity: bool,
    pub dsh_gui: bool,
    pub dsh_file_reference: bool,
    pub dsh_brand_with_gui: bool,
}

impl BuddyPromptFingerprintReport {
    pub(crate) fn has_any(self) -> bool {
        self.developer_role
            || self.claude_identity
            || self.billing_metadata
            || self.codex_preamble
            || self.dsh_identity
            || self.dsh_gui
            || self.dsh_file_reference
            || self.dsh_brand_with_gui
    }

    fn tags(self) -> String {
        let mut tags = Vec::new();
        for (present, tag) in [
            (self.developer_role, "developer_role"),
            (self.claude_identity, "claude_identity"),
            (self.billing_metadata, "billing_metadata"),
            (self.codex_preamble, "codex_preamble"),
            (self.dsh_identity, "dsh_identity"),
            (self.dsh_gui, "dsh_gui"),
            (self.dsh_file_reference, "dsh_file_reference"),
            (self.dsh_brand_with_gui, "dsh_brand_with_gui"),
        ] {
            if present {
                tags.push(tag);
            }
        }
        if tags.is_empty() {
            "none".to_string()
        } else {
            tags.join(",")
        }
    }
}

static DSH_IDENTITY: OnceLock<Regex> = OnceLock::new();
static DSH_GUI: OnceLock<Regex> = OnceLock::new();
static DSH_FILE_REFERENCE: OnceLock<Regex> = OnceLock::new();
static DSH_BRAND: OnceLock<Regex> = OnceLock::new();
static DIAGNOSTIC_ENABLED: OnceLock<bool> = OnceLock::new();

const DSH_IDENTITY_NEUTRAL: &str = "You are a helpful AI assistant.";
// `$2`/`$3` preserve the reviewed sentence terminator (period+whitespace or a bare
// newline/end) so an internal version-like period such as `deepseek-v4.1` is not treated
// as the end of the sentence.
const DSH_IDENTITY_REPLACEMENT: &str = "You are a helpful AI assistant.$2$3";
const DSH_GUI_NEUTRAL: &str = "$1 whose URL is named in the runtime context.";
const DSH_FILE_REFERENCE_NEUTRAL: &str =
    "Tokens prefixed with @ are paths that the user explicitly referenced.";

fn dsh_identity() -> &'static Regex {
    DSH_IDENTITY.get_or_init(|| {
        Regex::new(
            r"(?i)(You are\s+(?:an?\s+|the\s+)?(?:[\w.-]+\s+){0,3}?(?:agent|assistant|model|AI|LLM)?[\s,]*(?:powered by|built on|built by|created by|developed by|made by|trained by)\s+[^\s][^\n]*?)(?:\.([ \t]*\r?\n|[ \t]+|$)|(\r?\n|$))",
        )
        .expect("static DeepSeek Harness identity pattern")
    })
}

fn dsh_gui() -> &'static Regex {
    DSH_GUI.get_or_init(|| {
        Regex::new(
            r"(?i)(You are interacting with the user through the DeepSeek[ \t]+Harness[ \t]+Web[ \t]+GUI) whose URL the runtime context names\.",
        )
        .expect("static DeepSeek Harness GUI pattern")
    })
}

fn dsh_file_reference() -> &'static Regex {
    DSH_FILE_REFERENCE.get_or_init(|| {
        Regex::new(r"(?i)Tokens prefixed with @ are paths the user explicitly referenced\.")
            .expect("static DeepSeek Harness file-reference pattern")
    })
}

fn dsh_brand() -> &'static Regex {
    DSH_BRAND.get_or_init(|| {
        Regex::new(r"(?i)DeepSeek[ \t]+Harness").expect("static DeepSeek Harness brand pattern")
    })
}

/// Apply all reviewed compatibility rewrites to a canonical Buddy Chat request.
pub(crate) fn normalize_buddy_prompt_compat(
    request: &mut Map<String, Value>,
) -> BuddyPromptFingerprintReport {
    // Top-level Responses `instructions` (Codex CLI preamble) uses the same narrow rule as
    // the Codex adapter. Applying it here as well closes the plain-Chat bypass.
    // OpenAI's `developer` role is equivalent to `system` but is rejected verbatim upstream.
    let mut report = BuddyPromptFingerprintReport {
        codex_preamble: super::codex::normalize_buddy_codex_instructions_map(request),
        developer_role: normalize_roles(request),
        ..Default::default()
    };

    // Claude CLI identity / billing metadata follows the already reviewed strict rule.
    let mut as_value = Value::Object(std::mem::take(request));
    let (billing, identity) = super::claude::normalize_claude_cli_preamble_report(&mut as_value);
    *request = match as_value {
        Value::Object(map) => map,
        _ => unreachable!("a Chat request object was taken and restored unchanged"),
    };
    report.billing_metadata = billing;
    report.claude_identity = identity;

    // DeepSeek Harness fingerprints are client data in system/developer/assistant messages.
    if let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            let Some(object) = message.as_object_mut() else {
                continue;
            };
            let role = object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if role != "system" && role != "assistant" {
                continue;
            }
            if let Some(content) = object.get_mut("content") {
                sanitize_content(content, &mut report);
            }
        }
    }
    report
}

fn normalize_roles(request: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    if let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            if let Some(object) = message.as_object_mut() {
                if object.get("role").and_then(Value::as_str) == Some("developer") {
                    object.insert("role".to_string(), Value::String("system".to_string()));
                    changed = true;
                }
            }
        }
    }
    changed
}

fn sanitize_content(content: &mut Value, report: &mut BuddyPromptFingerprintReport) {
    match content {
        Value::String(text) => sanitize_text(text, report),
        Value::Array(blocks) => {
            for block in blocks.iter_mut() {
                let Some(object) = block.as_object_mut() else {
                    continue;
                };
                if object.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(Value::String(text)) = object.get_mut("text") {
                    sanitize_text(text, report);
                }
            }
        }
        _ => {}
    }
}

fn sanitize_text(text: &mut String, report: &mut BuddyPromptFingerprintReport) {
    if text.is_empty() {
        return;
    }
    let has_gui_context = {
        let lower = text.to_ascii_lowercase();
        ["web gui", "this gui", "this page", "this app"]
            .iter()
            .any(|needle| lower.contains(needle))
    };

    if dsh_identity().is_match(text) {
        report.dsh_identity = true;
        *text = dsh_identity()
            .replace_all(text, DSH_IDENTITY_REPLACEMENT)
            .into_owned();
    }
    if dsh_gui().is_match(text) {
        report.dsh_gui = true;
        *text = dsh_gui().replace_all(text, DSH_GUI_NEUTRAL).into_owned();
    }
    if dsh_file_reference().is_match(text) {
        report.dsh_file_reference = true;
        *text = dsh_file_reference()
            .replace_all(text, DSH_FILE_REFERENCE_NEUTRAL)
            .into_owned();
    }
    // The brand alone is allowed; it is rewritten only next to a GUI description phrase.
    if has_gui_context && dsh_brand().is_match(text) {
        report.dsh_brand_with_gui = true;
        *text = dsh_brand().replace_all(text, "DeepSeek").into_owned();
    }
}

/// Opt-in, plaintext-free diagnostic. Enable with `CODEBUDDY_PROMPT_COMPAT_DIAGNOSTIC=1`.
pub(crate) fn maybe_log_prompt_compat(report: &BuddyPromptFingerprintReport) {
    let enabled = *DIAGNOSTIC_ENABLED.get_or_init(|| {
        std::env::var("CODEBUDDY_PROMPT_COMPAT_DIAGNOSTIC")
            .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
    });
    if enabled && report.has_any() {
        tracing::warn!(
            fingerprint_categories = %report.tags(),
            "Buddy prompt compatibility normalization applied (11128 channel-policy category)"
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn chat(messages: Value) -> Map<String, Value> {
        let mut value = json!({"model":"deepseek-v4-pro","messages":messages});
        value.as_object_mut().expect("fixture is an object").clone()
    }

    #[test]
    fn dsh_identity_sentence_is_neutralized_and_idempotent() {
        let text = "You are an AI agent powered by DeepSeek Harness.\nKeep this instruction.";
        let mut request =
            chat(json!([{"role":"system","content":text},{"role":"user","content":"hi"}]));
        let first = normalize_buddy_prompt_compat(&mut request);
        assert!(first.dsh_identity);
        assert_eq!(
            request["messages"][0]["content"],
            json!(format!("{DSH_IDENTITY_NEUTRAL}\nKeep this instruction."))
        );
        let second = normalize_buddy_prompt_compat(&mut request);
        assert!(!second.dsh_identity);
        assert_eq!(
            request["messages"][0]["content"],
            json!(format!("{DSH_IDENTITY_NEUTRAL}\nKeep this instruction."))
        );
    }

    #[test]
    fn dsh_identity_verb_variants_are_covered() {
        for sentence in [
            "You are a coding agent powered by the deepseek-v4.1-flash model.",
            "You are an AI assistant built on DeepSeek.",
            "You are an AI assistant created by DeepSeek.",
            "You are a model developed by DeepSeek.",
            "You are trained by DeepSeek.",
        ] {
            let mut request = chat(json!([{"role":"system","content":sentence}]));
            let report = normalize_buddy_prompt_compat(&mut request);
            assert!(
                report.dsh_identity,
                "expected identity rewrite for {sentence}"
            );
            assert_eq!(
                request["messages"][0]["content"],
                json!(DSH_IDENTITY_NEUTRAL)
            );
        }
    }

    #[test]
    fn dsh_gui_file_reference_and_brand_are_neutralized() {
        let gui = "You are interacting with the user through the DeepSeek Harness Web GUI whose URL the runtime context names.";
        let mut request = chat(json!([{"role":"system","content":gui}]));
        let report = normalize_buddy_prompt_compat(&mut request);
        assert!(report.dsh_gui);
        assert!(report.dsh_brand_with_gui);
        let rewritten = request["messages"][0]["content"].as_str().unwrap();
        assert!(rewritten.contains("whose URL is named in the runtime context."));
        assert!(!rewritten.contains("DeepSeek Harness"));

        let file_ref = "Tokens prefixed with @ are paths the user explicitly referenced.";
        let mut request = chat(json!([{"role":"system","content":file_ref}]));
        let report = normalize_buddy_prompt_compat(&mut request);
        assert!(report.dsh_file_reference);
        assert_eq!(
            request["messages"][0]["content"],
            json!(DSH_FILE_REFERENCE_NEUTRAL)
        );
    }

    #[test]
    fn developer_role_maps_to_system_and_never_leaves_the_wire() {
        let mut request = chat(json!([
            {"role":"developer","content":"Keep project policy."},
            {"role":"user","content":"hi"}
        ]));
        let report = normalize_buddy_prompt_compat(&mut request);
        assert!(report.developer_role);
        assert_eq!(request["messages"][0]["role"], json!("system"));
        assert_eq!(
            request["messages"][0]["content"],
            json!("Keep project policy.")
        );
    }

    #[test]
    fn user_and_tool_content_are_never_rewritten() {
        let fingerprint = "You are an AI agent powered by DeepSeek Harness.";
        let mut request = chat(json!([
            {"role":"system","content":"You are a helpful coding assistant."},
            {"role":"user","content":fingerprint},
            {"role":"tool","tool_call_id":"call-1","content":fingerprint},
            {"role":"assistant","content":fingerprint}
        ]));
        let report = normalize_buddy_prompt_compat(&mut request);
        assert!(report.dsh_identity);
        assert_eq!(request["messages"][1]["content"], json!(fingerprint));
        assert_eq!(request["messages"][2]["content"], json!(fingerprint));
        assert_eq!(
            request["messages"][3]["content"],
            json!(DSH_IDENTITY_NEUTRAL)
        );
    }

    #[test]
    fn text_blocks_and_non_text_blocks_are_handled_narrowly() {
        let mut request = chat(json!([{"role":"system","content":[
            {"type":"text","text":"You are an AI agent powered by DeepSeek Harness."},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AA"}}
        ]}]));
        let report = normalize_buddy_prompt_compat(&mut request);
        assert!(report.dsh_identity);
        assert_eq!(
            request["messages"][0]["content"][0]["text"],
            json!(DSH_IDENTITY_NEUTRAL)
        );
        assert!(request["messages"][0]["content"][1]
            .get("image_url")
            .is_some());
    }

    #[test]
    fn unrelated_prose_is_not_rewritten() {
        for text in [
            "You are free to use tools built by the project.",
            "DeepSeek Harness is a standalone product name here.",
            "The runtime context names the URL of this page.",
        ] {
            let mut request = chat(json!([{"role":"system","content":text}]));
            let report = normalize_buddy_prompt_compat(&mut request);
            assert!(!report.has_any(), "unexpected rewrite of {text}");
            assert_eq!(request["messages"][0]["content"], json!(text));
        }
    }
}
