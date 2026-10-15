use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::{json, Map, Value};

const JSON_REF_MAX_DEPTH: usize = 64;
const JSON_REF_MAX_NODES: usize = 8_192;

#[derive(Debug)]
struct ToolCallBinding {
    raw_id: String,
    wire_id: String,
    name: String,
    content_index: usize,
}

pub(crate) fn validate_source_tool_choice(input: &Value) -> Result<(), String> {
    let Some(choice) = input.get("tool_choice") else {
        return Ok(());
    };
    if choice.is_null() {
        return Ok(());
    }

    let declared = source_declared_tool_names(input);
    let (choice_type, named) = match choice {
        Value::String(choice_type) => (choice_type.trim(), None),
        Value::Object(choice) => {
            let choice_type = choice
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "Antigravity tool_choice object requires a non-empty type".to_string()
                })?;
            let named = match choice_type {
                "function" | "custom" => choice
                    .get("name")
                    .or_else(|| choice.get("function").and_then(|value| value.get("name")))
                    .and_then(Value::as_str),
                "tool" => choice.get("name").and_then(Value::as_str),
                "tool_search" => Some("tool_search"),
                _ => None,
            };
            (choice_type, named)
        }
        _ => {
            return Err(
                "Antigravity tool_choice must be a string, object, null, or omitted".to_string(),
            )
        }
    };

    match choice_type {
        "auto" | "none" => Ok(()),
        "required" | "any" => {
            if declared.is_empty() {
                Err(format!(
                    "Antigravity tool_choice {choice_type} requires at least one declared tool"
                ))
            } else {
                Ok(())
            }
        }
        "function" | "custom" | "tool" | "tool_search" => {
            let name = named
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!("Antigravity named tool_choice {choice_type} requires a non-empty name")
                })?;
            if declared.contains(name) {
                Ok(())
            } else {
                Err(format!(
                    "Antigravity tool_choice names undeclared tool '{name}'"
                ))
            }
        }
        _ => Err(format!(
            "Antigravity does not support tool_choice type '{choice_type}'"
        )),
    }
}

pub(crate) fn apply_responses_reasoning_summary(
    source: &Value,
    request: &mut Value,
) -> Result<(), String> {
    let Some(effort) = source
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if matches!(
        effort.to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    ) {
        return Ok(());
    }

    let summary = source
        .pointer("/reasoning/summary")
        .or_else(|| source.pointer("/reasoning/generate_summary"));
    let include_thoughts = match summary {
        None => true,
        Some(Value::Null) => false,
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => false,
            "auto" | "concise" | "detailed" => true,
            value => {
                return Err(format!(
                    "Antigravity does not support reasoning summary mode '{value}'"
                ))
            }
        },
        Some(_) => {
            return Err(
                "Antigravity reasoning summary must be a string, null, or omitted".to_string(),
            )
        }
    };

    let request = request
        .as_object_mut()
        .ok_or_else(|| "Antigravity transformed request must be a JSON object".to_string())?;
    let generation_config = object_entry(request, "generationConfig")?;
    let thinking_config = object_entry(generation_config, "thinkingConfig")?;
    thinking_config.insert("includeThoughts".to_string(), Value::Bool(include_thoughts));
    Ok(())
}

pub(crate) fn normalize_wire_request(request: &mut Value) -> Result<(), String> {
    if !request.is_object() {
        return Err("Antigravity request must be a JSON object".to_string());
    }
    let is_v1internal_envelope = request.get("request").is_some()
        && (request.get("requestType").is_some()
            || request.get("project").is_some()
            || request.get("userAgent").is_some());
    if is_v1internal_envelope {
        let inner = request
            .get_mut("request")
            .filter(|value| value.is_object())
            .ok_or_else(|| "Antigravity request envelope must contain an object".to_string())?;
        normalize_wire_request_inner(inner)?;
        drop_final_gemini_thought_tail(request);
        return Ok(());
    }
    normalize_wire_request_inner(request)
}

fn normalize_wire_request_inner(request: &mut Value) -> Result<(), String> {
    validate_tool_field_aliases(request)?;
    validate_tool_name_boundaries(request)?;
    validate_tool_id_types(request)?;
    normalize_tool_call_ids(request)?;
    normalize_tool_names(request)?;
    stringify_ref_function_responses(request)?;
    preserve_function_response_adjacency(request);
    validate_fixed_thinking_budget(request)?;
    Ok(())
}

