use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeModelCapability {
    pub id: &'static str,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub input_modalities: &'static [&'static str],
    pub effort_levels: &'static [&'static str],
    pub dynamic_thinking: bool,
    pub rejects_disabled_thinking: bool,
    pub mid_conversation_system: bool,
    pub mid_conversation_tool_changes: bool,
    pub per_turn_effort: bool,
    pub per_turn_timing: bool,
    pub web_search: bool,
}

const TEXT_IMAGE: &[&str] = &["text", "image"];
const NO_EFFORT: &[&str] = &[];
const STANDARD_EFFORT: &[&str] = &["low", "medium", "high", "max"];
const FULL_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Sanitized subset of the model catalog embedded in official Claude Code
/// 2.1.280. This is the single runtime truth for model-gated OAuth behavior and
/// static model discovery; unknown models deliberately receive no capability.
pub const CLAUDE_MODEL_CAPABILITIES: &[ClaudeModelCapability] = &[
    ClaudeModelCapability {
        id: "claude-opus-5-5",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: true,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: true,
        per_turn_timing: true,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-fable-5-1",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: true,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: true,
        per_turn_timing: true,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-fable-5",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: true,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-opus-5",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: false,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-opus-4-8",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: false,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-sonnet-5",
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: FULL_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: false,
        mid_conversation_system: true,
        mid_conversation_tool_changes: true,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-opus-4-6",
        context_window: 200_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: STANDARD_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: false,
        mid_conversation_system: false,
        mid_conversation_tool_changes: false,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-sonnet-4-6",
        context_window: 200_000,
        max_output_tokens: 128_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: STANDARD_EFFORT,
        dynamic_thinking: true,
        rejects_disabled_thinking: false,
        mid_conversation_system: false,
        mid_conversation_tool_changes: false,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
    ClaudeModelCapability {
        id: "claude-haiku-4-5-20251001",
        context_window: 200_000,
        max_output_tokens: 64_000,
        input_modalities: TEXT_IMAGE,
        effort_levels: NO_EFFORT,
        dynamic_thinking: false,
        rejects_disabled_thinking: false,
        mid_conversation_system: false,
        mid_conversation_tool_changes: false,
        per_turn_effort: false,
        per_turn_timing: false,
        web_search: true,
    },
];

pub static CLAUDE_MODEL_IDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    CLAUDE_MODEL_CAPABILITIES
        .iter()
        .map(|capability| capability.id)
        .collect()
});

pub fn claude_model_capability(model: &str) -> Option<&'static ClaudeModelCapability> {
    let canonical = model.trim().trim_end_matches("[1m]");
    CLAUDE_MODEL_CAPABILITIES
        .iter()
        .find(|capability| capability.id == canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_5_5_capability_is_complete_and_suffix_aware() {
        let capability = claude_model_capability("claude-opus-5-5[1m]").unwrap();
        assert_eq!(capability.context_window, 1_000_000);
        assert_eq!(capability.max_output_tokens, 128_000);
        assert!(capability.dynamic_thinking);
        assert!(capability.rejects_disabled_thinking);
        assert!(capability.mid_conversation_system);
        assert!(capability.mid_conversation_tool_changes);
        assert!(capability.per_turn_effort);
        assert!(capability.per_turn_timing);
        assert_eq!(capability.effort_levels, FULL_EFFORT);
    }

    #[test]
    fn unknown_models_are_fail_closed() {
        assert!(claude_model_capability("claude-opus-next").is_none());
        assert!(claude_model_capability("vendor/claude-opus-5-5").is_none());
    }
}
