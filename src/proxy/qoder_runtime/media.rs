//! Bounded, original-image forwarding. This module never downloads media.
use serde_json::Value;

use crate::domain::providers::model::AppKind;
use crate::proxy::remote_image::{
    decode_image_data_uri, MAX_CODEX_IMAGES_TOTAL_BYTES, MAX_CODEX_IMAGE_BYTES,
    MAX_IMAGES_PER_REQUEST,
};

fn image_part(part: &Value) -> bool {
    matches!(
        part["type"].as_str(),
        Some("image" | "image_url" | "input_image")
    ) || ["inlineData", "inline_data", "fileData", "file_data"]
        .iter()
        .any(|key| part.get(key).is_some())
}

fn validate_url(url: &str, count: &mut usize, bytes: &mut usize) -> Result<(), String> {
    *count += 1;
    if *count > MAX_IMAGES_PER_REQUEST || url.is_empty() || url.trim() != url {
        return Err("Qoder image input has an invalid URL or exceeds the image count limit".into());
    }
    if url
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        let decoded = decode_image_data_uri(url, MAX_CODEX_IMAGE_BYTES)
            .map_err(|_| "Qoder image data requires valid base64, matching supported MIME/signature and bounded size")?;
        *bytes = bytes.saturating_add(decoded.data.len());
        if *bytes > MAX_CODEX_IMAGES_TOTAL_BYTES {
            return Err("Qoder inline images exceed the total decoded size limit".into());
        }
    } else {
        if url.len() > 16 * 1024 {
            return Err("Qoder image URL exceeds the length limit".into());
        }
        let parsed = url::Url::parse(url).map_err(|_| "Qoder image URL is invalid")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err("Qoder remote images require HTTPS without userinfo or a fragment".into());
        }
    }
    Ok(())
}

pub(super) fn validate_canonical(request: &Value, supports_vision: bool) -> Result<(), String> {
    let mut count = 0;
    let mut bytes = 0;
    for message in request["messages"].as_array().into_iter().flatten() {
        if message["content"].is_object() && image_part(&message["content"]) {
            return Err("Qoder image input must use a content array".into());
        }
        for part in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| image_part(part))
        {
            if !supports_vision {
                return Err(
                    "Qoder bound account's exact live model does not support image input".into(),
                );
            }
            if !matches!(message["role"].as_str(), Some("user" | "tool"))
                || part["type"] != "image_url"
            {
                return Err("Qoder images require user/tool image_url content parts".into());
            }
            let image = &part["image_url"];
            if image
                .get("detail")
                .is_some_and(|detail| !matches!(detail.as_str(), Some("auto" | "low" | "high")))
            {
                return Err("Qoder image detail must be auto, low or high".into());
            }
            let url = image["url"]
                .as_str()
                .ok_or("Qoder image_url requires a URL string")?;
            validate_url(url, &mut count, &mut bytes)?;
        }
    }
    Ok(())
}

fn require_array(parts: &Value) -> Result<(), String> {
    if parts.is_object() && image_part(parts) {
        return Err("Qoder media must occupy its own content/parts array element".into());
    }
    Ok(())
}

