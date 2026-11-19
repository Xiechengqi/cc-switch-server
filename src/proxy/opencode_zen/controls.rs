//! Model/protocol-aware controls. Unrepresentable semantics fail before send;
//! neither the generic bridges nor fingerprint shaping may silently weaken them.
use super::{
    catalog::ZenAdmission, catalog_unavailable, effort_suffix, FixedCodexModel, RequestAdaptations,
    ZenWire,
};
use crate::proxy::{transforms::response_input_items, ProxyError, ProxyRoute};
use serde_json::{json, Map, Value};

pub(super) const KEYS: &[&str] = &[
    "temperature",
    "top_p",
    "top_k",
    "stop",
    "stop_sequences",
    "parallel_tool_calls",
    "reasoning_effort",
    "reasoning",
    "thinking",
    "output_config",
    "response_format",
    "text",
    "include",
];

fn unsupported(parameter: &str) -> ProxyError {
    ProxyError::bad_request(format!("OpenCode Zen unsupported control: {parameter}; this model/protocol has no current reviewed mapping"))
}

pub(super) fn parameter_error(parameter: &str, message: &str) -> ProxyError {
    ProxyError::bad_request(format!(
        "unsupported parameter `{parameter}`: OpenCode Zen {message}"
    ))
}

pub(super) const EFFORT_RUNGS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];

fn effort_rank(effort: &str) -> Option<usize> {
    EFFORT_RUNGS.iter().position(|&rung| rung == effort)
}

fn source_effort<'a>(
    request: &'a Value,
    model: Option<&'a str>,
    route: ProxyRoute,
) -> Result<Option<&'a str>, ProxyError> {
    let mut effort = None;
    for explicit in [
        request.get("reasoning_effort"),
        request.pointer("/reasoning/effort"),
        request.pointer("/output_config/effort"),
        request.pointer("/thinking/effort"),
    ]
    .into_iter()
    .flatten()
    {
        effort = Some(
            explicit
                .as_str()
                .filter(|value| effort.is_none_or(|prior| prior == *value))
                .ok_or_else(|| {
                    if route == ProxyRoute::CodexResponses {
                        parameter_error(
                            "reasoning.effort",
                            "requires matching reasoning effort strings",
                        )
                    } else {
                        ProxyError::bad_request(
                            "OpenCode Zen reasoning effort values must be matching strings",
                        )
                    }
                })?,
        );
    }
    let suffix = model.and_then(effort_suffix);
    if suffix
        .zip(effort)
        .is_some_and(|(suffix, effort)| suffix != effort)
    {
        return Err(if route == ProxyRoute::CodexResponses {
            parameter_error(
                "reasoning.effort",
                "conflicts with the request model's effort suffix",
            )
        } else {
            ProxyError::bad_request("OpenCode Zen model suffix and reasoning effort conflict")
        });
    }
    Ok(effort.or(suffix))
}

struct EffortSelection<'a> {
    requested: Option<&'a str>,
    effective: Option<&'a str>,
}

fn select_effort<'a>(
    request: &'a Value,
    route: ProxyRoute,
    admitted: &'a ZenAdmission,
    fixed_codex: Option<FixedCodexModel<'a>>,
) -> Result<EffortSelection<'a>, ProxyError> {
    let model = request.get("model").and_then(Value::as_str);
    let requested = source_effort(
        request,
        fixed_codex.map_or(model, |fixed| fixed.requested_model),
        route,
    )?;
    let pinned = fixed_codex.and_then(|_| model.and_then(effort_suffix));
    let mut selection = EffortSelection {
        requested,
        effective: pinned.or(requested),
    };
    let Some(effort) = selection.effective else {
        return Ok(selection);
    };

    let caps = &admitted.capabilities;
    if fixed_codex.is_some() {
        if requested.is_some_and(|value| value != "none" && effort_rank(value).is_none()) {
            return Err(parameter_error(
                "reasoning.effort",
                "has an unknown effort value",
            ));
        }
        if pinned.is_some_and(|value| value != "none" && effort_rank(value).is_none()) {
            return Err(parameter_error(
                "reasoning.effort",
                "has an invalid fixed upstream effort suffix",
            ));
        }
        if requested == Some("none") && effort != "none" {
            return Err(parameter_error(
                "reasoning.effort",
                "cannot disable reasoning with the fixed upstream effort suffix",
            ));
        }
        if admitted.stale {
            return Err(catalog_unavailable());
        }
        if effort == "none" {
            if admitted.endpoint.wire() == ZenWire::Messages
                && caps.reasoning == Some(true)
                && caps.reasoning_toggle == Some(true)
            {
                return Ok(selection);
            }
            return Err(parameter_error(
                "reasoning.effort",
                "has no reviewed reasoning-disable mapping for the resolved model",
            ));
        }
    }
    if route == ProxyRoute::CodexResponses && (admitted.stale || caps.reasoning != Some(true)) {
        return Err(parameter_error(
            "reasoning.effort",
            "requires a current reasoning capability for the resolved model",
        ));
    }
    require(caps.reasoning, admitted.stale, "reasoning effort")?;
    if caps.reasoning_efforts.iter().any(|rung| rung == effort) {
        return Ok(selection);
    }
    if let Some(rank) = effort_rank(effort).filter(|_| fixed_codex.is_some() && pinned.is_none()) {
        let nearest = caps
            .reasoning_efforts
            .iter()
            .filter_map(|rung| effort_rank(rung).map(|position| (position, rung.as_str())))
            .min_by_key(|(position, _)| (position.abs_diff(rank), *position));
        if let Some((_, nearest)) = nearest {
            selection.effective = Some(nearest);
            return Ok(selection);
        }
    }

    let model = model.map(super::base_model_id).unwrap_or("");
    let menu = caps.reasoning_efforts.join(", ");
    if route == ProxyRoute::CodexResponses {
        let control = if pinned.is_some() {
            "fixed upstream effort suffix"
        } else {
            "reasoning effort"
        };
        return Err(parameter_error(
            "reasoning.effort",
            &format!("{control} is not in the model menu for {model} (available: {menu})"),
        ));
    }
    if route == ProxyRoute::ClaudeMessages && !menu.is_empty() {
        return Err(ProxyError::bad_request(format!(
            "OpenCode Zen reasoning effort is not in the model menu for {model}; available: {menu}"
        )));
    }
    Err(unsupported("reasoning effort is not in the model menu"))
}

fn require(supported: Option<bool>, stale: bool, parameter: &str) -> Result<(), ProxyError> {
    if supported != Some(true) || stale {
        Err(unsupported(parameter))
    } else {
        Ok(())
    }
}