fn validate_tool_field_aliases(request: &Value) -> Result<(), String> {
    let reject_pair = |object: &Map<String, Value>, primary: &str, alias: &str| {
        if object.contains_key(primary) && object.contains_key(alias) {
            Err(format!(
                "Antigravity request cannot contain both {primary} and {alias}"
            ))
        } else {
            Ok(())
        }
    };
    if let Some(object) = request.as_object() {
        reject_pair(object, "toolConfig", "tool_config")?;
    }
    for tool in request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(tool) = tool.as_object() {
            reject_pair(tool, "functionDeclarations", "function_declarations")?;
        }
    }
    for config in ["toolConfig", "tool_config"] {
        let Some(config) = request.get(config).and_then(Value::as_object) else {
            continue;
        };
        reject_pair(config, "functionCallingConfig", "function_calling_config")?;
        for function_config in ["functionCallingConfig", "function_calling_config"] {
            if let Some(function_config) = config.get(function_config).and_then(Value::as_object) {
                reject_pair(
                    function_config,
                    "allowedFunctionNames",
                    "allowed_function_names",
                )?;
            }
        }
    }
    for content in request
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
            let Some(part) = part.as_object() else {
                continue;
            };
            for (primary, alias) in [
                ("functionCall", "function_call"),
                ("functionResponse", "function_response"),
            ] {
                if part.contains_key(primary) && part.contains_key(alias) {
                    return Err(format!(
                        "Antigravity tool part cannot contain both {primary} and {alias}"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_tool_id_types(request: &Value) -> Result<(), String> {
    for content in request
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
            for (key, label) in [
                ("functionCall", "functionCall"),
                ("function_call", "functionCall"),
                ("functionResponse", "functionResponse"),
                ("function_response", "functionResponse"),
            ] {
                if part
                    .get(key)
                    .and_then(Value::as_object)
                    .and_then(|tool| tool.get("id"))
                    .is_some_and(|id| !id.is_string())
                {
                    return Err(format!("Antigravity {label} id must be a string"));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn drop_final_gemini_thought_tail(envelope: &mut Value) {
    // Only the outer envelope model is authoritative after model mapping and
    // web-search fallback. The raw transformed body's model can differ.
    let gemini_target = envelope
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.to_ascii_lowercase().contains("gemini"));
    if gemini_target {
        if let Some(inner) = envelope.get_mut("request") {
            drop_trailing_thought_only_model_turns(inner);
            append_trailing_user_after_media_function_response(inner);
        }
    }
}

fn append_trailing_user_after_media_function_response(request: &mut Value) {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };
    let needs_user = contents.last().is_some_and(|content| {
        matches!(
            content.get("role").and_then(Value::as_str),
            Some("model" | "assistant")
        ) && content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("functionResponse")
                        .or_else(|| part.get("function_response"))
                        .and_then(|response| response.get("parts"))
                        .and_then(Value::as_array)
                        .is_some_and(|nested| {
                            nested.iter().any(|item| {
                                ["inlineData", "inline_data", "fileData", "file_data"]
                                    .iter()
                                    .any(|key| item.get(*key).is_some())
                            })
                        })
                })
            })
    });
    if needs_user {
        contents.push(json!({"role":"user","parts":[{"text":""}]}));
    }
}

fn drop_trailing_thought_only_model_turns(request: &mut Value) {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };
    while contents.len() > 1
        && contents.last().is_some_and(|content| {
            matches!(
                content.get("role").and_then(Value::as_str),
                Some("model" | "assistant")
            ) && content
                .get("parts")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    !parts.is_empty()
                        && parts.iter().all(|part| {
                            part.as_object().is_some_and(|part| {
                                part.get("thought") == Some(&Value::Bool(true))
                                    && part.iter().all(|(key, value)| match key.as_str() {
                                        "thought" | "text" | "thoughtSignature" => true,
                                        "thought_signature" => value.is_string(),
                                        "extra_content" => value.as_object().is_some_and(|extra| {
                                            extra.len() == 1
                                                && extra
                                                    .get("google")
                                                    .and_then(Value::as_object)
                                                    .is_some_and(|google| {
                                                        google.len() == 1
                                                            && google
                                                                .get("thought_signature")
                                                                .is_some_and(Value::is_string)
                                                    })
                                        }),
                                        _ => false,
                                    })
                            })
                        })
                })
        })
    {
        contents.pop();
    }
}

fn validate_tool_name_boundaries(request: &Value) -> Result<(), String> {
    let check = |name: &Value| -> Result<(), String> {
        if name.as_str().is_some_and(|name| name.trim() != name) {
            return Err(
                "Antigravity function names must not have surrounding whitespace".to_string(),
            );
        }
        Ok(())
    };
    for tool in request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for declaration in tool
            .get("functionDeclarations")
            .or_else(|| tool.get("function_declarations"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = declaration.get("name") {
                check(name)?;
            }
        }
    }
    for content in request
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
            for key in [
                "functionCall",
                "function_call",
                "functionResponse",
                "function_response",
            ] {
                if let Some(name) = part.get(key).and_then(|value| value.get("name")) {
                    check(name)?;
                }
            }
        }
    }
    if let Some(allowed) = gemini_allowed_function_names(request) {
        for name in allowed {
            check(name)?;
        }
    }
    Ok(())
}

fn validate_fixed_thinking_budget(request: &Value) -> Result<(), String> {
    let Some(config) = request.get("generationConfig") else {
        return Ok(());
    };
    let Some(thinking) = config.get("thinkingConfig") else {
        return Ok(());
    };
    if thinking.get("thinkingLevel").is_some() {
        return Ok(());
    }
    let Some(budget) = thinking.get("thinkingBudget").and_then(Value::as_u64) else {
        return Ok(());
    };
    let Some(max_output) = config.get("maxOutputTokens").and_then(Value::as_u64) else {
        return Ok(());
    };
    if max_output <= budget {
        return Err(
            "Antigravity fixed thinkingBudget must be less than maxOutputTokens".to_string(),
        );
    }
    Ok(())
}

fn object_entry<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let value = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    value
        .as_object_mut()
        .ok_or_else(|| format!("Antigravity {key} must be a JSON object"))
}

fn aliased_field_mut<'a>(
    value: &'a mut Value,
    primary: &str,
    alias: &str,
) -> Option<&'a mut Value> {
    let object = value.as_object_mut()?;
    if object.contains_key(primary) {
        object.get_mut(primary)
    } else {
        object.get_mut(alias)
    }
}

fn source_declared_tool_names(input: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(tools) = input.get("tools").and_then(Value::as_array) else {
        return names;
    };
    for tool in tools {
        collect_source_tool_names(tool, &mut names);
    }
    names
}

