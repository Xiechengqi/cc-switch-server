//! Validate canonical image parts without fetching URLs or modifying image data.
//! Inspect message content only: tool schemas and JSON strings are user data.
use serde_json::Value;

use crate::proxy::remote_image::{
    decode_image_data_uri, MAX_CODEX_IMAGES_TOTAL_BYTES, MAX_CODEX_IMAGE_BYTES,
    MAX_IMAGES_PER_REQUEST,
};

const MAX_IMAGE_URL_BYTES: usize = 16 * 1024;

// Reject malformed native blocks before the shared bridge can skip them or
// prefer a text field. Never inspect tool arguments, schemas or JSON strings.
pub(crate) fn validate_buddy_source_images(
    app: crate::domain::providers::model::AppKind,
    request: &Value,
) -> Result<(), String> {
    use crate::domain::providers::model::AppKind;
    match app {
        AppKind::Claude => {
            if request["system"]
                .as_array()
                .into_iter()
                .flatten()
                .any(image_part)
            {
                return Err("CodeBuddy system instructions cannot contain image input".into());
            }
            for message in request["messages"].as_array().into_iter().flatten() {
                validate_anthropic_parts(&message["content"])?;
            }
        }
        AppKind::Codex if request.get("messages").is_none() => {
            let items = match request.get("input") {
                Some(Value::Array(items)) => items.as_slice(),
                Some(input) => std::slice::from_ref(input),
                None => &[],
            };
            for item in items {
                if image_part(item) || (item["content"].is_object() && image_part(&item["content"]))
                {
                    return Err(
                        "CodeBuddy Responses image input must be inside a message content array"
                            .into(),
                    );
                }
                for part in item["content"].as_array().into_iter().flatten() {
                    validate_response_part(part)?;
                }
                if matches!(
                    item["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output" | "tool_search_output")
                ) {
                    // Only the native output content array is structured media.
                    // Do not scan arbitrary tool-result objects or JSON text.
                    for part in item["output"].as_array().into_iter().flatten() {
                        validate_response_part(part)?;
                    }
                }
            }
        }
        AppKind::Gemini => {
            let system = request
                .get("systemInstruction")
                .or_else(|| request.get("system_instruction"));
            if system
                .and_then(|value| value["parts"].as_array())
                .into_iter()
                .flatten()
                .any(image_part)
            {
                return Err("CodeBuddy system instructions cannot contain image input".into());
            }
            for content in request["contents"].as_array().into_iter().flatten() {
                validate_gemini_parts(&content["parts"])?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_response_part(part: &Value) -> Result<(), String> {
    if part["type"] == "input_image"
        && !part
            .get("image_url")
            .or_else(|| part.get("url"))
            .is_some_and(nonempty_string)
    {
        return Err(
            "CodeBuddy Responses input_image requires an image_url; file_id images are unsupported"
                .into(),
        );
    }
    Ok(())
}

fn nonempty_string(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
}

fn validate_anthropic_parts(content: &Value) -> Result<(), String> {
    if content.is_object() && image_part(content) {
        return Err("CodeBuddy image input must use a content array".into());
    }
    for part in content.as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("image") => {
                let source = &part["source"];
                let valid = match source["type"].as_str() {
                    Some("url") => source.get("url").is_some_and(nonempty_string),
                    Some("base64") => {
                        source.get("data").is_some_and(nonempty_string)
                            && source["media_type"]
                                .as_str()
                                .is_some_and(|mime| mime.starts_with("image/"))
                    }
                    _ => false,
                };
                if !valid {
                    return Err(
                        "CodeBuddy image source requires a URL or base64 image data and MIME"
                            .into(),
                    );
                }
            }
            Some("tool_result") => validate_anthropic_parts(&part["content"])?,
            _ => {}
        }
    }
    Ok(())
}

fn validate_gemini_parts(content: &Value) -> Result<(), String> {
    if content.is_object() && image_part(content) {
        return Err("CodeBuddy Gemini image input must use a parts array".into());
    }
    for part in content.as_array().into_iter().flatten() {
        let inline = part.get("inlineData").or_else(|| part.get("inline_data"));
        let file = part.get("fileData").or_else(|| part.get("file_data"));
        if inline.is_some() || file.is_some() {
            if part.get("text").is_some()
                || part.get("thought").is_some()
                || part
                    .get("functionCall")
                    .or_else(|| part.get("function_call"))
                    .is_some()
                || part
                    .get("functionResponse")
                    .or_else(|| part.get("function_response"))
                    .is_some()
                || (inline.is_some() && file.is_some())
                || (part.get("inlineData").is_some() && part.get("inline_data").is_some())
                || (part.get("fileData").is_some() && part.get("file_data").is_some())
            {
                return Err("CodeBuddy Gemini image must occupy its own part".into());
            }
            let media = inline.or(file).unwrap();
            if !media
                .get("mimeType")
                .or_else(|| media.get("mime_type"))
                .and_then(Value::as_str)
                .is_some_and(|mime| mime.starts_with("image/"))
                || !if inline.is_some() {
                    media.get("data").is_some_and(nonempty_string)
                } else {
                    media
                        .get("fileUri")
                        .or_else(|| media.get("file_uri"))
                        .is_some_and(nonempty_string)
                }
            {
                return Err(
                    "CodeBuddy Gemini media requires image MIME and inline data or a URL".into(),
                );
            }
        }
        if let Some(result) = part
            .get("functionResponse")
            .or_else(|| part.get("function_response"))
        {
            validate_gemini_parts(&result["parts"])?;
        }
    }
    Ok(())
}

fn image_part(part: &Value) -> bool {
    matches!(
        part.get("type").and_then(Value::as_str),
        Some("image_url" | "input_image" | "image")
    ) || [
        "image_url",
        "image",
        "input_image",
        "inlineData",
        "inline_data",
        "fileData",
        "file_data",
    ]
    .iter()
    .any(|key| part.get(key).is_some())
}

pub(super) fn has_image_input(request: &Value) -> bool {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|message| match message.get("content") {
            Some(Value::Array(parts)) => parts.iter().any(image_part),
            Some(Value::Object(_)) => image_part(&message["content"]),
            _ => false,
        })
}

pub(super) fn validate_image_input(request: &Value) -> Result<(), String> {
    validate_with_limits(
        request,
        MAX_IMAGES_PER_REQUEST,
        MAX_CODEX_IMAGE_BYTES,
        MAX_CODEX_IMAGES_TOTAL_BYTES,
    )
}

fn validate_with_limits(
    request: &Value,
    max_images: usize,
    max_image_bytes: usize,
    max_total_bytes: usize,
) -> Result<(), String> {
    let mut image_count = 0_usize;
    let mut inline_bytes = 0_usize;
    for message in request["messages"].as_array().into_iter().flatten() {
        let Some(parts) = message["content"].as_array() else {
            if message["content"].is_object() && image_part(&message["content"]) {
                return Err("CodeBuddy image input must use a content array".into());
            }
            continue;
        };
        for part in parts.iter().filter(|part| image_part(part)) {
            if part["type"] != "image_url" {
                return Err("CodeBuddy canonical image input must use image_url blocks".into());
            }
            if !matches!(message["role"].as_str(), Some("user" | "tool")) {
                return Err("CodeBuddy image input requires a user or tool message".into());
            }
            image_count += 1;
            if image_count > max_images {
                return Err(format!("CodeBuddy image input exceeds {max_images} images"));
            }
            let image = &part["image_url"];
            let url = image["url"]
                .as_str()
                .filter(|url| !url.is_empty() && url.trim() == *url)
                .ok_or("CodeBuddy image_url must contain a non-empty url string")?;
            if let Some(detail) = image.get("detail") {
                if !matches!(detail.as_str(), Some("auto" | "low" | "high")) {
                    return Err("CodeBuddy image detail must be auto, low or high".into());
                }
            }
            if url
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
            {
                // The caller reserves the canonical working set before payload
                // preparation. Decode one image at a time, then drop it; the
                // original URI is forwarded without re-encoding or copying here.
                let decoded = decode_image_data_uri(url, max_image_bytes).map_err(|_| {
                    format!("CodeBuddy image data must be valid base64 with a matching supported image MIME and at most {max_image_bytes} decoded bytes")
                })?;
                inline_bytes = inline_bytes.saturating_add(decoded.data.len());
                if inline_bytes > max_total_bytes {
                    return Err(format!(
                        "CodeBuddy inline images exceed {max_total_bytes} total decoded bytes"
                    ));
                }
            } else {
                if url.len() > MAX_IMAGE_URL_BYTES {
                    return Err("CodeBuddy image URL exceeds the length limit".into());
                }
                let parsed =
                    reqwest::Url::parse(url).map_err(|_| "CodeBuddy image URL is invalid")?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(
                        "CodeBuddy image URL must use HTTPS without userinfo or a fragment".into(),
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    fn request(url: &str) -> Value {
        json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":url,"detail":"high"}}]}]})
    }

    #[test]
    fn buddy_images_enforce_decoded_limits_and_count_at_the_boundary() {
        let mut body = request(PNG);
        assert!(validate_with_limits(&body, 1, 8, 8).is_ok());
        assert!(validate_with_limits(&body, 1, 7, 8).is_err());
        let part = body["messages"][0]["content"][0].clone();
        body["messages"][0]["content"]
            .as_array_mut()
            .unwrap()
            .push(part);
        assert!(validate_with_limits(&body, 2, 8, 16).is_ok());
        assert!(validate_with_limits(&body, 1, 8, 16)
            .unwrap_err()
            .contains("1 images"));
        assert!(validate_with_limits(&body, 2, 8, 15)
            .unwrap_err()
            .contains("total decoded"));
    }

    #[test]
    fn buddy_images_reject_invalid_data_and_urls_without_reflecting_content() {
        for url in [
            "data:image/png;base64,private-invalid-base64",
            "data:image/jpeg;base64,iVBORw0KGgo=",
            "data:text/plain;base64,cHJpdmF0ZQ==",
            "data:image/png,iVBORw0KGgo=",
            "http://private.example/image.png",
            "file:///private/image.png",
            "https://private:secret@example.com/image.png",
            "https://example.com/image.png#private",
            " https://private.example/image.png",
            "private-invalid-url",
        ] {
            let error = validate_image_input(&request(url)).unwrap_err();
            assert!(!error.contains("private"), "{error}");
            assert!(!error.contains("iVBOR"), "{error}");
        }
        assert!(validate_image_input(&request(&format!(
            "https://example.com/{}",
            "x".repeat(MAX_IMAGE_URL_BYTES)
        )))
        .is_err());
        assert!(
            validate_image_input(&request("https://example.com/image.jpg?signature=private"))
                .is_ok()
        );
    }

    #[test]
    fn buddy_images_reject_malformed_blocks_roles_and_details() {
        for part in [
            json!({"type":"image_url"}),
            json!({"type":"image_url","image_url":42}),
            json!({"type":"image_url","image_url":{"url":PNG,"detail":true}}),
            json!({"type":"image_url","image_url":{"url":PNG,"detail":"unexpected"}}),
            json!({"image_url":{"url":PNG}}),
            json!({"type":"input_image","image_url":PNG}),
        ] {
            let body = json!({"messages":[{"role":"user","content":[part]}]});
            assert!(has_image_input(&body));
            assert!(validate_image_input(&body).is_err());
        }
        for role in ["system", "developer", "assistant"] {
            let mut body = request(PNG);
            body["messages"][0]["role"] = json!(role);
            assert!(validate_image_input(&body).is_err());
        }
        let mut body = request(PNG);
        body["messages"][0]["content"] = body["messages"][0]["content"][0].clone();
        assert!(validate_image_input(&body).is_err());
    }

    #[test]
    fn buddy_images_do_not_interpret_tool_schemas_or_json_text_as_media() {
        let body = json!({"messages":[{"role":"user","content":"{\"image_url\":\"private\"}"}],
            "metadata":{"image":"user-data"},
            "tools":[{"type":"function","function":{"name":"image","parameters":{"type":"object","properties":{"image_url":{"type":"string"},"image":{"type":"string"}}}}}]});
        assert!(!has_image_input(&body));
        assert!(validate_image_input(&body).is_ok());
    }

    #[test]
    fn buddy_source_images_fail_before_a_bridge_can_drop_or_hide_them() {
        use crate::domain::providers::model::AppKind;
        for (app, body) in [
            (
                AppKind::Claude,
                json!({"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png"}}]}]}),
            ),
            (
                AppKind::Claude,
                json!({"messages":[{"role":"user","content":{"type":"image","source":{"type":"url","url":"https://example.com/x.png"}}}]}),
            ),
            (
                AppKind::Claude,
                json!({"system":[{"type":"image","source":{"type":"url","url":"https://example.com/x.png"}}]}),
            ),
            (
                AppKind::Codex,
                json!({"input":[{"role":"user","content":[{"type":"input_image","file_id":"private-file"}]}]}),
            ),
            (
                AppKind::Codex,
                json!({"input":[{"role":"user","content":{"type":"input_image","image_url":PNG}}]}),
            ),
            (
                AppKind::Codex,
                json!({"input":[{"type":"input_image","image_url":PNG}]}),
            ),
            (
                AppKind::Codex,
                json!({"input":[{"type":"function_call_output","call_id":"image-call","output":[{"type":"input_image","file_id":"private-file"}]}]}),
            ),
            (
                AppKind::Gemini,
                json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/png"}}]}]}),
            ),
            (
                AppKind::Gemini,
                json!({"contents":[{"role":"user","parts":[{"text":"would hide image","inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}}]}]}),
            ),
            (
                AppKind::Gemini,
                json!({"contents":[{"role":"user","parts":{"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}}}]}),
            ),
            (
                AppKind::Gemini,
                json!({"systemInstruction":{"parts":[{"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}}]}}),
            ),
        ] {
            let error = validate_buddy_source_images(app, &body).unwrap_err();
            assert!(!error.contains("private-file"));
            assert!(!error.contains("iVBOR"));
        }
    }
}