fn validate_claude(
    parts: &Value,
    depth: usize,
    allow_media: bool,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err("Qoder tool media nesting exceeds the limit".into());
    }
    require_array(parts)?;
    for part in parts.as_array().into_iter().flatten() {
        if image_part(part) && (part["type"] != "image" || !allow_media) {
            return Err("Qoder Claude media requires a user image block".into());
        }
        match part["type"].as_str() {
            Some("image") => {
                let source = &part["source"];
                let uri = match source["type"].as_str() {
                    Some("url") => source["url"]
                        .as_str()
                        .ok_or("Qoder image URL must be a string")?
                        .to_string(),
                    Some("base64") => {
                        let data = source["data"]
                            .as_str()
                            .ok_or("Qoder image data must be a string")?;
                        let mime = source["media_type"]
                            .as_str()
                            .ok_or("Qoder image MIME must be a string")?;
                        format!("data:{mime};base64,{data}")
                    }
                    _ => {
                        return Err(
                            "Qoder image source requires URL or base64 image data/MIME".into()
                        )
                    }
                };
                validate_url(&uri, count, bytes)?;
            }
            Some("tool_result") => {
                validate_claude(&part["content"], depth + 1, allow_media, count, bytes)?
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_responses(parts: &Value, count: &mut usize, bytes: &mut usize) -> Result<(), String> {
    require_array(parts)?;
    for part in parts.as_array().into_iter().flatten() {
        if !image_part(part) {
            continue;
        }
        if part["type"] != "input_image" {
            return Err("Qoder Responses media requires input_image parts".into());
        }
        let url = part
            .get("image_url")
            .or_else(|| part.get("url"))
            .and_then(Value::as_str)
            .ok_or("Qoder Responses input_image requires image_url; file_id is unsupported")?;
        validate_url(url, count, bytes)?;
    }
    Ok(())
}

fn validate_gemini(
    parts: &Value,
    depth: usize,
    allow_media: bool,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err("Qoder tool media nesting exceeds the limit".into());
    }
    require_array(parts)?;
    for part in parts.as_array().into_iter().flatten() {
        let fields = ["inlineData", "inline_data", "fileData", "file_data"];
        let media: Vec<_> = fields
            .iter()
            .filter_map(|key| part.get(key).map(|value| (*key, value)))
            .collect();
        if image_part(part) && (media.is_empty() || !allow_media) {
            return Err("Qoder Gemini media requires a user inlineData/fileData part".into());
        }
        if !media.is_empty() {
            if media.len() != 1
                || [
                    "text",
                    "thought",
                    "functionCall",
                    "function_call",
                    "functionResponse",
                    "function_response",
                ]
                .iter()
                .any(|key| part.get(key).is_some())
            {
                return Err("Qoder Gemini media must occupy its own part".into());
            }
            let (key, value) = media[0];
            let mime = value
                .get("mimeType")
                .or_else(|| value.get("mime_type"))
                .and_then(Value::as_str)
                .filter(|mime| mime.starts_with("image/"))
                .ok_or("Qoder Gemini media requires an image MIME")?;
            let inline = matches!(key, "inlineData" | "inline_data");
            let data = if inline {
                value.get("data")
            } else {
                value.get("fileUri").or_else(|| value.get("file_uri"))
            }
            .and_then(Value::as_str)
            .ok_or("Qoder Gemini media requires inline data or a file URI")?;
            if (inline && (value.get("fileUri").is_some() || value.get("file_uri").is_some()))
                || (!inline && value.get("data").is_some())
            {
                return Err("Qoder Gemini media requires image MIME and inline data or URL".into());
            }
            let uri = if inline {
                format!("data:{mime};base64,{data}")
            } else {
                data.to_string()
            };
            validate_url(&uri, count, bytes)?;
        }
        if let Some(result) = part
            .get("functionResponse")
            .or_else(|| part.get("function_response"))
        {
            validate_gemini(&result["parts"], depth + 1, allow_media, count, bytes)?;
        }
    }
    Ok(())
}

/// Reject malformed native media BEFORE a generic bridge could drop it.
/// JSON strings, tool schemas and argument objects are deliberately opaque.
pub(crate) fn validate_source_images(app: AppKind, request: &Value) -> Result<(), String> {
    let mut count = 0;
    let mut bytes = 0;
    match app {
        AppKind::Claude => {
            require_array(&request["system"])?;
            if request["system"]
                .as_array()
                .into_iter()
                .flatten()
                .any(image_part)
            {
                return Err("Qoder system instructions cannot contain images".into());
            }
            for message in request["messages"].as_array().into_iter().flatten() {
                validate_claude(
                    &message["content"],
                    0,
                    message["role"] == "user",
                    &mut count,
                    &mut bytes,
                )?;
            }
        }
        AppKind::Codex if request.get("messages").is_none() => {
            let items = match request.get("input") {
                Some(Value::Array(items)) => items.as_slice(),
                Some(item) => std::slice::from_ref(item),
                None => &[],
            };
            for item in items {
                if image_part(item) {
                    return Err("Qoder Responses image input must be inside a message".into());
                }
                validate_responses(&item["content"], &mut count, &mut bytes)?;
                if matches!(
                    item["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output" | "tool_search_output")
                ) {
                    validate_responses(&item["output"], &mut count, &mut bytes)?;
                }
            }
        }
        AppKind::Gemini => {
            let system = request
                .get("systemInstruction")
                .or_else(|| request.get("system_instruction"));
            if let Some(system) = system {
                require_array(system)?;
                require_array(&system["parts"])?;
            }
            if system
                .and_then(|s| s["parts"].as_array())
                .into_iter()
                .flatten()
                .any(image_part)
            {
                return Err("Qoder system instructions cannot contain images".into());
            }
            for message in request["contents"].as_array().into_iter().flatten() {
                validate_gemini(
                    &message["parts"],
                    0,
                    message["role"] != "model",
                    &mut count,
                    &mut bytes,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}
