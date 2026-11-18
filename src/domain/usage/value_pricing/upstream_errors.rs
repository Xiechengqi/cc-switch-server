//! Billing evidence from a complete first-party HTTP error response. A status
//! code or a Provider label alone never proves that generation was rejected.
use serde_json::Value;

pub fn classify_upstream_error(
    profile: &str,
    endpoint: &str,
    status: u16,
    body: &[u8],
) -> Option<&'static str> {
    let url = url::Url::parse(endpoint).ok()?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let protocol = match (profile, host) {
        ("claude.official_oauth" | "claude.anthropic_api_key", "api.anthropic.com") => "anthropic",
        ("codex.openai_api_key", "api.openai.com") => "openai",
        ("claude.openai_oauth" | "codex.openai_oauth", "chatgpt.com")
            if url.path().starts_with("/backend-api/") =>
        {
            "openai"
        }
        ("gemini.google_api_key", "generativelanguage.googleapis.com") => "gemini",
        _ => return None,
    };
    if !matches!(status, 400 | 403 | 404 | 413 | 422 | 429 | 529 | 500..=599) {
        return None;
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = value.get("error")?.as_object()?;
    let valid = error.get("message").is_some_and(Value::is_string)
        && match protocol {
            "anthropic" => {
                value.get("type").and_then(Value::as_str) == Some("error")
                    && error.get("type").is_some_and(Value::is_string)
            }
            "openai" => {
                error.get("type").is_some_and(Value::is_string)
                    || error.get("code").is_some_and(Value::is_string)
            }
            "gemini" => {
                error.get("code").and_then(Value::as_u64) == Some(u64::from(status))
                    && error.get("status").is_some_and(Value::is_string)
            }
            _ => false,
        };
    if !valid
        || contains_usage(&value)
        || [
            "output",
            "choices",
            "content",
            "candidates",
            "data",
            "response",
        ]
        .iter()
        .any(|key| value.get(*key).is_some_and(|v| !v.is_null()))
    {
        return None;
    }
    Some(
        if matches!(status, 400 | 403 | 404 | 413 | 422 | 429 | 529) {
            "upstream_declared_rejection"
        } else {
            "upstream_error_no_output"
        },
    )
}

fn contains_usage(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            (matches!(
                key.as_str(),
                "usage"
                    | "usageMetadata"
                    | "usage_metadata"
                    | "input_tokens"
                    | "output_tokens"
                    | "prompt_tokens"
                    | "completion_tokens"
                    | "total_tokens"
                    | "resources"
            ) && !value.is_null())
                || contains_usage(value)
        }),
        Value::Array(items) => items.iter().any(contains_usage),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_errors_require_official_endpoint_protocol_and_no_usage() {
        let body = br#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#;
        let classify = |status, body: &[u8]| {
            classify_upstream_error(
                "claude.anthropic_api_key",
                "https://api.anthropic.com/v1/messages",
                status,
                body,
            )
        };
        for status in [400, 403, 404, 413, 422, 429, 529] {
            assert_eq!(classify(status, body), Some("upstream_declared_rejection"));
        }
        assert_eq!(classify(503, body), Some("upstream_error_no_output"));
        assert_eq!(classify(200, body), None);
        assert_eq!(classify(429, b"<html>gateway timeout</html>"), None);
        assert_eq!(classify(429, br#"{"error":{"message":"busy"}}"#), None);
        assert_eq!(classify(429, br#"{"type":"error","error":{"type":"x","message":"busy"},"usage":{"input_tokens":0}}"#), None);
        assert_eq!(
            classify(
                503,
                br#"{"type":"error","error":{"type":"x","message":"busy"},"output":[]}"#
            ),
            None
        );
        for endpoint in [
            "https://relay.example/v1/messages",
            "http://api.anthropic.com/v1/messages",
            "https://api.anthropic.com.evil.test/v1/messages",
        ] {
            assert_eq!(
                classify_upstream_error("claude.anthropic_api_key", endpoint, 429, body),
                None
            );
        }
        assert_eq!(
            classify_upstream_error(
                "claude.custom_http",
                "https://api.anthropic.com/v1/messages",
                429,
                body
            ),
            None
        );
        assert_eq!(
            classify_upstream_error(
                "codex.openai_oauth",
                "https://chatgpt.com/backend-api/codex/responses",
                429,
                br#"{"error":{"type":"rate_limit_error","message":"busy"}}"#
            ),
            Some("upstream_declared_rejection")
        );
        assert_eq!(
            classify_upstream_error(
                "gemini.google_api_key",
                "https://generativelanguage.googleapis.com/v1/models/x:generateContent",
                429,
                br#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"busy"}}"#
            ),
            Some("upstream_declared_rejection")
        );
    }
}
