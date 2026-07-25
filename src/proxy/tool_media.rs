use serde_json::{json, Map, Value};

pub(crate) const TOOL_MEDIA_MOVED_MARKER: &str =
    "[cc-switch-server: tool result media moved to native content]";
const WHOLE_DATA_URL_MIN_BYTES: usize = 8 * 1024;
const MAX_TRAVERSAL_DEPTH: usize = 128;
const MAX_TRAVERSAL_NODES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolMediaScope {
    ImagesOnly,
    InlineImagesOnly,
    AllSupported,
    ChatNative,
    ResponsesNative,
    AnthropicNative,
    GeminiNative,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToolMediaPart {
    Image {
        url: String,
        detail: Option<Value>,
    },
    File {
        file_id: Option<String>,
        file_data: Option<String>,
        filename: Option<String>,
        media_type: Option<String>,
        url: Option<String>,
    },
    Audio {
        data: Value,
        format: Value,
        media_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolMediaExtraction {
    pub sanitized: Value,
    pub media: Vec<ToolMediaPart>,
}

impl ToolMediaScope {
    fn allows(self, media: &ToolMediaPart) -> bool {
        match self {
            Self::ImagesOnly => matches!(media, ToolMediaPart::Image { .. }),
            Self::InlineImagesOnly => media.is_inline_image(),
            Self::AllSupported => true,
            Self::ChatNative => match media {
                ToolMediaPart::Image { .. } | ToolMediaPart::Audio { .. } => true,
                ToolMediaPart::File {
                    file_id, file_data, ..
                } => file_id.is_some() || file_data.is_some(),
            },
            Self::ResponsesNative => match media {
                ToolMediaPart::Image { .. } | ToolMediaPart::Audio { .. } => true,
                ToolMediaPart::File {
                    file_id,
                    file_data,
                    url,
                    ..
                } => file_id.is_some() || file_data.is_some() || url.is_some(),
            },
            Self::AnthropicNative => match media {
                ToolMediaPart::Image { .. } => true,
                ToolMediaPart::File { file_data, url, .. } => url.is_some() || file_data.is_some(),
                ToolMediaPart::Audio { .. } => false,
            },
            Self::GeminiNative => match media {
                ToolMediaPart::Image { .. } | ToolMediaPart::Audio { .. } => true,
                ToolMediaPart::File { file_data, url, .. } => url.is_some() || file_data.is_some(),
            },
        }
    }
}

impl ToolMediaPart {
    fn is_inline_image(&self) -> bool {
        matches!(self, Self::Image { url, .. } if parse_data_url(url).is_some_and(|(mime, data)| mime.starts_with("image/") && !data.is_empty()))
    }

    pub(crate) fn to_chat_part(&self) -> Option<Value> {
        match self {
            Self::Image { url, detail } => {
                let mut image_url = Map::new();
                image_url.insert("url".to_string(), json!(url));
                if let Some(detail) = detail {
                    image_url.insert("detail".to_string(), detail.clone());
                }
                Some(json!({"type": "image_url", "image_url": image_url}))
            }
            Self::File {
                file_id,
                file_data,
                filename,
                media_type,
                ..
            } if file_id.is_some() || file_data.is_some() => {
                let mut file = Map::new();
                if let Some(value) = file_id {
                    file.insert("file_id".to_string(), json!(value));
                }
                if let Some(value) = file_data {
                    file.insert(
                        "file_data".to_string(),
                        normalize_file_data(value, media_type.as_deref()),
                    );
                }
                if let Some(value) = filename {
                    file.insert("filename".to_string(), json!(value));
                }
                Some(json!({"type": "file", "file": file}))
            }
            Self::Audio { data, format, .. } => Some(json!({
                "type": "input_audio",
                "input_audio": {"data": data, "format": format}
            })),
            _ => None,
        }
    }

    pub(crate) fn to_responses_part(&self) -> Option<Value> {
        match self {
            Self::Image { url, detail } => {
                let mut part = Map::new();
                part.insert("type".to_string(), json!("input_image"));
                part.insert("image_url".to_string(), json!(url));
                if let Some(detail) = detail {
                    part.insert("detail".to_string(), detail.clone());
                }
                Some(Value::Object(part))
            }
            Self::File {
                file_id,
                file_data,
                filename,
                media_type,
                url,
                ..
            } if file_id.is_some() || file_data.is_some() || url.is_some() => {
                let mut part = Map::new();
                part.insert("type".to_string(), json!("input_file"));
                for (name, value) in [
                    ("file_id", file_id.as_ref()),
                    ("file_data", file_data.as_ref()),
                    ("filename", filename.as_ref()),
                ] {
                    if let Some(value) = value {
                        part.insert(
                            name.to_string(),
                            if name == "file_data" {
                                normalize_file_data(value, media_type.as_deref())
                            } else {
                                json!(value)
                            },
                        );
                    }
                }
                if let Some(url) = url {
                    part.insert("file_url".to_string(), json!(url));
                }
                Some(Value::Object(part))
            }
            Self::Audio { data, format, .. } => Some(json!({
                "type": "input_audio",
                "input_audio": {"data": data, "format": format}
            })),
            _ => None,
        }
    }

    pub(crate) fn to_anthropic_block(&self) -> Option<Value> {
        match self {
            Self::Image { url, .. } => {
                if let Some((media_type, data)) = parse_data_url(url) {
                    return Some(json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": media_type, "data": data}
                    }));
                }
                Some(json!({
                    "type": "image",
                    "source": {"type": "url", "url": url}
                }))
            }
            Self::File {
                file_data,
                filename,
                media_type,
                url,
                ..
            } => {
                if let Some(url) = url {
                    let mut block = json!({
                        "type": "document",
                        "source": {"type": "url", "url": url}
                    });
                    if let Some(filename) = filename {
                        block["title"] = json!(filename);
                    }
                    return Some(block);
                }
                let data = file_data.as_deref()?;
                let (detected_type, data) = parse_data_url(data)
                    .map(|(mime, payload)| (Some(mime.to_string()), payload.to_string()))
                    .unwrap_or((None, data.to_string()));
                let mut block = json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": detected_type.or_else(|| media_type.clone()).unwrap_or_else(|| "application/octet-stream".to_string()),
                        "data": data
                    }
                });
                if let Some(filename) = filename {
                    block["title"] = json!(filename);
                }
                Some(block)
            }
            Self::Audio { .. } => None,
        }
    }

    pub(crate) fn to_gemini_part(&self) -> Option<Value> {
        match self {
            Self::Image { url, .. } => {
                if let Some((media_type, data)) = parse_data_url(url) {
                    return Some(json!({
                        "inlineData": {"mimeType": media_type, "data": data}
                    }));
                }
                Some(json!({
                    "fileData": {"mimeType": "image/*", "fileUri": url}
                }))
            }
            Self::File {
                file_data,
                media_type,
                url,
                ..
            } => {
                if let Some(url) = url {
                    return Some(json!({
                        "fileData": {
                            "mimeType": media_type.as_deref().unwrap_or("application/octet-stream"),
                            "fileUri": url
                        }
                    }));
                }
                let data = file_data.as_deref()?;
                let (detected_type, payload) = parse_data_url(data)
                    .map(|(mime, payload)| (Some(mime), payload))
                    .unwrap_or((None, data));
                Some(json!({
                    "inlineData": {
                        "mimeType": detected_type.or(media_type.as_deref()).unwrap_or("application/octet-stream"),
                        "data": payload
                    }
                }))
            }
            Self::Audio {
                data, media_type, ..
            } => Some(json!({
                "inlineData": {
                    "mimeType": media_type.as_deref().unwrap_or("audio/wav"),
                    "data": data
                }
            })),
        }
    }
}

