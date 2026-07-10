use axum::http::{HeaderMap, StatusCode};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use bytes::Bytes;
use rand::RngCore;
use serde_json::{json, Map, Value};

use super::{ProxyError, ProxyRoute};

const DEFAULT_GROK_MODEL: &str = "grok-4.3";
const GROK_API_BASE: &str = "https://api.x.ai/v1";
const GROK_WS_URL: &str = "wss://api.x.ai/v1/responses";

pub(super) struct GrokForwardContract {
    pub session_id: Option<String>,
    pub headers: Vec<(&'static str, String)>,
}

pub(super) fn apply_forward_contract(
    body: &mut Bytes,
    downstream_headers: &HeaderMap,
    route: ProxyRoute,
    downstream_session_id: Option<&str>,
) -> Result<GrokForwardContract, ProxyError> {
    patch_grok_request_body(body, route)?;
    let model = request_model(body).unwrap_or_else(|| DEFAULT_GROK_MODEL.to_string());
    let session_id = grok_session_id(downstream_headers, downstream_session_id, &model);
    if let Some(session_id) = session_id.as_deref() {
        inject_prompt_cache_key(body, session_id)?;
    }
    let mut headers = vec![
        ("accept", "application/json, text/event-stream".to_string()),
        ("user-agent", "cc-switch-server-grok/1.0".to_string()),
    ];
    if let Some(openai_beta) = header_string(downstream_headers, "openai-beta") {
        headers.push(("openai-beta", openai_beta));
    }
    if let Some(session_id) = session_id.clone() {
        headers.push(("x-grok-conv-id", session_id));
    }
    Ok(GrokForwardContract {
        session_id,
        headers,
    })
}

pub(super) fn websocket_url() -> &'static str {
    GROK_WS_URL
}

pub(super) fn default_base_url() -> &'static str {
    GROK_API_BASE
}

pub(super) fn upstream_media_url(path: &str) -> String {
    let path = path.trim_start_matches('/');
    format!("{GROK_API_BASE}/{path}")
}

pub(super) fn patch_grok_request_body(
    body: &mut Bytes,
    route: ProxyRoute,
) -> Result<(), ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("Grok request body must be valid JSON: {error}"))
    })?;
    patch_grok_request_value(&mut value, route);
    *body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("Grok request encode failed: {error}")))?;
    Ok(())
}

pub(super) fn inject_prompt_cache_key(
    body: &mut Bytes,
    session_id: &str,
) -> Result<(), ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("Grok request body must be valid JSON: {error}"))
    })?;
    if let Some(object) = value.as_object_mut() {
        object
            .entry("prompt_cache_key".to_string())
            .or_insert_with(|| Value::String(session_id.to_string()));
    }
    *body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("Grok request encode failed: {error}")))?;
    Ok(())
}

fn patch_grok_request_value(value: &mut Value, route: ProxyRoute) {
    let model = {
        let Some(object) = value.as_object_mut() else {
            return;
        };
        let model = normalize_grok_model(
            object
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_GROK_MODEL),
        );
        object.insert("model".to_string(), Value::String(model.clone()));
        model
    };
    remove_recursive(value, "external_web_access");
    if let Some(object) = value.as_object_mut() {
        if route != ProxyRoute::CodexChatCompletions {
            object.remove("stream_options");
        }
        object.remove("background");
        remove_keys(
            object,
            &[
                "prompt_cache_retention",
                "safety_identifier",
                "service_tier",
            ],
        );
        sanitize_reasoning(object, &model);
        sanitize_tools(object);
    }
    strip_invalid_encrypted_content(value);
}

pub(super) fn parse_cooldown_until_ms(
    status: StatusCode,
    headers: &HeaderMap,
    now_ms: i64,
) -> Option<(i64, String)> {
    let until = if status == StatusCode::UNAUTHORIZED {
        now_ms.saturating_add(10 * 60_000)
    } else if status == StatusCode::FORBIDDEN {
        now_ms.saturating_add(30 * 60_000)
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        retry_after_until_ms(headers, now_ms)
            .or_else(|| rate_limit_reset_until_ms(headers, now_ms))
            .unwrap_or_else(|| now_ms.saturating_add(60_000))
    } else if status.is_server_error() {
        now_ms.saturating_add(2 * 60_000)
    } else {
        return None;
    };
    Some((until, cooldown_message(status, headers, until)))
}

