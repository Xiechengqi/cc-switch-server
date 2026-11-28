use super::ZenWire;
use crate::proxy::{anthropic_semantics, response_semantics, ProxyError};
use serde_json::Value;
use std::collections::BTreeSet;

fn invalid() -> ProxyError {
    ProxyError::bad_gateway("OpenCode Zen invalid JSON response or terminal")
}
fn string(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}
fn function(call: &Value, chat: bool) -> bool {
    let function = if chat {
        call.get("function").unwrap_or(&Value::Null)
    } else {
        call
    };
    string(call, if chat { "id" } else { "call_id" })
        && string(function, "name")
        && function
            .get("arguments")
            .and_then(Value::as_str)
            .is_some_and(|s| serde_json::from_str::<Value>(s).is_ok_and(|v| v.is_object()))
}
fn usage(value: &Value, wire: ZenWire) -> bool {
    let Some(usage) = value.get("usage").filter(|v| !v.is_null()) else {
        return true;
    };
    let required = if wire == ZenWire::Chat {
        ["prompt_tokens", "completion_tokens"]
    } else {
        ["input_tokens", "output_tokens"]
    };
    usage.is_object()
        && required
            .iter()
            .all(|key| usage.get(*key).and_then(Value::as_u64).is_some())
        && usage
            .get("total_tokens")
            .is_none_or(|v| v.as_u64().is_some())
        && [
            "prompt_tokens_details",
            "completion_tokens_details",
            "input_tokens_details",
            "output_tokens_details",
        ]
        .iter()
        .all(|key| {
            usage.get(*key).filter(|v| !v.is_null()).is_none_or(|v| {
                v.is_object()
                    && [
                        "cached_tokens",
                        "reasoning_tokens",
                        "audio_tokens",
                        "accepted_prediction_tokens",
                        "rejected_prediction_tokens",
                    ]
                    .iter()
                    .all(|k| v.get(*k).is_none_or(|n| n.as_u64().is_some()))
            })
        })
}

pub(in crate::proxy) fn validate(value: &Value, wire: ZenWire) -> Result<(), ProxyError> {
    if !value.is_object()
        || value.get("error").is_some_and(|v| !v.is_null())
        || !string(value, "id")
        || !string(value, "model")
        || !usage(value, wire)
    {
        return Err(invalid());
    }
    let valid = match wire {
        ZenWire::Chat => {
            let choice = value.pointer("/choices/0").ok_or_else(invalid)?;
            let message = choice
                .get("message")
                .filter(|v| v.is_object())
                .ok_or_else(invalid)?;
            value
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|v| v.len() == 1)
                && message.get("role").and_then(Value::as_str) == Some("assistant")
                && message
                    .get("content")
                    .is_none_or(|v| v.is_null() || v.is_string())
                && [
                    "reasoning_content",
                    "reasoning_text",
                    "reasoning",
                    "refusal",
                ]
                .iter()
                .all(|key| {
                    message
                        .get(*key)
                        .is_none_or(|v| v.is_null() || v.is_string())
                })
                && message
                    .get("function_call")
                    .filter(|v| !v.is_null())
                    .is_none_or(|v| {
                        string(v, "name")
                            && v.get("arguments").and_then(Value::as_str).is_some_and(|a| {
                                serde_json::from_str::<Value>(a).is_ok_and(|v| v.is_object())
                            })
                    })
                && (choice["finish_reason"] != "function_call"
                    || message.get("function_call").is_some_and(Value::is_object))
                && message
                    .get("tool_calls")
                    .filter(|v| !v.is_null())
                    .is_none_or(|v| {
                        v.as_array().is_some_and(|calls| {
                            calls.len() <= 128 && calls.iter().all(|c| function(c, true))
                        })
                    })
                && choice
                    .get("finish_reason")
                    .and_then(Value::as_str)
                    .is_some_and(|s| {
                        matches!(
                            s,
                            "stop" | "length" | "tool_calls" | "function_call" | "content_filter"
                        )
                    })
                && (choice["finish_reason"] != "tool_calls"
                    || message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|v| !v.is_empty()))
        }
        ZenWire::Responses => {
            if !matches!(
                response_semantics::classify_value(value),
                response_semantics::SemanticObservation::SuccessTerminal
                    | response_semantics::SemanticObservation::IncompleteTerminal
            ) {
                return Err(invalid());
            }
            value
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().all(|item| {
                        item.get("error").is_none_or(Value::is_null)
                            && item
                                .get("status")
                                .and_then(Value::as_str)
                                .is_none_or(|s| matches!(s, "completed" | "incomplete"))
                            && match item.get("type").and_then(Value::as_str) {
                                Some("message") => {
                                    item.get("role").and_then(Value::as_str) == Some("assistant")
                                        && item
                                            .get("content")
                                            .and_then(Value::as_array)
                                            .is_some_and(|parts| {
                                                parts.iter().all(|part| {
                                                    match part.get("type").and_then(Value::as_str) {
                                                        Some("output_text") => part
                                                            .get("text")
                                                            .is_some_and(Value::is_string),
                                                        Some("refusal") => part
                                                            .get("refusal")
                                                            .is_some_and(Value::is_string),
                                                        _ => false,
                                                    }
                                                })
                                            })
                                }
                                Some("function_call") => function(item, false),
                                Some("reasoning") => {
                                    item.get("summary").is_none_or(|v| v.is_array())
                                }
                                _ => false,
                            }
                    })
                })
        }
        ZenWire::Messages => {
            matches!(
                anthropic_semantics::inspect_json_value_with_timing(value, false, None),
                Ok(anthropic_semantics::AnthropicJsonObservation::Success)
            ) && value
                .get("stop_reason")
                .and_then(Value::as_str)
                .is_some_and(|s| {
                    matches!(
                        s,
                        "end_turn"
                            | "max_tokens"
                            | "stop_sequence"
                            | "tool_use"
                            | "pause_turn"
                            | "refusal"
                            | "model_context_window_exceeded"
                    )
                })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid())
    }
}