fn collect_source_tool_names(tool: &Value, names: &mut BTreeSet<String>) {
    if let Some(name) = tool
        .get("name")
        .or_else(|| tool.pointer("/function/name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        names.insert(name.to_string());
    }
    if let Some(tool_type) = tool
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if matches!(
            tool_type,
            "tool_search" | "web_search" | "computer" | "code_interpreter"
        ) {
            names.insert(tool_type.to_string());
        }
    }
    for child in tool
        .get("tools")
        .or_else(|| tool.get("children"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_source_tool_names(child, names);
    }
}

fn normalize_tool_call_ids(request: &mut Value) -> Result<(), String> {
    let Some(contents) = request.get("contents").and_then(Value::as_array) else {
        return Ok(());
    };
    let mut bindings = Vec::new();
    let mut raw_ids = HashSet::new();
    let mut wire_ids = HashSet::new();

    for (content_index, content) in contents.iter().enumerate() {
        for part in content
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(call) = part
                .get("functionCall")
                .or_else(|| part.get("function_call"))
                .and_then(Value::as_object)
            else {
                continue;
            };
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!("Antigravity functionCall in contents[{content_index}] requires a name")
                })?
                .to_string();
            let raw_id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("tool_call_{}", bindings.len()));
            if !raw_ids.insert(raw_id.clone()) {
                return Err(format!("duplicate Antigravity functionCall id '{raw_id}'"));
            }
            let base = sanitize_tool_call_id(&raw_id);
            let wire_id = unique_wire_id(&base, &mut wire_ids);
            bindings.push(ToolCallBinding {
                raw_id,
                wire_id,
                name,
                content_index,
            });
        }
    }

    if bindings.is_empty() {
        // A continuation may carry only functionResponse parts here. The
        // provider-scoped reasoning replay inserts the signed model call after
        // this early wire pass, then normalizes the completed history again.
        return Ok(());
    }

    let contents = request
        .get_mut("contents")
        .and_then(Value::as_array_mut)
        .expect("contents existed as an array during the immutable pass");
    let mut binding_index = 0usize;
    for content in contents.iter_mut() {
        for part in content
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let Some(call) = aliased_field_mut(part, "functionCall", "function_call")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            call.insert(
                "id".to_string(),
                Value::String(bindings[binding_index].wire_id.clone()),
            );
            binding_index += 1;
        }
    }

    let mut responded = HashSet::new();
    for (content_index, content) in contents.iter_mut().enumerate() {
        for part in content
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let Some(response) = aliased_field_mut(part, "functionResponse", "function_response")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            let response_id = response
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            let response_name = response
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let candidates = response_binding_candidates(
                &bindings,
                &responded,
                content_index,
                response_id.as_deref(),
                response_name.as_deref(),
            );
            if candidates.is_empty()
                && !bindings
                    .iter()
                    .any(|binding| binding.content_index < content_index)
            {
                // This result may belong to a signed call restored by the
                // provider replay cache. Do not invent a pairing here.
                continue;
            }
            if candidates.len() != 1 {
                return Err(match (response_id.as_deref(), response_name.as_deref()) {
                    (Some(id), _) => format!(
                        "Antigravity functionResponse id '{id}' does not uniquely match a preceding functionCall"
                    ),
                    (None, Some(name)) => format!(
                        "Antigravity functionResponse name '{name}' does not uniquely match a preceding functionCall"
                    ),
                    (None, None) => "Antigravity functionResponse requires an id or unique name"
                        .to_string(),
                });
            }
            let index = candidates[0];
            let binding = &bindings[index];
            if response_name
                .as_deref()
                .is_some_and(|name| name != binding.name)
            {
                return Err(format!(
                    "Antigravity functionResponse name '{}' does not match functionCall name '{}'",
                    response_name.as_deref().unwrap_or_default(),
                    binding.name
                ));
            }
            responded.insert(index);
            response.insert("id".to_string(), Value::String(binding.wire_id.clone()));
            response.insert("name".to_string(), Value::String(binding.name.clone()));
        }
    }
    Ok(())
}

fn response_binding_candidates(
    bindings: &[ToolCallBinding],
    responded: &HashSet<usize>,
    content_index: usize,
    response_id: Option<&str>,
    response_name: Option<&str>,
) -> Vec<usize> {
    let eligible = |index: usize, binding: &ToolCallBinding| {
        binding.content_index < content_index && !responded.contains(&index)
    };
    if let Some(response_id) = response_id {
        let raw_exact = bindings
            .iter()
            .enumerate()
            .filter(|(index, binding)| eligible(*index, binding))
            .filter(|(_, binding)| binding.raw_id == response_id)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !raw_exact.is_empty() {
            return raw_exact;
        }
        let wire_exact = bindings
            .iter()
            .enumerate()
            .filter(|(index, binding)| eligible(*index, binding))
            .filter(|(_, binding)| binding.wire_id == response_id)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !wire_exact.is_empty() {
            return wire_exact;
        }
        let compatibility_key = compatibility_tool_id(response_id);
        return bindings
            .iter()
            .enumerate()
            .filter(|(index, binding)| eligible(*index, binding))
            .filter(|(_, binding)| {
                compatibility_tool_id(&binding.raw_id) == compatibility_key
                    || compatibility_tool_id(&binding.wire_id) == compatibility_key
            })
            .map(|(index, _)| index)
            .collect();
    }

    let Some(response_name) = response_name else {
        return Vec::new();
    };
    bindings
        .iter()
        .enumerate()
        .filter(|(index, binding)| eligible(*index, binding))
        .filter(|(_, binding)| binding.name == response_name)
        .map(|(index, _)| index)
        .collect()
}

fn sanitize_tool_call_id(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "tool_call".to_string()
    } else {
        sanitized
    }
}

fn unique_wire_id(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn compatibility_tool_id(id: &str) -> String {
    id.chars()
        .filter(|character| !matches!(character, '_' | '-'))
        .collect()
}

fn normalize_tool_names(request: &mut Value) -> Result<(), String> {
    let declared = gemini_declared_tool_names(request)?;
    validate_gemini_tool_choice(request, &declared)?;

    let mut raw_names = Vec::new();
    raw_names.extend(declared.iter().cloned());
    if let Some(contents) = request.get("contents").and_then(Value::as_array) {
        for content in contents {
            for part in content
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for key in [
                    "functionCall",
                    "function_call",
                    "functionResponse",
                    "function_response",
                ] {
                    if let Some(name) = part
                        .get(key)
                        .and_then(|value| value.get("name"))
                        .and_then(Value::as_str)
                    {
                        raw_names.push(name.to_string());
                    }
                }
            }
        }
    }
    if let Some(allowed) = gemini_allowed_function_names(request) {
        for name in allowed {
            let name = name.as_str().ok_or_else(|| {
                "Antigravity allowedFunctionNames entries must be strings".to_string()
            })?;
            raw_names.push(name.to_string());
        }
    }

    let mut raw_to_wire = BTreeMap::new();
    let mut wire_to_raw = BTreeMap::<String, String>::new();
    for raw in raw_names {
        if raw.trim().is_empty() {
            return Err("Antigravity function names must not be empty".to_string());
        }
        let wire = sanitize_function_name(&raw);
        if let Some(existing) = wire_to_raw.get(&wire) {
            if existing != &raw {
                return Err(format!(
                    "Antigravity function names '{existing}' and '{raw}' collide after sanitization"
                ));
            }
        } else {
            wire_to_raw.insert(wire.clone(), raw.clone());
        }
        raw_to_wire.insert(raw, wire);
    }

    rewrite_gemini_tool_names(request, &raw_to_wire);
    let declared = gemini_declared_tool_names(request)?;
    validate_gemini_tool_choice(request, &declared)
}

