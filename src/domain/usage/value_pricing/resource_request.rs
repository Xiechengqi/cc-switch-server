//! Protocol facts before transformations. Missing evidence uses an explicit
//! versioned request bound; it is never interpreted as a free resource.
use super::*;
use crate::domain::usage::store::InferenceOperation;
use serde_json::Value;
const TOOL_CALL_BOUND: u64 = 8;
const AUDIO_TOKEN_BOUND: u64 = 128_000;

fn tool_resource(tool: &Value) -> Option<&'static str> {
    let kind = tool.get("type").and_then(Value::as_str).unwrap_or("");
    if kind.starts_with("web_search")
        || ["googleSearch", "google_search", "google_search_retrieval"]
            .iter()
            .any(|key| tool.get(*key).is_some())
    {
        Some("tool.web_search")
    } else if kind.starts_with("web_fetch")
        || ["urlContext", "url_context"]
            .iter()
            .any(|key| tool.get(*key).is_some())
    {
        Some("tool.web_fetch")
    } else if kind.starts_with("file_search") || tool.get("fileSearch").is_some() {
        Some("tool.file_search")
    } else if kind.starts_with("code_interpreter")
        || kind.starts_with("code_execution")
        || ["codeExecution", "code_execution"]
            .iter()
            .any(|key| tool.get(*key).is_some())
    {
        Some("tool.code_execution")
    } else if kind.starts_with("computer") {
        Some("tool.computer")
    } else if kind == "mcp" {
        Some("tool.mcp")
    } else if kind == "image_generation" {
        Some("image.output")
    } else {
        None
    }
}
fn qualifier(body: &Value, base: &str, fields: &[&str]) -> String {
    let parts: Vec<_> = fields
        .iter()
        .filter_map(|key| body.get(*key).and_then(Value::as_str))
        .filter(|part| {
            !part.is_empty()
                && part.len() <= 40
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
        })
        .collect();
    if parts.is_empty() {
        base.into()
    } else {
        format!("{base}/{}", parts.join("/"))
    }
}
fn audio_input(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str).is_some_and(|kind| {
                matches!(
                    kind,
                    "function"
                        | "custom"
                        | "function_call"
                        | "function_call_output"
                        | "tool_result"
                )
            }) {
                return false;
            }
            if map.get("type").and_then(Value::as_str) == Some("input_audio")
                || map.contains_key("input_audio")
            {
                return true;
            }
            if ["mimeType", "mime_type"].iter().any(|key| {
                map.get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|mime| mime.starts_with("audio/"))
            }) {
                return true;
            }
            map.iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "parameters" | "input_schema" | "metadata")
                })
                .any(|(_, v)| audio_input(v))
        }
        Value::Array(items) => items.iter().any(audio_input),
        _ => false,
    }
}
fn output_modality(body: &Value, modality: &str) -> bool {
    [
        "/modalities",
        "/generationConfig/responseModalities",
        "/generation_config/response_modalities",
    ]
    .iter()
    .any(|path| {
        body.pointer(path)
            .and_then(Value::as_array)
            .is_some_and(|modes| {
                modes.iter().any(|mode| {
                    mode.as_str()
                        .is_some_and(|mode| mode.eq_ignore_ascii_case(modality))
                })
            })
    })
}
pub fn resource_request_plan(body: &Value, operation: InferenceOperation) -> ResourcePlan {
    let body = body
        .get("request")
        .or_else(|| body.get("response"))
        .filter(|value| value.is_object())
        .unwrap_or(body);
    let mut plan = ResourcePlan {
        text_tokens: true,
        requires_v2: !is_metered_text_value(body),
        ..Default::default()
    };
    match operation {
        InferenceOperation::ImageGeneration | InferenceOperation::ImageEdit => {
            plan.text_tokens = false;
            plan.requires_v2 = true;
            plan.quantities.insert(
                qualifier(body, "image.output", &["size", "quality"]),
                body.get("n")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 100),
            );
        }
        InferenceOperation::VideoGeneration => {
            plan.text_tokens = false;
            plan.requires_v2 = true;
            let seconds = body
                .get("duration")
                .or_else(|| body.get("duration_seconds"))
                .and_then(Value::as_u64)
                .unwrap_or(15)
                .clamp(1, 3600);
            plan.quantities.insert(
                qualifier(body, "video.output_millisecond", &["resolution"]),
                seconds * 1000,
            );
        }
        _ => {}
    }
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(resource) = tool_resource(tool) {
            plan.requires_v2 = true;
            if !plan
                .quantities
                .keys()
                .any(|key| key.split('/').next() == Some(resource))
            {
                plan.quantities.insert(
                    if resource == "image.output" {
                        qualifier(tool, resource, &["size", "quality"])
                    } else {
                        resource.into()
                    },
                    TOOL_CALL_BOUND,
                );
            }
        }
    }
    if body.get("web_search_options").is_some_and(|v| !v.is_null()) {
        plan.requires_v2 = true;
        plan.quantities
            .insert("tool.web_search".into(), TOOL_CALL_BOUND);
    }
    if audio_input(body) {
        plan.requires_v2 = true;
        plan.quantities
            .insert("audio.input_token".into(), AUDIO_TOKEN_BOUND);
    }
    if output_modality(body, "audio") || body.get("audio").is_some_and(Value::is_object) {
        plan.requires_v2 = true;
        plan.quantities
            .insert("audio.output_token".into(), AUDIO_TOKEN_BOUND);
    }
    if output_modality(body, "image") {
        plan.requires_v2 = true;
        plan.quantities.entry("image.output".into()).or_insert(
            body.pointer("/generationConfig/candidateCount")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 100),
        );
    }
    plan
}

