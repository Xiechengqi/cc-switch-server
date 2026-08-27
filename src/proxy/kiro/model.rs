use serde_json::Value;

const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
const GPT_CONTEXT_WINDOW: u64 = 272_000;
const LARGE_CLAUDE_CONTEXT_WINDOW: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroModelFamily {
    Claude,
    Gpt,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroReasoningShape {
    None,
    ClaudeOutputConfig,
    GptReasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroCapabilitySource {
    Catalog,
    Static,
    ConservativeDefault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KiroResolvedModel {
    pub upstream_model_id: String,
    pub family: KiroModelFamily,
    pub context_window: u64,
    pub context_window_source: KiroCapabilitySource,
    pub reasoning_shape: KiroReasoningShape,
}

pub(crate) fn valid_model_id(model: &str) -> bool {
    let model = model.trim();
    !model.is_empty()
        && model.len() <= 128
        && model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'[' | b']')
        })
}

pub(crate) fn normalize_known_model(model: &str) -> Option<String> {
    let mut normalized = model.trim().to_ascii_lowercase();
    if !valid_model_id(&normalized) {
        return None;
    }
    loop {
        let mut stripped = false;
        for suffix in ["-thinking", "-latest"] {
            if let Some(value) = normalized.strip_suffix(suffix) {
                normalized = value.to_string();
                stripped = true;
            }
        }
        if !stripped {
            break;
        }
    }
    for suffix in ["[1m]", "-1m"] {
        if let Some(value) = normalized.strip_suffix(suffix) {
            normalized = value.to_string();
        }
    }
    if let Some((base, suffix)) = normalized.rsplit_once('-') {
        if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            normalized = base.to_string();
        }
    }
    if matches!(
        normalized.as_str(),
        "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
    ) {
        return Some(normalized);
    }

    let body = normalized.strip_prefix("claude-")?;
    const FAMILIES: &[&str] = &["sonnet", "opus", "haiku", "fable", "mythos"];
    for family in FAMILIES {
        if let Some(rest) = body.strip_prefix(family) {
            let rest = rest
                .strip_prefix('-')
                .or_else(|| rest.strip_prefix('.'))
                .unwrap_or(rest);
            let version = canonical_version(rest)?;
            return Some(format!("claude-{family}-{version}"));
        }
    }

    let parts = body.split('-').collect::<Vec<_>>();
    let family_index = parts.iter().position(|part| FAMILIES.contains(part))?;
    if family_index == 0 || family_index + 1 != parts.len() {
        return None;
    }
    let version = canonical_version(&parts[..family_index].join("-"))?;
    Some(format!("claude-{}-{version}", parts[family_index]))
}

fn canonical_version(raw: &str) -> Option<String> {
    let parts = raw
        .split(['-', '.'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [major] if major.bytes().all(|byte| byte.is_ascii_digit()) => Some((*major).to_string()),
        [major, minor]
            if major.bytes().all(|byte| byte.is_ascii_digit())
                && minor.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(format!("{major}.{minor}"))
        }
        _ => None,
    }
}

pub(crate) fn resolve_static(model: &str) -> Option<KiroResolvedModel> {
    let upstream_model_id = normalize_known_model(model)?;
    let known = crate::domain::providers::kiro::STATIC_MODEL_IDS
        .iter()
        .any(|candidate| *candidate == upstream_model_id);
    known.then(|| resolve_authorized_model(upstream_model_id, None))
}

pub(crate) fn resolve_catalog_authorized(
    model_id: &str,
    max_input_tokens: Option<u64>,
) -> KiroResolvedModel {
    resolve_authorized_model(model_id.trim().to_string(), max_input_tokens)
}

