use super::*;
use crate::domain::providers::model::AppKind;

pub(super) fn validate_image_url(value: &str) -> Result<(), String> {
    if value.starts_with("data:") {
        let (mime, data) = value
            .strip_prefix("data:")
            .and_then(|value| value.split_once(";base64,"))
            .ok_or("Trae image data URI must contain base64 image data")?;
        return validate_inline_image(mime, data);
    }
    let url = url::Url::parse(value).map_err(|_| "Trae image URL is invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Trae image URL must use HTTP(S) without embedded credentials".into());
    }
    Ok(())
}

fn validate_inline_image(mime: &str, data: &str) -> Result<(), String> {
    // Validate with fixed-size scratch space rather than allocating a second
    // decoded copy of every image outside the request's conversion budget.
    let mut decoder = base64::read::DecoderReader::new(
        data.as_bytes(),
        &base64::engine::general_purpose::STANDARD,
    );
    if !mime.starts_with("image/")
        || mime.len() <= "image/".len()
        || data.is_empty()
        || std::io::copy(&mut decoder, &mut std::io::sink()).is_err()
    {
        return Err(
            "Trae inline image requires an image MIME type and valid nonempty base64".into(),
        );
    }
    Ok(())
}

/// Inspect explicit protocol content blocks, including tool-result content.
/// Opaque tool schemas and arbitrary result JSON are never recursively inspected.
pub(super) fn validate_source_media(app: AppKind, input: &Value) -> Result<(), String> {
    if app == AppKind::Gemini {
        for content in input
            .get("contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for part in content
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                validate_gemini_part(part)?;
                if let Some(parts) = part
                    .get("functionResponse")
                    .or_else(|| part.get("function_response"))
                    .and_then(|response| response.get("parts"))
                    .and_then(Value::as_array)
                {
                    for part in parts {
                        validate_gemini_part(part)?;
                    }
                }
            }
        }
        return Ok(());
    }
    for key in ["messages", "input"] {
        let Some(items) = input.get(key) else {
            continue;
        };
        match items {
            Value::Array(items) => {
                for item in items {
                    validate_item(item)?;
                }
            }
            Value::Object(_) => validate_item(items)?,
            _ => {}
        }
    }
    Ok(())
}

fn validate_item(item: &Value) -> Result<(), String> {
    if item.get("type").and_then(Value::as_str) == Some("function_call_output") {
        // Typed output arrays can carry images; strings/objects remain opaque.
        return validate_blocks(item.get("output"));
    }
    validate_part(item)?;
    validate_blocks(item.get("content"))
}

fn validate_blocks(content: Option<&Value>) -> Result<(), String> {
    for part in content.and_then(Value::as_array).into_iter().flatten() {
        validate_part(part)?;
    }
    Ok(())
}

fn validate_part(part: &Value) -> Result<(), String> {
    match part.get("type").and_then(Value::as_str) {
        Some("image_url") => validate_image_url(
            part.pointer("/image_url/url")
                .and_then(Value::as_str)
                .ok_or("Trae image_url is missing URL")?,
        )?,
        Some("input_image") => validate_image_url(
            part.get("image_url")
                .and_then(Value::as_str)
                .ok_or("Trae input_image requires image_url; file IDs are unsupported")?,
        )?,
        Some("image") => {
            let source = part.get("source").ok_or("Trae image is missing source")?;
            match source.get("type").and_then(Value::as_str) {
                Some("base64") => validate_inline_image(
                    source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .ok_or("Trae image source is missing media_type")?,
                    source
                        .get("data")
                        .and_then(Value::as_str)
                        .ok_or("Trae image source is missing data")?,
                )?,
                Some("url") => validate_image_url(
                    source
                        .get("url")
                        .and_then(Value::as_str)
                        .ok_or("Trae image source is missing URL")?,
                )?,
                _ => return Err("Trae image source type is unsupported".into()),
            }
        }
        Some("input_audio" | "audio" | "video" | "document" | "file" | "input_file") => {
            return Err("Trae supports text and image media only".into())
        }
        Some("tool_result") => validate_blocks(part.get("content"))?,
        _ => {}
    }
    Ok(())
}

fn validate_gemini_part(part: &Value) -> Result<(), String> {
    for key in ["inlineData", "inline_data"] {
        if let Some(image) = part.get(key) {
            let mime = image
                .get("mimeType")
                .or_else(|| image.get("mime_type"))
                .and_then(Value::as_str)
                .ok_or("Trae inlineData is missing mimeType")?;
            let data = image
                .get("data")
                .and_then(Value::as_str)
                .ok_or("Trae inlineData is missing data")?;
            validate_inline_image(mime, data)?;
        }
    }
    for key in ["fileData", "file_data"] {
        if let Some(image) = part.get(key) {
            let mime = image
                .get("mimeType")
                .or_else(|| image.get("mime_type"))
                .and_then(Value::as_str)
                .ok_or("Trae fileData is missing mimeType")?;
            if !mime.starts_with("image/") {
                return Err("Trae supports image media only".into());
            }
            let uri = image
                .get("fileUri")
                .or_else(|| image.get("file_uri"))
                .and_then(Value::as_str)
                .ok_or("Trae fileData is missing fileUri")?;
            validate_image_url(uri)?;
        }
    }
    Ok(())
}