pub(super) fn image_edit_body(headers: &HeaderMap, body: Bytes) -> Result<Bytes, ProxyError> {
    let content_type = header_string(headers, "content-type").unwrap_or_default();
    if content_type
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
    {
        return multipart_image_edit_body(&content_type, &body);
    }
    let mut output = serde_json::from_slice::<Value>(&body).map_err(|error| {
        ProxyError::bad_request(format!("Grok image edit JSON body is invalid: {error}"))
    })?;
    if let Some(object) = output.as_object_mut() {
        object.remove("quality");
        object.remove("size");
        object.remove("style");
        object.remove("mask");
    }
    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("Grok image edit encode failed: {error}")))
}

fn multipart_image_edit_body(content_type: &str, body: &[u8]) -> Result<Bytes, ProxyError> {
    let boundary = content_type
        .split(';')
        .find_map(|part| part.trim().strip_prefix("boundary="))
        .map(|value| value.trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_request("multipart image edit is missing boundary"))?;
    let mut fields = Map::new();
    let mut image_urls = Vec::new();
    for part in split_multipart_parts(body, &boundary) {
        let Some((headers, data)) = split_part_headers(part) else {
            continue;
        };
        let disposition = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-disposition"))
            .map(|(_, value)| value.as_str())
            .unwrap_or_default();
        let Some(name) = multipart_disposition_param(disposition, "name") else {
            continue;
        };
        let content_type = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("image/png");
        if name == "image" || name == "images" || name == "image[]" {
            if image_urls.len() >= 3 {
                continue;
            }
            let data_url = format!(
                "data:{};base64,{}",
                content_type,
                STANDARD.encode(trim_part_data(data))
            );
            image_urls.push(json!({"type": "image_url", "url": data_url}));
        } else if !matches!(name.as_str(), "quality" | "size" | "style" | "mask") {
            let text = String::from_utf8_lossy(trim_part_data(data))
                .trim()
                .to_string();
            fields.insert(name, Value::String(text));
        }
    }
    match image_urls.len() {
        0 => {
            return Err(ProxyError::bad_request(
                "multipart image edit is missing image field",
            ));
        }
        1 => {
            fields.insert("image".to_string(), image_urls.remove(0));
        }
        _ => {
            fields.insert("image_urls".to_string(), Value::Array(image_urls));
        }
    }
    serde_json::to_vec(&Value::Object(fields))
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_request(format!("Grok multipart image edit encode failed: {error}"))
        })
}

fn split_multipart_parts<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let marker = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    while let Some(start_rel) = find_bytes(&body[cursor..], &marker) {
        let start = cursor + start_rel + marker.len();
        if body.get(start..start + 2) == Some(b"--") {
            break;
        }
        let content_start = if body.get(start..start + 2) == Some(b"\r\n") {
            start + 2
        } else if body.get(start..start + 1) == Some(b"\n") {
            start + 1
        } else {
            start
        };
        let Some(end_rel) = find_bytes(&body[content_start..], &marker) else {
            break;
        };
        let mut content_end = content_start + end_rel;
        while content_end > content_start && matches!(body[content_end - 1], b'\r' | b'\n') {
            content_end -= 1;
        }
        parts.push(&body[content_start..content_end]);
        cursor = content_start + end_rel;
    }
    parts
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

type MultipartHeaders = Vec<(String, String)>;
type MultipartPart<'a> = (MultipartHeaders, &'a [u8]);

fn split_part_headers(part: &[u8]) -> Option<MultipartPart<'_>> {
    let split = part
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            part.windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })?;
    let header_text = String::from_utf8_lossy(&part[..split.0]);
    let headers = header_text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    Some((headers, &part[split.0 + split.1..]))
}

fn multipart_disposition_param(disposition: &str, name: &str) -> Option<String> {
    disposition.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key.trim() == name).then(|| value.trim().trim_matches('"').to_string())
    })
}

fn trim_part_data(data: &[u8]) -> &[u8] {
    let mut end = data.len();
    while end > 0 && matches!(data[end - 1], b'\r' | b'\n') {
        end -= 1;
    }
    &data[..end]
}

