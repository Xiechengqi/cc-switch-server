//! Buddy always uses the exact bound catalog's default reasoning level.
//! Client controls are validated for shape, then removed before translation.
//! This never scans messages, tool arguments, schemas or reasoning history.

use serde_json::{Map, Value};

use super::CodeBuddyModelCapability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuddyReasoningDefaultSource {
    DeclaredDefault,
    Legacy,
    SoleAllowed,
}

impl BuddyReasoningDefaultSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeclaredDefault => "defaultEffort",
            Self::Legacy => "effort",
            Self::SoleAllowed => "soleEffort",
        }
    }
}

fn object<'a>(
    value: Option<&'a Value>,
    path: &str,
) -> Result<Option<&'a Map<String, Value>>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value)),
        Some(_) => Err(format!("CodeBuddy {path} must be an object")),
    }
}

fn string<'a>(value: Option<&'a Value>, path: &str) -> Result<Option<&'a str>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= 32 && !value.chars().any(char::is_control) => {
            Ok(Some(value.trim()).filter(|value| !value.is_empty()))
        }
        Some(_) => Err(format!("CodeBuddy {path} must be a short string")),
    }
}

fn boolean(value: Option<&Value>, path: &str) -> Result<Option<bool>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("CodeBuddy {path} must be boolean")),
    }
}

fn budget(value: Option<&Value>, path: &str, allow_auto: bool) -> Result<(), String> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(value) if value.as_u64().is_some() || (allow_auto && value.as_i64() == Some(-1)) => {
            Ok(())
        }
        Some(_) => Err(format!("CodeBuddy {path} must be an integer budget")),
    }
}

fn thinking_config(value: Option<&Value>, path: &str) -> Result<Option<String>, String> {
    let Some(config) = object(value, path)? else {
        return Ok(None);
    };
    let level = string(
        config.get("thinkingLevel"),
        &format!("{path}.thinkingLevel"),
    )?
    .or(string(
        config.get("thinking_level"),
        &format!("{path}.thinking_level"),
    )?);
    for key in ["thinkingBudget", "thinking_budget"] {
        budget(config.get(key), &format!("{path}.{key}"), true)?;
    }
    for key in ["includeThoughts", "include_thoughts"] {
        boolean(config.get(key), &format!("{path}.{key}"))?;
    }
    Ok(level.map(str::to_string).or_else(|| {
        ["thinkingBudget", "thinking_budget"]
            .into_iter()
            .find_map(|key| config.get(key).and_then(Value::as_i64))
            .map(|value| if value == 0 { "none" } else { "unspecified" }.to_string())
    }))
}

/// The returned label is diagnostic only. Conflicting valid controls do not
/// constrain selection; the server policy overrides them all. Unknown string
/// labels are accepted but never copied into logs or sent to the upstream.
pub(crate) fn requested_buddy_reasoning_effort(input: &Value) -> Result<Option<String>, String> {
    let root = input
        .as_object()
        .ok_or("CodeBuddy request must be an object")?;
    let reasoning = object(root.get("reasoning"), "reasoning")?;
    let nested = string(reasoning.and_then(|v| v.get("effort")), "reasoning.effort")?;
    for key in ["budget_tokens", "max_tokens", "budget"] {
        budget(
            reasoning.and_then(|v| v.get(key)),
            &format!("reasoning.{key}"),
            false,
        )?;
    }
    for key in ["enabled", "enable_thinking", "enable_reasoning"] {
        boolean(
            reasoning.and_then(|v| v.get(key)),
            &format!("reasoning.{key}"),
        )?;
    }
    let flat = string(root.get("reasoning_effort"), "reasoning_effort")?;
    let output = object(root.get("output_config"), "output_config")?;
    let output_effort = string(output.and_then(|v| v.get("effort")), "output_config.effort")?;
    let thinking = object(root.get("thinking"), "thinking")?;
    let thinking_effort = string(thinking.and_then(|v| v.get("effort")), "thinking.effort")?;
    let kind = string(thinking.and_then(|v| v.get("type")), "thinking.type")?;
    if thinking.is_some() && !matches!(kind, Some("enabled" | "adaptive" | "disabled")) {
        return Err("CodeBuddy thinking.type is invalid".into());
    }
    budget(
        thinking.and_then(|v| v.get("budget_tokens")),
        "thinking.budget_tokens",
        false,
    )?;
    let mut enabled = None;
    for key in ["enable_thinking", "enable_reasoning", "is_reasoning"] {
        enabled = enabled.or(boolean(root.get(key), key)?);
    }
    let mut gemini = None;
    for key in ["generationConfig", "generation_config"] {
        if let Some(config) = object(root.get(key), key)? {
            for thinking_key in ["thinkingConfig", "thinking_config"] {
                let effort =
                    thinking_config(config.get(thinking_key), &format!("{key}.{thinking_key}"))?;
                gemini = gemini.or(effort);
            }
        }
    }
    if let Some(metadata) = root.get("metadata").and_then(Value::as_object) {
        if let Some(config) = object(
            metadata.get("geminiGenerationConfig"),
            "metadata.geminiGenerationConfig",
        )? {
            gemini = gemini.or(thinking_config(
                config.get("thinkingConfig"),
                "metadata.geminiGenerationConfig.thinkingConfig",
            )?);
            // Validate the snake_case alias even when a higher-priority value exists.
            gemini = gemini.or(thinking_config(
                config.get("thinking_config"),
                "metadata.geminiGenerationConfig.thinking_config",
            )?);
        }
    }
    let effort = nested
        .or(flat)
        .or(output_effort)
        .or(thinking_effort)
        .map(str::to_string)
        .or(gemini)
        .or_else(|| {
            kind.map(|value| {
                if value == "disabled" {
                    "none"
                } else {
                    "unspecified"
                }
                .to_string()
            })
        })
        .or_else(|| enabled.map(|value| if value { "unspecified" } else { "none" }.to_string()));
    Ok(effort.map(|value| {
        let value = value.to_ascii_lowercase();
        match value.as_str() {
            "none" | "off" | "disabled" => "none".to_string(),
            "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "auto" | "unspecified" => {
                value
            }
            _ => "unrecognized".to_string(),
        }
    }))
}

