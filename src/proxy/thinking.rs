use serde_json::{json, Map, Value};

const MAX_THINKING_BUDGET: u64 = 32_000;
const MAX_TOKENS_VALUE: u64 = 64_000;
const MIN_MAX_TOKENS_FOR_BUDGET: u64 = MAX_THINKING_BUDGET + 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ThinkingPipelineConfig {
    pub(super) optimizer_enabled: bool,
    pub(super) signature_rectifier_enabled: bool,
    pub(super) budget_rectifier_enabled: bool,
}

impl ThinkingPipelineConfig {
    pub(super) fn disabled() -> Self {
        Self {
            optimizer_enabled: false,
            signature_rectifier_enabled: false,
            budget_rectifier_enabled: false,
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.optimizer_enabled || self.signature_rectifier_enabled || self.budget_rectifier_enabled
    }
}

pub(super) fn apply_thinking_pipeline(body: &mut Value, config: &ThinkingPipelineConfig) {
    if !config.is_enabled() {
        return;
    }
    if config.optimizer_enabled {
        optimize_thinking(body);
    }
    if config.budget_rectifier_enabled {
        rectify_thinking_budget(body);
    }
    if config.signature_rectifier_enabled {
        rectify_thinking_signature(body);
    }
}

fn optimize_thinking(body: &mut Value) {
    let Some(model) = body
        .get("model")
        .and_then(Value::as_str)
        .map(|model| model.to_ascii_lowercase())
    else {
        return;
    };

    if model.contains("haiku") {
        return;
    }

    if uses_adaptive_thinking(&model) {
        body["thinking"] = json!({"type": "adaptive"});
        body["output_config"] = json!({"effort": "max"});
        return;
    }

    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(16_384);
    let budget_target = max_tokens.saturating_sub(1);
    let thinking_type = body
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str);

    match thinking_type {
        None | Some("disabled") => {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_target
            });
        }
        Some("enabled") => {
            let current_budget = body
                .get("thinking")
                .and_then(|thinking| thinking.get("budget_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if current_budget < budget_target {
                body["thinking"]["budget_tokens"] = json!(budget_target);
            }
        }
        _ => {}
    }
    append_beta(body, "interleaved-thinking-2025-05-14");
}

fn uses_adaptive_thinking(model: &str) -> bool {
    let normalized = model.replace('.', "-");
    ["opus-4-8", "opus-4-7", "opus-4-6", "sonnet-4-6"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn append_beta(body: &mut Value, beta: &str) {
    match body.get_mut("anthropic_beta") {
        Some(Value::Array(items)) => {
            if !items.iter().any(|item| item.as_str() == Some(beta)) {
                items.push(json!(beta));
            }
        }
        Some(Value::Null) | None => {
            body["anthropic_beta"] = json!([beta]);
        }
        _ => {
            body["anthropic_beta"] = json!([beta]);
        }
    }
}

fn rectify_thinking_signature(body: &mut Value) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };

    for message in messages {
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut next_content = Vec::with_capacity(content.len());
        let mut modified = false;
        for block in content.iter() {
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") | Some("redacted_thinking") => {
                    modified = true;
                    continue;
                }
                _ => {}
            }

            if block.get("signature").is_some() {
                let mut block = block.clone();
                if let Some(object) = block.as_object_mut() {
                    object.remove("signature");
                    modified = true;
                    next_content.push(Value::Object(object.clone()));
                    continue;
                }
            }
            next_content.push(block.clone());
        }

        if modified {
            *content = next_content;
        }
    }

    if should_remove_top_level_thinking(body) {
        if let Some(object) = body.as_object_mut() {
            object.remove("thinking");
        }
    }
}

fn should_remove_top_level_thinking(body: &Value) -> bool {
    let thinking_enabled = body
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        == Some("enabled");
    if !thinking_enabled {
        return false;
    }

    let Some(content) =
        body.get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().rev().find(|message| {
                    message.get("role").and_then(Value::as_str) == Some("assistant")
                })
            })
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
    else {
        return false;
    };
    if content.is_empty() {
        return false;
    }

    let first_block_type = content
        .first()
        .and_then(|block| block.get("type"))
        .and_then(Value::as_str);
    let missing_thinking_prefix =
        first_block_type != Some("thinking") && first_block_type != Some("redacted_thinking");
    missing_thinking_prefix
        && content
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
}

