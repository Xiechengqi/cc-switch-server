//! Explicit COSY generation support, evidenced by the pinned official bundle.
use serde_json::{Map, Value};

pub(crate) fn source_controls(
    app: crate::domain::providers::model::AppKind,
    input: &Value,
) -> Result<Map<String, Value>, String> {
    use crate::domain::providers::model::AppKind;
    let mut controls = Map::new();
    for field in [
        "temperature",
        "top_p",
        "top_k",
        "max_tokens",
        "max_completion_tokens",
        "max_output_tokens",
        "stop",
        "stop_sequences",
        "seed",
        "presence_penalty",
        "frequency_penalty",
        "logprobs",
        "top_logprobs",
        "response_format",
        "n",
        "parallel_tool_calls",
        "preserve_thinking",
        "context_length",
        "qoder_text_tool_recovery",
    ] {
        if let Some(value) = input.get(field) {
            controls.insert(field.into(), value.clone());
        }
    }
    if app == AppKind::Gemini {
        if let Some(config) = input
            .get("generationConfig")
            .or_else(|| input.get("generation_config"))
            .filter(|v| !v.is_null())
        {
            if !config.is_object() {
                return Err("Qoder Gemini generationConfig must be an object".into());
            }
            for (source, alias, target) in [
                ("temperature", "temperature", "temperature"),
                ("topP", "top_p", "top_p"),
                ("topK", "top_k", "top_k"),
                ("maxOutputTokens", "max_output_tokens", "max_tokens"),
                ("stopSequences", "stop_sequences", "stop"),
                ("candidateCount", "candidate_count", "n"),
            ] {
                if let Some(value) = config.get(source).or_else(|| config.get(alias)) {
                    controls.insert(target.into(), value.clone());
                }
            }
            if config
                .get("responseMimeType")
                .or_else(|| config.get("response_mime_type"))
                .is_some_and(|mime| mime != "text/plain")
                || [
                    "responseSchema",
                    "response_schema",
                    "responseJsonSchema",
                    "response_json_schema",
                    "seed",
                    "logprobs",
                    "responseLogprobs",
                    "response_logprobs",
                    "presencePenalty",
                    "presence_penalty",
                    "frequencyPenalty",
                    "frequency_penalty",
                ]
                .iter()
                .any(|key| config.get(key).is_some_and(|v| !v.is_null()))
            {
                return Err("Qoder Gemini structured/advanced generation controls are not supported by the verified generation contract".into());
            }
        }
    }
    // Explicitly clear any effort guessed by a generic bridge when the
    // source supplied no level (including Gemini's dynamic budget marker).
    let effort = source_reasoning_effort(app, input)?;
    controls.insert(
        "reasoning_effort".into(),
        effort.map_or(Value::Null, |effort| Value::String(effort.into())),
    );
    generation_parameters(&controls)?;
    if app == AppKind::Claude {
        if let Some(choice) = input.get("tool_choice").filter(|v| !v.is_null()) {
            let kind = choice.as_str().or_else(|| choice["type"].as_str());
            if !matches!(kind, Some("auto" | "any" | "none" | "tool")) {
                return Err("Qoder Claude tool_choice type is unsupported".into());
            }
            if choice
                .get("disable_parallel_tool_use")
                .is_some_and(|v| v != &Value::Bool(false))
            {
                return Err(
                    "Qoder cannot disable parallel tool use in the verified generation contract"
                        .into(),
                );
            }
        }
    }
    if app == AppKind::Gemini {
        if let Some(config) = input
            .get("toolConfig")
            .or_else(|| input.get("tool_config"))
            .filter(|v| !v.is_null())
        {
            let calling = config
                .get("functionCallingConfig")
                .or_else(|| config.get("function_calling_config"));
            if !config.is_object() || calling.is_some_and(|v| !v.is_object()) {
                return Err("Qoder Gemini toolConfig must be an object".into());
            }
            if calling.and_then(|v| v.get("mode")).is_some_and(|v| {
                !matches!(
                    v.as_str(),
                    Some("AUTO" | "ANY" | "NONE" | "MODE_UNSPECIFIED")
                )
            }) {
                return Err("Qoder Gemini tool calling mode is unsupported".into());
            }
        }
    }
    Ok(controls)
}