fn gemini_declared_tool_names(request: &Value) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for tool in request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let declarations = tool
            .get("functionDeclarations")
            .or_else(|| tool.get("function_declarations"));
        let Some(declarations) = declarations else {
            continue;
        };
        let declarations = declarations
            .as_array()
            .ok_or_else(|| "Antigravity functionDeclarations must be an array".to_string())?;
        for declaration in declarations {
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "Antigravity function declaration requires a non-empty name".to_string()
                })?;
            names.insert(name.to_string());
        }
    }
    Ok(names)
}

fn validate_gemini_tool_choice(request: &Value, declared: &BTreeSet<String>) -> Result<(), String> {
    let Some(config) = request
        .get("toolConfig")
        .or_else(|| request.get("tool_config"))
    else {
        return Ok(());
    };
    let config = config
        .as_object()
        .ok_or_else(|| "Antigravity toolConfig must be a JSON object".to_string())?;
    let Some(function_config) = config
        .get("functionCallingConfig")
        .or_else(|| config.get("function_calling_config"))
    else {
        return Ok(());
    };
    let function_config = function_config
        .as_object()
        .ok_or_else(|| "Antigravity functionCallingConfig must be a JSON object".to_string())?;
    let mode = function_config
        .get("mode")
        .map(|mode| {
            mode.as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_ascii_uppercase)
                .ok_or_else(|| {
                    "Antigravity functionCallingConfig.mode must be a non-empty string".to_string()
                })
        })
        .transpose()?
        .unwrap_or_else(|| "AUTO".to_string());
    if !matches!(
        mode.as_str(),
        "AUTO" | "ANY" | "NONE" | "VALIDATED" | "FORCED" | "REQUIRED"
    ) {
        return Err(format!(
            "Antigravity does not support function calling mode '{mode}'"
        ));
    }
    if matches!(mode.as_str(), "ANY" | "FORCED" | "REQUIRED") && declared.is_empty() {
        return Err(format!(
            "Antigravity function calling mode {mode} requires a declared function"
        ));
    }
    if let Some(allowed) = function_config
        .get("allowedFunctionNames")
        .or_else(|| function_config.get("allowed_function_names"))
    {
        let allowed = allowed
            .as_array()
            .ok_or_else(|| "Antigravity allowedFunctionNames must be an array".to_string())?;
        if allowed.is_empty() {
            return Err("Antigravity allowedFunctionNames must not be empty".to_string());
        }
        if mode == "NONE" {
            return Err(
                "Antigravity NONE tool choice cannot include allowedFunctionNames".to_string(),
            );
        }
        for name in allowed {
            let name = name
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "Antigravity allowedFunctionNames entries must be non-empty strings".to_string()
                })?;
            if !declared.contains(name) {
                return Err(format!(
                    "Antigravity tool choice names undeclared function '{name}'"
                ));
            }
        }
    }
    Ok(())
}

fn gemini_allowed_function_names(request: &Value) -> Option<&Vec<Value>> {
    request
        .get("toolConfig")
        .or_else(|| request.get("tool_config"))
        .and_then(|config| {
            config
                .get("functionCallingConfig")
                .or_else(|| config.get("function_calling_config"))
        })
        .and_then(|config| {
            config
                .get("allowedFunctionNames")
                .or_else(|| config.get("allowed_function_names"))
        })
        .and_then(Value::as_array)
}

fn sanitize_function_name(raw: &str) -> String {
    let mut output = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output
        .as_bytes()
        .first()
        .is_none_or(|first| !first.is_ascii_alphabetic() && *first != b'_')
    {
        output.insert(0, '_');
    }
    output.truncate(64);
    output
}

fn rewrite_gemini_tool_names(request: &mut Value, names: &BTreeMap<String, String>) {
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let declarations =
                aliased_field_mut(tool, "functionDeclarations", "function_declarations")
                    .and_then(Value::as_array_mut);
            for declaration in declarations.into_iter().flatten() {
                rewrite_name_field(declaration, names);
            }
        }
    }
    if let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) {
        for content in contents {
            for part in content
                .get_mut("parts")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                for key in [
                    "functionCall",
                    "function_call",
                    "functionResponse",
                    "function_response",
                ] {
                    if let Some(value) = part.get_mut(key) {
                        rewrite_name_field(value, names);
                    }
                }
            }
        }
    }
    if let Some(config) = aliased_field_mut(request, "toolConfig", "tool_config")
        .and_then(|value| {
            aliased_field_mut(value, "functionCallingConfig", "function_calling_config")
        })
        .and_then(Value::as_object_mut)
    {
        let allowed = if config.contains_key("allowedFunctionNames") {
            config.get_mut("allowedFunctionNames")
        } else {
            config.get_mut("allowed_function_names")
        };
        if let Some(allowed) = allowed.and_then(Value::as_array_mut) {
            for name in allowed {
                if let Some(raw) = name.as_str() {
                    if let Some(wire) = names.get(raw) {
                        *name = Value::String(wire.clone());
                    }
                }
            }
        }
    }
}

fn rewrite_name_field(value: &mut Value, names: &BTreeMap<String, String>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(raw) = object.get("name").and_then(Value::as_str) else {
        return;
    };
    if let Some(wire) = names.get(raw) {
        object.insert("name".to_string(), Value::String(wire.clone()));
    }
}

