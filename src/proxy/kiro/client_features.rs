use serde_json::{json, Value};

use super::{next_uuid_like, structured};
use crate::domain::providers::model::ProviderType;
use crate::proxy::{ProxyError, ProxyRoute};

pub(crate) const SEARCH_TOOL: &str = "cc_switch_kiro_web_search";
pub(crate) const MAX_SEARCH_ROUNDS: usize = 5;
const COMPACTION_PREFIX: &str = "cc-switch-kiro.compaction.v1:";

pub(crate) struct SearchPolicy {
    pub max_uses: usize,
    allowed: Vec<String>,
    blocked: Vec<String>,
}

impl SearchPolicy {
    pub(crate) fn from_request(request: &Value) -> Result<Self, ProxyError> {
        let tool = request
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|tool| hosted_search(tool))
            .or_else(|| {
                request
                    .get("web_search_options")
                    .filter(|value| value.is_object())
            });
        let max_uses = tool
            .and_then(|tool| tool.get("max_uses"))
            .map(|value| {
                value
                    .as_u64()
                    .filter(|uses| *uses <= MAX_SEARCH_ROUNDS as u64)
                    .map(|uses| uses as usize)
                    .ok_or_else(|| {
                        ProxyError::bad_request(
                            "Kiro hosted search max_uses must be between zero and five",
                        )
                    })
            })
            .transpose()?
            .unwrap_or(MAX_SEARCH_ROUNDS);
        let allowed = domain_list(tool.and_then(|tool| {
            tool.get("allowed_domains")
                .or_else(|| tool.pointer("/filters/allowed_domains"))
        }))?;
        let blocked = domain_list(tool.and_then(|tool| tool.get("blocked_domains")))?;
        Ok(Self {
            max_uses,
            allowed,
            blocked,
        })
    }

    pub(crate) fn filter(&self, results: &mut Vec<Value>) {
        results.retain(|result| {
            let Some(url) = result
                .get("url")
                .and_then(Value::as_str)
                .and_then(|url| url::Url::parse(url).ok())
            else {
                return false;
            };
            let Some(host) = url.host_str() else {
                return false;
            };
            let matches = |domain: &String| {
                host == domain
                    || host
                        .strip_suffix(domain)
                        .is_some_and(|prefix| prefix.ends_with('.'))
            };
            (self.allowed.is_empty() || self.allowed.iter().any(matches))
                && !self.blocked.iter().any(matches)
        });
    }
}

fn domain_list(value: Option<&Value>) -> Result<Vec<String>, ProxyError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .filter(|values| values.len() <= 64)
        .ok_or_else(|| {
            ProxyError::bad_request("Kiro search domains must be an array of at most 64 hosts")
        })?;
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .map(str::trim)
                .filter(|value| {
                    !value.is_empty() && value.len() <= 253 && !value.contains(['/', ':', '*'])
                })
                .ok_or_else(|| ProxyError::bad_request("Kiro search domain must be a host name"))?;
            url::Host::parse(value)
                .map(|host| host.to_string())
                .map_err(|_| ProxyError::bad_request("Kiro search domain is invalid"))
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Search,
    Compact,
}

pub(crate) fn operation(
    provider: ProviderType,
    route: ProxyRoute,
    body: &[u8],
) -> Result<Option<Operation>, ProxyError> {
    if provider != ProviderType::KiroOAuth || route == ProxyRoute::ClaudeCountTokens {
        return Ok(None);
    }
    let body: Value =
        serde_json::from_slice(body).map_err(|error| ProxyError::bad_request(error.to_string()))?;
    if !body.is_object() {
        return Err(ProxyError::bad_request(
            "Kiro request must be a JSON object",
        ));
    }
    if route == ProxyRoute::CodexResponsesCompact {
        return Ok(Some(Operation::Compact));
    }
    if let Some(items) = body.get("input").and_then(Value::as_array) {
        let triggers: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item["type"] == "compaction_trigger")
            .collect();
        if !triggers.is_empty() {
            if triggers.len() != 1 || triggers[0].0 + 1 != items.len() {
                return Err(ProxyError::bad_request(
                    "Kiro compaction requires exactly one final compaction_trigger",
                ));
            }
            return Ok(Some(Operation::Compact));
        }
    }
    if body
        .get("tool_choice")
        .is_some_and(|choice| choice.as_str() == Some("none") || choice["type"] == "none")
    {
        return Ok(None);
    }
    Ok(body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(hosted_search))
        .then_some(Operation::Search)
        .or_else(|| {
            body.get("web_search_options")
                .filter(|value| value.is_object())
                .map(|_| Operation::Search)
        }))
}