// Validate before generic bridges can turn an off switch or an exact
// token budget into an unrelated effort default. This only selects the
// existing Qoder directive; it introduces no new upstream control field.
fn source_reasoning_effort(
    app: crate::domain::providers::model::AppKind,
    input: &Value,
) -> Result<Option<&'static str>, String> {
    use crate::domain::providers::model::AppKind;
    let mut selected = None;
    if input
        .get("reasoning_budget_tokens")
        .is_some_and(|v| !v.is_null())
    {
        return Err("Qoder cannot enforce an exact reasoning token budget; use an effort level or omit the control".into());
    }
    if let Some(value) = input.get("reasoning_effort").filter(|v| !v.is_null()) {
        merge_source_effort(&mut selected, value)?;
    }
    if let Some(reasoning) = input.get("reasoning").filter(|v| !v.is_null()) {
        if !reasoning.is_object() {
            return Err("Qoder reasoning must be an object".into());
        }
        if reasoning.get("budget_tokens").is_some_and(|v| !v.is_null()) {
            return Err("Qoder cannot enforce an exact reasoning token budget; use an effort level or omit the control".into());
        }
        if let Some(value) = reasoning.get("effort").filter(|v| !v.is_null()) {
            merge_source_effort(&mut selected, value)?;
        }
    }
    if app == AppKind::Claude {
        if let Some(config) = input.get("output_config").filter(|v| !v.is_null()) {
            if !config.is_object() {
                return Err("Qoder output_config must be an object".into());
            }
            if let Some(value) = config.get("effort").filter(|v| !v.is_null()) {
                merge_source_effort(&mut selected, value)?;
            }
        }
        if let Some(thinking) = input.get("thinking").filter(|v| !v.is_null()) {
            let kind = thinking
                .get("type")
                .and_then(Value::as_str)
                .ok_or("Qoder thinking requires an explicit type")?;
            if thinking.get("budget_tokens").is_some_and(|v| !v.is_null()) {
                return Err("Qoder cannot enforce an exact reasoning token budget; use an effort level or omit the control".into());
            }
            if let Some(value) = thinking.get("effort").filter(|v| !v.is_null()) {
                merge_source_effort(&mut selected, value)?;
            }
            match kind {
                "disabled" => merge_source_effort(&mut selected, &Value::String("none".into()))?,
                "enabled" | "adaptive" => {
                    if selected == Some("none") {
                        return Err("Qoder reasoning controls conflict".into());
                    }
                    // Preserve the existing no-level enabled default.
                    selected.get_or_insert("medium");
                }
                _ => return Err("Qoder thinking type is unsupported".into()),
            }
        }
    }
    if app == AppKind::Gemini {
        for name in ["generationConfig", "generation_config"] {
            let Some(config) = input.get(name).filter(|v| !v.is_null()) else {
                continue;
            };
            if !config.is_object() {
                return Err("Qoder generationConfig must be an object".into());
            }
            for name in ["thinkingConfig", "thinking_config"] {
                let Some(thinking) = config.get(name).filter(|v| !v.is_null()) else {
                    continue;
                };
                if !thinking.is_object() {
                    return Err("Qoder thinkingConfig must be an object".into());
                }
                for name in ["thinkingLevel", "thinking_level"] {
                    if let Some(value) = thinking.get(name).filter(|v| !v.is_null()) {
                        merge_source_effort(&mut selected, value)?;
                    }
                }
                for name in ["thinkingBudget", "thinking_budget"] {
                    let Some(value) = thinking.get(name).filter(|v| !v.is_null()) else {
                        continue;
                    };
                    match value.as_i64() {
                        Some(0) => merge_source_effort(&mut selected, &Value::String("none".into()))?,
                        Some(-1) => (), // Dynamic/default, never a concrete effort.
                        _ => return Err("Qoder cannot enforce an exact reasoning token budget; use an effort level or omit the control".into()),
                    }
                }
            }
        }
    }
    Ok(selected)
}

