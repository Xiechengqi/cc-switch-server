use serde_json::Value;

const ERROR_INFO_TYPE: &str = "type.googleapis.com/google.rpc.ErrorInfo";
const RETRY_INFO_TYPE: &str = "type.googleapis.com/google.rpc.RetryInfo";
pub(crate) const MAX_SHORT_RETRY_DELAY_MS: u64 = 2_000;
const MAX_PARSED_RETRY_DELAY_MS: u64 = 8 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AntigravityLimitKind {
    RateLimit,
    ModelCapacity,
}

impl AntigravityLimitKind {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit_exceeded",
            Self::ModelCapacity => "model_capacity_exhausted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AntigravityRetryInfo {
    pub kind: AntigravityLimitKind,
    pub retry_delay_ms: u64,
    pub model: String,
}

impl AntigravityRetryInfo {
    pub(crate) fn is_short_delay(&self) -> bool {
        self.retry_delay_ms <= MAX_SHORT_RETRY_DELAY_MS
    }
}

pub(crate) fn parse_google_rpc_retry(
    http_status: u16,
    body: &[u8],
) -> Option<AntigravityRetryInfo> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let error = document.get("error")?.as_object()?;
    let rpc_status = error.get("status")?.as_str()?;
    let expected = match (http_status, rpc_status) {
        (429, "RESOURCE_EXHAUSTED") => ("RATE_LIMIT_EXCEEDED", AntigravityLimitKind::RateLimit),
        (503, "UNAVAILABLE") => (
            "MODEL_CAPACITY_EXHAUSTED",
            AntigravityLimitKind::ModelCapacity,
        ),
        _ => return None,
    };
    let details = error.get("details")?.as_array()?;
    let mut matched_reason = false;
    let mut model = None;
    let mut retry_delay_ms = None;
    for detail in details.iter().filter_map(Value::as_object) {
        match detail.get("@type").and_then(Value::as_str) {
            Some(ERROR_INFO_TYPE) => {
                if detail.get("reason").and_then(Value::as_str) == Some(expected.0) {
                    matched_reason = true;
                    model = detail
                        .get("metadata")
                        .and_then(Value::as_object)
                        .and_then(|metadata| metadata.get("model"))
                        .and_then(Value::as_str)
                        .and_then(normalize_model_scope);
                }
            }
            Some(RETRY_INFO_TYPE) => {
                retry_delay_ms = detail
                    .get("retryDelay")
                    .and_then(Value::as_str)
                    .and_then(parse_protobuf_duration_ms);
            }
            _ => {}
        }
    }
    Some(AntigravityRetryInfo {
        kind: expected.1,
        retry_delay_ms: retry_delay_ms?,
        model: model.filter(|_| matched_reason)?,
    })
}

fn normalize_model_scope(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 160 {
        return None;
    }
    let value = if let Some(model) = value.strip_prefix("models/") {
        model
    } else if let Some((_, model)) = value.rsplit_once("/models/") {
        model
    } else if value.contains('/') {
        return None;
    } else {
        value
    }
    .trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
}

fn parse_protobuf_duration_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    let seconds = value.strip_suffix('s')?;
    if seconds.is_empty() || seconds.starts_with('-') || seconds.starts_with('+') {
        return None;
    }
    let (whole, fractional) = seconds.split_once('.').unwrap_or((seconds, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fractional.len() > 9
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole_ms = whole.parse::<u64>().ok()?.checked_mul(1_000)?;
    let fractional_ms = if fractional.is_empty() {
        0
    } else {
        let nanos = format!("{fractional:0<9}").parse::<u64>().ok()?;
        nanos.saturating_add(999_999) / 1_000_000
    };
    let delay = whole_ms.checked_add(fractional_ms)?;
    (delay <= MAX_PARSED_RETRY_DELAY_MS).then_some(delay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(status: &str, reason: &str, model: &str, delay: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "error": {
                "status": status,
                "details": [
                    {
                        "@type": ERROR_INFO_TYPE,
                        "reason": reason,
                        "metadata": {"model": model}
                    },
                    {
                        "@type": RETRY_INFO_TYPE,
                        "retryDelay": delay
                    }
                ]
            }
        }))
        .unwrap()
    }

    #[test]
    fn parses_exact_rate_limit_and_capacity_contracts() {
        assert_eq!(
            parse_google_rpc_retry(
                429,
                &body(
                    "RESOURCE_EXHAUSTED",
                    "RATE_LIMIT_EXCEEDED",
                    "models/gemini-2.5-pro",
                    "0.201506475s"
                )
            ),
            Some(AntigravityRetryInfo {
                kind: AntigravityLimitKind::RateLimit,
                retry_delay_ms: 202,
                model: "gemini-2.5-pro".to_string(),
            })
        );
        assert_eq!(
            parse_google_rpc_retry(
                503,
                &body(
                    "UNAVAILABLE",
                    "MODEL_CAPACITY_EXHAUSTED",
                    "publishers/anthropic/models/claude-sonnet-4-5",
                    "2s"
                )
            ),
            Some(AntigravityRetryInfo {
                kind: AntigravityLimitKind::ModelCapacity,
                retry_delay_ms: 2_000,
                model: "claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn rejects_mismatched_or_ambiguous_google_errors() {
        for (http_status, status, reason) in [
            (429, "UNAVAILABLE", "MODEL_CAPACITY_EXHAUSTED"),
            (503, "RESOURCE_EXHAUSTED", "RATE_LIMIT_EXCEEDED"),
            (429, "RESOURCE_EXHAUSTED", "QUOTA_EXHAUSTED"),
        ] {
            assert!(parse_google_rpc_retry(
                http_status,
                &body(status, reason, "gemini-2.5-pro", "1s")
            )
            .is_none());
        }
        assert!(parse_google_rpc_retry(429, b"not json").is_none());
        assert!(parse_google_rpc_retry(
            429,
            &body(
                "RESOURCE_EXHAUSTED",
                "RATE_LIMIT_EXCEEDED",
                "invalid model/value",
                "1s"
            )
        )
        .is_none());
    }

    #[test]
    fn protobuf_duration_is_strict_bounded_and_rounds_up() {
        assert_eq!(parse_protobuf_duration_ms("0.000000001s"), Some(1));
        assert_eq!(parse_protobuf_duration_ms("1.001s"), Some(1_001));
        for invalid in ["", "1", "-1s", "+1s", "1ms", "1.0000000000s", "999999999s"] {
            assert_eq!(parse_protobuf_duration_ms(invalid), None, "{invalid}");
        }
    }
}
