use serde::Deserialize;
use serde_json::Value;

use super::error::WireError;
use super::frame::Frame;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    AssistantResponse {
        content: String,
    },
    Code {
        content: String,
    },
    Reasoning(Value),
    ToolUse {
        tool_use_id: String,
        name: String,
        input: String,
        stop: bool,
    },
    Usage {
        event_type: String,
        payload: Value,
    },
    End,
    Unknown {
        event_type: String,
        payload: Value,
    },
}

#[derive(Deserialize)]
struct TextPayload {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolUsePayload {
    tool_use_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: String,
    #[serde(default)]
    stop: bool,
}

pub(crate) fn parse_event(frame: Frame) -> Result<Event, WireError> {
    let message_type = frame.headers.message_type();
    if message_type.is_none() && frame.headers.get(":message-type").is_some() {
        return Err(WireError::InvalidHeaderValue("message type"));
    }
    if let Some(message_type @ ("error" | "exception")) = message_type {
        let payload = parse_json(&frame.payload, message_type)?;
        let message = payload
            .get("message")
            .or_else(|| payload.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(WireError::Upstream {
            message_type: message_type.to_string(),
            code: frame
                .headers
                .error_code()
                .or_else(|| frame.headers.exception_type())
                .map(str::to_string),
            message: truncate(message, 4096),
        });
    }
    if let Some(message_type) = message_type.filter(|value| *value != "event") {
        return Err(WireError::InvalidMessageType(message_type.to_string()));
    }
    let event_type = frame
        .headers
        .event_type()
        .ok_or(WireError::MissingEventType)?
        .to_string();
    match event_type.as_str() {
        "assistantResponseEvent" => {
            let payload: TextPayload = parse_typed(&frame.payload, &event_type)?;
            Ok(Event::AssistantResponse {
                content: payload.content,
            })
        }
        "codeEvent" => {
            let payload: TextPayload = parse_typed(&frame.payload, &event_type)?;
            Ok(Event::Code {
                content: payload.content,
            })
        }
        "reasoningContentEvent" => {
            let payload = parse_json(&frame.payload, &event_type)?;
            validate_reasoning_payload(&payload, &event_type)?;
            Ok(Event::Reasoning(payload))
        }
        "toolUseEvent" => {
            let payload: ToolUsePayload = parse_typed(&frame.payload, &event_type)?;
            if payload.tool_use_id.trim().is_empty() {
                return Err(WireError::InvalidEventPayload {
                    event_type,
                    message: "toolUseId is required".to_string(),
                });
            }
            Ok(Event::ToolUse {
                tool_use_id: payload.tool_use_id,
                name: payload.name,
                input: payload.input,
                stop: payload.stop,
            })
        }
        "contextUsageEvent"
        | "metricsEvent"
        | "messageMetadataEvent"
        | "metadataEvent"
        | "meteringEvent" => {
            let payload = parse_json(&frame.payload, &event_type)?;
            if matches!(
                event_type.as_str(),
                "messageMetadataEvent" | "metadataEvent"
            ) {
                validate_token_usage_payload(&payload, &event_type)?;
            }
            Ok(Event::Usage {
                event_type,
                payload,
            })
        }
        "endEvent" => {
            // The terminal marker may be payload-free, but nonempty bytes
            // must not bypass the JSON validation applied to other events.
            if !frame.payload.is_empty() {
                parse_json(&frame.payload, &event_type)?;
            }
            Ok(Event::End)
        }
        _ => Ok(Event::Unknown {
            event_type: event_type.clone(),
            payload: parse_json(&frame.payload, &event_type)?,
        }),
    }
}

fn parse_json(payload: &[u8], event_type: &str) -> Result<Value, WireError> {
    serde_json::from_slice(payload).map_err(|error| WireError::InvalidEventPayload {
        event_type: event_type.to_string(),
        message: error.to_string(),
    })
}

fn validate_reasoning_payload(payload: &Value, event_type: &str) -> Result<(), WireError> {
    let value = payload.get("reasoningContentEvent").unwrap_or(payload);
    match value {
        Value::String(_) | Value::Null => Ok(()),
        Value::Object(fields) => {
            for name in [
                "text",
                "content",
                "reasoningContent",
                "signature",
                "redactedContent",
                "redacted_content",
            ] {
                if fields
                    .get(name)
                    .is_some_and(|field| !field.is_string() && !field.is_null())
                {
                    return Err(WireError::InvalidEventPayload {
                        event_type: event_type.to_string(),
                        message: "reasoning field must be a string or null".to_string(),
                    });
                }
            }
            Ok(())
        }
        _ => Err(WireError::InvalidEventPayload {
            event_type: event_type.to_string(),
            message: "reasoning payload must be an object or string".to_string(),
        }),
    }
}

fn validate_token_usage_payload(payload: &Value, event_type: &str) -> Result<(), WireError> {
    let metadata = payload
        .get("messageMetadataEvent")
        .or_else(|| payload.get("metadataEvent"))
        .unwrap_or(payload);
    let Some(token_usage) = metadata.get("tokenUsage") else {
        return Ok(());
    };
    if token_usage.is_null() {
        return Ok(());
    }
    let invalid = || WireError::InvalidEventPayload {
        event_type: event_type.to_string(),
        message: "tokenUsage counters must be 32-bit integers".to_string(),
    };
    let fields = token_usage.as_object().ok_or_else(&invalid)?;
    for key in [
        "uncachedInputTokens",
        "uncached_input_tokens",
        "inputTokens",
        "input_tokens",
        "cacheReadInputTokens",
        "cache_read_input_tokens",
        "cacheReadTokens",
        "cacheWriteInputTokens",
        "cache_write_input_tokens",
        "cacheCreationInputTokens",
        "cache_creation_input_tokens",
        "outputTokens",
        "output_tokens",
        "completionTokens",
        "completion_tokens",
    ] {
        if fields.get(key).is_some_and(|value| {
            value
                .as_i64()
                .and_then(|count| i32::try_from(count).ok())
                .is_none()
        }) {
            return Err(invalid());
        }
    }
    Ok(())
}

fn parse_typed<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    event_type: &str,
) -> Result<T, WireError> {
    serde_json::from_slice(payload).map_err(|error| WireError::InvalidEventPayload {
        event_type: event_type.to_string(),
        message: error.to_string(),
    })
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