fn merge_source_effort(selected: &mut Option<&'static str>, value: &Value) -> Result<(), String> {
    let raw = value
        .as_str()
        .filter(|raw| raw.len() <= 32)
        .ok_or("Qoder reasoning effort must be a supported short string")?;
    let effort = match raw.trim().to_ascii_lowercase().as_str() {
        "none" | "disabled" | "off" => "none",
        "minimal" | "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" | "very_high" | "very-high" | "max" => "max",
        _ => {
            return Err(
                "Qoder reasoning effort is unsupported; use none, low, medium, high or max".into(),
            )
        }
    };
    if selected.is_some_and(|old| old != effort) {
        return Err("Qoder reasoning controls conflict".into());
    }
    *selected = Some(effort);
    Ok(())
}

pub(super) fn validate_catalog(config: &Map<String, Value>) -> Result<(), String> {
    if config.get("is_vl").is_some_and(|value| !value.is_boolean()) {
        return Err("Qoder live model is_vl must be boolean".into());
    }
    for field in [
        "max_input_tokens",
        "max_output_tokens",
        "default_context_window",
    ] {
        if config
            .get(field)
            .is_some_and(|value| value.as_u64().is_none_or(|n| n == 0))
        {
            return Err(format!(
                "Qoder live model {field} must be a positive integer"
            ));
        }
    }
    if let Some(windows) = config.get("available_context_windows") {
        let windows = windows
            .as_array()
            .ok_or("Qoder context windows must be an array")?;
        if windows.is_empty()
            || windows.len() > 32
            || windows.iter().any(|v| v.as_u64().is_none_or(|n| n == 0))
        {
            return Err("Qoder context windows require 1..32 positive integers".into());
        }
        if config
            .get("default_context_window")
            .is_some_and(|v| !windows.contains(v))
        {
            return Err("Qoder default context is not an available context window".into());
        }
        if let Some(max) = config.get("max_input_tokens").and_then(Value::as_u64) {
            if windows.iter().any(|v| v.as_u64().is_some_and(|n| n > max)) {
                return Err("Qoder context window exceeds the live input limit".into());
            }
        }
    }
    if let (Some(default), Some(max)) = (
        config.get("default_context_window").and_then(Value::as_u64),
        config.get("max_input_tokens").and_then(Value::as_u64),
    ) {
        if default > max {
            return Err("Qoder default context exceeds the live input limit".into());
        }
    }
    Ok(())
}

pub(super) fn context_capability(
    site: crate::domain::qoder::QoderSite,
    key: &str,
    config: &Map<String, Value>,
) -> Option<super::QoderContextCapability> {
    let windows = config
        .get("available_context_windows")
        .and_then(Value::as_array);
    let max_input_tokens = config
        .get("default_context_window")
        .and_then(Value::as_u64)
        .or_else(|| windows.and_then(|values| values.iter().filter_map(Value::as_u64).max()))
        .or_else(|| config.get("max_input_tokens").and_then(Value::as_u64));
    max_input_tokens
        .map(|max_input_tokens| super::QoderContextCapability {
            max_input_tokens,
            runtime_selectable: windows.is_some()
                || super::qoder_context_capability(site, key)
                    .is_some_and(|capability| capability.runtime_selectable),
        })
        .or_else(|| super::qoder_context_capability(site, key))
}