fn rectify_thinking_budget(body: &mut Value) {
    if body
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        == Some("adaptive")
    {
        return;
    }

    if !body.get("thinking").is_some_and(Value::is_object) {
        body["thinking"] = Value::Object(Map::new());
    }

    let Some(thinking) = body.get_mut("thinking").and_then(Value::as_object_mut) else {
        return;
    };
    thinking.insert("type".to_string(), Value::String("enabled".to_string()));
    thinking.insert(
        "budget_tokens".to_string(),
        Value::Number(MAX_THINKING_BUDGET.into()),
    );

    let max_tokens = body.get("max_tokens").and_then(Value::as_u64);
    if max_tokens.is_none() || max_tokens < Some(MIN_MAX_TOKENS_FOR_BUDGET) {
        body["max_tokens"] = Value::Number(MAX_TOKENS_VALUE.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn optimizer_config() -> ThinkingPipelineConfig {
        ThinkingPipelineConfig {
            optimizer_enabled: true,
            signature_rectifier_enabled: false,
            budget_rectifier_enabled: false,
        }
    }

    #[test]
    fn optimizer_uses_adaptive_for_new_opus_and_sonnet_models() {
        for model in [
            "anthropic/claude-opus-4.8",
            "anthropic.claude-opus-4-6-20250514-v1:0",
            "anthropic.claude-sonnet-4-6-20250514-v1:0",
        ] {
            let mut body = json!({
                "model": model,
                "thinking": {"type": "enabled", "budget_tokens": 8000},
                "messages": [{"role": "user", "content": "hello"}]
            });

            apply_thinking_pipeline(&mut body, &optimizer_config());

            assert_eq!(body["thinking"]["type"], "adaptive");
            assert!(body["thinking"].get("budget_tokens").is_none());
            assert_eq!(body["output_config"]["effort"], "max");
            assert!(body.get("anthropic_beta").is_none());
        }
    }

    #[test]
    fn optimizer_skips_haiku_models() {
        let mut body = json!({
            "model": "anthropic.claude-haiku-4-5-20250514-v1:0",
            "max_tokens": 8192,
            "messages": [{"role": "user", "content": "hello"}]
        });
        let original = body.clone();

        apply_thinking_pipeline(&mut body, &optimizer_config());

        assert_eq!(body, original);
    }

    #[test]
    fn optimizer_injects_or_upgrades_legacy_budget() {
        let mut body = json!({
            "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
            "max_tokens": 8192,
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [{"role": "user", "content": "hello"}]
        });

        apply_thinking_pipeline(&mut body, &optimizer_config());

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8191);
        assert!(body["anthropic_beta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str() == Some("interleaved-thinking-2025-05-14")));
    }

    #[test]
    fn signature_rectifier_removes_legacy_thinking_blocks_and_signatures() {
        let mut body = json!({
            "model": "claude-test",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "t", "signature": "sig"},
                    {"type": "text", "text": "hello", "signature": "sig_text"},
                    {"type": "redacted_thinking", "data": "r", "signature": "sig_redacted"}
                ]
            }]
        });
        let config = ThinkingPipelineConfig {
            optimizer_enabled: false,
            signature_rectifier_enabled: true,
            budget_rectifier_enabled: false,
        };

        apply_thinking_pipeline(&mut body, &config);

        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0].get("signature").is_none());
    }

    #[test]
    fn signature_rectifier_removes_top_level_enabled_thinking_for_tool_use_without_prefix() {
        let mut body = json!({
            "model": "claude-test",
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [{
                "role": "assistant",
                "content": [{"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {}}]
            }]
        });
        let config = ThinkingPipelineConfig {
            optimizer_enabled: false,
            signature_rectifier_enabled: true,
            budget_rectifier_enabled: false,
        };

        apply_thinking_pipeline(&mut body, &config);

        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn budget_rectifier_sets_safe_budget_and_max_tokens_but_skips_adaptive() {
        let mut body = json!({
            "model": "claude-test",
            "thinking": {"type": "enabled", "budget_tokens": 512},
            "max_tokens": 1024
        });
        let config = ThinkingPipelineConfig {
            optimizer_enabled: false,
            signature_rectifier_enabled: false,
            budget_rectifier_enabled: true,
        };

        apply_thinking_pipeline(&mut body, &config);

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], MAX_THINKING_BUDGET);
        assert_eq!(body["max_tokens"], MAX_TOKENS_VALUE);

        body["thinking"] = json!({"type": "adaptive", "budget_tokens": 512});
        body["max_tokens"] = json!(1024);
        apply_thinking_pipeline(&mut body, &config);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["budget_tokens"], 512);
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn disabled_config_is_noop() {
        let mut body = json!({
            "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let original = body.clone();

        apply_thinking_pipeline(&mut body, &ThinkingPipelineConfig::disabled());

        assert_eq!(body, original);
    }
}