pub(in crate::proxy) fn stream_usage(value: &Value) -> Result<(), ProxyError> {
    if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
        if !usage.is_object()
            || [
                "prompt_tokens",
                "completion_tokens",
                "input_tokens",
                "output_tokens",
                "total_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .iter()
            .any(|k| usage.get(*k).is_some_and(|n| n.as_u64().is_none()))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Validate only protocol-owned call locations, never opaque input/arguments.
pub(in crate::proxy) fn client_tools(
    value: &Value,
    names: &BTreeSet<String>,
) -> Result<(), ProxyError> {
    let check = |name: Option<&str>| {
        if name.is_some_and(|n| names.contains(n)) {
            Ok(())
        } else {
            Err(ProxyError::bad_gateway(
                "OpenCode Zen called a tool unavailable to the client",
            ))
        }
    };
    if let Some(message) = value.pointer("/choices/0/message") {
        for call in message
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            check(call.pointer("/function/name").and_then(Value::as_str))?;
        }
        if let Some(function) = message.get("function_call").filter(|v| !v.is_null()) {
            check(function.get("name").and_then(Value::as_str))?;
        }
    }
    let item = |item: &Value| {
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some("tool_use" | "function_call" | "custom_tool_call")
        ) {
            check(item.get("name").and_then(Value::as_str))
        } else {
            Ok(())
        }
    };
    for pointer in [
        "/output",
        "/response/output",
        "/content",
        "/message/content",
    ] {
        for value in value
            .pointer(pointer)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            item(value)?;
        }
    }
    for pointer in ["/item", "/content_block"] {
        if let Some(value) = value.pointer(pointer) {
            item(value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn zen_json_validation_rejects_malformed_success_and_nested_failures() {
        for value in [
            json!({"choices":[{"message":null,"finish_reason":"stop"}]}),
            json!({"id":"c","model":"free","choices":[{"message":{"role":"user","content":"bad"},"finish_reason":"stop"}]}),
            json!({"id":"c","model":"free","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":-1,"completion_tokens":1}}),
        ] {
            assert!(validate(&value, ZenWire::Chat).is_err());
        }
        assert!(validate(&json!({"id":"r","model":"free","status":"completed","output":[],"response":{"error":{"code":"bad"}}}),ZenWire::Responses).is_err());
        assert!(validate(&json!({"id":"m","model":"free","type":"message","role":"assistant","content":[],"stop_reason":"unknown","usage":{"input_tokens":1,"output_tokens":0}}),ZenWire::Messages).is_err());
        assert!(validate(&json!({"id":"c","model":"free","choices":[{"message":{"role":"assistant","content":null,"refusal":"declined"},"finish_reason":"content_filter"}]}),ZenWire::Chat).is_ok());
    }
}