pub(super) fn select_context(
    request: &Map<String, Value>,
    config: &Map<String, Value>,
    default: Option<super::QoderContextCapability>,
) -> Result<Option<super::QoderContextCapability>, String> {
    let Some(value) = request.get("context_length").filter(|v| !v.is_null()) else {
        return Ok(default);
    };
    let n = value
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("Qoder context_length must be a positive integer")?;
    let valid = if let Some(windows) = config
        .get("available_context_windows")
        .and_then(Value::as_array)
    {
        windows.contains(value)
    } else {
        default.is_some_and(|capability| n <= capability.max_input_tokens)
    };
    if !valid {
        return Err(
            "Qoder context_length is not authorized by the bound model's live catalog".into(),
        );
    }
    Ok(Some(super::QoderContextCapability {
        max_input_tokens: n,
        runtime_selectable: true,
    }))
}

pub(super) fn generation_parameters(request: &Map<String, Value>) -> Result<Value, String> {
    let mut parameters = Map::new();
    for (field, max) in [("temperature", 2.0), ("top_p", 1.0)] {
        if let Some(value) = request.get(field).filter(|v| !v.is_null()) {
            if !value
                .as_f64()
                .is_some_and(|n| n.is_finite() && n >= 0.0 && n <= max)
            {
                return Err(format!(
                    "Qoder {field} must be a number between 0 and {max}"
                ));
            }
            parameters.insert(field.into(), value.clone());
        }
    }
    if let Some(value) = request.get("top_k").filter(|v| !v.is_null()) {
        if value.as_u64().is_none_or(|n| n == 0) {
            return Err("Qoder top_k must be a positive integer".into());
        }
        parameters.insert("top_k".into(), value.clone());
    }
    for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if request
            .get(field)
            .filter(|v| !v.is_null())
            .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0))
        {
            return Err(format!("Qoder {field} must be a positive integer"));
        }
    }
    if let Some(value) = request.get("preserve_thinking").filter(|v| !v.is_null()) {
        if !value.is_boolean() {
            return Err("Qoder preserve_thinking must be boolean".into());
        }
        parameters.insert("preserve_thinking".into(), value.clone());
    }
    for field in [
        "stop",
        "stop_sequences",
        "presence_penalty",
        "frequency_penalty",
        "seed",
        "logprobs",
        "top_logprobs",
        "response_format",
    ] {
        if request.get(field).is_some_and(|v| !v.is_null()) {
            return Err(format!(
                "Qoder does not support {field} in the verified generation contract"
            ));
        }
    }
    if request
        .get("n")
        .is_some_and(|v| !v.is_null() && v.as_u64() != Some(1))
    {
        return Err("Qoder supports exactly one output choice".into());
    }
    if request
        .get("parallel_tool_calls")
        .is_some_and(|v| !v.is_null() && v != &Value::Bool(true))
    {
        return Err(
            "Qoder cannot disable parallel_tool_calls in the verified generation contract".into(),
        );
    }
    Ok(Value::Object(parameters))
}

pub(super) fn preserve_history_reasoning(
    source: &Map<String, Value>,
    target: &mut Value,
) -> Result<(), String> {
    for field in ["reasoning_content", "reasoning_content_signature"] {
        if let Some(value) = source.get(field).filter(|v| !v.is_null()) {
            if !value.is_string() {
                return Err(format!("Qoder historical {field} must be a string"));
            }
            target[field] = value.clone();
        }
    }
    if let Some(item) = source.get("reasoning_item").filter(|v| !v.is_null()) {
        if !item.is_object()
            || item["type"]
                .as_str()
                .is_some_and(|kind| kind != "reasoning")
        {
            return Err("Qoder historical reasoning_item must be a reasoning object".into());
        }
        target["reasoning_item"] = item.clone();
    } else if let Some(text) = source
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        target["reasoning_item"] =
            serde_json::json!({"type":"reasoning","summary":[{"type":"summary_text","text":text}]});
    }
    Ok(())
}