fn resolve_authorized_model(
    upstream_model_id: String,
    max_input_tokens: Option<u64>,
) -> KiroResolvedModel {
    let lower = upstream_model_id.to_ascii_lowercase();
    let family = if lower.starts_with("claude-") {
        KiroModelFamily::Claude
    } else if lower.starts_with("gpt-") {
        KiroModelFamily::Gpt
    } else {
        KiroModelFamily::Unknown
    };
    let (fallback_context, fallback_source) = fallback_context_window(&upstream_model_id);
    let (context_window, context_window_source) = max_input_tokens
        .filter(|tokens| *tokens > 0)
        .map(|tokens| (tokens, KiroCapabilitySource::Catalog))
        .unwrap_or((fallback_context, fallback_source));
    let reasoning_shape = if matches!(
        lower.as_str(),
        "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
    ) {
        KiroReasoningShape::GptReasoning
    } else if supports_claude_reasoning(&lower) {
        KiroReasoningShape::ClaudeOutputConfig
    } else {
        KiroReasoningShape::None
    };
    KiroResolvedModel {
        upstream_model_id,
        family,
        context_window,
        context_window_source,
        reasoning_shape,
    }
}

pub(crate) fn fallback_context_window(model: &str) -> (u64, KiroCapabilitySource) {
    let raw = model.trim().to_ascii_lowercase();
    let explicitly_large = raw.ends_with("[1m]") || raw.ends_with("-1m");
    let mapped = normalize_known_model(model).unwrap_or(raw);
    if explicitly_large {
        return (LARGE_CLAUDE_CONTEXT_WINDOW, KiroCapabilitySource::Static);
    }
    if mapped.starts_with("gpt-") {
        return (GPT_CONTEXT_WINDOW, KiroCapabilitySource::Static);
    }
    if [
        "claude-sonnet-4.6",
        "claude-sonnet-4.8",
        "claude-sonnet-5",
        "claude-opus-4.6",
        "claude-opus-4.7",
        "claude-opus-4.8",
        "claude-opus-5",
        "claude-fable-5",
    ]
    .contains(&mapped.as_str())
    {
        return (LARGE_CLAUDE_CONTEXT_WINDOW, KiroCapabilitySource::Static);
    }
    (
        DEFAULT_CONTEXT_WINDOW,
        KiroCapabilitySource::ConservativeDefault,
    )
}

fn supports_claude_reasoning(model: &str) -> bool {
    matches!(
        model,
        "claude-opus-4.6"
            | "claude-opus-4.7"
            | "claude-opus-4.8"
            | "claude-opus-5"
            | "claude-sonnet-4.6"
            | "claude-sonnet-5"
            | "claude-fable-5"
    )
}

pub(crate) fn additional_request_fields(
    body: &Value,
    model: &KiroResolvedModel,
) -> Result<Option<Value>, &'static str> {
    if body.pointer("/thinking/type").and_then(Value::as_str) == Some("disabled")
        || model.reasoning_shape == KiroReasoningShape::None
    {
        return Ok(None);
    }
    let thinking_type = body.pointer("/thinking/type").and_then(Value::as_str);
    let explicit_effort = body
        .pointer("/output_config/effort")
        .and_then(Value::as_str);
    let normalized_model = model.upstream_model_id.to_ascii_lowercase();
    if normalized_model == "claude-opus-4.6" && thinking_type != Some("adaptive") {
        return Ok(None);
    }
    if !matches!(thinking_type, Some("adaptive" | "enabled")) && explicit_effort.is_none() {
        return Ok(None);
    }
    let effort = normalize_effort(
        explicit_effort
            .or_else(|| effort_from_budget(body))
            .unwrap_or("high"),
        &normalized_model,
    )?;
    Ok(match model.reasoning_shape {
        KiroReasoningShape::GptReasoning => Some(serde_json::json!({
            "reasoning": { "effort": effort }
        })),
        KiroReasoningShape::ClaudeOutputConfig => Some(serde_json::json!({
            "output_config": { "effort": effort }
        })),
        KiroReasoningShape::None => None,
    })
}

fn effort_from_budget(body: &Value) -> Option<&'static str> {
    let budget = body.pointer("/thinking/budget_tokens")?.as_i64()?;
    Some(match budget {
        ..=4_000 => "low",
        4_001..=16_000 => "medium",
        16_001..=64_000 => "high",
        _ => "xhigh",
    })
}

