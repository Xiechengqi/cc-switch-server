use std::hash::Hasher;
use std::sync::OnceLock;

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;
use twox_hash::XxHash64;

use super::ProxyError;

const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
const CCH_SEED: u64 = 0x6E52736AC806831E;
const BILLING_PREFIX: &str = "x-anthropic-billing-header:";
const BILLING_BLOCK_TEXT: &str =
    "x-anthropic-billing-header: cc_version=2.1.119.47e; cc_entrypoint=sdk-cli; cch=00000;\n\nYou are Claude Code, Anthropic's official CLI for Claude.";

pub(crate) fn apply_forward_contract(
    url: &mut String,
    body: &mut Bytes,
    client_headers: &HeaderMap,
) -> Result<(&'static str, String), ProxyError> {
    *url = ensure_claude_oauth_beta_query(url);
    if !body.is_empty() {
        let mut value = serde_json::from_slice(body).map_err(|error| {
            ProxyError::bad_request(format!(
                "claude oauth request body must be valid json: {error}"
            ))
        })?;
        value = sign_claude_oauth_messages_body(ensure_claude_oauth_billing_header_system(value));
        *body = Bytes::from(serde_json::to_vec(&value).map_err(|error| {
            ProxyError::bad_request(format!("claude oauth request body encode failed: {error}"))
        })?);
    }
    Ok(anthropic_beta_header(client_headers))
}

pub(super) fn anthropic_beta_header(client_headers: &HeaderMap) -> (&'static str, String) {
    (
        "anthropic-beta",
        build_anthropic_beta_value(client_headers, true),
    )
}

fn ensure_claude_oauth_beta_query(url: &str) -> String {
    let (base, query) = split_endpoint_and_query(url);
    match query {
        Some(query) if !query.is_empty() => {
            if query.split('&').any(|part| part == "beta=true") {
                url.to_string()
            } else {
                format!("{base}?beta=true&{query}")
            }
        }
        _ => format!("{base}?beta=true"),
    }
}

fn split_endpoint_and_query(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    }
}

fn sign_claude_oauth_messages_body(mut body: Value) -> Value {
    let Some(system) = body.get("system").and_then(|value| value.as_array()) else {
        return body;
    };
    let Some(first_block) = system.first() else {
        return body;
    };
    let Some(text) = first_block.get("text").and_then(|value| value.as_str()) else {
        return body;
    };
    if !text.starts_with(BILLING_PREFIX) {
        return body;
    }
    if !cch_signature_present(text) {
        return body;
    }

    let unsigned_text = replace_cch_value(text, "00000");
    body["system"][0]["text"] = Value::String(unsigned_text.clone());

    let Ok(unsigned_body) = serde_json::to_vec(&body) else {
        return body;
    };

    let mut hasher = XxHash64::with_seed(CCH_SEED);
    hasher.write(&unsigned_body);
    let cch = format!("{:05x}", hasher.finish() & 0xFFFFF);
    let signed_text = replace_cch_value(&unsigned_text, &cch);
    body["system"][0]["text"] = Value::String(signed_text);
    body
}

fn ensure_claude_oauth_billing_header_system(mut body: Value) -> Value {
    if body
        .get("system")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .is_some_and(|t| t.starts_with(BILLING_PREFIX))
    {
        return body;
    }

    let billing_block = serde_json::json!({"type": "text", "text": BILLING_BLOCK_TEXT});
    let existing_system = body.as_object_mut().and_then(|o| o.remove("system"));

    let mut blocks: Vec<Value> = match existing_system {
        Some(Value::String(s)) if !s.is_empty() => {
            vec![serde_json::json!({"type": "text", "text": s})]
        }
        Some(Value::String(_)) | Some(Value::Null) | None => Vec::new(),
        Some(Value::Array(arr)) => arr,
        _ => Vec::new(),
    };

    blocks.insert(0, billing_block);
    body["system"] = Value::Array(blocks);
    body
}