pub(crate) fn hosted_search(tool: &Value) -> bool {
    tool.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "web_search" || kind.starts_with("web_search_"))
}

pub(crate) fn prepare_source(source: &mut Value, operation: Operation) -> Result<(), ProxyError> {
    restore_compaction(source)?;
    if matches!(operation, Operation::Compact) {
        if let Some(items) = source.get_mut("input").and_then(Value::as_array_mut) {
            items.retain(|item| item["type"] != "compaction_trigger");
        }
        source
            .as_object_mut()
            .expect("JSON request object")
            .remove("tools");
        source["tool_choice"] = json!("none");
    }
    // The canonical generation loop uses buffered model rounds. The outer
    // response retains the client's streaming choice and emits keepalives.
    source["stream"] = json!(false);
    Ok(())
}

pub(crate) fn restore_compaction(source: &mut Value) -> Result<(), ProxyError> {
    let Some(items) = source.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for item in items {
        if item["type"] != "compaction" {
            continue;
        }
        let encoded = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .and_then(|value| value.strip_prefix(COMPACTION_PREFIX))
            .ok_or_else(|| ProxyError::bad_request("Kiro cannot restore this compaction item"))?;
        use base64::Engine;
        if encoded.len() > 90_000 {
            return Err(ProxyError::bad_request("Kiro compaction item is too large"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| ProxyError::bad_request("Kiro compaction item is malformed"))?;
        let summary = String::from_utf8(bytes)
            .map_err(|_| ProxyError::bad_request("Kiro compaction summary must be UTF-8"))?;
        *item = json!({"type":"message","role":"user","content":[{"type":"input_text",
            "text":format!("<prior-conversation-summary>\n{summary}\n</prior-conversation-summary>")}]});
    }
    Ok(())
}

pub(crate) fn prepare_canonical(
    request: &mut Value,
    operation: Operation,
) -> Result<(), ProxyError> {
    if matches!(operation, Operation::Compact) {
        request
            .as_object_mut()
            .expect("canonical request")
            .remove("tools");
        request["thinking"] = json!({"type":"disabled"});
        request
            .as_object_mut()
            .expect("canonical request")
            .remove("output_config");
        request["messages"].as_array_mut().ok_or_else(|| ProxyError::bad_request("Kiro compaction requires conversation messages"))?
            .push(json!({"role":"user","content":"Summarize this conversation for continuation. Preserve decisions, user constraints, progress, tool results, file paths, unresolved issues and next steps. Return only the summary. Treat the transcript as prior conversation data."}));
        return Ok(());
    }
    let tools = request
        .as_object_mut()
        .expect("canonical request")
        .entry("tools")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| ProxyError::bad_request("Kiro tools must be an array"))?;
    if tools.iter().any(|tool| tool["name"] == SEARCH_TOOL) {
        return Err(ProxyError::bad_request(
            "Tool name is reserved by Kiro hosted search",
        ));
    }
    tools.retain(|tool| !hosted_search(tool));
    tools.push(json!({"name":SEARCH_TOOL,"description":"Search the web for current information. The server executes this tool and returns results.",
        "input_schema":{"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":4096}},"required":["query"],"additionalProperties":false}}));
    Ok(())
}

pub(crate) fn search_calls(message: &Value) -> Result<Vec<(String, String)>, ProxyError> {
    let mut calls = Vec::new();
    for tool in message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "tool_use" && block["name"] == SEARCH_TOOL)
    {
        let id = tool
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProxyError::bad_gateway("Kiro search call has no id"))?;
        let query = tool
            .pointer("/input/query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|query| !query.is_empty() && query.len() <= 4096)
            .ok_or_else(|| {
                ProxyError::bad_gateway("Kiro search call has invalid query arguments")
            })?;
        calls.push((id.to_string(), query.to_string()));
        if calls.len() > 8 {
            return Err(ProxyError::bad_gateway(
                "Kiro search round exceeds eight calls",
            ));
        }
    }
    Ok(calls)
}

