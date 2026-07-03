use std::collections::HashSet;
use std::io::Read;

use axum::http::{HeaderMap, HeaderValue};
use bytes::Bytes;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use serde_json::{Map, Value};

use super::ProxyError;

const UNSUPPORTED_IMAGE_MARKER: &str = "[Unsupported Image]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RequestGovernanceConfig {
    pub(super) body_filter_enabled: bool,
    pub(super) media_sanitizer_enabled: bool,
    pub(super) media_heuristic_enabled: bool,
    pub(super) private_field_whitelist: Vec<String>,
}

impl RequestGovernanceConfig {
    pub(super) fn disabled() -> Self {
        Self {
            body_filter_enabled: false,
            media_sanitizer_enabled: false,
            media_heuristic_enabled: false,
            private_field_whitelist: Vec::new(),
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.body_filter_enabled || self.media_sanitizer_enabled
    }
}

pub(super) fn govern_request_body(
    body: &mut Value,
    settings: &Value,
    config: &RequestGovernanceConfig,
) {
    if !config.is_enabled() {
        return;
    }

    if config.body_filter_enabled {
        filter_private_params_with_whitelist(body, &config.private_field_whitelist);
    }

    if config.media_sanitizer_enabled {
        replace_images_for_text_only_model(body, settings, config.media_heuristic_enabled);
    }
}

pub(super) fn decode_request_body_for_proxy(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Bytes, ProxyError> {
    let Some(codings) = content_encoding_tokens(headers) else {
        return Ok(body);
    };
    if codings.iter().any(|coding| !is_supported_coding(coding)) {
        return Err(ProxyError::bad_request(format!(
            "unsupported request content-encoding: {}",
            codings.join(", ")
        )));
    }
    decode_content_codings(body, &codings).map_err(|error| {
        ProxyError::bad_request(format!(
            "failed to decode request content-encoding {}: {error}",
            codings.join(", ")
        ))
    })
}

pub(super) struct ResponseDecodeResult {
    pub(super) body: Bytes,
    pub(super) preserve_content_encoding: bool,
}

pub(super) fn decode_response_body_for_proxy(
    headers: &HeaderMap,
    body: Bytes,
) -> ResponseDecodeResult {
    let Some(codings) = content_encoding_tokens(headers) else {
        return ResponseDecodeResult {
            body,
            preserve_content_encoding: false,
        };
    };
    if codings.iter().any(|coding| !is_supported_coding(coding)) {
        return ResponseDecodeResult {
            body,
            preserve_content_encoding: true,
        };
    }
    match decode_content_codings(body.clone(), &codings) {
        Ok(decoded) => ResponseDecodeResult {
            body: decoded,
            preserve_content_encoding: false,
        },
        Err(_) => ResponseDecodeResult {
            body,
            preserve_content_encoding: true,
        },
    }
}

pub(super) fn content_encoding_value(headers: &HeaderMap) -> Option<HeaderValue> {
    headers.get(axum::http::header::CONTENT_ENCODING).cloned()
}

fn filter_private_params_with_whitelist(body: &mut Value, whitelist: &[String]) {
    let whitelist = whitelist.iter().map(String::as_str).collect::<HashSet<_>>();
    filter_private_params_in_place(body, &whitelist, false);
}

fn filter_private_params_in_place(
    value: &mut Value,
    whitelist: &HashSet<&str>,
    preserve_object_keys: bool,
) {
    match value {
        Value::Object(object) => {
            let keys = object.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if !preserve_object_keys
                    && is_private_key(&key)
                    && !whitelist.contains(key.as_str())
                {
                    object.remove(&key);
                    continue;
                }
                let child_preserves_schema_names = is_json_schema_name_map(&key);
                if let Some(child) = object.get_mut(&key) {
                    filter_private_params_in_place(child, whitelist, child_preserves_schema_names);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                filter_private_params_in_place(item, whitelist, false);
            }
        }
        _ => {}
    }
}

fn is_private_key(key: &str) -> bool {
    key.starts_with('_')
}

fn is_json_schema_name_map(key: &str) -> bool {
    matches!(
        key,
        "properties" | "patternProperties" | "definitions" | "$defs"
    )
}

fn replace_images_for_text_only_model(body: &mut Value, settings: &Value, allow_heuristic: bool) {
    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return;
    };

    let should_replace = match explicit_image_support_for_model(settings, model) {
        Some(true) => false,
        Some(false) => true,
        None => allow_heuristic && is_known_text_only_model(model),
    };
    if should_replace {
        replace_image_blocks(body);
    }
}

fn explicit_image_support_for_model(settings: &Value, model: &str) -> Option<bool> {
    let mut support = None;
    for source in [
        settings
            .get("modelCatalog")
            .or_else(|| settings.get("model_catalog")),
        settings.get("models"),
    ]
    .into_iter()
    .flatten()
    {
        support = support.or_else(|| explicit_image_support_from_source(source, model));
    }
    support
}

fn explicit_image_support_from_source(source: &Value, model: &str) -> Option<bool> {
    if let Some(models) = source.get("models") {
        if let Some(support) = explicit_image_support_from_source(models, model) {
            return Some(support);
        }
    }

    match source {
        Value::Array(items) => items
            .iter()
            .find(|entry| model_entry_matches(entry, None, model))
            .and_then(image_support_from_model_entry),
        Value::Object(object) => object.iter().find_map(|(key, entry)| {
            if model_entry_matches(entry, Some(key), model) {
                image_support_from_model_entry(entry)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn model_entry_matches(entry: &Value, key: Option<&str>, model: &str) -> bool {
    if key.is_some_and(|key| same_model_name(key, model)) {
        return true;
    }
    if let Some(model_name) = entry.as_str() {
        return same_model_name(model_name, model);
    }
    [
        "id",
        "name",
        "model",
        "upstreamModel",
        "upstream_model",
        "actualModel",
        "actual_model",
    ]
    .iter()
    .any(|key| {
        entry
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| same_model_name(value, model))
    })
}

fn same_model_name(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn image_support_from_model_entry(entry: &Value) -> Option<bool> {
    for key in ["supportsImage", "supports_image", "vision"] {
        if let Some(value) = entry.get(key).and_then(value_as_bool) {
            return Some(value);
        }
    }

    [
        entry.get("input"),
        entry.pointer("/modalities/input"),
        entry.get("input_modalities"),
        entry.get("inputModalities"),
    ]
    .into_iter()
    .flatten()
    .next()
    .map(modality_value_contains_image)
}

fn modality_value_contains_image(value: &Value) -> bool {
    match value {
        Value::String(item) => item.trim().eq_ignore_ascii_case("image"),
        Value::Array(items) => items.iter().any(modality_value_contains_image),
        Value::Object(object) => object.values().any(modality_value_contains_image),
        _ => false,
    }
}

fn replace_image_blocks(value: &mut Value) {
    if let Some(messages) = value.get_mut("messages") {
        replace_images_in_message_list(messages);
    }
    if let Some(input) = value.get_mut("input") {
        replace_images_in_content_value(input);
    }
    if let Some(content) = value.get_mut("content") {
        replace_images_in_content_value(content);
    }
}

fn replace_images_in_message_list(value: &mut Value) {
    let Value::Array(messages) = value else {
        return;
    };
    for message in messages {
        if let Some(content) = message.get_mut("content") {
            replace_images_in_content_value(content);
        }
    }
}

fn replace_images_in_content_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(replacement) = image_block_replacement(object) {
                *value = replacement;
                return;
            }
            if let Some(content) = object.get_mut("content") {
                replace_images_in_content_value(content);
            }
        }
        Value::Array(items) => {
            for item in items {
                replace_images_in_content_value(item);
            }
        }
        _ => {}
    }
}

fn image_block_replacement(object: &Map<String, Value>) -> Option<Value> {
    let block_type = object.get("type").and_then(Value::as_str)?;
    let replacement_type = match block_type {
        "image" | "image_url" => "text",
        "input_image" => "input_text",
        _ => return None,
    };
    let mut replacement = Map::new();
    replacement.insert(
        "type".to_string(),
        Value::String(replacement_type.to_string()),
    );
    replacement.insert(
        "text".to_string(),
        Value::String(UNSUPPORTED_IMAGE_MARKER.to_string()),
    );
    if let Some(cache_control) = object.get("cache_control") {
        replacement.insert("cache_control".to_string(), cache_control.clone());
    }
    Some(Value::Object(replacement))
}

fn is_known_text_only_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    let tail = normalized.rsplit('/').next().unwrap_or(&normalized).trim();
    let exact_tails = [
        "ark-code-latest",
        "deepseek-chat",
        "deepseek-reasoner",
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        "glm-5.1",
        "kat-coder",
        "kat-coder-pro",
        "kat-coder-pro v1",
        "kat-coder-pro v2",
        "kat-coder-pro-v1",
        "kat-coder-pro-v2",
        "ling-2.5-1t",
        "longcat-flash-chat",
        "mimo-v2.5-pro",
        "us.deepseek.r1-v1",
    ];
    if exact_tails.contains(&tail) {
        return true;
    }
    ["minimax-m2.7", "qwen3-coder", "step-3.5-flash"]
        .iter()
        .any(|prefix| tail.starts_with(prefix))
}

fn content_encoding_tokens(headers: &HeaderMap) -> Option<Vec<String>> {
    let value = headers
        .get(axum::http::header::CONTENT_ENCODING)?
        .to_str()
        .ok()?;
    let codings = value
        .split(',')
        .map(|coding| coding.trim().to_ascii_lowercase())
        .filter(|coding| !coding.is_empty() && coding != "identity")
        .collect::<Vec<_>>();
    if codings.is_empty() {
        None
    } else {
        Some(codings)
    }
}

fn is_supported_coding(coding: &str) -> bool {
    matches!(coding, "gzip" | "x-gzip" | "deflate")
}

fn decode_content_codings(body: Bytes, codings: &[String]) -> std::io::Result<Bytes> {
    let mut current = body.to_vec();
    for coding in codings.iter().rev() {
        current = decode_single_coding(coding, &current)?;
    }
    Ok(Bytes::from(current))
}

fn decode_single_coding(coding: &str, body: &[u8]) -> std::io::Result<Vec<u8>> {
    match coding {
        "gzip" | "x-gzip" => read_all(GzDecoder::new(body)),
        "deflate" => {
            read_all(ZlibDecoder::new(body)).or_else(|_| read_all(DeflateDecoder::new(body)))
        }
        _ => Ok(body.to_vec()),
    }
}

fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn value_as_bool(value: &Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    let value = value.as_str()?.trim().to_ascii_lowercase();
    if ["true", "1", "yes", "on", "enabled"].contains(&value.as_str()) {
        return Some(true);
    }
    if ["false", "0", "no", "off", "disabled"].contains(&value.as_str()) {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::http::header::CONTENT_ENCODING;
    use axum::http::{HeaderMap, HeaderValue};
    use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
    use flate2::Compression;
    use serde_json::json;

    use super::*;

    fn governance_config() -> RequestGovernanceConfig {
        RequestGovernanceConfig {
            body_filter_enabled: true,
            media_sanitizer_enabled: true,
            media_heuristic_enabled: false,
            private_field_whitelist: vec!["_metadata".to_string()],
        }
    }

    #[test]
    fn filters_private_params_but_preserves_schema_property_names() {
        let mut body = json!({
            "_internal": true,
            "_metadata": {"kept": true},
            "tools": [{
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "_privatePropertyName": {
                            "type": "string",
                            "_internal_note": "drop"
                        }
                    },
                    "$defs": {
                        "_definitionName": {
                            "type": "object",
                            "_internal_note": "drop"
                        }
                    }
                }
            }]
        });

        govern_request_body(
            &mut body,
            &json!({}),
            &RequestGovernanceConfig {
                media_sanitizer_enabled: false,
                ..governance_config()
            },
        );

        assert!(body.get("_internal").is_none());
        assert_eq!(body["_metadata"]["kept"], true);
        assert!(body
            .pointer("/tools/0/input_schema/properties/_privatePropertyName")
            .is_some());
        assert!(body
            .pointer("/tools/0/input_schema/properties/_privatePropertyName/_internal_note")
            .is_none());
        assert!(body
            .pointer("/tools/0/input_schema/$defs/_definitionName")
            .is_some());
        assert!(body
            .pointer("/tools/0/input_schema/$defs/_definitionName/_internal_note")
            .is_none());
    }

    #[test]
    fn explicit_text_only_model_replaces_images_and_preserves_cache_control() {
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "tools": [{
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "image"}
                    }
                }
            }],
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {
                        "type": "image_url",
                        "image_url": {"url": "data:image/png;base64,AA=="},
                        "cache_control": {"type": "ephemeral"}
                    }
                ]
            }]
        });
        let settings = json!({
            "modelCatalog": {
                "models": [
                    {"id": "deepseek-v4-pro", "supportsImage": false}
                ]
            }
        });

        govern_request_body(&mut body, &settings, &governance_config());

        assert_eq!(
            body.pointer("/messages/0/content/1/type")
                .and_then(Value::as_str),
            Some("text")
        );
        assert_eq!(
            body.pointer("/messages/0/content/1/text")
                .and_then(Value::as_str),
            Some(UNSUPPORTED_IMAGE_MARKER)
        );
        assert_eq!(
            body.pointer("/messages/0/content/1/cache_control/type")
                .and_then(Value::as_str),
            Some("ephemeral")
        );
        assert_eq!(
            body.pointer("/tools/0/input_schema/properties/kind/type")
                .and_then(Value::as_str),
            Some("image")
        );
    }

    #[test]
    fn vision_model_catalog_entry_preserves_images() {
        let mut body = json!({
            "model": "glm-5.1",
            "messages": [{
                "role": "user",
                "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}]
            }]
        });
        let settings = json!({
            "modelCatalog": {
                "models": [{"id": "glm-5.1", "inputModalities": ["text", "image"]}]
            }
        });

        govern_request_body(&mut body, &settings, &governance_config());

        assert_eq!(
            body.pointer("/messages/0/content/0/type")
                .and_then(Value::as_str),
            Some("image_url")
        );
    }

    #[test]
    fn heuristic_text_only_replacement_requires_opt_in() {
        let source = json!({
            "model": "openrouter/qwen3-coder-plus",
            "input": [{
                "role": "user",
                "content": [{"type": "input_image", "image_url": "data:image/png;base64,AA=="}]
            }]
        });
        let mut disabled = source.clone();
        govern_request_body(&mut disabled, &json!({}), &governance_config());
        assert_eq!(
            disabled
                .pointer("/input/0/content/0/type")
                .and_then(Value::as_str),
            Some("input_image")
        );

        let mut enabled = source;
        govern_request_body(
            &mut enabled,
            &json!({}),
            &RequestGovernanceConfig {
                media_heuristic_enabled: true,
                ..governance_config()
            },
        );
        assert_eq!(
            enabled
                .pointer("/input/0/content/0/type")
                .and_then(Value::as_str),
            Some("input_text")
        );
        assert_eq!(
            enabled
                .pointer("/input/0/content/0/text")
                .and_then(Value::as_str),
            Some(UNSUPPORTED_IMAGE_MARKER)
        );
    }

    #[test]
    fn decodes_gzip_and_deflate_request_bodies() {
        let body = br#"{"model":"test"}"#;
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(body).unwrap();
        let gzip = gzip.finish().unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(
            decode_request_body_for_proxy(&headers, Bytes::from(gzip)).unwrap(),
            Bytes::from_static(body)
        );

        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(body).unwrap();
        let zlib = zlib.finish().unwrap();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("deflate"));
        assert_eq!(
            decode_request_body_for_proxy(&headers, Bytes::from(zlib)).unwrap(),
            Bytes::from_static(body)
        );

        let mut raw = DeflateEncoder::new(Vec::new(), Compression::default());
        raw.write_all(body).unwrap();
        let raw = raw.finish().unwrap();
        assert_eq!(
            decode_request_body_for_proxy(&headers, Bytes::from(raw)).unwrap(),
            Bytes::from_static(body)
        );
    }

    #[test]
    fn response_decode_preserves_unsupported_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("br"));
        let decoded = decode_response_body_for_proxy(&headers, Bytes::from_static(b"opaque"));

        assert_eq!(decoded.body, Bytes::from_static(b"opaque"));
        assert!(decoded.preserve_content_encoding);
    }
}