pub(super) fn sticky_media_session_key(path: &str, body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok();
    if path.contains("/videos/") {
        if let Some(request_id) = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|item| !item.is_empty() && *item != "generations")
        {
            return Some(format!("grok-video:{request_id}"));
        }
        if let Some(request_id) = value
            .as_ref()
            .and_then(|value| value.get("request_id"))
            .and_then(Value::as_str)
        {
            return Some(format!("grok-video:{request_id}"));
        }
    }
    None
}

pub(super) fn video_session_key_from_response(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    [
        "/request_id",
        "/requestId",
        "/id",
        "/data/request_id",
        "/data/requestId",
        "/data/id",
    ]
    .into_iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .map(str::trim)
    .filter(|request_id| !request_id.is_empty())
    .map(|request_id| format!("grok-video:{request_id}"))
}

pub(super) fn ws_request_body(mut value: Value, session_id: Option<&str>) -> Value {
    patch_grok_request_value(&mut value, ProxyRoute::CodexResponses);
    if let Some(session_id) = session_id {
        if let Some(object) = value.as_object_mut() {
            object
                .entry("prompt_cache_key".to_string())
                .or_insert_with(|| Value::String(session_id.to_string()));
        }
    }
    remove_recursive(&mut value, "stream_options");
    remove_recursive(&mut value, "background");
    if let Some(object) = value.as_object_mut() {
        object.remove("stream");
        object.insert("store".to_string(), Value::Bool(true));
        if object.get("previous_response_id").is_some() {
            object.remove("instructions");
        }
    }
    json!({
        "type": "response.create",
        "response": value,
    })
}

pub(super) fn ws_message_body(mut value: Value, session_id: Option<&str>) -> Value {
    if value.get("type").is_none() {
        return ws_request_body(value, session_id);
    }
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|event_type| event_type == "response.create")
    {
        if let Some(response) = value.get_mut("response") {
            patch_grok_request_value(response, ProxyRoute::CodexResponses);
            if let Some(session_id) = session_id {
                if let Some(object) = response.as_object_mut() {
                    object
                        .entry("prompt_cache_key".to_string())
                        .or_insert_with(|| Value::String(session_id.to_string()));
                }
            }
        }
    }
    value
}

pub(super) fn new_session_id() -> String {
    random_session_id()
}

fn request_model(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn normalize_grok_model(model: &str) -> String {
    match model.trim() {
        "" | "grok" | "grok-latest" => DEFAULT_GROK_MODEL.to_string(),
        "grok-build" => "grok-build-0.1".to_string(),
        "grok-composer" => "grok-composer-2.5-fast".to_string(),
        "grok-4.20-reasoning" => "grok-4.20-0309-reasoning".to_string(),
        "grok-4.20-non-reasoning" => "grok-4.20-0309-non-reasoning".to_string(),
        other => other.to_string(),
    }
}

fn grok_session_id(
    headers: &HeaderMap,
    downstream_session_id: Option<&str>,
    model: &str,
) -> Option<String> {
    if model.starts_with("grok-composer-") {
        return Some(random_session_id());
    }
    header_string(headers, "x-grok-conv-id")
        .or_else(|| header_string(headers, "x-session-id"))
        .or_else(|| downstream_session_id.map(str::to_string))
        .or_else(|| Some(random_session_id()))
}

fn random_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sanitize_reasoning(object: &mut Map<String, Value>, model: &str) {
    if grok_model_supports_reasoning_effort(model) {
        return;
    }
    object.remove("reasoning");
    object.remove("reasoning_effort");
}

fn grok_model_supports_reasoning_effort(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("grok-3-mini")
        || model.starts_with("grok-4.20-multi-agent")
        || model.starts_with("grok-4.3")
}

fn sanitize_tools(object: &mut Map<String, Value>) {
    let tool_choice = object.get("tool_choice").cloned();
    let should_drop_choice = {
        let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) else {
            return;
        };
        tools.retain(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(allowed_tool_type)
        });
        if tools.is_empty() {
            object.remove("tools");
            object.remove("tool_choice");
            return;
        }
        let tools_snapshot = tools.clone();
        tool_choice
            .as_ref()
            .is_some_and(|choice| should_drop_tool_choice(choice, &tools_snapshot))
    };
    if should_drop_choice {
        object.remove("tool_choice");
    }
}