fn build_anthropic_beta_value(headers: &HeaderMap, is_claude_oauth: bool) -> String {
    let mut betas = vec![CLAUDE_CODE_BETA.to_string()];
    if is_claude_oauth {
        betas.push(CLAUDE_OAUTH_BETA.to_string());
    }

    if let Some(beta) = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
    {
        for item in beta
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            if !betas.iter().any(|existing| existing == item) {
                betas.push(item.to_string());
            }
        }
    }

    if is_claude_oauth && !betas.iter().any(|item| item == INTERLEAVED_THINKING_BETA) {
        betas.push(INTERLEAVED_THINKING_BETA.to_string());
    }

    betas.join(",")
}

fn cch_signature_present(text: &str) -> bool {
    find_cch_range(text).is_some()
}

fn replace_cch_value(text: &str, replacement: &str) -> String {
    let Some((start, end)) = find_cch_range(text) else {
        return text.to_string();
    };
    let mut output = String::with_capacity(text.len() - (end - start) + replacement.len());
    output.push_str(&text[..start]);
    output.push_str("cch=");
    output.push_str(replacement);
    output.push(';');
    output.push_str(&text[end..]);
    output
}

fn find_cch_range(text: &str) -> Option<(usize, usize)> {
    static NEEDLE: OnceLock<&'static str> = OnceLock::new();
    let needle = NEEDLE.get_or_init(|| "cch=");
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(needle) {
        let start = search_from + rel;
        if start > 0 {
            let prev = bytes[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                search_from = start + 1;
                continue;
            }
        }
        let hex_start = start + needle.len();
        if hex_start + 6 > bytes.len() {
            return None;
        }
        let hex_part = &text[hex_start..hex_start + 5];
        if hex_part.chars().all(|c| c.is_ascii_hexdigit()) && bytes[hex_start + 5] == b';' {
            return Some((start, hex_start + 6));
        }
        search_from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_beta_query_appends_or_merges() {
        assert_eq!(
            ensure_claude_oauth_beta_query("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
        assert_eq!(
            ensure_claude_oauth_beta_query(
                "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
            ),
            "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
        );
        assert_eq!(
            ensure_claude_oauth_beta_query("https://api.anthropic.com/v1/messages?foo=bar"),
            "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
        );
    }

    #[test]
    fn inject_billing_header_when_no_system() {
        let body = json!({"model": "claude-opus-4-7", "max_tokens": 16, "messages": []});
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 1);
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(BILLING_PREFIX));
    }

    #[test]
    fn inject_billing_header_prepends_when_string_system() {
        let body = json!({"model": "x", "max_tokens": 1, "system": "Be helpful.", "messages": []});
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 2);
        assert!(system[0]["text"]
            .as_str()
            .unwrap_or("")
            .starts_with(BILLING_PREFIX));
        assert_eq!(system[1]["text"].as_str().unwrap_or(""), "Be helpful.");
    }

    #[test]
    fn inject_billing_header_noop_when_billing_block_already_present() {
        let original_text =
            "x-anthropic-billing-header: cc_version=2.1; cch=abcde;\n\nYou are Claude Code.";
        let body = json!({
            "model": "x",
            "max_tokens": 1,
            "system": [{"type": "text", "text": original_text}],
            "messages": []
        });
        let result = ensure_claude_oauth_billing_header_system(body);
        let system = result["system"].as_array().expect("system must be array");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["text"].as_str().unwrap_or(""), original_text);
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_includes_oauth_marker() {
        let headers = HeaderMap::new();
        let beta = build_anthropic_beta_value(&headers, true);
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14"
        );
    }

    #[test]
    fn anthropic_beta_for_claude_oauth_merges_existing_markers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            axum::http::HeaderValue::from_static("custom-beta,claude-code-20250219"),
        );
        let beta = build_anthropic_beta_value(&headers, true);
        assert_eq!(
            beta,
            "claude-code-20250219,oauth-2025-04-20,custom-beta,interleaved-thinking-2025-05-14"
        );
    }

    #[test]
    fn sign_claude_oauth_messages_body_recomputes_cch() {
        let body = json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 16,
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=2.1.119.47e; cc_entrypoint=sdk-cli; cch=00000;\n\nYou are Claude Code."
            }],
            "messages": [{"role": "user", "content": "hi"}]
        });
        let signed = sign_claude_oauth_messages_body(body);
        let text = signed["system"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("cch="));
        assert!(!text.contains("cch=00000;"));
    }
}