pub(crate) fn extract_tool_media(
    output: &Value,
    scope: ToolMediaScope,
) -> Option<ToolMediaExtraction> {
    let mut sanitized = output.clone();
    let mut media = Vec::new();
    let mut budget = MAX_TRAVERSAL_NODES;
    let replaced = strip_media(
        &mut sanitized,
        scope,
        &mut media,
        TOOL_MEDIA_MOVED_MARKER,
        0,
        &mut budget,
    );
    if replaced == 0 {
        return None;
    }
    Some(ToolMediaExtraction { sanitized, media })
}

pub(crate) fn replace_media_with_text(
    value: &mut Value,
    scope: ToolMediaScope,
    replacement: &str,
) -> usize {
    let mut media = Vec::new();
    let mut budget = usize::MAX;
    strip_media(value, scope, &mut media, replacement, 0, &mut budget)
}

pub(crate) fn tool_output_contains_media(output: &Value, scope: ToolMediaScope) -> bool {
    let mut budget = MAX_TRAVERSAL_NODES;
    contains_media(output, scope, 0, &mut budget)
}

pub(crate) fn sanitized_tool_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub(crate) fn queue_chat_media(pending: &mut Vec<Value>, call_id: &str, media: &[ToolMediaPart]) {
    let parts = media
        .iter()
        .filter_map(ToolMediaPart::to_chat_part)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return;
    }
    pending.push(json!({
        "type": "text",
        "text": format!("[cc-switch-server: media output of tool call {call_id}]")
    }));
    pending.extend(parts);
}

