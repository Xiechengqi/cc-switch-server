use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::proxy::ProxyError;

const MAX_IMAGES: usize = 20;
const MAX_ENCODED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 20 * 1024 * 1024;
const RESIZE_THRESHOLD_BYTES: usize = 400_000;
const MAX_LONG_SIDE: u32 = 1568;
const MAX_DECODE_LONG_SIDE: usize = 16_384;
const MAX_DECODE_PIXELS: usize = 64 * 1024 * 1024;
const JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone)]
struct PreparedImage {
    media_type: &'static str,
    data: String,
    decoded_bytes: usize,
}

pub(super) fn prepare_anthropic_images(body: &mut Value) -> Result<(), ProxyError> {
    let mut hashes = Vec::new();
    visit_image_blocks(body, &mut |block| {
        let data = image_data(block).ok_or_else(|| {
            ProxyError::bad_request("Anthropic image block must contain string base64 source.data")
        })?;
        hashes.push(hash(data));
        Ok(())
    })?;
    if hashes.len() > MAX_IMAGES {
        return Err(ProxyError::bad_request(format!(
            "Kiro request contains {} images; maximum is {MAX_IMAGES}",
            hashes.len()
        )));
    }
    let mut last_occurrence = HashMap::new();
    for (index, digest) in hashes.into_iter().enumerate() {
        last_occurrence.insert(digest, index);
    }

    let mut ordinal = 0usize;
    let mut total_bytes = 0usize;
    visit_image_blocks(body, &mut |block| {
        let data = image_data(block)
            .expect("image blocks were validated in the first pass")
            .to_string();
        let digest = hash(&data);
        let keep = last_occurrence.get(&digest).copied() == Some(ordinal);
        ordinal = ordinal.saturating_add(1);
        if !keep {
            *block = json!({
                "type": "text",
                "text": "[image omitted: identical to a newer screenshot]"
            });
            return Ok(());
        }
        let prepared = prepare_image(&data)?;
        total_bytes = total_bytes.saturating_add(prepared.decoded_bytes);
        if total_bytes > MAX_TOTAL_OUTPUT_BYTES {
            return Err(ProxyError::bad_request(format!(
                "Kiro image payload exceeds {} MiB after processing",
                MAX_TOTAL_OUTPUT_BYTES / (1024 * 1024)
            )));
        }
        let Some(source) = block.get_mut("source").and_then(Value::as_object_mut) else {
            return Err(ProxyError::bad_request(
                "Anthropic image block must contain a base64 source",
            ));
        };
        source.insert("type".to_string(), json!("base64"));
        source.insert("media_type".to_string(), json!(prepared.media_type));
        source.insert("data".to_string(), json!(prepared.data));
        Ok(())
    })
}