pub(crate) fn clear_buddy_reasoning_controls(input: &mut Value) -> Result<Option<String>, String> {
    let requested = requested_buddy_reasoning_effort(input)?;
    let root = input.as_object_mut().expect("validated request object");
    for key in [
        "reasoning",
        "reasoning_effort",
        "thinking",
        "enable_thinking",
        "enable_reasoning",
        "is_reasoning",
    ] {
        root.remove(key);
    }
    if let Some(output) = root.get_mut("output_config").and_then(Value::as_object_mut) {
        output.remove("effort");
        if output.is_empty() {
            root.remove("output_config");
        }
    }
    for key in ["generationConfig", "generation_config"] {
        if let Some(config) = root.get_mut(key).and_then(Value::as_object_mut) {
            config.remove("thinkingConfig");
            config.remove("thinking_config");
        }
    }
    if let Some(config) = root
        .get_mut("metadata")
        .and_then(|v| v.get_mut("geminiGenerationConfig"))
        .and_then(Value::as_object_mut)
    {
        config.remove("thinkingConfig");
        config.remove("thinking_config");
    }
    Ok(requested)
}

impl CodeBuddyModelCapability {
    pub fn upstream_default_reasoning_effort(&self) -> Result<Option<&str>, String> {
        if !self.reasoning_capability_known {
            return Err(format!("CodeBuddy model {} has unknown reasoning capability in the bound account's live catalog", self.id));
        }
        if !self.supports_reasoning {
            return Ok(None);
        }
        self.default_reasoning_effort.as_deref().ok_or_else(|| {
            format!("CodeBuddy model {} has no unambiguous default reasoning effort in the bound account's live catalog", self.id)
        }).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::codebuddy::CodeBuddySite;
    use crate::proxy::codebuddy_runtime::{
        build_codebuddy_payload, parse_codebuddy_model_catalog, resolve_codebuddy_model_id,
    };
    use serde_json::json;

    fn caps(extra: Value) -> Result<CodeBuddyModelCapability, String> {
        let mut model = json!({"id":"hy3"});
        model
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        Ok(
            parse_codebuddy_model_catalog(&json!({"models":[model]}), CodeBuddySite::Cn)?
                .capabilities["hy3"]
                .clone(),
        )
    }

    #[test]
    fn buddy_default_resolution_preserves_max_and_xhigh_and_never_uses_menu_order() {
        for (reasoning, expected, source) in [
            (
                json!({"defaultEffort":"MAX","effort":"high","supportedEfforts":["low","xhigh","max"]}),
                "max",
                BuddyReasoningDefaultSource::DeclaredDefault,
            ),
            (
                json!({"defaultEffort":"  ","effort":"xhigh","supportedEfforts":["max","xhigh"]}),
                "xhigh",
                BuddyReasoningDefaultSource::Legacy,
            ),
            (
                json!({"supportedEfforts":["HIGH","high"]}),
                "high",
                BuddyReasoningDefaultSource::SoleAllowed,
            ),
        ] {
            let capability = caps(json!({"supportsReasoning":true,"reasoning":reasoning})).unwrap();
            assert_eq!(
                capability.upstream_default_reasoning_effort().unwrap(),
                Some(expected)
            );
            assert_eq!(capability.reasoning_default_source, Some(source));
            for effort in [
                "low",
                "medium",
                "high",
                "none",
                "max",
                "xhigh",
                "future-client-level",
            ] {
                let request = json!({"messages":[{"role":"user","content":"ping"}], "reasoning_effort":effort});
                assert_eq!(
                    build_codebuddy_payload(&request, "hy3", &capability).unwrap()
                        ["reasoning_effort"],
                    expected
                );
            }
        }
        for extra in [
            json!({}),
            json!({"supportsReasoning":true}),
            json!({"reasoning":{"supportedEfforts":["low","high"]}}),
        ] {
            assert!(caps(extra)
                .unwrap()
                .upstream_default_reasoning_effort()
                .is_err());
        }
        let no_reasoning = caps(json!({"supportsReasoning":false})).unwrap();
        let payload = build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"ping"}],"reasoning_effort":"high"}),
            "hy3",
            &no_reasoning,
        )
        .unwrap();
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn buddy_catalog_rejects_invalid_and_contradictory_defaults() {
        for extra in [
            json!({"supportsReasoning":false,"onlyReasoning":true}),
            json!({"onlyReasoning":true,"reasoning":{"canDisableThinking":true,"effort":"high"}}),
            json!({"onlyReasoning":true,"reasoning":{"canDisableThinking":"false","effort":"high"}}),
            json!({"supportsReasoning":false,"reasoning":{"effort":"high"}}),
            json!({"reasoning":{"defaultEffort":"max","supportedEfforts":["low","high"]}}),
            json!({"reasoning":{"defaultEffort":42,"effort":"high"}}),
            json!({"reasoning":{"supportedEfforts":"high"}}),
            json!({"reasoning":{"supportedEfforts":["high",false]}}),
            json!({"reasoning":{"effort":"none"}}),
            json!({"reasoning":{"effort":"unknown"}}),
            json!({"reasoning":true}),
        ] {
            assert!(caps(extra.clone()).is_err(), "accepted {extra}");
        }
        let optional = caps(json!({"reasoning":{"canDisableThinking":true,"defaultEffort":"none","supportedEfforts":["none","high"]}})).unwrap();
        assert_eq!(
            optional.upstream_default_reasoning_effort().unwrap(),
            Some("none")
        );
    }

    #[test]
    fn buddy_control_cleanup_is_scoped_and_preserves_non_reasoning_settings_and_history() {
        let history = json!([{"role":"assistant","content":"answer","reasoning_content":"keep this", "tool_calls":[{"id":"1","function":{"name":"f","arguments":"{\"reasoning_effort\":\"low\"}"}}]}]);
        let mut input = json!({"messages":history,"max_tokens":4096,"temperature":0.7,
            "reasoning":{"effort":"high","budget_tokens":300},"reasoning_effort":"low",
            "thinking":{"type":"disabled"},"output_config":{"effort":"max","format":{"type":"json_schema"}},
            "enable_thinking":false,"enable_reasoning":true,"is_reasoning":false,
            "generationConfig":{"thinkingConfig":{"thinkingLevel":"LOW"},"responseSchema":{"type":"object"}},
            "metadata":{"geminiGenerationConfig":{"thinkingConfig":{"thinkingBudget":-1},"responseMimeType":"application/json"},"reasoning_effort":"caller-data"},
            "tools":[{"type":"function","function":{"name":"f","parameters":{"properties":{"reasoning":{"type":"string"}}}}}]});
        let original = input.clone();
        assert_eq!(
            clear_buddy_reasoning_controls(&mut input)
                .unwrap()
                .as_deref(),
            Some("high")
        );
        for key in ["messages", "max_tokens", "temperature", "tools"] {
            assert_eq!(input[key], original[key]);
        }
        assert_eq!(
            input["output_config"],
            json!({"format":{"type":"json_schema"}})
        );
        assert_eq!(
            input["generationConfig"],
            json!({"responseSchema":{"type":"object"}})
        );
        assert_eq!(
            input["metadata"],
            json!({"geminiGenerationConfig":{"responseMimeType":"application/json"},"reasoning_effort":"caller-data"})
        );
        for key in [
            "reasoning",
            "reasoning_effort",
            "thinking",
            "enable_thinking",
            "enable_reasoning",
            "is_reasoning",
        ] {
            assert!(input.get(key).is_none());
        }
    }

    #[test]
    fn buddy_malformed_controls_fail_before_cleanup_or_auto_selection() {
        let catalog = parse_codebuddy_model_catalog(
            &json!({"models":[{"id":"hy3","supportsReasoning":false}]}),
            CodeBuddySite::Cn,
        )
        .unwrap();
        for input in [
            json!({"reasoning":true}),
            json!({"reasoning_effort":3}),
            json!({"reasoning":{"effort":false}}),
            json!({"thinking":[]}),
            json!({"thinking":{"type":"enabled","budget_tokens":"1024"}}),
            json!({"output_config":{"effort":3}}),
            json!({"enable_thinking":"true"}),
            json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":3}}}),
            json!({"reasoning_effort":"high","generation_config":{"thinking_config":{"thinking_budget":-2}}}),
            json!({"metadata":{"geminiGenerationConfig":{"thinkingConfig":false}}}),
        ] {
            let mut request = input.clone();
            assert!(
                clear_buddy_reasoning_controls(&mut request).is_err(),
                "accepted {input}"
            );
            assert_eq!(request, input);
            assert!(
                resolve_codebuddy_model_id(CodeBuddySite::Cn, "auto", &catalog, &input).is_err()
            );
        }
    }
}