pub(crate) fn flush_chat_media(messages: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({"role": "user", "content": std::mem::take(pending)}));
    }
}

fn contains_media(value: &Value, scope: ToolMediaScope, depth: usize, budget: &mut usize) -> bool {
    if depth > MAX_TRAVERSAL_DEPTH || *budget == 0 {
        return false;
    }
    *budget -= 1;
    if media_part(value).is_some_and(|media| scope.allows(&media)) {
        return true;
    }
    match value {
        Value::String(text) => {
            if whole_image_data_url(text).is_some_and(|media| scope.allows(&media)) {
                return true;
            }
            parse_nested_json(text)
                .as_ref()
                .is_some_and(|nested| contains_media(nested, scope, depth + 1, budget))
        }
        Value::Array(items) => items
            .iter()
            .any(|item| contains_media(item, scope, depth + 1, budget)),
        Value::Object(object) => object
            .values()
            .any(|item| contains_media(item, scope, depth + 1, budget)),
        _ => false,
    }
}

fn strip_media(
    value: &mut Value,
    scope: ToolMediaScope,
    media: &mut Vec<ToolMediaPart>,
    replacement: &str,
    depth: usize,
    budget: &mut usize,
) -> usize {
    if depth > MAX_TRAVERSAL_DEPTH || *budget == 0 {
        return 0;
    }
    *budget -= 1;
    if let Some(part) = media_part(value).filter(|part| scope.allows(part)) {
        let replacement_type = if matches!(
            value.get("type").and_then(Value::as_str),
            Some("input_image" | "input_file" | "input_audio")
        ) {
            "input_text"
        } else {
            "text"
        };
        let cache_control = value.get("cache_control").cloned();
        media.push(part);
        *value = json!({"type": replacement_type, "text": replacement});
        if let Some(cache_control) = cache_control {
            value["cache_control"] = cache_control;
        }
        return 1;
    }
    match value {
        Value::String(text) => {
            if let Some(part) = whole_image_data_url(text).filter(|part| scope.allows(part)) {
                media.push(part);
                *text = replacement.to_string();
                return 1;
            }
            let Some(mut nested) = parse_nested_json(text) else {
                return 0;
            };
            let replaced = strip_media(&mut nested, scope, media, replacement, depth + 1, budget);
            if replaced > 0 {
                *text = serde_json::to_string(&nested).unwrap_or_else(|_| text.clone());
            }
            replaced
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| strip_media(item, scope, media, replacement, depth + 1, budget))
            .sum(),
        Value::Object(object) => object
            .values_mut()
            .map(|item| strip_media(item, scope, media, replacement, depth + 1, budget))
            .sum(),
        _ => 0,
    }
}