fn stringify_ref_function_responses(request: &mut Value) -> Result<(), String> {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for content in contents {
        for part in content
            .get_mut("parts")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let Some(response) = aliased_field_mut(part, "functionResponse", "function_response")
                .and_then(Value::as_object_mut)
                .and_then(|response| response.get_mut("response"))
            else {
                continue;
            };
            if normalized_opaque_ref_response(response)? {
                continue;
            }
            let mut nodes = 0usize;
            if contains_string_json_ref(response, 0, &mut nodes)? {
                let opaque = match &*response {
                    Value::String(value) => value.clone(),
                    value => serde_json::to_string(value).map_err(|error| {
                        format!("encode Antigravity functionResponse result: {error}")
                    })?,
                };
                *response = json!({"result": opaque});
            }
        }
    }
    Ok(())
}

fn normalized_opaque_ref_response(response: &Value) -> Result<bool, String> {
    let Some(object) = response.as_object() else {
        return Ok(false);
    };
    if object.len() != 1 {
        return Ok(false);
    }
    let Some(Value::String(result)) = object.get("result") else {
        return Ok(false);
    };
    let Ok(parsed) = serde_json::from_str::<Value>(result) else {
        return Ok(false);
    };
    let mut nodes = 0usize;
    contains_string_json_ref(&parsed, 0, &mut nodes)
}