/// Counters are cumulative maxima. Output calls use stable IDs. Both storage
/// and the number of retained IDs are bounded independently of response size.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsageObserver {
    quantities: BTreeMap<String, u64>,
    items: BTreeMap<String, String>,
    overflow: bool,
}
impl ResourceUsageObserver {
    pub fn replace_usage(&mut self, usage: Option<ResourceUsage>) {
        *self = Self::default();
        if let Some(usage) = usage {
            self.quantities = usage.quantities;
            self.overflow = usage.estimated;
        }
    }
    fn counter(&mut self, key: &str, count: u64) {
        let old = self.quantities.entry(key.into()).or_default();
        *old = (*old).max(count);
    }
    fn item(&mut self, id: &str, resource: &str) {
        if id.len() > 200 || self.items.len() >= 1024 && !self.items.contains_key(id) {
            self.overflow = true;
            return;
        }
        if let Some(previous) = self.items.insert(id.into(), resource.into()) {
            if previous != resource {
                self.overflow = true;
            }
        }
    }
    pub fn observe(&mut self, value: &Value) {
        let root = value
            .get("response")
            .or_else(|| value.get("message"))
            .unwrap_or(value);
        for (path, key) in [
            (
                "/usage/server_tool_use/web_search_requests",
                "tool.web_search",
            ),
            (
                "/usage/server_tool_use/web_fetch_requests",
                "tool.web_fetch",
            ),
            (
                "/usage/output_tokens_details/audio_tokens",
                "audio.output_token",
            ),
            (
                "/usage/completion_tokens_details/audio_tokens",
                "audio.output_token",
            ),
        ] {
            if let Some(n) = root.pointer(path).and_then(Value::as_u64) {
                self.counter(key, n);
            }
        }
        for prefix in [
            "/usage/input_tokens_details",
            "/usage/prompt_tokens_details",
        ] {
            if let Some(total) = root
                .pointer(&format!("{prefix}/audio_tokens"))
                .and_then(Value::as_u64)
            {
                let cached_audio = root
                    .pointer(&format!("{prefix}/cached_tokens_details/audio_tokens"))
                    .and_then(Value::as_u64);
                if cached_audio.is_none()
                    && root
                        .pointer(&format!("{prefix}/cached_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
                {
                    self.overflow = true;
                }
                let cached = cached_audio.unwrap_or(0);
                if let Some(fresh) = total.checked_sub(cached) {
                    self.counter("audio.input_token", fresh);
                } else {
                    self.overflow = true;
                }
            }
        }
        for entry in value.get("item").into_iter().chain(
            root.get("output")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        ) {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let resource = match entry.get("type").and_then(Value::as_str).unwrap_or("") {
                "web_search_call" => "tool.web_search",
                "web_fetch_call" => "tool.web_fetch",
                "file_search_call" => "tool.file_search",
                "code_interpreter_call" => "tool.code_execution",
                "computer_call" => "tool.computer",
                "mcp_call" => "tool.mcp",
                "image_generation_call" => "image.output",
                _ => continue,
            };
            let final_item = value.get("type").and_then(Value::as_str)
                == Some("response.output_item.done")
                || root.get("output").is_some();
            if entry.get("status").and_then(Value::as_str) == Some("completed")
                || final_item && entry.get("status").is_none()
            {
                self.item(id, resource);
            }
        }
        // Anthropic's cumulative server_tool_use counters remain preferred;
        // block IDs provide positive evidence on interrupted streams.
        if let (Some(index), Some(block)) = (
            value.get("index").and_then(Value::as_u64),
            value.get("content_block"),
        ) {
            if block.get("type").and_then(Value::as_str) == Some("server_tool_use") {
                let key = match block.get("name").and_then(Value::as_str) {
                    Some("web_search") => Some("tool.web_search"),
                    Some("web_fetch") => Some("tool.web_fetch"),
                    _ => None,
                };
                if let Some(key) = key {
                    self.item(&format!("anthropic:{index}"), key);
                }
            }
        }
        if let Some(metadata) = root.get("usageMetadata") {
            for (field, key) in [
                ("promptTokensDetails", "audio.input_token"),
                ("candidatesTokensDetails", "audio.output_token"),
            ] {
                for detail in metadata
                    .get(field)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if detail
                        .get("modality")
                        .and_then(Value::as_str)
                        .is_some_and(|m| m.eq_ignore_ascii_case("audio"))
                    {
                        if let Some(total) = detail.get("tokenCount").and_then(Value::as_u64) {
                            if field == "promptTokensDetails"
                                && metadata
                                    .get("cachedContentTokenCount")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                                    > 0
                                && metadata.get("cacheTokensDetails").is_none()
                            {
                                self.overflow = true;
                            }
                            let cached = if field == "promptTokensDetails" {
                                metadata
                                    .get("cacheTokensDetails")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .filter(|item| {
                                        item.get("modality")
                                            .and_then(Value::as_str)
                                            .is_some_and(|m| m.eq_ignore_ascii_case("audio"))
                                    })
                                    .filter_map(|item| {
                                        item.get("tokenCount").and_then(Value::as_u64)
                                    })
                                    .try_fold(0u64, |sum, n| sum.checked_add(n))
                            } else {
                                Some(0)
                            };
                            if let Some(fresh) = cached.and_then(|cached| total.checked_sub(cached))
                            {
                                self.counter(key, fresh);
                            } else {
                                self.overflow = true;
                            }
                        }
                    }
                }
            }
        }
        for candidate in root
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(queries) = candidate
                .pointer("/groundingMetadata/webSearchQueries")
                .and_then(Value::as_array)
            {
                self.counter("tool.web_search", queries.len() as u64);
                self.overflow = true;
            }
            if let Some(urls) = candidate
                .pointer("/urlContextMetadata/urlMetadata")
                .and_then(Value::as_array)
            {
                self.counter("tool.web_fetch", urls.len() as u64);
                self.overflow = true;
            }
            for (index, part) in candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let candidate = candidate.get("index").and_then(Value::as_u64).unwrap_or(0);
                if part.get("executableCode").is_some() {
                    self.overflow = true;
                    self.item(
                        &format!("gemini-code:{candidate}:{index}"),
                        "tool.code_execution",
                    );
                }
                if part
                    .pointer("/inlineData/mimeType")
                    .and_then(Value::as_str)
                    .is_some_and(|mime| mime.starts_with("image/"))
                {
                    self.overflow = true;
                    self.item(&format!("gemini-image:{candidate}:{index}"), "image.output");
                }
            }
        }
    }
    pub fn has_positive_usage(&self) -> bool {
        !self.items.is_empty() || self.quantities.values().any(|count| *count > 0)
    }
    pub fn usage(&self) -> Option<ResourceUsage> {
        if self.quantities.is_empty() && self.items.is_empty() {
            return None;
        }
        let mut quantities = self.quantities.clone();
        let mut counts = BTreeMap::<String, u64>::new();
        for key in self.items.values() {
            *counts.entry(key.clone()).or_default() += 1;
        }
        for (key, count) in counts {
            let old = quantities.entry(key).or_default();
            *old = (*old).max(count);
        }
        Some(ResourceUsage {
            quantities,
            source: "upstream.protocol".into(),
            estimated: self.overflow,
            version: RESOURCE_ESTIMATE_VERSION.into(),
        })
    }
    pub fn retained_bytes(&self) -> usize {
        self.items
            .iter()
            .map(|(k, v)| k.capacity() + v.capacity() + 64)
            .sum::<usize>()
            + self.quantities.len() * 192
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn declarations_require_prices_client_schemas_do_not() {
        let plan = resource_request_plan(
            &json!({"tools":[{"type":"function","name":"find","parameters":{"audio":{"type":"string"}}}]}),
            InferenceOperation::Responses,
        );
        assert!(plan.quantities.is_empty());
        let plan = resource_request_plan(
            &json!({"tools":[{"type":"web_search_preview"}],"modalities":["text","audio"]}),
            InferenceOperation::Responses,
        );
        assert!(plan.requires_v2);
        assert_eq!(plan.quantities["tool.web_search"], 8);
        assert_eq!(plan.quantities["audio.output_token"], AUDIO_TOKEN_BOUND);
    }
    #[test]
    fn resource_evidence_is_duplicate_safe() {
        let event = json!({"type":"response.output_item.done","item":{"id":"call-1","type":"web_search_call","status":"completed"}});
        let mut observer = ResourceUsageObserver::default();
        observer.observe(&event);
        observer.observe(&event);
        observer.observe(&json!({"usage":{"server_tool_use":{"web_search_requests":2}}}));
        observer.observe(&json!({"usage":{"server_tool_use":{"web_search_requests":1}}}));
        assert_eq!(observer.usage().unwrap().quantities["tool.web_search"], 2);
    }
    #[test]
    fn dimensions_and_audio_components_are_explicit() {
        let plan = resource_request_plan(
            &json!({"n":2,"size":"1024x1024","quality":"high"}),
            InferenceOperation::ImageGeneration,
        );
        assert_eq!(plan.quantities["image.output/1024x1024/high"], 2);
        assert!(!plan.text_tokens);
        let mut observer = ResourceUsageObserver::default();
        observer.observe(&json!({"usage":{"prompt_tokens_details":{"audio_tokens":100,"cached_tokens_details":{"audio_tokens":40}}}}));
        assert_eq!(
            observer.usage().unwrap().quantities["audio.input_token"],
            60
        );
    }
}