fn prepare_image(data: &str) -> Result<PreparedImage, ProxyError> {
    if data.len() > MAX_ENCODED_INPUT_BYTES.saturating_mul(4) / 3 + 8 {
        return Err(ProxyError::bad_request(format!(
            "Kiro image exceeds {} MiB encoded input limit",
            MAX_ENCODED_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let decoded = BASE64
        .decode(data.as_bytes())
        .map_err(|_| ProxyError::bad_request("Anthropic image source is not valid base64"))?;
    if decoded.len() > MAX_ENCODED_INPUT_BYTES {
        return Err(ProxyError::bad_request(format!(
            "Kiro image exceeds {} MiB decoded input limit",
            MAX_ENCODED_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let format = magic_format(&decoded)
        .ok_or_else(|| ProxyError::bad_request("Anthropic image has an unsupported byte format"))?;
    let dimensions = imagesize::blob_size(&decoded)
        .map_err(|_| ProxyError::bad_request("Anthropic image dimensions could not be read"))?;
    if dimensions.width > MAX_DECODE_LONG_SIDE
        || dimensions.height > MAX_DECODE_LONG_SIDE
        || dimensions.width.saturating_mul(dimensions.height) > MAX_DECODE_PIXELS
    {
        return Err(ProxyError::bad_request(
            "Anthropic image dimensions exceed the Kiro decode budget",
        ));
    }
    let long_side = dimensions.width.max(dimensions.height) as u32;
    if format == "image/gif"
        || (decoded.len() <= RESIZE_THRESHOLD_BYTES && long_side <= MAX_LONG_SIDE)
    {
        return Ok(PreparedImage {
            media_type: format,
            data: BASE64.encode(&decoded),
            decoded_bytes: decoded.len(),
        });
    }

    let image = image::load_from_memory(&decoded)
        .map_err(|_| ProxyError::bad_request("Anthropic image could not be decoded"))?;
    let resized = if image.width().max(image.height()) > MAX_LONG_SIDE {
        image.resize(MAX_LONG_SIDE, MAX_LONG_SIDE, FilterType::Triangle)
    } else {
        image
    };
    let rgb = resized.to_rgb8();
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| ProxyError::bad_request("Anthropic image could not be re-encoded"))?;
    Ok(PreparedImage {
        media_type: "image/jpeg",
        data: BASE64.encode(&encoded),
        decoded_bytes: encoded.len(),
    })
}

fn magic_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn hash(data: &str) -> [u8; 32] {
    Sha256::digest(data.as_bytes()).into()
}

fn image_data(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("image"))
        .then(|| block.pointer("/source/data").and_then(Value::as_str))
        .flatten()
}

fn visit_image_blocks(
    body: &mut Value,
    visitor: &mut impl FnMut(&mut Value) -> Result<(), ProxyError>,
) -> Result<(), ProxyError> {
    if let Some(system) = body.get_mut("system") {
        visit_content_blocks(system, visitor)?;
    }
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for message in messages {
        if let Some(content) = message.get_mut("content") {
            visit_content_blocks(content, visitor)?;
        }
    }
    Ok(())
}

fn visit_content_blocks(
    value: &mut Value,
    visitor: &mut impl FnMut(&mut Value) -> Result<(), ProxyError>,
) -> Result<(), ProxyError> {
    match value {
        Value::Array(items) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("image") => visitor(item)?,
                    Some("tool_result") => {
                        if let Some(content) = item.get_mut("content") {
                            visit_content_blocks(content, visitor)?;
                        }
                    }
                    _ => {}
                }
            }
        }
        Value::Object(_) => match value.get("type").and_then(Value::as_str) {
            Some("image") => visitor(value)?,
            Some("tool_result") => {
                if let Some(content) = value.get_mut("content") {
                    visit_content_blocks(content, visitor)?;
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE_PROTOCOL_JSON: &str =
        include_str!("../../../assets/contract/kiro-wire-protocol.json");

    const PNG_1X1: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    #[test]
    fn corrects_mime_and_keeps_only_newest_duplicate() {
        let mut body = json!({
            "messages": [
                {"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":PNG_1X1}}]},
                {"role":"assistant","content":"seen"},
                {"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":PNG_1X1}}]}
            ]
        });
        prepare_anthropic_images(&mut body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/0/type"),
            Some(&json!("text"))
        );
        assert_eq!(
            body.pointer("/messages/2/content/0/source/media_type"),
            Some(&json!("image/png"))
        );
    }

    #[test]
    fn rejects_invalid_base64_before_forwarding() {
        let mut body = json!({
            "messages": [{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"not base64"}}]}]
        });
        assert!(prepare_anthropic_images(&mut body).is_err());
    }

    #[test]
    fn rejects_image_without_string_base64_data_before_forwarding() {
        for source in [json!({"type": "base64"}), json!({"data": 42})] {
            let mut body = json!({
                "messages": [{"role":"user","content":[{"type":"image","source":source}]}]
            });
            let error = prepare_anthropic_images(&mut body).unwrap_err();
            assert!(error.message.contains("source.data"));
        }
    }

    #[test]
    fn does_not_treat_tool_input_type_fields_as_content_images() {
        let mut body = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "classify",
                        "input": {"type": "image", "label": "diagram"}
                    }]
                },
                {"role": "user", "content": "continue"}
            ]
        });
        prepare_anthropic_images(&mut body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/0/input/type"),
            Some(&json!("image"))
        );
    }

    #[test]
    fn image_budgets_match_the_wire_protocol_fixture() {
        let fixture: Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        let images = &fixture["images"];
        assert_eq!(images["maxImages"], MAX_IMAGES);
        assert_eq!(
            images["maxDecodedInputBytesPerImage"],
            MAX_ENCODED_INPUT_BYTES
        );
        assert_eq!(images["maxTotalOutputBytes"], MAX_TOTAL_OUTPUT_BYTES);
        assert_eq!(images["resizeThresholdBytes"], RESIZE_THRESHOLD_BYTES);
        assert_eq!(images["maxLongSide"], MAX_LONG_SIDE);
        assert_eq!(images["maxDecodeLongSide"], MAX_DECODE_LONG_SIDE);
        assert_eq!(images["maxDecodePixels"], MAX_DECODE_PIXELS);
        assert_eq!(images["jpegQuality"], JPEG_QUALITY);
        assert_eq!(images["mimeFromMagicBytes"], true);
        assert_eq!(images["duplicatePolicy"], "keep_newest");
    }
}