fn media_part(value: &Value) -> Option<ToolMediaPart> {
    let object = value.as_object()?;
    if let Some(inline) = object
        .get("inlineData")
        .or_else(|| object.get("inline_data"))
        .and_then(Value::as_object)
    {
        return inline_media_part(inline);
    }
    if let Some(file) = object
        .get("fileData")
        .or_else(|| object.get("file_data"))
        .and_then(Value::as_object)
    {
        let media_type = string_field(file, &["mimeType", "mime_type"]);
        let url = string_field(file, &["fileUri", "file_uri", "url"])?;
        if media_type.as_deref().is_some_and(is_image_mime) {
            return Some(ToolMediaPart::Image {
                url,
                detail: object.get("detail").cloned(),
            });
        }
        return Some(ToolMediaPart::File {
            file_id: None,
            file_data: None,
            filename: string_field(object, &["filename"]),
            media_type,
            url: Some(url),
        });
    }

    match object.get("type").and_then(Value::as_str) {
        Some("image_url" | "input_image") => image_from_openai(object),
        Some("image") => image_from_typed_object(object),
        Some("input_file") => file_from_openai(object),
        Some("file") => object
            .get("file")
            .and_then(Value::as_object)
            .and_then(file_from_openai),
        Some("document") => document_from_anthropic(object),
        Some("input_audio") => object
            .get("input_audio")
            .and_then(Value::as_object)
            .and_then(audio_from_object),
        _ => loose_data_image(object),
    }
}

fn image_from_openai(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    let image_url = object.get("image_url")?;
    let (url, nested_detail) = match image_url {
        Value::String(url) if !url.trim().is_empty() => (url.clone(), None),
        Value::Object(image) => (string_field(image, &["url"])?, image.get("detail").cloned()),
        _ => return None,
    };
    Some(ToolMediaPart::Image {
        url,
        detail: nested_detail.or_else(|| object.get("detail").cloned()),
    })
}

fn image_from_typed_object(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    if let Some(source) = object.get("source").and_then(Value::as_object) {
        let media_type = string_field(source, &["media_type", "mime_type", "mimeType"])
            .unwrap_or_else(|| "image/png".to_string());
        if !is_image_mime(&media_type) {
            return None;
        }
        if let Some(url) = string_field(source, &["url"]) {
            return Some(ToolMediaPart::Image {
                url,
                detail: object.get("detail").cloned(),
            });
        }
        if let Some(data) = string_field(source, &["data"]) {
            return Some(ToolMediaPart::Image {
                url: normalize_data_url(&media_type, &data),
                detail: object.get("detail").cloned(),
            });
        }
    }
    let media_type = string_field(object, &["mimeType", "mime_type"])?;
    if !is_image_mime(&media_type) {
        return None;
    }
    let data = string_field(object, &["data"])?;
    Some(ToolMediaPart::Image {
        url: normalize_data_url(&media_type, &data),
        detail: object.get("detail").cloned(),
    })
}

fn loose_data_image(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    if object.get("type").is_some() {
        return None;
    }
    let value = object.get("image_url")?;
    let url = value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))?;
    parse_data_url(url)
        .is_some_and(|(mime, _)| is_image_mime(mime))
        .then(|| ToolMediaPart::Image {
            url: url.to_string(),
            detail: object.get("detail").cloned(),
        })
}

fn document_from_anthropic(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    let source = object.get("source")?.as_object()?;
    let media_type = string_field(source, &["media_type", "mime_type", "mimeType"]);
    let source_type = source.get("type").and_then(Value::as_str);
    let url = matches!(source_type, Some("url"))
        .then(|| string_field(source, &["url"]))
        .flatten();
    let file_data = matches!(source_type, Some("base64"))
        .then(|| string_field(source, &["data"]))
        .flatten();
    (url.is_some() || file_data.is_some()).then(|| ToolMediaPart::File {
        file_id: None,
        file_data,
        filename: string_field(object, &["title", "filename"]),
        media_type,
        url,
    })
}