fn tools(request: &Value, route: ProxyRoute, admitted: &ZenAdmission) -> Result<(), ProxyError> {
    let Some(value) = request.get("tools").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    let list = value
        .as_array()
        .ok_or_else(|| ProxyError::bad_request("OpenCode Zen tools must be an array"))?;
    if !list.is_empty() && admitted.capabilities.tools == Some(false) {
        return Err(unsupported("tools"));
    }
    for tool in list {
        let kind = tool.get("type").and_then(Value::as_str);
        let regular = matches!(kind, Some("function"))
            || (kind.is_none() && tool.get("name").is_some() && tool.get("input_schema").is_some());
        let emulated = route == ProxyRoute::CodexResponses
            && admitted.endpoint.wire() != ZenWire::Responses
            && matches!(kind, Some("custom" | "namespace" | "tool_search"));
        if !regular && !emulated {
            if matches!(kind, Some("web_search" | "web_search_preview")) {
                return Err(unsupported(
                    "hosted web_search; disable Codex CLI web_search for this provider",
                ));
            }
            return Err(unsupported("hosted or native advanced tools"));
        }
        if tool["type"] == "namespace" {
            let children = tool.get("tools").or_else(|| tool.get("children"));
            let children = children
                .filter(|v| v.is_array())
                .ok_or_else(|| unsupported("namespace tools"))?;
            if children.as_array().unwrap().iter().any(|child| {
                !matches!(
                    child.get("type").and_then(Value::as_str),
                    Some("function" | "custom")
                )
            }) {
                return Err(unsupported("nested namespace or hosted tool"));
            }
            tools(&json!({"tools":children}), route, admitted)?;
        }
        if kind == Some("custom")
            && tool
                .get("format")
                .filter(|v| !v.is_null())
                .is_some_and(|format| {
                    format
                        .as_object()
                        .is_none_or(|fields| fields.len() != 1 || format["type"] != "text")
                })
        {
            return Err(parameter_error(
                "tools.format",
                "has no reviewed mapping for this custom tool grammar",
            ));
        }
        if tool
            .get("strict")
            .or_else(|| tool.pointer("/function/strict"))
            .is_some_and(|v| !v.is_boolean())
        {
            return Err(ProxyError::bad_request(
                "OpenCode Zen tools.strict must be a boolean",
            ));
        }
        if tool
            .get("strict")
            .or_else(|| tool.pointer("/function/strict"))
            == Some(&Value::Bool(true))
        {
            require(
                admitted.capabilities.strict_tools,
                admitted.stale,
                "tools.strict",
            )?;
        }
    }
    Ok(())
}

