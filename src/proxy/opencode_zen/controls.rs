//! Model/protocol-aware controls. Unrepresentable semantics fail before send;
//! neither the generic bridges nor fingerprint shaping may silently weaken them.
use super::{catalog::ZenAdmission, ZenWire};
use crate::proxy::{ProxyError, ProxyRoute};
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

pub(in crate::proxy) fn requested_effort(request: &Value) -> Option<&str> {
    request
        .get("reasoning_effort")
        .or_else(|| request.pointer("/reasoning/effort"))
        .or_else(|| request.pointer("/output_config/effort"))
        .or_else(|| request.pointer("/thinking/effort"))
        .and_then(Value::as_str)
        .or_else(|| {
            let model = request.get("model")?.as_str()?.trim();
            model
                .strip_suffix(')')?
                .rsplit_once('(')
                .map(|(_, suffix)| suffix.trim())
        })
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
        if kind == Some("custom") && tool.pointer("/format/type").is_some_and(|v| v != "text") {
            return Err(unsupported("custom tool grammar"));
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
) -> Result<Map<String, Value>, ProxyError> {
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
        if !ALLOWED.contains(&key.as_str()) && !KEYS.contains(&key.as_str()) {
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
                "reasoning" => &["effort", "summary"],
                "thinking" => &["type", "effort", "budget_tokens"],
                "output_config" => &["effort", "format"],
                _ => &["format", "verbosity"],
            };
            for field in value.keys() {
                if !allowed.contains(&field.as_str()) {
                    return Err(unsupported("nested control field"));
                }
            }
        }
    }
    if let Some(items) = request.get("input").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("compaction_trigger" | "compaction") => return Err(unsupported("compaction")),
                Some("additional_tools") if admitted.endpoint.wire() == ZenWire::Responses => {
                    return Err(unsupported("native additional_tools"))
                }
                _ => {}
            }
        }
    }
    if let Some(items) = request.get("input").and_then(Value::as_array) {
        for item in items.iter().filter(|i| {
            matches!(
                i["type"].as_str(),
                Some("additional_tools" | "tool_search_output")
            )
        }) {
            tools(item, route, admitted)?;
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
    let effort = requested_effort(request);
    let suffix = request
        .get("model")
        .and_then(Value::as_str)
        .and_then(|m| m.trim().strip_suffix(')'))
        .and_then(|m| m.rsplit_once('('))
        .map(|(_, e)| e.trim());
    if suffix.is_some() && suffix != effort {
        return Err(ProxyError::bad_request(
            "OpenCode Zen model suffix and reasoning effort conflict",
        ));
    }
    for explicit in [
        request.get("reasoning_effort"),
        request.pointer("/reasoning/effort"),
        request.pointer("/output_config/effort"),
        request.pointer("/thinking/effort"),
    ]
    .into_iter()
    .flatten()
    {
        if explicit.as_str().is_none() || explicit.as_str() != effort {
            return Err(ProxyError::bad_request(
                "OpenCode Zen reasoning effort values must be matching strings",
            ));
        }
    }
    if let Some(effort) = effort {
        require(caps.reasoning, admitted.stale, "reasoning effort")?;
        if !caps.reasoning_efforts.iter().any(|e| e == effort) {
            return Err(unsupported("reasoning effort is not in the model menu"));
        }
        match wire {
            ZenWire::Chat => {
                out.insert("reasoning_effort".into(), json!(effort));
            }
            ZenWire::Responses => {
                out.insert("reasoning".into(), json!({"effort":effort}));
            }
            ZenWire::Messages => {
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
    if let Some(verbosity) = request
        .pointer("/text/verbosity")
        .filter(|value| !value.is_null())
    {
        if verbosity.as_str() != Some("medium") {
            return Err(unsupported("text.verbosity"));
        }
    }
    if let Some(thinking) = request.get("thinking") {
        let has_native = thinking.get("type").is_some() || thinking.get("budget_tokens").is_some();
        if wire != ZenWire::Messages && has_native {
            return Err(unsupported("thinking budget/toggle on this protocol"));
        }
        require(caps.reasoning, admitted.stale, "thinking")?;
        let kind = thinking.get("type").and_then(Value::as_str);
        if thinking.get("budget_tokens").is_some() && kind != Some("enabled") {
            return Err(ProxyError::bad_request(
                "OpenCode Zen thinking budget requires type enabled",
            ));
        }
        if kind == Some("disabled") && effort.is_some() {
            return Err(ProxyError::bad_request(
                "OpenCode Zen disabled thinking conflicts with reasoning effort",
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
                if super::output_limit(request, Some(admitted))? <= budget {
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
    Ok(out)
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
        catalog::ZenModelCapabilities, prepare_for_model, CatalogEndpoint,
    };

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
