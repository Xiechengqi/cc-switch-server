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
        "reasoningContentEvent" => Ok(Event::Reasoning(parse_json(&frame.payload, &event_type)?)),
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
        | "meteringEvent" => Ok(Event::Usage {
            event_type: event_type.clone(),
            payload: parse_json(&frame.payload, &event_type)?,
        }),
        "endEvent" => Ok(Event::End),
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