/// Canonical JSON-schema format, independent of the ingress nesting.
fn format(request: &Value) -> Result<Option<Value>, ProxyError> {
    let formats: Vec<_> = [
        request.get("response_format"),
        request.pointer("/text/format"),
        request.pointer("/output_config/format"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if formats.windows(2).any(|v| v[0] != v[1]) {
        return Err(ProxyError::bad_request(
            "OpenCode Zen response format controls conflict",
        ));
    }
    let value = request
        .get("response_format")
        .or_else(|| request.pointer("/text/format"))
        .or_else(|| request.pointer("/output_config/format"));
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_object() {
        return Err(ProxyError::bad_request(
            "OpenCode Zen response format must be an object",
        ));
    }
    match value.get("type").and_then(Value::as_str) {
        Some("text" | "json_object") => {
            if value.as_object().is_some_and(|v| v.len() != 1) {
                return Err(unsupported("response format field"));
            }
            Ok((value["type"] == "json_object").then(|| json!({"type":"json_object"})))
        }
        Some("json_schema") => {
            if value.get("json_schema").is_some()
                && value.as_object().is_some_and(|v| {
                    v.keys()
                        .any(|k| !["type", "json_schema"].contains(&k.as_str()))
                })
            {
                return Err(unsupported("JSON schema wrapper field"));
            }
            let schema = value.get("json_schema").unwrap_or(value);
            if !schema.get("schema").is_some_and(Value::is_object) {
                return Err(ProxyError::bad_request(
                    "OpenCode Zen JSON schema must be an object",
                ));
            }
            for (key, field) in schema.as_object().into_iter().flatten() {
                if !["name", "description", "schema", "strict", "type"].contains(&key.as_str())
                    || (key == "strict" && !field.is_boolean())
                {
                    return Err(unsupported("JSON schema format field"));
                }
            }
            let mut out = schema.clone();
            out["type"] = json!("json_schema");
            if out.get("name").is_none() {
                out["name"] = json!("response");
            }
            if out.get("name").and_then(Value::as_str).is_none_or(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            }) {
                return Err(ProxyError::bad_request(
                    "OpenCode Zen JSON schema name is invalid",
                ));
            }
            Ok(Some(out))
        }
        _ => Err(unsupported("response format type")),
    }
}

pub(super) fn translate(
    request: &Value,
    route: ProxyRoute,
    admitted: &ZenAdmission,
    fixed_codex: Option<FixedCodexModel<'_>>,
) -> Result<(Map<String, Value>, RequestAdaptations), ProxyError> {
    let object = request
        .as_object()
        .ok_or_else(|| ProxyError::bad_request("OpenCode Zen request must be an object"))?;
    const ALLOWED: &[&str] = &[
        "model",
        "messages",
        "input",
        "system",
        "instructions",
        "tools",
        "tool_choice",
        "stream",
        "stream_options",
        "max_tokens",
        "max_completion_tokens",
        "max_output_tokens",
        "n",
        "store",
        "previous_response_id",
        "metadata",
        "client_metadata",
        "user",
        "safety_identifier",
        "prompt_cache_key",
        "cache_control",
    ];
    for key in object.keys() {
        let known = ALLOWED.contains(&key.as_str())
            || KEYS.contains(&key.as_str())
            || (route == ProxyRoute::ClaudeMessages && key == "context_management");
        if !known {
            return Err(unsupported("unknown request field"));
        }
    }
    if request.get("client_metadata").is_some_and(|value| {
        !value.is_null()
            && !value
                .as_object()
                .is_some_and(|map| map.values().all(Value::is_string))
    }) {
        return Err(ProxyError::bad_request(
            "OpenCode Zen client_metadata must be an object of strings",
        ));
    }
    if let Some(options) = request.get("stream_options") {
        if !options.as_object().is_some_and(|m| {
            m.iter()
                .all(|(k, v)| k == "include_usage" && v.is_boolean())
        }) {
            return Err(unsupported("stream_options field"));
        }
    }
    for key in ["reasoning", "thinking", "output_config", "text"] {
        if let Some(value) = request.get(key) {
            let value = value.as_object().ok_or_else(|| {
                ProxyError::bad_request(format!("OpenCode Zen {key} must be an object"))
            })?;
            let allowed: &[&str] = match key {
                "reasoning" if route == ProxyRoute::CodexResponses => {
                    &["effort", "summary", "context"]
                }
                "reasoning" => &["effort", "summary"],
                "thinking" if route == ProxyRoute::ClaudeMessages => {
                    &["type", "effort", "budget_tokens", "display"]
                }
                "thinking" => &["type", "effort", "budget_tokens"],
                "output_config" => &["effort", "format"],
                _ => &["format", "verbosity"],
            };
            for field in value.keys() {
                if !allowed.contains(&field.as_str()) {
                    return Err(parameter_error(
                        key,
                        "has an unsupported nested control field",
                    ));
                }
            }
        }
    }
    for item in response_input_items(request) {
        match item["type"].as_str() {
            Some("compaction_trigger" | "compaction" | "context_compaction") => {
                return Err(unsupported("compaction"));
            }
            Some("additional_tools") if admitted.endpoint.wire() == ZenWire::Responses => {
                return Err(parameter_error(
                    "input.additional_tools",
                    "has no reviewed native mapping on this model/protocol",
                ));
            }
            Some("additional_tools" | "tool_search_output") => tools(item, route, admitted)?,
            _ => {}
        }
    }
    tools(request, route, admitted)?;
    let wire = admitted.endpoint.wire();
    let caps = &admitted.capabilities;
    let mut out = Map::new();
    for (key, supported, max) in [
        ("temperature", caps.temperature, 2.0),
        ("top_p", caps.top_p, 1.0),
        ("top_k", caps.top_k, 1_000_000.0),
    ] {
        if let Some(value) = request.get(key) {
            require(supported, admitted.stale, key)?;
            let n = value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0 && *n <= max)
                .ok_or_else(|| {
                    ProxyError::bad_request(format!(
                        "OpenCode Zen {key} is outside its supported numeric range"
                    ))
                })?;
            if key == "top_k" && (n.fract() != 0.0 || wire == ZenWire::Responses) {
                return Err(unsupported(key));
            }
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(stop) = request
        .get("stop")
        .or_else(|| request.get("stop_sequences"))
    {
        if wire == ZenWire::Responses {
            return Err(unsupported("stop sequences on Responses"));
        }
        let valid = stop
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 8192)
            || stop.as_array().is_some_and(|list| {
                list.len() <= 4
                    && list
                        .iter()
                        .all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 8192))
            });
        if !valid {
            return Err(ProxyError::bad_request(
                "OpenCode Zen stop sequences are invalid",
            ));
        }
        let value = if wire == ZenWire::Messages && stop.is_string() {
            json!([stop])
        } else {
            stop.clone()
        };
        out.insert(
            if wire == ZenWire::Messages {
                "stop_sequences"
            } else {
                "stop"
            }
            .into(),
            value,
        );
    }
    let selection = select_effort(request, route, admitted, fixed_codex)?;
    let effort = selection.effective;
    if let Some(effort) = effort {
        match (wire, effort) {
            (ZenWire::Chat, _) => {
                out.insert("reasoning_effort".into(), json!(effort));
            }
            (ZenWire::Responses, _) => {
                out.insert("reasoning".into(), json!({"effort":effort}));
            }
            (ZenWire::Messages, "none") => {
                out.insert("thinking".into(), json!({"type":"disabled"}));
            }
            (ZenWire::Messages, _) => {
                out.insert("output_config".into(), json!({"effort":effort}));
            }
        }
    }
    if let Some(summary) = request
        .pointer("/reasoning/summary")
        .filter(|value| !value.is_null())
    {
        let summary = summary
            .as_str()
            .filter(|value| matches!(*value, "auto" | "concise" | "detailed"))
            .ok_or_else(|| unsupported("reasoning.summary"))?;
        if summary != "auto" {
            if wire != ZenWire::Responses {
                return Err(unsupported("reasoning.summary on this protocol"));
            }
            require(caps.reasoning, admitted.stale, "reasoning.summary")?;
        }
        if wire == ZenWire::Responses && caps.reasoning == Some(true) && !admitted.stale {
            out.entry("reasoning").or_insert_with(|| json!({}))["summary"] = json!(summary);
        }
    }
    let mut defaulted_verbosity = false;
    if let Some(verbosity) = request
        .pointer("/text/verbosity")
        .filter(|value| !value.is_null())
    {
        match verbosity.as_str() {
            Some("medium") => {}
            Some("low" | "high") if fixed_codex.is_some() => defaulted_verbosity = true,
            _ => {
                return Err(parameter_error(
                    "text.verbosity",
                    "has no reviewed mapping for the requested value",
                ))
            }
        }
    }
    if let Some(thinking) = request.get("thinking") {
        if route == ProxyRoute::ClaudeMessages
            && thinking
                .get("display")
                .is_some_and(|value| !value.is_null())
        {
            return Err(unsupported(
                "Claude thinking.display requires native signed thinking; set CLAUDE_CODE_DISABLE_THINKING=1 and use a supported effort",
            ));
        }
        let has_native = thinking.get("type").is_some() || thinking.get("budget_tokens").is_some();
        if wire != ZenWire::Messages && has_native {
            if route == ProxyRoute::ClaudeMessages && thinking["type"] == "adaptive" {
                return Err(unsupported(
                    "Claude adaptive thinking; set CLAUDE_CODE_DISABLE_THINKING=1 and use a supported effort",
                ));
            }
            return Err(unsupported("thinking budget/toggle on this protocol"));
        }
        require(caps.reasoning, admitted.stale, "thinking")?;
        let kind = thinking.get("type").and_then(Value::as_str);
        if thinking.get("budget_tokens").is_some() && kind != Some("enabled") {
            return Err(ProxyError::bad_request(
                "OpenCode Zen thinking budget requires type enabled",
            ));
        }
        if kind == Some("disabled") && effort.is_some_and(|value| value != "none") {
            return Err(ProxyError::bad_request(
                "OpenCode Zen disabled thinking conflicts with reasoning effort",
            ));
        }
        if has_native && kind != Some("disabled") && effort == Some("none") {
            return Err(parameter_error(
                "reasoning.effort",
                "conflicts with enabled thinking",
            ));
        }
        match kind {
            Some("disabled") => {
                require(caps.reasoning_toggle, admitted.stale, "thinking toggle")?;
                out.insert("thinking".into(), json!({"type":"disabled"}));
            }
            Some("enabled") => {
                let budget = thinking.get("budget_tokens").and_then(Value::as_u64).filter(|n| *n>=1024)
                    .ok_or_else(||ProxyError::bad_request("OpenCode Zen enabled thinking requires an integer budget_tokens >= 1024"))?;
                if super::output_limit(request, Some(admitted), route)? <= budget {
                    return Err(ProxyError::bad_request(
                        "OpenCode Zen thinking budget must be below the output limit",
                    ));
                }
                out.insert(
                    "thinking".into(),
                    json!({"type":"enabled","budget_tokens":budget}),
                );
            }
            Some("adaptive") => {
                out.insert("thinking".into(), json!({"type":"adaptive"}));
            }
            None if effort.is_some() => {}
            _ => return Err(unsupported("thinking type")),
        }
    }
    if let Some(format) = format(request)? {
        require(caps.structured_output, admitted.stale, "structured output")?;
        match wire {
            ZenWire::Chat => {
                if format["type"] == "json_schema" {
                    let mut schema = format;
                    schema.as_object_mut().unwrap().remove("type");
                    out.insert(
                        "response_format".into(),
                        json!({"type":"json_schema","json_schema":schema}),
                    );
                } else {
                    out.insert("response_format".into(), format);
                }
            }
            ZenWire::Responses => {
                out.insert("text".into(), json!({"format":format}));
            }
            ZenWire::Messages => {
                if format["type"] != "json_schema" {
                    return Err(unsupported("json_object on Messages"));
                }
                if format.get("strict").is_some() || format.get("description").is_some() {
                    return Err(unsupported("JSON schema strict/description on Messages"));
                }
                let config = out.entry("output_config").or_insert_with(|| json!({}));
                config["format"] = json!({"type":"json_schema","schema":format["schema"]});
            }
        }
    }
    if let Some(include) = request.get("include") {
        if !include.as_array().is_some_and(|v| {
            v.iter()
                .all(|v| v.as_str() == Some("reasoning.encrypted_content"))
        }) {
            return Err(unsupported("include"));
        }
        if wire == ZenWire::Responses {
            out.insert("include".into(), include.clone());
        }
    }
    if let Some(choice) = request.get("tool_choice") {
        let kind = choice
            .as_str()
            .or_else(|| choice.get("type").and_then(Value::as_str))
            .ok_or_else(|| ProxyError::bad_request("OpenCode Zen tool_choice is invalid"))?;
        let canonical = match kind {
            "auto" => "auto",
            "none" => "none",
            "required" | "any" => "required",
            "function" | "tool" | "custom" => "tool",
            _ => return Err(unsupported("tool_choice type")),
        };
        if !caps.tool_choices.is_empty()
            && !caps
                .tool_choices
                .iter()
                .any(|v| v == canonical || (canonical == "required" && v == "any"))
        {
            return Err(unsupported("tool_choice is not in the model menu"));
        }
        if let Some(map) = choice.as_object() {
            if map.keys().any(|k| {
                ![
                    "type",
                    "name",
                    "function",
                    "namespace",
                    "disable_parallel_tool_use",
                ]
                .contains(&k.as_str())
            }) {
                return Err(unsupported("tool_choice field"));
            }
        }
    }
    if let Some(disabled) = request.pointer("/tool_choice/disable_parallel_tool_use") {
        let disabled = disabled.as_bool().ok_or_else(|| {
            ProxyError::bad_request("OpenCode Zen disable_parallel_tool_use must be a boolean")
        })?;
        if request
            .get("parallel_tool_calls")
            .is_some_and(|p| p.as_bool() != Some(!disabled))
        {
            return Err(ProxyError::bad_request(
                "OpenCode Zen parallel tool controls conflict",
            ));
        }
    }
    let parallel = request.get("parallel_tool_calls").cloned().or_else(|| {
        request
            .pointer("/tool_choice/disable_parallel_tool_use")
            .and_then(Value::as_bool)
            .map(|v| Value::Bool(!v))
    });
    if let Some(parallel) = parallel {
        let parallel = parallel.as_bool().ok_or_else(|| {
            ProxyError::bad_request("OpenCode Zen parallel_tool_calls must be a boolean")
        })?;
        // Applied after the shared tool-choice bridge has run.
        out.insert("parallel_tool_calls".into(), json!(parallel));
    }
    Ok((
        out,
        RequestAdaptations {
            requested_effort: selection.requested.map(str::to_owned),
            effective_effort: effort.map(str::to_owned),
            defaulted_verbosity,
            ..Default::default()
        },
    ))
}