fn contains_string_json_ref(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<bool, String> {
    if depth > JSON_REF_MAX_DEPTH {
        return Err("Antigravity functionResponse exceeds JSON reference depth limit".to_string());
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > JSON_REF_MAX_NODES {
        return Err("Antigravity functionResponse exceeds JSON reference node limit".to_string());
    }
    match value {
        Value::Object(object) => {
            if object.get("$ref").is_some_and(Value::is_string) {
                return Ok(true);
            }
            for child in object.values() {
                if contains_string_json_ref(child, depth + 1, nodes)? {
                    return Ok(true);
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                if contains_string_json_ref(child, depth + 1, nodes)? {
                    return Ok(true);
                }
            }
        }
        Value::String(value) if value.contains("$ref") => {
            if let Ok(parsed) = serde_json::from_str::<Value>(value) {
                if matches!(parsed, Value::Object(_) | Value::Array(_))
                    && contains_string_json_ref(&parsed, depth + 1, nodes)?
                {
                    return Ok(true);
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn preserve_function_response_adjacency(request: &mut Value) {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };
    let original = std::mem::take(contents);
    let mut output = Vec::with_capacity(original.len());
    let mut index = 0usize;
    while index < original.len() {
        let content = original[index].clone();
        let call_ids = content_function_call_ids(&content);
        output.push(content);
        index += 1;
        if call_ids.is_empty() {
            continue;
        }

        let run_start = index;
        while index < original.len()
            && original[index].get("role").and_then(Value::as_str) == Some("user")
        {
            index += 1;
        }
        if run_start == index {
            continue;
        }

        let mut responses = Vec::new();
        let mut remainder = Vec::new();
        for mut user in original[run_start..index].iter().cloned() {
            let Some(parts) = user.get_mut("parts").and_then(Value::as_array_mut) else {
                remainder.push(user);
                continue;
            };
            let mut other_parts = Vec::with_capacity(parts.len());
            for part in std::mem::take(parts) {
                let response_id = part
                    .get("functionResponse")
                    .or_else(|| part.get("function_response"))
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str);
                if response_id.is_some_and(|id| call_ids.contains(id)) {
                    responses.push(part);
                } else {
                    other_parts.push(part);
                }
            }
            if !other_parts.is_empty() {
                *parts = other_parts;
                remainder.push(user);
            }
        }
        if responses.is_empty() {
            output.extend(original[run_start..index].iter().cloned());
        } else {
            output.push(json!({"role": "user", "parts": responses}));
            output.extend(remainder);
        }
    }
    *contents = output;
}

fn content_function_call_ids(content: &Value) -> BTreeSet<String> {
    content
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("functionCall")
                .or_else(|| part.get("function_call"))
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_trailing_thought_only_turns_are_removed_without_touching_other_history() {
        let user = json!({"role":"user","parts":[{"text":"hello"}]});
        let thought = json!({"role":"model","parts":[{"text":"plan","thought":true,"thoughtSignature":"sig"}]});
        let mixed_text =
            json!({"role":"model","parts":[{"text":"plan","thought":true},{"text":"answer"}]});
        let mixed_call = json!({"role":"model","parts":[{"text":"plan","thought":true},{"functionCall":{"name":"lookup","args":{}}}]});
        let mixed_media = json!({"role":"model","parts":[{"text":"plan","thought":true,"inlineData":{"mimeType":"image/png","data":"AA=="}}]});
        for preserved in [mixed_text, mixed_call, mixed_media] {
            let mut request = json!({"model":"gemini-3.5-pro","project":"project","request":{"contents":[user, preserved]}});
            let part_count = request["request"]["contents"][1]["parts"]
                .as_array()
                .unwrap()
                .len();
            normalize_wire_request(&mut request).unwrap();
            assert_eq!(request["request"]["contents"].as_array().unwrap().len(), 2);
            assert_eq!(
                request["request"]["contents"][1]["parts"]
                    .as_array()
                    .unwrap()
                    .len(),
                part_count
            );
        }

        let mut request = json!({"model":"gemini-3.5-pro","project":"project","request":{"contents":[user, thought, thought]}});
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request["request"]["contents"], json!([user]));
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request["request"]["contents"], json!([user]));

        let mut only_thought =
            json!({"model":"gemini-3.5-pro","project":"project","request":{"contents":[thought]}});
        normalize_wire_request(&mut only_thought).unwrap();
        assert_eq!(only_thought["request"]["contents"], json!([thought]));

        let mut claude = json!({"model":"claude-sonnet-4-6","project":"project","request":{"model":"gemini-3.5-pro","contents":[user, thought]}});
        normalize_wire_request(&mut claude).unwrap();
        assert_eq!(claude["request"]["contents"], json!([user, thought]));

        let mut mapped_gemini = json!({"model":"gemini-3.5-pro","project":"project","request":{"model":"claude-sonnet-4-6","contents":[user, thought]}});
        normalize_wire_request(&mut mapped_gemini).unwrap();
        assert_eq!(mapped_gemini["request"]["contents"], json!([user]));

        let mut raw_body = json!({"model":"gemini-3.5-pro","contents":[user, thought]});
        normalize_wire_request(&mut raw_body).unwrap();
        assert_eq!(raw_body["contents"], json!([user, thought]));
    }

    #[test]
    fn gemini_trailing_assistant_thought_only_turn_is_removed() {
        let user = json!({"role":"user","parts":[{"text":"hello"}]});
        let thought = json!({"role":"assistant","parts":[{"text":"plan","thought":true,"thoughtSignature":"sig"}]});
        let mut request = json!({
            "model":"gemini-3.5-pro",
            "project":"project",
            "request":{"contents":[user, thought, thought]}
        });
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request["request"]["contents"], json!([user]));

        for preserved in [
            json!({"role":"assistant","parts":[{"text":"plan","thought":true},{"text":"answer"}]}),
            json!({"role":"assistant","parts":[{"functionCall":{"name":"lookup","args":{}}}]}),
            json!({"role":"assistant","parts":[{"text":"plan","thought":true,"inlineData":{"mimeType":"image/png","data":"AA=="}}]}),
        ] {
            let mut request = json!({
                "model":"gemini-3.5-pro",
                "project":"project",
                "request":{"contents":[user, preserved]}
            });
            normalize_wire_request(&mut request).unwrap();
            let contents = request["request"]["contents"].as_array().unwrap();
            assert_eq!(contents.len(), 2);
            assert_eq!(contents[1]["role"], "assistant");
            assert_eq!(
                contents[1]["parts"].as_array().unwrap().len(),
                preserved["parts"].as_array().unwrap().len()
            );
        }

        let mut claude = json!({
            "model":"claude-sonnet-4-6",
            "project":"project",
            "request":{"contents":[user, thought]}
        });
        normalize_wire_request(&mut claude).unwrap();
        assert_eq!(claude["request"]["contents"], json!([user, thought]));
    }

    #[test]
    fn gemini_trailing_thought_with_known_signature_aliases_is_removed() {
        let user = json!({"role":"user","parts":[{"text":"hello"}]});
        for thought in [
            json!({"role":"model","parts":[{"text":"plan","thought":true,"thought_signature":"sig"}]}),
            json!({"role":"model","parts":[{"text":"plan","thought":true,"extra_content":{"google":{"thought_signature":"sig"}}}]}),
        ] {
            let mut request = json!({
                "model":"gemini-3.5-pro",
                "project":"project",
                "request":{"contents":[user, thought]}
            });
            normalize_wire_request(&mut request).unwrap();
            assert_eq!(request["request"]["contents"], json!([user]));
        }

        let unknown = json!({"role":"model","parts":[{"text":"plan","thought":true,"extra_content":{"google":{"thought_signature":"sig","other":"keep"}}}]});
        let mut request = json!({
            "model":"gemini-3.5-pro",
            "project":"project",
            "request":{"contents":[user, unknown]}
        });
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request["request"]["contents"], json!([user, unknown]));
    }

    #[test]
    fn gemini_model_role_media_tool_result_needs_trailing_user_turn() {
        let mut request = json!({
            "model":"gemini-3.5-flash", "project":"project",
            "request":{"contents":[
                {"role":"model","parts":[{"functionCall":{"id":"call_1","name":"view_image","args":{}}}]},
                {"role":"model","parts":[{"functionResponse":{
                    "id":"call_1", "name":"view_image", "response":{"result":""},
                    "parts":[{"inlineData":{"mimeType":"image/png","data":"AAAA"}}]
                }}]}
            ]}
        });
        normalize_wire_request(&mut request).unwrap();
        let contents = request
            .pointer("/request/contents")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(contents.last().unwrap()["role"], "user");
        assert_eq!(contents.last().unwrap()["parts"], json!([{"text":""}]));
        assert!(contents[1]
            .pointer("/parts/0/functionResponse/parts/0/inlineData")
            .is_some());
        let once = request.clone();
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request, once);

        for (nested, expect_user) in [
            (
                json!({"fileData":{"mimeType":"image/png","fileUri":"gs://bucket/a.png"}}),
                true,
            ),
            (json!({"text":"ordinary result"}), false),
        ] {
            let mut request = json!({"model":"gemini-3.5-flash","project":"project","request":{
                "contents":[{"role":"model","parts":[{"functionResponse":{
                    "name":"view_image","response":{"result":""},"parts":[nested]
                }}]}]
            }});
            normalize_wire_request(&mut request).unwrap();
            assert_eq!(
                request["request"]["contents"].as_array().unwrap().len() == 2,
                expect_user
            );
        }

        for role_or_model in ["user", "claude-sonnet-4-6"] {
            let mut request = once.clone();
            request["request"]["contents"].as_array_mut().unwrap().pop();
            if role_or_model == "user" {
                request["request"]["contents"][1]["role"] = json!("user");
            } else {
                request["model"] = json!(role_or_model);
            }
            normalize_wire_request(&mut request).unwrap();
            assert_eq!(request["request"]["contents"].as_array().unwrap().len(), 2);
        }
    }

    #[test]
    fn fixed_thinking_budget_cannot_exceed_output_ceiling() {
        for mut request in [
            json!({"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingBudget":2048}}}),
            json!({"project":"p","request":{"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingBudget":2048}}}}),
        ] {
            let error = normalize_wire_request(&mut request).unwrap_err();
            assert!(error.contains("thinkingBudget"));
        }
        for mut request in [
            json!({"generationConfig":{"maxOutputTokens":2049,"thinkingConfig":{"thinkingBudget":2048}}}),
            json!({"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingBudget":-1}}}),
            json!({"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingLevel":"high"}}}),
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":2048}}}),
        ] {
            normalize_wire_request(&mut request).unwrap();
        }
        let mut explicit_model = json!({
            "project":"p",
            "model":"gemini-3.5-flash-medium",
            "request":{"generationConfig":{"maxOutputTokens":2048,"thinkingConfig":{"thinkingBudget":1024}}}
        });
        normalize_wire_request(&mut explicit_model).unwrap();
        assert_eq!(explicit_model["model"], "gemini-3.5-flash-medium");
        assert_eq!(
            explicit_model.pointer("/request/generationConfig/thinkingConfig/thinkingBudget"),
            Some(&json!(1024))
        );
    }

    #[test]
    fn responses_reasoning_summary_is_independent_from_reasoning_effort() {
        for (summary, expected) in [
            (None, true),
            (Some(json!("auto")), true),
            (Some(json!("detailed")), true),
            (Some(json!("none")), false),
            (Some(Value::Null), false),
        ] {
            let mut source = json!({"reasoning": {"effort": "high"}});
            if let Some(summary) = summary {
                source["reasoning"]["summary"] = summary;
            }
            let mut request = json!({
                "generationConfig": {"thinkingConfig": {"thinkingLevel": "high"}}
            });
            apply_responses_reasoning_summary(&source, &mut request).unwrap();
            assert_eq!(
                request.pointer("/generationConfig/thinkingConfig/includeThoughts"),
                Some(&json!(expected))
            );
            assert_eq!(
                request.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
                Some(&json!("high"))
            );
        }

        let mut no_reasoning = json!({});
        apply_responses_reasoning_summary(
            &json!({"reasoning": {"summary": "auto"}}),
            &mut no_reasoning,
        )
        .unwrap();
        assert!(no_reasoning.get("generationConfig").is_none());
    }

    #[test]
    fn source_tool_choice_rejects_unknown_empty_and_undeclared_names() {
        let base = json!({
            "tools": [{"type": "function", "name": "lookup"}]
        });
        for choice in [
            json!("future"),
            json!({"type": "future"}),
            json!({"type": "function", "name": ""}),
            json!({"type": "function", "name": "missing"}),
        ] {
            let mut input = base.clone();
            input["tool_choice"] = choice;
            assert!(validate_source_tool_choice(&input).is_err());
        }
        for choice in [
            json!("auto"),
            json!("required"),
            json!("none"),
            json!({"type": "function", "name": "lookup"}),
        ] {
            let mut input = base.clone();
            input["tool_choice"] = choice;
            validate_source_tool_choice(&input).unwrap();
        }
    }

    #[test]
    fn wire_rejects_ambiguous_tool_configuration_aliases() {
        let declarations = json!([{"name":"lookup","parameters":{"type":"object"}}]);
        for mut request in [
            json!({"tools":[{"functionDeclarations":declarations,"function_declarations":[{"name":"delete_all"}]}]}),
            json!({"tools":[{"functionDeclarations":declarations}],"toolConfig":{"functionCallingConfig":{"mode":"AUTO"}},"tool_config":{"functionCallingConfig":{"mode":"ANY"}}}),
            json!({"tools":[{"functionDeclarations":declarations}],"toolConfig":{"functionCallingConfig":{"mode":"AUTO"},"function_calling_config":{"mode":"ANY"}}}),
            json!({"tools":[{"functionDeclarations":declarations}],"toolConfig":{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["lookup"],"allowed_function_names":["delete_all"]}}}),
        ] {
            assert!(normalize_wire_request(&mut request).is_err(), "{request}");
        }

        for mut request in [
            json!({"tools":[{"functionDeclarations":declarations}],"toolConfig":{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["lookup"]}}}),
            json!({"project":"p","request":{"tools":[{"function_declarations":declarations}],"tool_config":{"function_calling_config":{"mode":"ANY","allowed_function_names":["lookup"]}}}}),
        ] {
            normalize_wire_request(&mut request).unwrap();
            let once = request.clone();
            normalize_wire_request(&mut request).unwrap();
            assert_eq!(request, once);
        }
    }

    #[test]
    fn wire_normalization_preserves_adjacency_refs_and_collision_safe_ids() {
        let mut request = json!({
            "tools": [{"functionDeclarations": [
                {"name": "lookup", "parameters": {"type": "object"}},
                {"name": "write", "parameters": {"type": "object"}}
            ]}],
            "contents": [
                {"role": "model", "parts": [
                    {"text": "private", "thought": true, "thoughtSignature": "sig"},
                    {"functionCall": {"id": "call_a|b", "name": "lookup", "args": {}}},
                    {"functionCall": {"id": "call_a_b", "name": "write", "args": {}}}
                ]},
                {"role": "user", "parts": [{"text": "keep after results"}]},
                {"role": "user", "parts": [{"functionResponse": {
                    "id": "call_a_b", "name": "write", "response": {"ok": true}
                }}]},
                {"role": "user", "parts": [{"functionResponse": {
                    "id": "call_a|b", "name": "lookup",
                    "response": {"schema": {"$ref": "#/components/schemas/Error"}}
                }}]}
            ]
        });
        normalize_wire_request(&mut request).unwrap();

        assert_eq!(
            request.pointer("/contents/0/parts/1/functionCall/id"),
            Some(&json!("call_a_b"))
        );
        assert_eq!(
            request.pointer("/contents/0/parts/2/functionCall/id"),
            Some(&json!("call_a_b_1"))
        );
        assert_eq!(request["contents"][1]["parts"].as_array().unwrap().len(), 2);
        assert_eq!(
            request.pointer("/contents/1/parts/0/functionResponse/id"),
            Some(&json!("call_a_b_1"))
        );
        assert_eq!(
            request.pointer("/contents/1/parts/1/functionResponse/id"),
            Some(&json!("call_a_b"))
        );
        assert!(request
            .pointer("/contents/1/parts/1/functionResponse/response/result")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("#/components/schemas/Error")));
        assert_eq!(
            request.pointer("/contents/2/parts/0/text"),
            Some(&json!("keep after results"))
        );
        assert_eq!(
            request.pointer("/contents/0/parts/0/thoughtSignature"),
            Some(&json!("sig"))
        );

        let once = request.clone();
        normalize_wire_request(&mut request).unwrap();
        assert_eq!(request, once);
    }

    #[test]
    fn wire_normalization_uses_only_unique_compatibility_ids() {
        let mut unique = json!({"contents": [
            {"role": "model", "parts": [{"functionCall": {
                "id": "call573", "name": "lookup", "args": {}
            }}]},
            {"role": "user", "parts": [{"functionResponse": {
                "id": "call_573", "name": "lookup", "response": {"ok": true}
            }}]}
        ]});
        normalize_wire_request(&mut unique).unwrap();
        assert_eq!(
            unique.pointer("/contents/1/parts/0/functionResponse/id"),
            Some(&json!("call573"))
        );

        let mut ambiguous = json!({"contents": [
            {"role": "model", "parts": [
                {"functionCall": {"id": "call573", "name": "lookup", "args": {}}},
                {"functionCall": {"id": "call-573", "name": "lookup", "args": {}}}
            ]},
            {"role": "user", "parts": [{"functionResponse": {
                "id": "call_573", "name": "lookup", "response": {"ok": true}
            }}]}
        ]});
        assert!(normalize_wire_request(&mut ambiguous).is_err());
    }

    #[test]
    fn wire_normalization_rejects_name_collisions_and_preserves_decoys() {
        let mut collision = json!({
            "tools": [{"functionDeclarations": [
                {"name": "1tool"}, {"name": "_1tool"}
            ]}],
            "contents": [{"role": "user", "parts": [{"text": "ping"}]}]
        });
        assert!(normalize_wire_request(&mut collision).is_err());

        let mut decoy = json!({"contents": [
            {"role": "model", "parts": [{"functionCall": {
                "id": "call_1", "name": "lookup",
                "args": {"schema": {"$ref": "#/arguments/must-remain"}}
            }}]},
            {"role": "user", "parts": [{"functionResponse": {
                "id": "call_1", "name": "lookup", "response": {"ok": true}
            }}]}
        ]});
        normalize_wire_request(&mut decoy).unwrap();
        assert_eq!(
            decoy.pointer("/contents/0/parts/0/functionCall/args/schema/$ref"),
            Some(&json!("#/arguments/must-remain"))
        );
        assert_eq!(
            decoy.pointer("/contents/1/parts/0/functionResponse/response/ok"),
            Some(&json!(true))
        );
    }

    #[test]
    fn wire_normalization_rejects_padded_name_identity_drift() {
        for mut request in [
            json!({"tools": [{"functionDeclarations": [{"name": " lookup "}]}]}),
            json!({"contents": [{"role": "model", "parts": [{"functionCall": {
                "id": "call_1", "name": " lookup ", "args": {}
            }}]}]}),
            json!({"contents": [
                {"role": "model", "parts": [{"functionCall": {
                    "id": "call_1", "name": "lookup", "args": {}
                }}]},
                {"role": "user", "parts": [{"functionResponse": {
                    "id": "call_1", "name": " lookup ", "response": {"ok": true}
                }}]}
            ]}),
        ] {
            assert!(
                normalize_wire_request(&mut request).is_err(),
                "boundary whitespace must not change a tool's wire identity: {request}"
            );
        }

        let mut internal_space = json!({
            "tools": [{"functionDeclarations": [{"name": "lookup file"}]}],
            "contents": [{"role": "model", "parts": [{"functionCall": {
                "id": "call_1", "name": "lookup file", "args": {}
            }}]}]
        });
        normalize_wire_request(&mut internal_space).unwrap();
        assert_eq!(
            internal_space.pointer("/tools/0/functionDeclarations/0/name"),
            Some(&json!("lookup_file"))
        );
        assert_eq!(
            internal_space.pointer("/contents/0/parts/0/functionCall/name"),
            Some(&json!("lookup_file"))
        );
    }

    #[test]
    fn explicit_non_string_tool_ids_cannot_be_replaced_or_repaired() {
        for mut request in [
            json!({"contents":[{"role":"model","parts":[{"functionCall":{
                "id":42,"name":"lookup","args":{}
            }}]}]}),
            json!({"contents":[
                {"role":"model","parts":[{"functionCall":{"id":"call-a","name":"lookup","args":{}}}]},
                {"role":"user","parts":[{"functionResponse":{
                    "id":42,"name":"lookup","response":{"ok":true}
                }}]}
            ]}),
            json!({"contents":[{"role":"user","parts":[{"functionResponse":{
                "id":null,"name":"lookup","response":{"ok":true}
            }}]}]}),
            json!({"project":"fixture-project","requestType":"agent","request":{
                "contents":[{"role":"model","parts":[{"function_call":{
                    "id":false,"name":"lookup","args":{}
                }}]}]
            }}),
        ] {
            let error = normalize_wire_request(&mut request).unwrap_err();
            assert!(error.contains("id"), "{error}");
        }

        let mut missing_id = json!({"contents":[
            {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"ok":true}}}]}
        ]});
        normalize_wire_request(&mut missing_id).unwrap();
        assert_eq!(
            missing_id.pointer("/contents/0/parts/0/functionCall/id"),
            missing_id.pointer("/contents/1/parts/0/functionResponse/id")
        );
    }

    #[test]
    fn tool_parts_cannot_carry_both_spellings_of_one_identity() {
        for mut request in [
            json!({"contents":[{"role":"model","parts":[{
                "functionCall":{"id":"call-a","name":"lookup","args":{}},
                "function_call":{"id":"call-b","name":"lookup","args":{}}
            }]}]}),
            json!({"contents":[
                {"role":"model","parts":[{"functionCall":{"id":"call-a","name":"lookup","args":{}}}]},
                {"role":"user","parts":[{
                    "functionResponse":{"id":"call-a","name":"lookup","response":{"ok":true}},
                    "function_response":{"id":"call-b","name":"lookup","response":{"ok":false}}
                }]}
            ]}),
            json!({"contents":[{"role":"model","parts":[{
                "functionCall":{"id":"call a","name":"lookup","args":{}},
                "function_call":{"id":"call a","name":"lookup","args":{}}
            }]}]}),
        ] {
            assert!(
                normalize_wire_request(&mut request).is_err(),
                "one tool identity must not have competing wire spellings: {request}"
            );
        }

        for key in ["functionCall", "function_call"] {
            let mut request = json!({"contents":[{"role":"model","parts":[{}]}]});
            request["contents"][0]["parts"][0][key] =
                json!({"id":"call-a","name":"lookup","args":{}});
            normalize_wire_request(&mut request).unwrap();
        }
    }
}