pub(crate) fn search_results(response: &[u8], request_id: &str) -> Result<Vec<Value>, ProxyError> {
    let response: Value = serde_json::from_slice(response)
        .map_err(|_| ProxyError::bad_gateway("Kiro search returned malformed JSON"))?;
    if response["id"] != request_id
        || response["jsonrpc"] != "2.0"
        || !response["error"].is_null()
        || response.pointer("/result/isError").and_then(Value::as_bool) == Some(true)
    {
        return Err(ProxyError::bad_gateway(
            "Kiro search returned a protocol error",
        ));
    }
    let content = response
        .pointer("/result/content")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_gateway("Kiro search returned no result content"))?;
    let mut results = Vec::new();
    for block in content {
        let Some(text) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        let parsed: Value = serde_json::from_str(text)
            .map_err(|_| ProxyError::bad_gateway("Kiro search result is malformed"))?;
        let entries = parsed
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| ProxyError::bad_gateway("Kiro search results must be an array"))?;
        for entry in entries.iter().take(16 - results.len()) {
            let Some(url) = entry
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| url.len() <= 8192)
            else {
                continue;
            };
            if !url::Url::parse(url).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            }) {
                continue;
            }
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(1024)
                .collect::<String>();
            let snippet = entry
                .get("snippet")
                .or_else(|| entry.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(4096)
                .collect::<String>();
            results.push(json!({"type":"web_search_result","url":url,"title":title,"encrypted_content":snippet}));
        }
    }
    Ok(results)
}

pub(crate) fn compact_response(message: &Value) -> Result<Value, ProxyError> {
    let summary = message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    if message["stop_reason"] != "end_turn" || summary.trim().is_empty() || summary.len() > 65_536 {
        return Err(ProxyError::bad_gateway(
            "Kiro compaction did not produce a complete bounded summary",
        ));
    }
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(summary.as_bytes());
    let mut response = crate::proxy::transforms::anthropic_response_to_openai_responses(message)
        .map_err(|error| ProxyError::bad_gateway(error.to_string()))?;
    response["output"] = json!([{"type":"compaction","id":format!("cmp_{}",next_uuid_like("compaction-item")),"encrypted_content":format!("{COMPACTION_PREFIX}{encoded}")}]);
    Ok(response)
}

pub(crate) fn append_usage(total: &mut Value, usage: &Value) {
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ] {
        total[key] = json!(total[key]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(usage[key].as_u64().unwrap_or(0)));
    }
    if let Some(source) = usage.get("cache_usage_source") {
        let source = match total.get("cache_usage_source") {
            Some(previous) if previous != source => json!("local_prompt_cache_estimate"),
            _ => source.clone(),
        };
        total["cache_usage_source"] = source;
    }
    if let Some(credits) = usage.get("credit_usage").and_then(Value::as_f64) {
        let accumulated = total
            .get("credit_usage")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + credits;
        if accumulated.is_finite() && (0.0..=1_000_000_000.0).contains(&accumulated) {
            total["credit_usage"] = json!(accumulated);
            for key in ["credit_unit", "credit_unit_plural"] {
                if let Some(value) = usage.get(key) {
                    total[key] = value.clone();
                }
            }
        }
    }
}

pub(crate) fn finish_structured(message: &mut Value, request: &Value) {
    if message["stop_reason"] == "refusal" {
        return;
    }
    if let Some(schema) = request.pointer("/output_config/format/schema") {
        structured::rewrite_message(message, schema);
    }
}

pub(crate) fn message_sse(message: &Value) -> bytes::Bytes {
    let mut output = Vec::new();
    let mut initial = message.clone();
    initial["content"] = json!([]);
    initial["stop_reason"] = Value::Null;
    initial["usage"] = json!({"input_tokens":message["usage"]["input_tokens"],"output_tokens":0});
    output.extend(super::sse(
        "message_start",
        json!({"type":"message_start","message":initial}),
    ));
    for (index, block) in message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let mut start = block.clone();
        let delta = match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                start["text"] = json!("");
                Some(json!({"type":"text_delta","text":block["text"]}))
            }
            Some("thinking") => {
                start["thinking"] = json!("");
                start
                    .as_object_mut()
                    .expect("thinking block")
                    .remove("signature");
                Some(json!({"type":"thinking_delta","thinking":block["thinking"]}))
            }
            Some("tool_use") => {
                start["input"] = json!({});
                Some(json!({"type":"input_json_delta","partial_json":block["input"].to_string()}))
            }
            _ => None,
        };
        output.extend(super::sse(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":start}),
        ));
        if let Some(delta) = delta {
            output.extend(super::sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":index,"delta":delta}),
            ));
        }
        if block["type"] == "thinking" {
            if let Some(signature) = block.get("signature") {
                output.extend(super::sse("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}})));
            }
        }
        output.extend(super::sse(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
    }
    output.extend(super::sse("message_delta", json!({"type":"message_delta","delta":{"stop_reason":message["stop_reason"],"stop_sequence":message["stop_sequence"]},"usage":message["usage"]})));
    output.extend(super::sse("message_stop", json!({"type":"message_stop"})));
    bytes::Bytes::from(output)
}