fn allowed_tool_type(value: &str) -> bool {
    matches!(
        value,
        "code_execution"
            | "code_interpreter"
            | "collections_search"
            | "file_search"
            | "function"
            | "mcp"
            | "shell"
            | "web_search"
            | "x_search"
    )
}

fn should_drop_tool_choice(choice: &Value, tools: &[Value]) -> bool {
    if choice
        .as_str()
        .is_some_and(|value| matches!(value, "auto" | "none" | "required"))
    {
        return false;
    }
    let Some(choice_type) = choice
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| choice.pointer("/function/type").and_then(Value::as_str))
    else {
        return true;
    };
    !tools.iter().any(|tool| {
        tool.get("type")
            .and_then(Value::as_str)
            .is_some_and(|tool_type| tool_type == choice_type)
    })
}

fn remove_keys(object: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        object.remove(*key);
    }
}

fn remove_recursive(value: &mut Value, key: &str) {
    match value {
        Value::Object(object) => {
            object.remove(key);
            for value in object.values_mut() {
                remove_recursive(value, key);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_recursive(item, key);
            }
        }
        _ => {}
    }
}

fn strip_invalid_encrypted_content(value: &mut Value) {
    strip_invalid_encrypted_content_inner(value);
}

fn strip_invalid_encrypted_content_inner(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => {
            let mut invalid = object
                .get("encrypted_content")
                .is_some_and(|content| !is_valid_grok_encrypted_content(content));
            if invalid {
                object.remove("encrypted_content");
            }
            for value in object.values_mut() {
                invalid |= strip_invalid_encrypted_content_inner(value);
            }
            invalid
        }
        Value::Array(items) => {
            items.retain_mut(|item| !strip_invalid_encrypted_content_inner(item));
            false
        }
        _ => false,
    }
}

fn is_valid_grok_encrypted_content(value: &Value) -> bool {
    let Some(content) = value.as_str() else {
        return false;
    };
    let trimmed = content.trim();
    if trimmed.len() < 50 || trimmed.len() > 8 * 1024 * 1024 {
        return false;
    }
    if trimmed.contains('=') || trimmed.starts_with("gAAAA") {
        return false;
    }
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(trimmed) else {
        return false;
    };
    if decoded.len() < 32 || decoded.len() > 8 * 1024 * 1024 {
        return false;
    }
    normalized_shannon_entropy(&decoded) >= 0.85
}