fn file_from_openai(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    let file_id = string_field(object, &["file_id"]);
    let file_data = string_field(object, &["file_data"]);
    let url = string_field(object, &["file_url", "url"]);
    (file_id.is_some() || file_data.is_some() || url.is_some()).then(|| ToolMediaPart::File {
        file_id,
        file_data,
        filename: string_field(object, &["filename"]),
        media_type: string_field(object, &["mime_type", "mimeType"]),
        url,
    })
}

fn inline_media_part(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    let media_type = string_field(object, &["mimeType", "mime_type"])
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let data = string_field(object, &["data"])?;
    let text = data.as_str();
    if is_image_mime(&media_type) {
        return Some(ToolMediaPart::Image {
            url: normalize_data_url(&media_type, text),
            detail: None,
        });
    }
    if media_type.starts_with("audio/") {
        return Some(ToolMediaPart::Audio {
            data: json!(data),
            format: json!(media_type.split('/').nth(1).unwrap_or("wav")),
            media_type: Some(media_type),
        });
    }
    Some(ToolMediaPart::File {
        file_id: None,
        file_data: Some(data),
        filename: None,
        media_type: Some(media_type),
        url: None,
    })
}

fn audio_from_object(object: &Map<String, Value>) -> Option<ToolMediaPart> {
    let data = string_field(object, &["data"])?;
    let format = string_field(object, &["format"])?;
    Some(ToolMediaPart::Audio {
        data: Value::String(data),
        format: Value::String(format),
        media_type: None,
    })
}

fn whole_image_data_url(value: &str) -> Option<ToolMediaPart> {
    let trimmed = value.trim();
    (trimmed.len() >= WHOLE_DATA_URL_MIN_BYTES)
        .then(|| parse_data_url(trimmed))
        .flatten()
        .filter(|(mime, data)| is_image_mime(mime) && !data.is_empty())
        .map(|_| ToolMediaPart::Image {
            url: trimmed.to_string(),
            detail: None,
        })
}

fn parse_nested_json(value: &str) -> Option<Value> {
    let trimmed = value.trim();
    if !matches!(
        trimmed.as_bytes().first(),
        Some(b'{') | Some(b'[') | Some(b'"')
    ) {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn string_field(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        object
            .get(*name)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    })
}

fn normalize_data_url(media_type: &str, data: &str) -> String {
    if parse_data_url(data).is_some() {
        data.to_string()
    } else {
        format!("data:{media_type};base64,{data}")
    }
}

fn normalize_file_data(data: &str, media_type: Option<&str>) -> Value {
    if parse_data_url(data).is_some() {
        return Value::String(data.to_string());
    }
    Value::String(format!(
        "data:{};base64,{data}",
        media_type.unwrap_or("application/octet-stream")
    ))
}

fn parse_data_url(value: &str) -> Option<(&str, &str)> {
    let value = value.trim();
    let comma = value.find(',')?;
    let header = value.get(..comma)?;
    if !header
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        || !header
            .get(header.len().saturating_sub(7)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(";base64"))
    {
        return None;
    }
    let media_type = &header[5..header.len().saturating_sub(7)];
    (!media_type.is_empty()).then(|| (media_type, &value[comma + 1..]))
}

