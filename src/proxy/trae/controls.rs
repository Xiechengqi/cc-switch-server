use super::*;
use crate::domain::providers::model::AppKind;

/// Capture client controls before the generic bridge can drop an alias or
/// replace an invalid value with a default. Model limits are checked later
/// against the bound channel's runtime capability.
pub(crate) struct TraeSourceControls(Map<String, Value>);

impl TraeSourceControls {
    pub(crate) fn from_request(app: AppKind, input: &Value) -> Result<Self, String> {
        media::validate_source_media(app, input)?;
        let mut controls = Map::new();
        for key in [
            "max_tokens",
            "max_completion_tokens",
            "max_output_tokens",
            "temperature",
            "top_p",
            "stop",
            "n",
            "response_format",
            "logprobs",
            "top_logprobs",
            "logit_bias",
            "seed",
            "frequency_penalty",
            "presence_penalty",
            "top_k",
            "parallel_tool_calls",
            "reasoning_effort",
            "reasoning",
            "reasoning_effort_level",
            "enable_thinking",
            "enable_reasoning",
            "is_reasoning",
            "is_max_mode",
        ] {
            if let Some(value) = input.get(key) {
                controls.insert(key.into(), value.clone());
            }
        }
        if let Some(format) = input.pointer("/text/format") {
            merge(&mut controls, "response_format", format)?;
        }
        if let Some(format) = input.pointer("/output_config/format") {
            merge(&mut controls, "response_format", format)?;
        }
        capture_tool_controls(app, input, &mut controls)?;
        if app == AppKind::Claude {
            if let Some(stop) = input.get("stop_sequences") {
                merge(&mut controls, "stop", stop)?;
            }
            if let Some(effort) = input.pointer("/output_config/effort") {
                merge_effort(&mut controls, effort)?;
            }
            if let Some(thinking) = input.get("thinking").filter(|value| !value.is_null()) {
                let kind = thinking
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or("Trae thinking requires an explicit type")?;
                if !matches!(kind, "enabled" | "adaptive" | "disabled") {
                    return Err("Trae thinking type is unsupported".into());
                }
                if let Some(effort) = thinking.get("effort") {
                    merge_effort(&mut controls, effort)?;
                }
                if kind == "disabled" {
                    merge_effort(&mut controls, &json!("none"))?;
                } else {
                    // A token budget cannot be enforced by Trae's effort-only
                    // wire contract. Require an effort instead of inventing one.
                    if thinking
                        .get("budget_tokens")
                        .is_some_and(|value| !value.is_null())
                    {
                        return Err("Trae cannot enforce thinking.budget_tokens; use thinking.effort or output_config.effort".into());
                    }
                    if requested_reasoning_level(&controls)?.is_none() {
                        merge_effort(&mut controls, &json!("medium"))?;
                    }
                }
            }
        }
        if app == AppKind::Gemini {
            for config_key in ["generationConfig", "generation_config"] {
                let Some(config) = input.get(config_key) else {
                    continue;
                };
                if !config.is_object() {
                    return Err("Trae generationConfig must be an object".into());
                }
                for (keys, target) in [
                    (
                        &["maxOutputTokens", "max_output_tokens"][..],
                        "max_output_tokens",
                    ),
                    (&["temperature"][..], "temperature"),
                    (&["topP", "top_p"][..], "top_p"),
                    (&["topK", "top_k"][..], "top_k"),
                    (&["stopSequences", "stop_sequences"][..], "stop"),
                    (&["candidateCount", "candidate_count"][..], "n"),
                    (&["seed"][..], "seed"),
                    (
                        &["frequencyPenalty", "frequency_penalty"][..],
                        "frequency_penalty",
                    ),
                    (
                        &["presencePenalty", "presence_penalty"][..],
                        "presence_penalty",
                    ),
                    (&["responseLogprobs", "response_logprobs"][..], "logprobs"),
                    (&["logprobs"][..], "top_logprobs"),
                ] {
                    for key in keys {
                        if let Some(value) = config.get(*key) {
                            merge(&mut controls, target, value)?;
                        }
                    }
                }
                for key in [
                    "responseSchema",
                    "response_schema",
                    "responseJsonSchema",
                    "response_json_schema",
                ] {
                    if config.get(key).is_some_and(|value| !value.is_null()) {
                        return Err("Trae structured output constraints are unsupported".into());
                    }
                }
                for key in ["responseMimeType", "response_mime_type"] {
                    if config.get(key).is_some_and(|value| {
                        !value.is_null() && value.as_str() != Some("text/plain")
                    }) {
                        return Err("Trae supports only plain text output".into());
                    }
                }
                for key in ["thinkingConfig", "thinking_config"] {
                    let Some(thinking) = config.get(key) else {
                        continue;
                    };
                    if !thinking.is_object() {
                        return Err("Trae thinkingConfig must be an object".into());
                    }
                    for level in ["thinkingLevel", "thinking_level"] {
                        if let Some(value) = thinking.get(level) {
                            merge_effort(&mut controls, value)?;
                        }
                    }
                    for budget in ["thinkingBudget", "thinking_budget"] {
                        if let Some(value) = thinking.get(budget) {
                            match value.as_i64() {
                                Some(0) => merge_effort(&mut controls, &json!("none"))?,
                                Some(-1) => {} // Automatic upstream effort; do not fabricate a level.
                                _ => {
                                    return Err(
                                        "Trae cannot enforce thinkingBudget; use thinkingLevel"
                                            .into(),
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
        for key in [
            "reasoning_budget_tokens",
            "context_length",
            "max_input_tokens",
        ] {
            if input.get(key).is_some_and(|value| !value.is_null()) {
                return Err(format!("Trae cannot enforce {key}"));
            }
        }
        let effort = requested_reasoning_level(&controls)?;
        for key in [
            "reasoning",
            "reasoning_effort",
            "reasoning_effort_level",
            "enable_thinking",
            "enable_reasoning",
            "is_reasoning",
        ] {
            controls.remove(key);
        }
        if let Some(effort) = effort {
            controls.insert("reasoning_effort".into(), json!(effort));
        }
        normalize_generation_controls_with_limit(&mut controls, None)?;
        Ok(Self(controls))
    }

    pub(crate) fn apply(self, output: &mut Value) {
        let output = output.as_object_mut().expect("Trae canonical Chat object");
        // Clear inferred bridge defaults. Explicit values below are authoritative.
        for key in [
            "max_tokens",
            "max_completion_tokens",
            "max_output_tokens",
            "reasoning",
            "reasoning_effort",
            "reasoning_effort_level",
        ] {
            output.remove(key);
        }
        output.extend(self.0);
    }
}

fn merge(controls: &mut Map<String, Value>, key: &str, value: &Value) -> Result<(), String> {
    if let Some(existing) = controls.get(key).filter(|value| !value.is_null()) {
        if !value.is_null() && existing != value {
            return Err(format!("Trae {key} controls conflict"));
        }
    }
    if !value.is_null() {
        controls.insert(key.into(), value.clone());
    }
    Ok(())
}

fn merge_effort(controls: &mut Map<String, Value>, value: &Value) -> Result<(), String> {
    let effort = value
        .as_str()
        .and_then(normalize_reasoning_level)
        .ok_or("unsupported Trae reasoning effort")?;
    if requested_reasoning_level(controls)?.is_some_and(|current| current != effort) {
        return Err("Trae reasoning controls conflict".into());
    }
    controls.insert("reasoning_effort_level".into(), json!(effort));
    Ok(())
}

fn capture_tool_controls(
    app: AppKind,
    input: &Value,
    controls: &mut Map<String, Value>,
) -> Result<(), String> {
    if app == AppKind::Claude {
        if let Some(choice) = input.get("tool_choice").filter(|value| !value.is_null()) {
            let kind = choice
                .as_str()
                .or_else(|| choice.get("type").and_then(Value::as_str))
                .ok_or("Trae tool_choice requires an explicit type")?;
            let canonical = match kind {
                "auto" | "none" => json!(kind),
                "any" => json!("required"),
                "tool" => {
                    json!({"type":"function","function":{"name": exact_function_name(choice.get("name"))?}})
                }
                _ => return Err("Trae Anthropic tool_choice type is unsupported".into()),
            };
            merge(controls, "tool_choice", &canonical)?;
            if let Some(disabled) = choice.get("disable_parallel_tool_use") {
                let disabled = disabled
                    .as_bool()
                    .ok_or("Trae disable_parallel_tool_use must be a boolean")?;
                merge(controls, "parallel_tool_calls", &json!(!disabled))?;
            }
        }
    }
    if app != AppKind::Gemini {
        return Ok(());
    }
    let Some(config) = aliased_control(input, "toolConfig", "tool_config")? else {
        return Ok(());
    };
    if !config.is_object() {
        return Err("Trae toolConfig must be an object".into());
    }
    let Some(config) = aliased_control(config, "functionCallingConfig", "function_calling_config")?
    else {
        return Ok(());
    };
    if !config.is_object() {
        return Err("Trae functionCallingConfig must be an object".into());
    }
    let mode = match config.get("mode") {
        None => "AUTO".to_string(),
        Some(value) => value
            .as_str()
            .ok_or("Trae function calling mode must be text")?
            .to_ascii_uppercase(),
    };
    if !matches!(mode.as_str(), "AUTO" | "ANY" | "NONE") {
        return Err("Trae function calling mode supports AUTO, ANY or NONE".into());
    }
    let mut declared = BTreeSet::new();
    for tool in input
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(functions) =
            aliased_control(tool, "functionDeclarations", "function_declarations")?
        {
            for function in functions
                .as_array()
                .ok_or("Trae function declarations must be an array")?
            {
                let name = exact_function_name(function.get("name"))?;
                if !declared.insert(name) {
                    return Err("Trae function declarations contain duplicate names".into());
                }
            }
        }
    }
    let mut allowed = BTreeSet::new();
    if let Some(names) = aliased_control(config, "allowedFunctionNames", "allowed_function_names")?
    {
        let names = names
            .as_array()
            .filter(|names| !names.is_empty())
            .ok_or("Trae allowedFunctionNames must be a nonempty array")?;
        for name in names {
            let name = exact_function_name(Some(name))?;
            if !declared.contains(name) || !allowed.insert(name) {
                return Err(
                    "Trae allowedFunctionNames must contain unique declared function names".into(),
                );
            }
        }
        if mode == "NONE" {
            return Err("Trae allowedFunctionNames conflicts with NONE mode".into());
        }
    }
    if mode == "ANY" && declared.is_empty() {
        return Err("Trae ANY mode requires declared functions".into());
    }
    let choice = match mode.as_str() {
        "NONE" => json!("none"),
        "ANY" if allowed.len() == 1 => {
            json!({"type":"function","function":{"name":allowed.first().unwrap()}})
        }
        "ANY" => json!("required"),
        _ => json!("auto"),
    };
    merge(controls, "tool_choice", &choice)
}

fn exact_function_name(value: Option<&Value>) -> Result<&str, String> {
    value
        .and_then(Value::as_str)
        .filter(|name| {
            !name.is_empty()
                && name.trim() == *name
                && name.len() <= 256
                && !name.chars().any(char::is_control)
        })
        .ok_or_else(|| "Trae requires an exact nonempty function name".into())
}

fn aliased_control<'a>(
    input: &'a Value,
    camel: &str,
    snake: &str,
) -> Result<Option<&'a Value>, String> {
    let first = input.get(camel).filter(|value| !value.is_null());
    let second = input.get(snake).filter(|value| !value.is_null());
    if first
        .zip(second)
        .is_some_and(|(first, second)| first != second)
    {
        return Err(format!("Trae {camel} aliases conflict"));
    }
    Ok(first.or(second))
}