fn normalized_shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for byte in bytes {
        counts[*byte as usize] += 1;
    }
    let len = bytes.len() as f64;
    let entropy = counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = count as f64 / len;
            -probability * probability.log2()
        })
        .sum::<f64>();
    let max_entropy = len.min(256.0).log2().min(8.0);
    if max_entropy <= 0.0 {
        0.0
    } else {
        entropy / max_entropy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn json_body(value: Value) -> Bytes {
        Bytes::from(serde_json::to_vec(&value).expect("test JSON should encode"))
    }

    #[test]
    fn normalizes_grok_model_aliases() {
        assert_eq!(normalize_grok_model("grok"), "grok-4.3");
        assert_eq!(normalize_grok_model("grok-build"), "grok-build-0.1");
        assert_eq!(
            normalize_grok_model("grok-composer"),
            "grok-composer-2.5-fast"
        );
        assert_eq!(
            normalize_grok_model("grok-4.20-reasoning"),
            "grok-4.20-0309-reasoning"
        );
        assert_eq!(
            normalize_grok_model("grok-4.20-non-reasoning"),
            "grok-4.20-0309-non-reasoning"
        );
        assert_eq!(normalize_grok_model("grok-custom"), "grok-custom");
    }

    #[test]
    fn injects_prompt_cache_key_without_overwriting_existing_value() {
        let mut body = json_body(json!({"model": "grok"}));
        inject_prompt_cache_key(&mut body, "session-1").unwrap();
        let value = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(value["prompt_cache_key"], "session-1");

        let mut body = json_body(json!({"model": "grok", "prompt_cache_key": "client"}));
        inject_prompt_cache_key(&mut body, "session-1").unwrap();
        let value = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(value["prompt_cache_key"], "client");
    }

    #[test]
    fn strips_invalid_encrypted_content_without_rejecting_request() {
        let mut body = json_body(json!({
            "model": "grok",
            "input": [
                {"type": "message", "encrypted_content": "gAAAAinvalid-from-other-provider"},
                {"type": "message", "content": "keep"}
            ],
            "metadata": {"encrypted_content": "not-valid-base64url"}
        }));

        patch_grok_request_body(&mut body, ProxyRoute::CodexResponses).unwrap();
        let value = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(value["input"].as_array().unwrap().len(), 1);
        assert_eq!(value["input"][0]["content"], "keep");
        assert!(value["metadata"].get("encrypted_content").is_none());
    }

    #[test]
    fn ws_request_body_reuses_grok_body_patch() {
        let value = ws_request_body(
            json!({
                "model": "grok-build",
                "stream_options": {"include_usage": true},
                "tools": [{"type": "unsupported"}, {"type": "function"}]
            }),
            Some("session-1"),
        );
        let response = &value["response"];
        assert_eq!(response["model"], "grok-build-0.1");
        assert!(response.get("stream_options").is_none());
        assert_eq!(response["tools"].as_array().unwrap().len(), 1);
        assert_eq!(response["prompt_cache_key"], "session-1");
    }

    #[test]
    fn ws_message_body_sanitizes_existing_response_create() {
        let value = ws_message_body(
            json!({
                "type": "response.create",
                "response": {
                    "model": "grok-composer",
                    "external_web_access": true,
                    "tools": [{"type": "unsupported"}],
                    "input": [{"encrypted_content": "gAAAAinvalid"}]
                }
            }),
            Some("session-1"),
        );
        let response = &value["response"];
        assert_eq!(response["model"], "grok-composer-2.5-fast");
        assert!(response.get("external_web_access").is_none());
        assert!(response.get("tools").is_none());
        assert_eq!(response["input"].as_array().unwrap().len(), 0);
        assert_eq!(response["prompt_cache_key"], "session-1");
    }

    #[test]
    fn video_session_key_from_response_accepts_common_shapes() {
        assert_eq!(
            video_session_key_from_response(br#"{"request_id":"vid_1"}"#).as_deref(),
            Some("grok-video:vid_1")
        );
        assert_eq!(
            video_session_key_from_response(br#"{"data":{"id":"vid_2"}}"#).as_deref(),
            Some("grok-video:vid_2")
        );
    }

    #[test]
    fn cooldown_parses_retry_after_http_date() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2037 07:28:00 GMT"),
        );
        let until = retry_after_until_ms(&headers, 1_700_000_000_000).unwrap();
        assert!(until > 1_700_000_000_000);
    }
}

pub(super) fn retry_after_until_ms(headers: &HeaderMap, now_ms: i64) -> Option<i64> {
    let value = header_string(headers, "retry-after")?;
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return Some(now_ms.saturating_add(seconds.max(0).saturating_mul(1000)));
    }
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|time| time.timestamp_millis())
        .filter(|until| *until > now_ms)
}

fn rate_limit_reset_until_ms(headers: &HeaderMap, now_ms: i64) -> Option<i64> {
    [
        "x-ratelimit-reset-requests",
        "x-ratelimit-reset-tokens",
        "x-ratelimit-reset",
    ]
    .into_iter()
    .filter_map(|name| header_string(headers, name))
    .filter_map(|value| value.trim().parse::<i64>().ok())
    .map(|value| {
        if value < 10_000_000_000 {
            value.saturating_mul(1000)
        } else {
            value
        }
    })
    .filter(|until| *until > now_ms)
    .min()
}

fn cooldown_message(status: StatusCode, headers: &HeaderMap, until: i64) -> String {
    let tier = header_string(headers, "xai-subscription-tier");
    let entitlement = header_string(headers, "xai-entitlement-status");
    format!(
        "grok upstream returned {}; cooling account until {until}{}{}",
        status.as_u16(),
        tier.map(|tier| format!("; tier={tier}"))
            .unwrap_or_default(),
        entitlement
            .map(|entitlement| format!("; entitlement={entitlement}"))
            .unwrap_or_default()
    )
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
