use axum::http::HeaderMap;
use serde_json::Value;

const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
const HELPER_MODEL: &str = "claude-haiku-4-5-20251001";

const HELPER_CORE_WITH_REDACTION: &str = "oauth-2025-04-20,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05";
const HELPER_CORE_WITHOUT_REDACTION: &str = "oauth-2025-04-20,interleaved-thinking-2025-05-14,thinking-token-count-2026-05-13,context-management-2025-06-27,prompt-caching-scope-2026-01-05";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeClientClass {
    NativeCli,
    SdkCli,
    VsCode,
    NativeHelper,
    ThirdPartyAnthropic,
}

impl ClaudeClientClass {
    pub(crate) fn is_confirmed_native(self) -> bool {
        !matches!(self, Self::ThirdPartyAnthropic)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NativeCli => "native_cli",
            Self::SdkCli => "sdk_cli",
            Self::VsCode => "vscode",
            Self::NativeHelper => "native_helper",
            Self::ThirdPartyAnthropic => "third_party_anthropic",
        }
    }
}

pub(crate) fn detect_claude_client(
    headers: &HeaderMap,
    body: &Value,
    count_tokens: bool,
) -> ClaudeClientClass {
    let user_agent = header(headers, "user-agent").unwrap_or_default();
    let Some(entrypoint) = parse_native_user_agent(user_agent) else {
        return ClaudeClientClass::ThirdPartyAnthropic;
    };
    if header(headers, "x-app") != Some("cli") {
        return ClaudeClientClass::ThirdPartyAnthropic;
    }

    let metadata = metadata_is_native(body, count_tokens);
    let beta = normalized_beta_header(headers);
    let standard = beta.split(',').any(|item| item == CLAUDE_CODE_BETA) && metadata;
    if standard {
        return match entrypoint {
            "cli" => ClaudeClientClass::NativeCli,
            "sdk-cli" => ClaudeClientClass::SdkCli,
            "claude-vscode" => ClaudeClientClass::VsCode,
            _ => ClaudeClientClass::ThirdPartyAnthropic,
        };
    }

    if entrypoint == "cli" && metadata && !count_tokens && helper_profile_matches(&beta, body) {
        return ClaudeClientClass::NativeHelper;
    }
    ClaudeClientClass::ThirdPartyAnthropic
}

fn parse_native_user_agent(user_agent: &str) -> Option<&str> {
    let user_agent = user_agent.trim();
    let version = user_agent.strip_prefix("claude-cli/")?;
    let (version, details) = version.split_once(" (external, ")?;
    if version.is_empty()
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    let details = details.strip_suffix(')')?;
    let entrypoint = details.split(',').next()?.trim();
    matches!(entrypoint, "cli" | "sdk-cli" | "claude-vscode").then_some(entrypoint)
}

fn metadata_is_native(body: &Value, count_tokens: bool) -> bool {
    if count_tokens {
        return true;
    }
    let Some(metadata) = body.get("metadata").and_then(Value::as_object) else {
        return false;
    };
    metadata
        .get("user_id")
        .and_then(Value::as_str)
        .is_some_and(valid_user_id)
        || metadata
            .get("session_id")
            .or_else(|| metadata.get("sessionId"))
            .and_then(Value::as_str)
            .is_some_and(valid_identifier)
}

fn valid_user_id(value: &str) -> bool {
    let value = value.trim();
    value.contains("_session_") && value.len() <= 512 && valid_identifier(value)
}

fn valid_identifier(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 512
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'))
}

fn helper_profile_matches(beta: &str, body: &Value) -> bool {
    if body.get("model").and_then(Value::as_str) != Some(HELPER_MODEL) {
        return false;
    }
    let core = beta == HELPER_CORE_WITH_REDACTION
        || beta == HELPER_CORE_WITHOUT_REDACTION
        || helper_trailing_profile(beta, HELPER_CORE_WITH_REDACTION)
        || helper_trailing_profile(beta, HELPER_CORE_WITHOUT_REDACTION);
    if !core {
        return false;
    }
    let max_tokens = body.get("max_tokens").and_then(Value::as_u64);
    let messages = body.get("messages").and_then(Value::as_array);
    max_tokens.is_some_and(|value| value > 0 && value <= 8_192)
        && messages.is_some_and(|messages| !messages.is_empty())
        && body
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_some_and(|tools| tools.len() <= 2))
}

fn helper_trailing_profile(beta: &str, core: &str) -> bool {
    let Some(trailing) = beta
        .strip_prefix(core)
        .and_then(|value| value.strip_prefix(','))
    else {
        return false;
    };
    matches!(
        trailing,
        "structured-outputs-2025-12-15"
            | "structured-outputs-2025-12-15,fallback-credit-2026-06-01"
            | "advisor-tool-2026-03-01,structured-outputs-2025-12-15,cache-diagnosis-2026-04-07"
    )
}

fn normalized_beta_header(headers: &HeaderMap) -> String {
    headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn native_headers(beta: &'static str, entrypoint: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-app", HeaderValue::from_static("cli"));
        headers.insert(
            "user-agent",
            HeaderValue::from_str(&format!("claude-cli/2.1.258 (external, {entrypoint})")).unwrap(),
        );
        headers.insert("anthropic-beta", HeaderValue::from_static(beta));
        headers
    }

    #[test]
    fn standard_native_requires_all_strong_signals() {
        let body = json!({"metadata":{"user_id":"user_a_account__session_abc"}});
        let headers = native_headers(CLAUDE_CODE_BETA, "cli");
        assert_eq!(
            detect_claude_client(&headers, &body, false),
            ClaudeClientClass::NativeCli
        );

        let mut no_x_app = headers.clone();
        no_x_app.remove("x-app");
        assert_eq!(
            detect_claude_client(&no_x_app, &body, false),
            ClaudeClientClass::ThirdPartyAnthropic
        );
        assert_eq!(
            detect_claude_client(&headers, &json!({}), false),
            ClaudeClientClass::ThirdPartyAnthropic
        );
    }

    #[test]
    fn user_agent_alone_never_confirms_native() {
        let headers = native_headers("", "cli");
        assert_eq!(
            detect_claude_client(&headers, &json!({}), false),
            ClaudeClientClass::ThirdPartyAnthropic
        );
    }

    #[test]
    fn exact_markerless_helper_profile_is_confirmed() {
        let headers = native_headers(HELPER_CORE_WITH_REDACTION, "cli");
        let body = json!({
            "model": HELPER_MODEL,
            "max_tokens": 1024,
            "metadata": {"user_id":"user_a_account__session_abc"},
            "messages": [{"role":"user","content":"classify"}]
        });
        assert_eq!(
            detect_claude_client(&headers, &body, false),
            ClaudeClientClass::NativeHelper
        );

        let mut near_miss = headers;
        near_miss.insert(
            "anthropic-beta",
            HeaderValue::from_static("oauth-2025-04-20,interleaved-thinking-2025-05-14"),
        );
        assert_eq!(
            detect_claude_client(&near_miss, &body, false),
            ClaudeClientClass::ThirdPartyAnthropic
        );
    }
}