fn normalize_effort(raw: &str, model: &str) -> Result<&'static str, &'static str> {
    let effort = match raw.trim().to_ascii_lowercase().as_str() {
        "none" => "none",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" | "x-high" | "x_high" => "xhigh",
        "max" => "max",
        _ => return Err("unsupported Kiro reasoning effort"),
    };
    if effort == "none" && !model.starts_with("gpt-") {
        return Ok("high");
    }
    let supports_xhigh = model.contains("opus-4.7")
        || model.contains("opus-4.8")
        || model.contains("fable-5")
        || model.contains("mythos-5")
        || model.contains("claude-5")
        || model.starts_with("gpt-");
    if effort == "xhigh" && !supports_xhigh {
        Ok("high")
    } else {
        Ok(effort)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_known_claude_aliases_and_future_families() {
        assert_eq!(
            normalize_known_model("claude-sonnet-4-8-latest").as_deref(),
            Some("claude-sonnet-4.8")
        );
        assert_eq!(
            normalize_known_model("claude-3-5-sonnet-20241022").as_deref(),
            Some("claude-sonnet-3.5")
        );
        assert_eq!(
            normalize_known_model("claude-fable-5-thinking").as_deref(),
            Some("claude-fable-5")
        );
    }

    #[test]
    fn catalog_limit_overrides_static_context_window() {
        let resolved = resolve_catalog_authorized("claude-sonnet-4.6", Some(900_000));
        assert_eq!(resolved.context_window, 900_000);
        assert_eq!(
            resolved.context_window_source,
            KiroCapabilitySource::Catalog
        );
        assert_eq!(fallback_context_window("gpt-5.6-sol").0, 272_000);
        assert_eq!(fallback_context_window("claude-opus-4.8").0, 1_000_000);
        assert_eq!(
            fallback_context_window("claude-sonnet-4-5[1m]").0,
            1_000_000
        );
        assert_eq!(fallback_context_window("claude-haiku-4.5-1m").0, 1_000_000);
    }

    #[test]
    fn static_authorization_is_exact_and_catalog_ids_pass_through_verbatim() {
        assert!(resolve_static("claude-sonnet-500").is_none());
        assert!(resolve_static("gpt-5.6-unknown").is_none());

        let future = resolve_catalog_authorized("claude-haiku-6-20260827", None);
        assert_eq!(future.upstream_model_id, "claude-haiku-6-20260827");
        assert_eq!(future.context_window, 200_000);
        assert_eq!(
            future.context_window_source,
            KiroCapabilitySource::ConservativeDefault
        );
    }

    #[test]
    fn uses_distinct_gpt_and_claude_reasoning_shapes() {
        let gpt = resolve_static("gpt-5.6-sol").unwrap();
        assert_eq!(
            additional_request_fields(
                &json!({"thinking":{"type":"enabled","budget_tokens":8000}}),
                &gpt
            )
            .unwrap(),
            Some(json!({"reasoning":{"effort":"medium"}}))
        );
        let claude = resolve_static("claude-opus-4.8").unwrap();
        assert_eq!(
            additional_request_fields(
                &json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"xhigh"}}),
                &claude
            )
            .unwrap(),
            Some(json!({"output_config":{"effort":"xhigh"}}))
        );
        let unconfirmed = resolve_static("claude-sonnet-4.8").unwrap();
        assert_eq!(unconfirmed.reasoning_shape, KiroReasoningShape::None);
        let future = resolve_catalog_authorized("claude-sonnet-5.1", None);
        assert_eq!(future.reasoning_shape, KiroReasoningShape::None);

        let uppercase_opus = resolve_catalog_authorized("Claude-Opus-4.6", None);
        assert_eq!(
            uppercase_opus.reasoning_shape,
            KiroReasoningShape::ClaudeOutputConfig
        );
        assert_eq!(
            additional_request_fields(
                &json!({"thinking":{"type":"enabled","budget_tokens":8000}}),
                &uppercase_opus,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn rejects_unknown_reasoning_effort() {
        let model = resolve_static("claude-opus-4.8").unwrap();
        assert_eq!(
            additional_request_fields(
                &json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"turbo"}}),
                &model,
            ),
            Err("unsupported Kiro reasoning effort")
        );
    }
}