fn is_image_mime(value: &str) -> bool {
    value
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_image_url() -> String {
        format!(
            "data:image/png;base64,{}",
            "a".repeat(WHOLE_DATA_URL_MIN_BYTES)
        )
    }

    #[test]
    fn extracts_anthropic_mcp_openai_and_gemini_images() {
        let output = json!({
            "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}},
                {"type": "image", "mimeType": "image/jpeg", "data": "BB=="},
                {"type": "input_image", "image_url": "https://example.com/a.png"},
                {"inlineData": {"mimeType": "image/webp", "data": "CC=="}}
            ]
        });
        let extraction = extract_tool_media(&output, ToolMediaScope::AllSupported).unwrap();
        assert_eq!(extraction.media.len(), 4);
        assert!(extraction
            .media
            .iter()
            .all(|media| media.to_chat_part().is_some()));
    }

    #[test]
    fn extracts_media_from_json_string_without_clamping_other_content() {
        let long_text = "a".repeat(16 * 1024);
        let output = json!({
            "content": serde_json::to_string(&json!({
                "parts": [{"inlineData": {"mimeType": "image/png", "data": "AA=="}}],
                "longText": long_text
            })).unwrap()
        });
        let extraction = extract_tool_media(&output, ToolMediaScope::AllSupported).unwrap();
        assert_eq!(extraction.media.len(), 1);
        assert!(extraction.sanitized["content"]
            .as_str()
            .is_some_and(|text| text.contains(&long_text)));
    }

    #[test]
    fn extracts_whole_large_data_url_but_not_embedded_or_small_values() {
        assert!(
            extract_tool_media(&json!(large_image_url()), ToolMediaScope::ImagesOnly).is_some()
        );
        assert!(extract_tool_media(
            &json!(format!("prefix {}", large_image_url())),
            ToolMediaScope::ImagesOnly
        )
        .is_none());
        assert!(extract_tool_media(
            &json!("data:image/png;base64,AA=="),
            ToolMediaScope::ImagesOnly
        )
        .is_none());
    }

    #[test]
    fn no_media_path_is_value_stable() {
        let output = json!({"ok": true, "content": ["plain text", {"url": "https://example.com"}]});
        assert!(extract_tool_media(&output, ToolMediaScope::AllSupported).is_none());
        assert!(!tool_output_contains_media(
            &output,
            ToolMediaScope::AllSupported
        ));
    }

    #[test]
    fn inline_image_scope_rejects_remote_images_files_and_audio() {
        let remote = json!({"type": "image_url", "image_url": "https://example.com/a.png"});
        let file = json!({"type": "input_file", "file_id": "file_1"});
        let audio =
            json!({"type": "input_audio", "input_audio": {"data": "AA==", "format": "wav"}});
        assert!(extract_tool_media(&remote, ToolMediaScope::InlineImagesOnly).is_none());
        assert!(extract_tool_media(&file, ToolMediaScope::InlineImagesOnly).is_none());
        assert!(extract_tool_media(&audio, ToolMediaScope::InlineImagesOnly).is_none());
    }

    #[test]
    fn target_scopes_leave_unsupported_media_in_tool_output() {
        let remote_file = json!({
            "fileData": {"mimeType": "application/pdf", "fileUri": "https://example.com/a.pdf"}
        });
        let audio =
            json!({"type": "input_audio", "input_audio": {"data": "AA==", "format": "wav"}});

        assert!(extract_tool_media(&remote_file, ToolMediaScope::ChatNative).is_none());
        assert!(extract_tool_media(&audio, ToolMediaScope::AnthropicNative).is_none());
        assert!(extract_tool_media(&remote_file, ToolMediaScope::ResponsesNative).is_some());
        assert!(extract_tool_media(&audio, ToolMediaScope::GeminiNative).is_some());

        let mixed = json!({
            "parts": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}},
                {"type": "input_audio", "input_audio": {"data": "a".repeat(16 * 1024), "format": "wav"}}
            ]
        });
        let extraction = extract_tool_media(&mixed, ToolMediaScope::AnthropicNative).unwrap();
        assert_eq!(extraction.media.len(), 1);
        assert_eq!(
            extraction.sanitized.pointer("/parts/1/input_audio/data"),
            mixed.pointer("/parts/1/input_audio/data")
        );
    }

    #[test]
    fn responses_files_support_remote_urls_and_anthropic_omits_null_titles() {
        let remote_file = media_part(&json!({
            "fileData": {"mimeType": "application/pdf", "fileUri": "https://example.com/a.pdf"}
        }))
        .unwrap();

        assert_eq!(
            remote_file.to_responses_part(),
            Some(json!({
                "type": "input_file",
                "file_url": "https://example.com/a.pdf"
            }))
        );
        let anthropic = remote_file.to_anthropic_block().unwrap();
        assert!(anthropic.get("title").is_none());

        let chat_file = media_part(&json!({
            "type": "file",
            "file": {"file_data": "cGRm", "filename": "a.pdf", "mime_type": "application/pdf"}
        }))
        .unwrap();
        assert_eq!(
            chat_file.to_responses_part().unwrap()["file_data"],
            "data:application/pdf;base64,cGRm"
        );

        let null_title = media_part(&json!({
            "type": "document",
            "source": {"type": "base64", "media_type": "application/pdf", "data": "cGRm"},
            "title": null
        }))
        .unwrap()
        .to_anthropic_block()
        .unwrap();
        assert!(null_title.get("title").is_none());
    }

    #[test]
    fn null_and_empty_file_fields_are_not_moved_out_of_tool_output() {
        let output = json!({
            "content": [
                {"type": "input_file", "file_id": null, "file_data": null, "filename": null},
                {"type": "input_file", "file_id": " ", "file_data": "", "filename": " "},
                {
                    "type": "document",
                    "source": {"type": "base64", "media_type": "application/pdf", "data": null},
                    "title": null
                }
            ]
        });

        assert!(extract_tool_media(&output, ToolMediaScope::AllSupported).is_none());
        assert!(!tool_output_contains_media(
            &output,
            ToolMediaScope::AllSupported
        ));

        let valid_id = media_part(&json!({
            "type": "input_file",
            "file_id": "file_1",
            "filename": null
        }))
        .unwrap()
        .to_responses_part()
        .unwrap();
        assert_eq!(valid_id["file_id"], "file_1");
        assert!(valid_id.get("filename").is_none());
    }

    #[test]
    fn null_empty_and_non_string_audio_fields_are_not_moved_out_of_tool_output() {
        let output = json!({
            "content": [
                {"type": "input_audio", "input_audio": {"data": null, "format": "wav"}},
                {"type": "input_audio", "input_audio": {"data": "AA==", "format": null}},
                {"type": "input_audio", "input_audio": {"data": " ", "format": "wav"}},
                {"type": "input_audio", "input_audio": {"data": "AA==", "format": ""}},
                {"type": "input_audio", "input_audio": {"data": ["AA=="], "format": "wav"}}
            ]
        });

        assert!(extract_tool_media(&output, ToolMediaScope::AllSupported).is_none());
        assert!(!tool_output_contains_media(
            &output,
            ToolMediaScope::AllSupported
        ));
    }

    #[test]
    fn proxy_bridge_contract_fixture_emits_native_tool_media() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/tool_media.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "tool-result-native-media");
        assert_eq!(fixture["category"], "tool_media");
        assert_eq!(fixture["scope"], "all_supported");

        let extraction =
            extract_tool_media(&fixture["output"], ToolMediaScope::AllSupported).unwrap();
        assert_eq!(extraction.sanitized, fixture["expected"]["sanitized"]);
        let chat = extraction
            .media
            .iter()
            .filter_map(ToolMediaPart::to_chat_part)
            .collect::<Vec<_>>();
        let responses = extraction
            .media
            .iter()
            .filter_map(ToolMediaPart::to_responses_part)
            .collect::<Vec<_>>();
        let anthropic = extraction
            .media
            .iter()
            .filter_map(ToolMediaPart::to_anthropic_block)
            .collect::<Vec<_>>();
        let gemini = extraction
            .media
            .iter()
            .filter_map(ToolMediaPart::to_gemini_part)
            .collect::<Vec<_>>();

        assert_eq!(Value::Array(chat), fixture["expected"]["chatParts"]);
        assert_eq!(
            Value::Array(responses),
            fixture["expected"]["responsesParts"]
        );
        assert_eq!(
            Value::Array(anthropic),
            fixture["expected"]["anthropicBlocks"]
        );
        assert_eq!(Value::Array(gemini), fixture["expected"]["geminiParts"]);
    }
}
