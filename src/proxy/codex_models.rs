#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexModelCapability {
    pub id: &'static str,
    pub reasoning_efforts: &'static [&'static str],
    pub input_modalities: &'static [&'static str],
}

const STANDARD_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];
const GPT_56_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];
const GPT_56_LUNA_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const TEXT_IMAGE: &[&str] = &["text", "image"];

pub(crate) const BUILTIN_CODEX_MODELS: &[CodexModelCapability] = &[
    CodexModelCapability {
        id: "gpt-5.6-sol",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
    CodexModelCapability {
        id: "gpt-5.6-terra",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
    CodexModelCapability {
        id: "gpt-5.6-luna",
        reasoning_efforts: GPT_56_LUNA_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
];

pub(crate) fn capability_for_model(model: &str) -> Option<&'static CodexModelCapability> {
    let model = normalize_model_id(model);
    BUILTIN_CODEX_MODELS
        .iter()
        .find(|capability| model == capability.id)
}

pub(crate) fn normalize_reasoning_effort(model: &str, effort: &str) -> String {
    let model = normalize_model_id(model);
    let effort = effort.trim().to_ascii_lowercase();
    if let Some(capability) = BUILTIN_CODEX_MODELS
        .iter()
        .find(|capability| model.starts_with(capability.id))
    {
        if capability.reasoning_efforts.contains(&effort.as_str()) {
            return effort;
        }
        return match effort.as_str() {
            "ultra" if capability.reasoning_efforts.contains(&"max") => "max".to_string(),
            "max" | "ultra" => "xhigh".to_string(),
            _ => effort,
        };
    }
    if parse_gpt_version(&model).is_some() && !STANDARD_EFFORTS.contains(&effort.as_str()) {
        return match effort.as_str() {
            "max" | "ultra" => "xhigh".to_string(),
            _ => effort,
        };
    }
    effort
}

fn normalize_model_id(model: &str) -> String {
    model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
        .replace('_', "-")
}

fn parse_gpt_version(model: &str) -> Option<(u32, u32)> {
    let rest = model.strip_prefix("gpt-")?;
    let mut parts = rest.split(|character: char| !character.is_ascii_digit() && character != '.');
    let version = parts.next()?;
    let mut numbers = version.split('.');
    let major = numbers.next()?.parse().ok()?;
    let minor = numbers.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_gpt_56_reasoning_from_capability_registry() {
        assert_eq!(normalize_reasoning_effort("gpt-5.6-sol", "ultra"), "ultra");
        assert_eq!(normalize_reasoning_effort("gpt-5.6-luna", "ultra"), "max");
        assert_eq!(normalize_reasoning_effort("gpt-5.5", "max"), "xhigh");
        assert_eq!(normalize_reasoning_effort("vendor/model", "max"), "max");
        assert_eq!(BUILTIN_CODEX_MODELS[0].input_modalities, ["text", "image"]);
    }
}
