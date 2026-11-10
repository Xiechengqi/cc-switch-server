//! Ollama Cloud effort aliases are a request contract, not model entitlement.
use serde_json::{json, Value};

use crate::domain::providers::model::AppKind;

use super::transforms::TransformError;
use super::ProxyError;

/// Cloud completion requires both a usable finish reason and the wire terminal.
#[derive(Debug, Default)]
pub(super) struct ChatStreamTerminal {
    finished: bool,
    done: bool,
}

impl ChatStreamTerminal {
    pub(super) fn observe(&mut self, value: &Value) -> Result<(), ProxyError> {
        if self.done {
            return Err(ProxyError::bad_gateway(
                "Ollama Chat emitted data after [DONE]",
            ));
        }
        if let Some(reason) = value
            .pointer("/choices/0/finish_reason")
            .filter(|reason| !reason.is_null())
        {
            if reason
                .as_str()
                .is_none_or(|reason| reason.trim().is_empty())
            {
                return Err(ProxyError::bad_gateway(
                    "Ollama Chat finish_reason must be a nonempty string",
                ));
            }
            self.finished = true;
        }
        Ok(())
    }

    pub(super) fn complete(&mut self) -> Result<(), ProxyError> {
        if !self.finished || self.done {
            return Err(ProxyError::bad_gateway(
                "Ollama Chat [DONE] requires one preceding finish_reason",
            ));
        }
        self.done = true;
        Ok(())
    }

    pub(super) fn finish(&self) -> Result<(), ProxyError> {
        if !self.done {
            return Err(ProxyError::bad_gateway(
                "Ollama Chat stream ended before [DONE]",
            ));
        }
        Ok(())
    }
}

pub(super) fn normalize_effort(value: &Value) -> Result<Option<Value>, TransformError> {
    let effort = value
        .as_str()
        .ok_or_else(|| TransformError::new("Ollama effort must be a string"))?
        .trim()
        .to_ascii_lowercase();
    let effort = match effort.as_str() {
        "" => return Ok(None),
        "ultra" | "xhigh" | "max" => "max",
        "high" => "high",
        "medium" => "medium",
        "minimal" | "low" => "low",
        "none" | "off" | "disabled" => "none",
        _ => return Err(TransformError::new("unsupported Ollama reasoning effort")),
    };
    Ok(Some(json!(effort)))
}

fn merge_effort(selected: &mut Option<Value>, candidate: &Value) -> Result<(), TransformError> {
    if let Some(candidate) = normalize_effort(candidate)? {
        if selected.as_ref().is_some_and(|value| value != &candidate) {
            return Err(TransformError::new(
                "conflicting Ollama reasoning effort controls",
            ));
        }
        *selected = Some(candidate);
    }
    Ok(())
}

fn validate_containers(input: &Value, names: &[&str]) -> Result<(), TransformError> {
    for name in names {
        if input.get(name).is_some_and(|value| !value.is_object()) {
            return Err(TransformError::new(
                "Ollama reasoning controls must be objects",
            ));
        }
    }
    Ok(())
}

/// Validate before the generic bridge can omit unknown or non-string controls.
pub(super) fn normalize_source(input: &mut Value, app: AppKind) -> Result<(), TransformError> {
    let (containers, pointers): (&[&str], &[&str]) = match app {
        AppKind::Claude => (
            &["output_config", "thinking"],
            &["/output_config/effort", "/thinking/effort"],
        ),
        AppKind::Codex => (&["reasoning"], &["/reasoning/effort", "/reasoning_effort"]),
        AppKind::Gemini => {
            return Err(TransformError::new(
                "Ollama Cloud has no Gemini App contract",
            ))
        }
    };
    validate_containers(input, containers)?;
    let mut selected = None;
    for pointer in pointers {
        if let Some(value) = input.pointer(pointer) {
            merge_effort(&mut selected, value)?;
        }
    }
    if app == AppKind::Claude
        && input.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
    {
        merge_effort(&mut selected, &json!("none"))?;
    }
    let object = input
        .as_object_mut()
        .ok_or_else(|| TransformError::new("Ollama request must be an object"))?;
    for name in containers {
        if let Some(container) = object.get_mut(*name).and_then(Value::as_object_mut) {
            let had_effort = container.remove("effort").is_some();
            if had_effort && container.is_empty() {
                object.remove(*name);
            }
        }
    }
    if app == AppKind::Codex {
        object.remove("reasoning_effort");
    }
    if let Some(selected) = selected {
        let field = if app == AppKind::Claude {
            "output_config"
        } else {
            "reasoning"
        };
        object.entry(field).or_insert_with(|| json!({}))["effort"] = selected;
    }
    Ok(())
}

/// Canonicalize every supported ingress, including native Chat and Claude.
pub(super) fn normalize_chat(output: &mut Value) -> Result<(), TransformError> {
    let mut selected = None;
    for value in [
        output.pointer("/reasoning/effort"),
        output.get("reasoning_effort"),
    ]
    .into_iter()
    .flatten()
    {
        merge_effort(&mut selected, value)?;
    }
    let object = output
        .as_object_mut()
        .ok_or_else(|| TransformError::new("Ollama request must be an object"))?;
    if let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut) {
        reasoning.remove("effort");
        if reasoning.is_empty() {
            object.remove("reasoning");
        } else if let Some(selected) = &selected {
            // Official Ollama prefers a present nested reasoning object. Keep
            // any other controls and make its effort agree with the Chat field.
            reasoning.insert("effort".to_string(), selected.clone());
        }
    }
    if let Some(selected) = selected {
        object.insert("reasoning_effort".to_string(), selected);
    } else {
        object.remove("reasoning_effort");
    }
    Ok(())
}