pub(super) fn apply(
    converted: &mut Value,
    source: &Value,
    controls: Map<String, Value>,
    wire: ZenWire,
) {
    let map = converted
        .as_object_mut()
        .expect("validated converted request");
    for key in KEYS {
        map.remove(*key);
    }
    for (key, value) in controls {
        map.insert(key, value);
    }
    if wire == ZenWire::Messages {
        if let Some(parallel) = map.remove("parallel_tool_calls").and_then(|v| v.as_bool()) {
            let choice = map
                .entry("tool_choice")
                .or_insert_with(|| json!({"type":"auto"}));
            choice["disable_parallel_tool_use"] = json!(!parallel);
        }
    }
    // Shared general-purpose transforms intentionally do not all preserve
    // strictness. This driver must keep the caller's explicit contract.
    let mut normalized = source.clone();
    // Reuse the shared namespace identity mapping for strict fields too.
    if source.get("input").is_some() && wire != ZenWire::Responses {
        let _ =
            crate::proxy::transforms::normalize_grok_responses_tool_compatibility(&mut normalized);
    }
    let source_tools = normalized.get("tools").and_then(Value::as_array);
    for tool in map
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let tool_name = super::tool_name(tool).map(str::to_string);
        let explicit = source_tools
            .into_iter()
            .flatten()
            .find(|t| super::tool_name(t).map(str::to_string) == tool_name)
            .and_then(|t| t.get("strict").or_else(|| t.pointer("/function/strict")))
            .cloned();
        if let Some(strict) = explicit {
            if let Some(function) = tool.get_mut("function") {
                function["strict"] = strict;
            } else {
                tool["strict"] = strict;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::opencode_zen::{
        catalog::ZenModelCapabilities, CatalogEndpoint, PreparedUpstream,
    };

    fn prepare_for_model(
        request: &Value,
        route: ProxyRoute,
        admitted: &ZenAdmission,
    ) -> Result<PreparedUpstream, ProxyError> {
        super::super::prepare_for_model(request, route, admitted, None)
    }

    fn prepare_fixed(
        request: &Value,
        admitted: &ZenAdmission,
        requested_model: &str,
    ) -> Result<PreparedUpstream, ProxyError> {
        super::super::prepare_for_model(
            request,
            ProxyRoute::CodexResponses,
            admitted,
            Some(FixedCodexModel {
                requested_model: Some(requested_model),
            }),
        )
    }

    fn admission(endpoint: CatalogEndpoint) -> ZenAdmission {
        ZenAdmission {
            endpoint,
            stale: false,
            context_window: Some(200_000),
            max_output_tokens: Some(16_000),
            capabilities: ZenModelCapabilities {
                tools: Some(true),
                strict_tools: Some(true),
                tool_choices: vec!["auto".into(), "required".into(), "tool".into()],
                reasoning: Some(true),
                reasoning_toggle: Some(true),
                reasoning_efforts: vec!["low".into(), "high".into()],
                chat_reasoning_content: endpoint.wire() == ZenWire::Chat,
                structured_output: Some(true),
                temperature: Some(true),
                top_p: Some(true),
                top_k: Some(true),
                input_modalities: vec!["text".into()],
            },
        }
    }
    fn source(route: ProxyRoute) -> Value {
        let mut request = json!({"model":"test-free(high)","stream":true});
        match route {
            ProxyRoute::ClaudeMessages => {
                request["messages"] = json!([{"role":"user","content":"hello"}]);
                request["tools"] =
                    json!([{"name":"lookup","input_schema":{"type":"object"},"strict":true}]);
                request["output_config"] = json!({"effort":"high","format":{"type":"json_schema","schema":{"type":"object"}}});
            }
            ProxyRoute::CodexResponses => {
                request["input"] = json!("hello");
                request["tools"] = json!([{"type":"function","name":"lookup","parameters":{"type":"object"},"strict":true}]);
                request["reasoning"] = json!({"effort":"high"});
                request["text"] = json!({"verbosity":"medium","format":{"type":"json_schema","name":"answer","schema":{"type":"object"}}});
            }
            _ => {
                request["messages"] = json!([{"role":"user","content":"hello"}]);
                request["tools"] = json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object"},"strict":true}}]);
                request["reasoning_effort"] = json!("high");
                request["response_format"] = json!({"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object"}}});
            }
        }
        request["temperature"] = json!(0.4);
        request["top_p"] = json!(0.9);
        request["parallel_tool_calls"] = json!(false);
        request
    }

    #[test]
    fn opencode_zen_fixed_codex_effort_uses_the_current_target_menu() {
        let mut model = admission(CatalogEndpoint::Chat);
        let mut request = json!({"model":"hy4-flash-free","input":"hello"});
        for (menu, incoming, effective) in [
            (&["high"][..], "low", "high"),
            (&["high"][..], "medium", "high"),
            (&["max", "high", "low"][..], "medium", "low"),
            (&["max", "high", "low"][..], "xhigh", "high"),
            (&["max", "high", "low"][..], "low", "low"),
        ] {
            model.capabilities.reasoning_efforts = menu.iter().map(|rung| (*rung).into()).collect();
            request["reasoning"] = json!({"effort":incoming});
            let prepared = prepare_fixed(&request, &model, "gpt-6.2-sol").unwrap();
            let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert_eq!(sent["reasoning_effort"], effective);
            assert_eq!(
                prepared.adaptations.requested_effort.as_deref(),
                Some(incoming)
            );
            assert_eq!(
                prepared.adaptations.effective_effort.as_deref(),
                Some(effective)
            );
        }

        request["model"] = json!("hy4-flash-free(high)");
        let prepared = prepare_fixed(&request, &model, "gpt-6.2-astra(low)").unwrap();
        assert_eq!(
            prepared.adaptations.requested_effort.as_deref(),
            Some("low")
        );
        assert_eq!(
            prepared.adaptations.effective_effort.as_deref(),
            Some("high")
        );
        let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(sent["model"], "hy4-flash-free");
        assert_eq!(sent["reasoning_effort"], "high");
        model.capabilities.reasoning_efforts = vec!["low".into()];
        let error = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap_err();
        assert!(error.message.contains("fixed upstream effort suffix"));

        request["model"] = json!("hy4-flash-free");
        request.as_object_mut().unwrap().remove("reasoning");
        model.stale = true;
        let prepared = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap();
        let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(sent.get("reasoning_effort").is_none());
        assert!(prepared.adaptations.requested_effort.is_none());
        assert!(prepared.adaptations.effective_effort.is_none());
    }

    #[test]
    fn opencode_zen_fixed_codex_validates_effort_before_applying_the_operator_pin() {
        let model = admission(CatalogEndpoint::Chat);
        for (client_model, fields) in [
            ("gpt-6.2-sol", json!({"reasoning":{"effort":null}})),
            ("gpt-6.2-sol", json!({"reasoning":{"effort":5}})),
            ("gpt-6.2-sol", json!({"reasoning":{"effort":"future"}})),
            (
                "gpt-6.2-sol",
                json!({"reasoning":{"effort":"medium"},"reasoning_effort":"low"}),
            ),
            ("gpt-6.2-sol(high)", json!({"reasoning":{"effort":"low"}})),
            ("gpt-6.2-sol", json!({"reasoning":{"effort":"none"}})),
        ] {
            let mut request = json!({"model":"hy4-flash-free(high)","input":"hello"});
            request
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let error = prepare_fixed(&request, &model, client_model).unwrap_err();
            assert_eq!(error.error_param(), Some("reasoning.effort"));
            assert!(!error.retryable());
        }
        let request =
            json!({"model":"hy4-flash-free","input":"hello","reasoning":{"effort":"none"}});
        let error = prepare_fixed(&request, &model, "gpt-6.2-sol").unwrap_err();
        assert!(error
            .message
            .contains("no reviewed reasoning-disable mapping"));

        let mut native = admission(CatalogEndpoint::Messages);
        let prepared = prepare_fixed(&request, &native, "gpt-6.2-sol").unwrap();
        let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(sent["thinking"], json!({"type":"disabled"}));
        assert_eq!(
            prepared.adaptations.effective_effort.as_deref(),
            Some("none")
        );
        native.capabilities.reasoning_toggle = None;
        assert_eq!(
            prepare_fixed(&request, &native, "gpt-6.2-sol")
                .unwrap_err()
                .error_param(),
            Some("reasoning.effort")
        );
    }

    #[test]
    fn opencode_zen_fixed_codex_verbosity_keeps_response_format_validation() {
        let mut model = admission(CatalogEndpoint::Chat);
        let mut request = json!({"model":"hy4-flash-free","input":"hello","text":{
            "verbosity":"low","format":{"type":"json_schema","name":"result","schema":{"type":"object"}}
        }});
        for verbosity in ["low", "medium", "high"] {
            request["text"]["verbosity"] = json!(verbosity);
            let prepared = prepare_fixed(&request, &model, "gpt-6.2-sol").unwrap();
            let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
            assert_eq!(
                sent["response_format"]["json_schema"]["schema"],
                json!({"type":"object"})
            );
            assert!(sent.get("text").is_none());
            assert_eq!(
                prepared.adaptations.defaulted_verbosity,
                verbosity != "medium"
            );
        }
        request["text"]["verbosity"] = json!(true);
        assert_eq!(
            prepare_fixed(&request, &model, "gpt-6.2-sol")
                .unwrap_err()
                .error_param(),
            Some("text.verbosity")
        );
        request["text"]["verbosity"] = json!("high");
        model.capabilities.structured_output = None;
        assert!(prepare_fixed(&request, &model, "gpt-6.2-sol")
            .unwrap_err()
            .message
            .contains("structured output"));
    }

    #[test]
    fn opencode_zen_fixed_codex_validates_scoped_history_before_omitting_plain_items() {
        let mut model = admission(CatalogEndpoint::Chat);
        let thought = |text| {
            json!({"type":"reasoning","content":null,"encrypted_content":null,
                "summary":[{"type":"summary_text","text":text}]
            })
        };
        let mut request = json!({"model":"fixed-free","reasoning":{"context":"all_turns"},"input":[
            {"type":"message","role":"user","content":"question one"},
            thought("older reasoning"),
            {"type":"message","role":"assistant","content":"answer one"},
            {"type":"message","role":"user","content":"question two"},
            thought("\ncurrent reasoning \t"),
            {"type":"message","role":"assistant","content":"answer two"}
        ]});
        for (scope, earlier) in [("all_turns", true), ("auto", true), ("current_turn", false)] {
            request["reasoning"]["context"] = json!(scope);
            let prepared = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap();
            let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
            let messages = sent["messages"].as_array().unwrap();
            let replies: Vec<_> = messages
                .iter()
                .filter(|message| message["role"] == "assistant")
                .collect();
            assert_eq!(
                replies[0]["reasoning_content"].as_str(),
                earlier.then_some("older reasoning")
            );
            assert_eq!(replies[1]["reasoning_content"], "current reasoning");
        }
        model.capabilities.chat_reasoning_content = false;
        request["reasoning"]["context"] = json!("all_turns");
        let prepared = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap();
        let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
        let messages = sent["messages"].as_array().unwrap();
        assert!(messages
            .iter()
            .all(|message| message.get("reasoning_content").is_none()));
        assert!(messages
            .iter()
            .any(|message| message["content"] == "answer one"));
        assert!(messages
            .iter()
            .any(|message| message["content"] == "answer two"));
        assert_eq!(prepared.adaptations.omitted_plain_reasoning, 2);

        request.as_object_mut().unwrap().remove("reasoning");
        request["input"][4]["encrypted_content"] = json!("opaque");
        let error = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap_err();
        assert_eq!(error.error_param(), Some("reasoning.context"));
        for kind in ["item_reference", "compaction", "compaction_trigger"] {
            request["input"][4] = json!({"type":kind,"id":"remote-item"});
            assert_eq!(
                prepare_fixed(&request, &model, "gpt-6.2-astra")
                    .unwrap_err()
                    .error_param(),
                Some("reasoning.context")
            );
        }
        request["input"][4] = thought("current reasoning");
        model.stale = true;
        model.capabilities.chat_reasoning_content = true;
        let error = prepare_fixed(&request, &model, "gpt-6.2-astra").unwrap_err();
        assert_eq!(error.status, 503);
        assert!(error.retryable());
    }

    #[test]
    fn opencode_zen_codex_lite_validates_controls_and_preserves_the_requested_history() {
        let fixture: Value = serde_json::from_str(super::super::CODEX_LITE_FIXTURE).unwrap();
        let mut request = fixture["request"].clone();
        request["model"] = json!("resolved-free");
        let route = ProxyRoute::CodexResponses;
        let mut model = admission(CatalogEndpoint::Chat);
        model.capabilities.reasoning_efforts = vec!["high".into()];
        model.capabilities.chat_reasoning_content = false;
        let prepared = prepare_for_model(&request, route, &model).unwrap();
        assert!(prepared.codex_exec_tool.is_some());
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(wire["reasoning_effort"], "high");
        assert!(wire.get("reasoning").is_none());
        assert!(wire.get("client_metadata").is_none());
        let tool = wire["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"].as_str() == prepared.codex_exec_tool.as_deref())
            .unwrap();
        assert_eq!(
            tool["function"]["parameters"]["properties"]["input"]["minLength"],
            1
        );
        assert_eq!(
            tool["function"]["parameters"]["additionalProperties"],
            false
        );
        for (field, parameter) in [
            ("reasoning", "reasoning.effort"),
            ("text", "text.verbosity"),
        ] {
            let mut default = request.clone();
            default[field] = fixture["defaultControls"][field].clone();
            let error = prepare_for_model(&default, route, &model).unwrap_err();
            assert_eq!(error.error_param(), Some(parameter));
        }
        let mut malformed = request.clone();
        malformed["reasoning"]["effort"] = json!(5);
        assert_eq!(
            prepare_for_model(&malformed, route, &model)
                .unwrap_err()
                .error_param(),
            Some("reasoning.effort")
        );
        request["reasoning"]["context"] = json!("future-scope");
        assert_eq!(
            prepare_for_model(&request, route, &model)
                .unwrap_err()
                .error_param(),
            Some("reasoning.context")
        );
        request["reasoning"]["context"] = json!("all_turns");
        let previous = json!({"type":"reasoning","id":"old","summary":[{"type":"summary_text","text":"preserve me"}],
            "content":null,"encrypted_content":null});
        request["input"].as_array_mut().unwrap().extend([
            previous.clone(),
            json!({"type":"message","role":"assistant","content":"prior reply"}),
            json!({"type":"message","role":"user","content":"next turn"}),
        ]);
        assert_eq!(
            prepare_for_model(&request, route, &model)
                .unwrap_err()
                .error_param(),
            Some("reasoning.context")
        );
        model.capabilities.chat_reasoning_content = true;
        let wire: Value =
            serde_json::from_slice(&prepare_for_model(&request, route, &model).unwrap().body)
                .unwrap();
        assert!(wire["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["reasoning_content"] == "preserve me"));
        request["input"][2]["encrypted_content"] = json!("unavailable-ciphertext");
        assert_eq!(
            prepare_for_model(&request, route, &model)
                .unwrap_err()
                .error_param(),
            Some("reasoning.context")
        );
        request["reasoning"]["context"] = json!("current_turn");
        let wire: Value =
            serde_json::from_slice(&prepare_for_model(&request, route, &model).unwrap().body)
                .unwrap();
        assert!(!wire.to_string().contains("preserve me"));
        request["input"].as_array_mut().unwrap().push(previous);
        assert_eq!(
            prepare_for_model(&request, route, &model)
                .unwrap_err()
                .error_param(),
            Some("reasoning.context")
        );
    }

    #[test]
    fn opencode_zen_codex_lite_admits_only_reviewed_exec_and_matching_history() {
        let fixture: Value = serde_json::from_str(super::super::CODEX_LITE_FIXTURE).unwrap();
        let request = fixture["request"].clone();
        let route = ProxyRoute::CodexResponses;
        let model = admission(CatalogEndpoint::Chat);
        let mut invalid = request.clone();
        invalid["input"][0]["tools"][0]["tools"][0]["format"]["definition"] =
            json!("start: /other/");
        assert_eq!(
            prepare_for_model(&invalid, route, &model)
                .unwrap_err()
                .error_param(),
            Some("tools.format")
        );
        invalid["input"] = invalid["input"][0].clone();
        assert_eq!(
            prepare_for_model(&invalid, route, &model)
                .unwrap_err()
                .error_param(),
            Some("tools.format")
        );
        let error =
            prepare_for_model(&request, route, &admission(CatalogEndpoint::Messages)).unwrap_err();
        assert_eq!(error.error_param(), Some("tools.format"));
        let mut ambiguous = request.clone();
        ambiguous["tools"] = json!([{
            "type":"namespace","name":"functions",
            "tools":[{"type":"custom","name":"exec","format":{"type":"text"}}]
        }]);
        assert_eq!(
            prepare_for_model(&ambiguous, route, &model)
                .unwrap_err()
                .error_param(),
            Some("input.additional_tools")
        );
        for items in [
            vec![fixture["toolResult"].clone(), fixture["toolCall"].clone()],
            vec![fixture["toolResult"].clone()],
            vec![fixture["toolCall"].clone()],
        ] {
            let mut follow_up = request.clone();
            follow_up["input"].as_array_mut().unwrap().extend(items);
            assert_eq!(
                prepare_for_model(&follow_up, route, &model)
                    .unwrap_err()
                    .error_param(),
                Some("input.custom_tool_call_output")
            );
        }
    }

    #[test]
    fn zen_claude_cli_reports_model_controls_and_retains_only_representable_thinking() {
        let fixture: Value = serde_json::from_str(super::super::CLAUDE_CLI_FIXTURE).unwrap();
        let mut admitted = admission(CatalogEndpoint::Chat);
        admitted.max_output_tokens = Some(32_000);
        let mut request = fixture["request"].clone();
        request["model"] = json!("resolved-free");
        request
            .as_object_mut()
            .unwrap()
            .extend(fixture["defaultControls"].as_object().unwrap().clone());
        let prepare = |request: &Value, admitted: &ZenAdmission| {
            prepare_for_model(request, ProxyRoute::ClaudeMessages, admitted)
        };
        let error = prepare(&request, &admitted).unwrap_err();
        assert!(error
            .message
            .contains("resolved-free (requested 128000, maximum 32000)"));
        request["max_tokens"] = 32000.into();
        let error = prepare(&request, &admitted).unwrap_err();
        assert!(error.message.contains("available: low, high"));
        request["output_config"]["effort"] = json!("high");
        assert!(prepare(&request, &admitted)
            .unwrap_err()
            .message
            .contains("thinking.display"));
        request["thinking"]
            .as_object_mut()
            .unwrap()
            .remove("display");
        assert!(prepare(&request, &admitted)
            .unwrap_err()
            .message
            .contains("CLAUDE_CODE_DISABLE_THINKING=1"));
        request.as_object_mut().unwrap().remove("thinking");
        request["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":[{"type":"thinking","thinking":"synthetic reasoning","signature":""},{"type":"text","text":"a previous answer"}]}),
            json!({"role":"user","content":"next"}),
        ]);
        let prepared = prepare(&request, &admitted).unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(wire.get("context_management").is_none());
        let history = wire["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "assistant")
            .unwrap();
        assert_eq!(history["reasoning_content"], "synthetic reasoning");
        request["messages"][1]["content"][0]["signature"] = json!("opaque-native-signature");
        assert!(prepare(&request, &admitted)
            .unwrap_err()
            .message
            .contains("cannot retain"));
        admitted = admission(CatalogEndpoint::Messages);
        admitted.max_output_tokens = Some(32_000);
        let prepared = prepare(&request, &admitted).unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(
            wire["messages"][1]["content"][0],
            request["messages"][1]["content"][0]
        );
        request["context_management"]["edits"][0]["keep"] = json!(1);
        assert!(prepare(&request, &admitted)
            .unwrap_err()
            .message
            .contains("keep=all"));
    }

    #[test]
    fn zen_claude_cli_rejects_tool_references_outside_the_client_search_result() {
        let fixture: Value = serde_json::from_str(super::super::CLAUDE_CLI_FIXTURE).unwrap();
        let mut request = fixture["request"].clone();
        request["max_tokens"] = 8000.into();
        request["messages"].as_array_mut().unwrap().extend([
            fixture["searchTurn"].clone(),
            fixture["searchResult"].clone(),
        ]);
        let admitted = admission(CatalogEndpoint::Chat);
        let prepare =
            |request: &Value| prepare_for_model(request, ProxyRoute::ClaudeMessages, &admitted);
        assert!(prepare(&request)
            .unwrap_err()
            .message
            .contains("no current client tool schema"));
        request["tools"]
            .as_array_mut()
            .unwrap()
            .push(fixture["recentTool"].clone());
        request["messages"][1]["content"][0]["name"] = json!("AnotherTool");
        assert!(prepare(&request)
            .unwrap_err()
            .message
            .contains("preceding client ToolSearch call"));
        request["messages"][1]["content"][0]["name"] = json!("ToolSearch");
        request["tools"][2]["defer_loading"] = json!("true");
        assert!(prepare(&request)
            .unwrap_err()
            .message
            .contains("boolean defer_loading"));
        request["tools"][2]["defer_loading"] = json!(true);
        request["messages"].as_array_mut().unwrap().truncate(1);
        request["tool_choice"] = json!({"type":"tool","name":"EchoProbe"});
        assert!(prepare(&request)
            .unwrap_err()
            .message
            .contains("cannot force an unavailable deferred tool"));
    }

    #[test]
    fn zen_claude_keeps_tool_failure_in_chat_and_native_results() {
        let request = json!({
            "model":"chat-free","max_tokens":8000,
            "messages":[
                {"role":"user","content":"Use the tool"},
                {"role":"assistant","content":[{"type":"tool_use","id":"synthetic-fail","name":"echo","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"synthetic-fail","is_error":true,"content":"synthetic failure"}]}
            ],
            "tools":[{"name":"echo","input_schema":{"type":"object"}}]
        });
        let chat = prepare_for_model(
            &request,
            ProxyRoute::ClaudeMessages,
            &admission(CatalogEndpoint::Chat),
        )
        .unwrap();
        let wire: Value = serde_json::from_slice(&chat.body).unwrap();
        let tool = wire["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["role"] == "tool")
            .unwrap();
        assert_eq!(tool["tool_call_id"], "synthetic-fail");
        assert_eq!(
            tool["content"],
            "[Tool execution failed]\nsynthetic failure"
        );
        let native = prepare_for_model(
            &request,
            ProxyRoute::ClaudeMessages,
            &admission(CatalogEndpoint::Messages),
        )
        .unwrap();
        let wire: Value = serde_json::from_slice(&native.body).unwrap();
        assert_eq!(
            wire["messages"][2]["content"][0],
            request["messages"][2]["content"][0]
        );
    }

    #[test]
    fn zen_codex_cli_envelope_defaults_survive_all_wires_and_modes_without_metadata_leaks() {
        for endpoint in [
            CatalogEndpoint::Chat,
            CatalogEndpoint::Responses,
            CatalogEndpoint::Messages,
        ] {
            for stale in [false, true] {
                for stream in [false, true] {
                    let mut request: Value =
                        serde_json::from_str(super::super::CODEX_CLI_FIXTURE).unwrap();
                    request["text"] = json!({"verbosity":"medium"});
                    request["stream"] = json!(stream);
                    let mut admitted = admission(endpoint);
                    admitted.stale = stale;
                    let prepared =
                        prepare_for_model(&request, ProxyRoute::CodexResponses, &admitted).unwrap();
                    assert_eq!(prepared.stream, stream);
                    let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
                    assert_eq!(sent["stream"], true);
                    assert!(sent.get("client_metadata").is_none());
                    assert!(sent.get("prompt_cache_key").is_none());
                    assert!(!sent.to_string().contains("synthetic-client-"));
                    assert!(!sent.to_string().contains("private-workspace"));
                    let tool = sent["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|tool| super::super::tool_name(tool) == Some("echo_probe"))
                        .unwrap();
                    let schema = tool
                        .pointer("/function/parameters")
                        .or_else(|| tool.get("parameters"))
                        .or_else(|| tool.get("input_schema"))
                        .unwrap();
                    assert_eq!(
                        schema["properties"]["client_metadata"]["properties"]["mode"]["type"],
                        "string"
                    );
                    if endpoint.wire() == ZenWire::Responses {
                        assert_eq!(sent["include"], request["include"]);
                        if !stale {
                            assert_eq!(sent["reasoning"]["summary"], "auto");
                        } else {
                            assert!(sent.get("reasoning").is_none());
                        }
                    } else {
                        assert!(sent.get("include").is_none());
                        assert!(sent.get("reasoning").is_none());
                    }
                    assert!(sent.get("text").is_none());
                }
            }
        }
    }

    #[test]
    fn zen_codex_cli_envelope_does_not_weaken_explicit_generation_controls() {
        let fixture: Value = serde_json::from_str(super::super::CODEX_CLI_FIXTURE).unwrap();
        for endpoint in [
            CatalogEndpoint::Chat,
            CatalogEndpoint::Responses,
            CatalogEndpoint::Messages,
        ] {
            let mut request = fixture.clone();
            request["reasoning"]["effort"] = json!("high");
            let mut admitted = admission(endpoint);
            let prepared =
                prepare_for_model(&request, ProxyRoute::CodexResponses, &admitted).unwrap();
            let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
            let effort = match endpoint.wire() {
                ZenWire::Chat => &sent["reasoning_effort"],
                ZenWire::Responses => &sent["reasoning"]["effort"],
                ZenWire::Messages => &sent["output_config"]["effort"],
            };
            assert_eq!(effort, "high");
            admitted.capabilities.reasoning = Some(false);
            assert!(prepare_for_model(&request, ProxyRoute::CodexResponses, &admitted).is_err());
            for changes in [
                json!({"client_metadata":[]}),
                json!({"client_metadata":{"flag":true}}),
                json!({"reasoning":{"summary":true}}),
                json!({"reasoning":{"summary":"unexpected"}}),
                json!({"reasoning":{"private-unknown-field":true}}),
                json!({"text":{"verbosity":"high"}}),
                json!({"text":{"verbosity":"low"}}),
                json!({"text":{"verbosity":false}}),
                json!({"include":["unreviewed.projection"]}),
                json!({"private-sensitive-unknown-value":true}),
                json!({"tools":[{"type":"web_search"}]}),
            ] {
                let mut invalid = fixture.clone();
                invalid
                    .as_object_mut()
                    .unwrap()
                    .extend(changes.as_object().unwrap().clone());
                let error =
                    prepare_for_model(&invalid, ProxyRoute::CodexResponses, &admitted).unwrap_err();
                assert_eq!(error.status, 400);
                assert!(!error.message.contains("private-unknown"));
                assert!(!error.message.contains("private-sensitive"));
            }
        }
    }

    #[test]
    fn zen_codex_cli_summary_styles_require_native_fresh_reasoning_support() {
        for summary in ["concise", "detailed"] {
            for endpoint in [
                CatalogEndpoint::Chat,
                CatalogEndpoint::Responses,
                CatalogEndpoint::Messages,
            ] {
                let mut request: Value =
                    serde_json::from_str(super::super::CODEX_CLI_FIXTURE).unwrap();
                request["reasoning"]["summary"] = json!(summary);
                let mut admitted = admission(endpoint);
                let result = prepare_for_model(&request, ProxyRoute::CodexResponses, &admitted);
                if endpoint.wire() == ZenWire::Responses {
                    let sent: Value = serde_json::from_slice(&result.unwrap().body).unwrap();
                    assert_eq!(sent["reasoning"]["summary"], summary);
                    admitted.stale = true;
                    assert!(
                        prepare_for_model(&request, ProxyRoute::CodexResponses, &admitted).is_err()
                    );
                } else {
                    assert!(result.is_err());
                }
            }
        }
    }

    #[test]
    fn zen_controls_survive_all_nine_protocol_bridges_without_prompt_emulation() {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
        ] {
            for endpoint in [
                CatalogEndpoint::Chat,
                CatalogEndpoint::Responses,
                CatalogEndpoint::Messages,
            ] {
                let request = source(route);
                let prepared = prepare_for_model(&request, route, &admission(endpoint)).unwrap();
                let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
                assert_eq!(sent["model"], "test-free");
                assert_eq!(sent["temperature"], 0.4);
                assert_eq!(sent["top_p"], 0.9);
                assert!(!sent.to_string().contains("You must respond"));
                let tool = sent["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| super::super::tool_name(t) == Some("lookup"))
                    .unwrap();
                match endpoint.wire() {
                    ZenWire::Chat => {
                        assert_eq!(sent["reasoning_effort"], "high");
                        assert_eq!(sent["response_format"]["type"], "json_schema");
                        assert_eq!(sent["parallel_tool_calls"], false);
                        assert_eq!(tool["function"]["strict"], true);
                        assert_eq!(sent["max_completion_tokens"], 16_000);
                    }
                    ZenWire::Responses => {
                        assert_eq!(sent["reasoning"]["effort"], "high");
                        assert_eq!(sent["text"]["format"]["type"], "json_schema");
                        assert_eq!(sent["parallel_tool_calls"], false);
                        assert_eq!(tool["strict"], true);
                    }
                    ZenWire::Messages => {
                        assert_eq!(sent["output_config"]["effort"], "high");
                        assert_eq!(sent["output_config"]["format"]["schema"]["type"], "object");
                        assert_eq!(sent["tool_choice"]["disable_parallel_tool_use"], true);
                        assert_eq!(tool["strict"], true);
                    }
                }
            }
        }
    }

    #[test]
    fn zen_controls_fail_closed_for_stale_unknown_conflicting_and_unrepresentable_fields() {
        let mut admitted = admission(CatalogEndpoint::Chat);
        let request = source(ProxyRoute::CodexChatCompletions);
        admitted.stale = true;
        assert!(prepare_for_model(&request, ProxyRoute::CodexChatCompletions, &admitted).is_err());
        admitted.stale = false;
        for changes in [
            json!({"reasoning_effort":"low"}),
            json!({"max_tokens":10,"max_output_tokens":20}),
            json!({"stop":"a","stop_sequences":["b"]}),
            json!({"stream_options":{"include_usage":true,"private_unknown":true}}),
            json!({"tool_choice":{"type":"auto","disable_parallel_tool_use":false}}),
            json!({"max_tokens":32_000}),
            json!({"private-sensitive-unknown-value":true}),
        ] {
            let mut changed = request.clone();
            changed
                .as_object_mut()
                .unwrap()
                .extend(changes.as_object().unwrap().clone());
            let error = prepare_for_model(&changed, ProxyRoute::CodexChatCompletions, &admitted)
                .unwrap_err();
            assert_eq!(error.status, 400);
            assert!(!error.message.contains("private-sensitive"));
        }
        admitted.endpoint = CatalogEndpoint::Messages;
        let mut changed = request;
        changed["response_format"]["json_schema"]["strict"] = json!(true);
        assert!(prepare_for_model(&changed, ProxyRoute::CodexChatCompletions, &admitted).is_err());
        admitted.endpoint = CatalogEndpoint::Responses;
        let mut changed = source(ProxyRoute::CodexChatCompletions);
        changed["stop"] = json!("end");
        assert!(prepare_for_model(&changed, ProxyRoute::CodexChatCompletions, &admitted).is_err());
    }

    #[test]
    fn zen_controls_map_stop_and_native_thinking_without_budget_guessing() {
        let mut admitted = admission(CatalogEndpoint::Messages);
        let request = json!({"model":"test-free","messages":[{"role":"user","content":"hi"}],"stop":"END","thinking":{"type":"enabled","budget_tokens":2048},"max_tokens":4096});
        let sent: Value = serde_json::from_slice(
            &prepare_for_model(&request, ProxyRoute::CodexChatCompletions, &admitted)
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(sent["stop_sequences"], json!(["END"]));
        assert_eq!(sent["thinking"]["budget_tokens"], 2048);
        admitted.endpoint = CatalogEndpoint::Chat;
        assert!(prepare_for_model(&request, ProxyRoute::CodexChatCompletions, &admitted).is_err());
    }
}
