//! Qoder-specific history preservation on top of the shared protocol bridge.
use crate::proxy::{reasoning_bridge, transforms};
use serde_json::{json, Value};
use transforms::TransformError;

pub(crate) fn claude_to_chat(input: &Value) -> Result<Value, TransformError> {
    // Convert controls/system once, then each message exactly once. This also
    // retains reasoning-only assistant turns that generic Chat bridges omit.
    let mut header = Value::Object(
        input
            .as_object()
            .ok_or_else(|| TransformError::new("Qoder request must be an object"))?
            .iter()
            .filter(|(key, _)| key.as_str() != "messages")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    header["messages"] = json!([]);
    let mut output = transforms::anthropic_to_openai_chat(&header)?;
    let mut messages = match std::mem::take(&mut output["messages"]) {
        Value::Array(messages) => messages,
        _ => Vec::new(),
    };
    for source in input["messages"]
        .as_array()
        .ok_or_else(|| TransformError::new("Qoder messages must be an array"))?
    {
        let mut converted = transforms::anthropic_to_openai_chat(&json!({"messages":[source]}))?;
        let mut converted = match std::mem::take(&mut converted["messages"]) {
            Value::Array(messages) => messages,
            _ => Vec::new(),
        };
        if source["role"] == "assistant" {
            let mut text = String::new();
            let mut signature: Option<String> = None;
            let mut reasoning_item = None;
            for block in source["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("thinking") => {
                        if let Some(thinking) = block.get("thinking") {
                            text.push_str(thinking.as_str().ok_or_else(|| {
                                TransformError::new("Qoder thinking must be a string")
                            })?);
                        }
                        if let Some(value) = block.get("signature") {
                            let value = value.as_str().ok_or_else(|| {
                                TransformError::new("Qoder thinking signature must be a string")
                            })?;
                            if value.starts_with("ccswitch-server-reasoning-v1:") {
                                let item =
                                    reasoning_bridge::openai_reasoning_item_from_anthropic_block(
                                        block,
                                    )
                                    .ok_or_else(|| {
                                        TransformError::new(
                                            "Qoder reasoning bridge envelope is invalid",
                                        )
                                    })?;
                                if reasoning_item.replace(item).is_some() {
                                    return Err(TransformError::new(
                                        "Qoder assistant has multiple opaque reasoning items",
                                    ));
                                }
                            } else if !value.is_empty() {
                                if signature.as_deref().is_some_and(|old| old != value) {
                                    return Err(TransformError::new(
                                        "Qoder assistant has conflicting thinking signatures",
                                    ));
                                }
                                signature = Some(value.to_string());
                            }
                        }
                    }
                    Some("redacted_thinking") => {
                        // Only authenticated Server envelopes are decoded.
                        // Native opaque data is left to Qoder's own verifier.
                        let data = block["data"].as_str().ok_or_else(|| {
                            TransformError::new("Qoder redacted thinking requires string data")
                        })?;
                        let item = if data.starts_with("ccswitch-server-reasoning-v1:") {
                            reasoning_bridge::openai_reasoning_item_from_anthropic_block(block)
                                .ok_or_else(|| {
                                    TransformError::new(
                                        "Qoder reasoning bridge envelope is invalid",
                                    )
                                })?
                        } else {
                            json!({"type":"reasoning", "encrypted_content":data})
                        };
                        if reasoning_item.replace(item).is_some() {
                            return Err(TransformError::new(
                                "Qoder assistant has multiple opaque reasoning items",
                            ));
                        }
                    }
                    _ => {}
                }
            }
            if !text.is_empty() || signature.is_some() || reasoning_item.is_some() {
                if !converted
                    .iter()
                    .any(|message| message["role"] == "assistant")
                {
                    converted.insert(0, json!({"role":"assistant","content":""}));
                }
                let assistant = converted
                    .iter_mut()
                    .find(|message| message["role"] == "assistant")
                    .expect("assistant inserted above");
                if !text.is_empty() {
                    assistant["reasoning_content"] = Value::String(text);
                }
                if let Some(signature) = signature {
                    assistant["reasoning_content_signature"] = Value::String(signature);
                }
                if let Some(item) = reasoning_item {
                    assistant["reasoning_item"] = item;
                }
            }
        }
        messages.extend(converted);
    }
    output["messages"] = Value::Array(messages);
    for field in [
        "top_k",
        "context_length",
        "preserve_thinking",
        "qoder_text_tool_recovery",
    ] {
        if let Some(value) = input.get(field) {
            output[field] = value.clone();
        }
    }
    Ok(output)
}
