use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CacheInjectionConfig {
    pub(super) enabled: bool,
    pub(super) ttl: String,
}

impl CacheInjectionConfig {
    pub(super) fn disabled() -> Self {
        Self {
            enabled: false,
            ttl: "5m".to_string(),
        }
    }
}

pub(super) fn inject_prompt_cache(body: &mut Value, config: &CacheInjectionConfig) {
    if !config.enabled {
        return;
    }

    let existing = count_existing(body);
    upgrade_existing_ttl(body, &config.ttl);

    let mut budget = 4_usize.saturating_sub(existing);
    if budget == 0 {
        return;
    }

    if budget > 0 {
        if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
            if let Some(last) = tools.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(object) = last.as_object_mut() {
                        object.insert("cache_control".to_string(), make_cache_control(&config.ttl));
                    }
                    budget -= 1;
                }
            }
        }
    }

    if budget > 0 {
        if let Some(text) = body
            .get("system")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            body["system"] = json!([{"type": "text", "text": text}]);
        }

        if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
            if let Some(last) = system.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(object) = last.as_object_mut() {
                        object.insert("cache_control".to_string(), make_cache_control(&config.ttl));
                    }
                    budget -= 1;
                }
            }
        }
    }

    if budget > 0 {
        if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
            if let Some(assistant_message) = messages
                .iter_mut()
                .rev()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            {
                if let Some(content) = assistant_message
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                {
                    if let Some(block) = content.iter_mut().rev().find(|block| {
                        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                        block_type != "thinking" && block_type != "redacted_thinking"
                    }) {
                        if block.get("cache_control").is_none() {
                            if let Some(object) = block.as_object_mut() {
                                object.insert(
                                    "cache_control".to_string(),
                                    make_cache_control(&config.ttl),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn make_cache_control(ttl: &str) -> Value {
    if ttl == "5m" {
        json!({"type": "ephemeral"})
    } else {
        json!({"type": "ephemeral", "ttl": ttl})
    }
}

fn count_existing(body: &Value) -> usize {
    let mut count = 0;

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        count += tools
            .iter()
            .filter(|tool| tool.get("cache_control").is_some())
            .count();
    }

    if let Some(system) = body.get("system").and_then(Value::as_array) {
        count += system
            .iter()
            .filter(|block| block.get("cache_control").is_some())
            .count();
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(content) = message.get("content").and_then(Value::as_array) {
                count += content
                    .iter()
                    .filter(|block| block.get("cache_control").is_some())
                    .count();
            }
        }
    }

    count
}

fn upgrade_existing_ttl(body: &mut Value, ttl: &str) {
    let upgrade = |value: &mut Value| {
        if let Some(cache_control) = value
            .get_mut("cache_control")
            .and_then(Value::as_object_mut)
        {
            if ttl == "5m" {
                cache_control.remove("ttl");
            } else {
                cache_control.insert("ttl".to_string(), json!(ttl));
            }
        }
    };

    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            upgrade(tool);
        }
    }

    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
        for block in system {
            upgrade(block);
        }
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in content {
                    upgrade(block);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(ttl: &str) -> CacheInjectionConfig {
        CacheInjectionConfig {
            enabled: true,
            ttl: ttl.to_string(),
        }
    }

    #[test]
    fn empty_body_without_injection_targets_is_unchanged() {
        let mut body = json!({
            "model": "test",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });
        let original = body.clone();

        inject_prompt_cache(&mut body, &config("1h"));

        assert_eq!(body, original);
    }

    #[test]
    fn injects_tools_system_and_last_assistant_breakpoints() {
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}, {"name": "tool2"}],
            "system": [{"type": "text", "text": "sys prompt"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}
            ]
        });

        inject_prompt_cache(&mut body, &config("1h"));

        assert_eq!(body["tools"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }

    #[test]
    fn existing_four_breakpoints_only_upgrade_ttl() {
        let mut body = json!({
            "model": "test",
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral", "ttl": "5m"}},
                {"name": "t2", "cache_control": {"type": "ephemeral", "ttl": "5m"}}
            ],
            "system": [
                {"type": "text", "text": "sys", "cache_control": {"type": "ephemeral", "ttl": "5m"}}
            ],
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "text", "text": "ok", "cache_control": {"type": "ephemeral", "ttl": "5m"}}
                ]}
            ]
        });

        inject_prompt_cache(&mut body, &config("1h"));

        assert_eq!(body["tools"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(body["tools"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }

    #[test]
    fn existing_two_breakpoints_leave_two_new_slots() {
        let mut body = json!({
            "model": "test",
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral"}},
                {"name": "t2", "cache_control": {"type": "ephemeral"}}
            ],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}
            ]
        });

        inject_prompt_cache(&mut body, &config("1h"));

        assert!(body["system"][0].get("cache_control").is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn system_string_is_converted_to_text_block_array() {
        let mut body = json!({
            "model": "test",
            "system": "You are a helpful assistant",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });

        inject_prompt_cache(&mut body, &config("1h"));

        assert!(body["system"].is_array());
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(body["system"][0]["text"], "You are a helpful assistant");
        assert!(body["system"][0].get("cache_control").is_some());
    }

    #[test]
    fn ttl_5m_omits_ttl_field() {
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });

        inject_prompt_cache(&mut body, &config("5m"));

        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0]["cache_control"].get("ttl").is_none());
    }

    #[test]
    fn disabled_config_is_noop() {
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [{"role": "assistant", "content": [{"type": "text", "text": "ok"}]}]
        });
        let original = body.clone();

        inject_prompt_cache(&mut body, &CacheInjectionConfig::disabled());

        assert_eq!(body, original);
    }

    #[test]
    fn assistant_injection_skips_thinking_blocks() {
        let mut body = json!({
            "model": "test",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm"},
                    {"type": "text", "text": "result"},
                    {"type": "redacted_thinking", "data": "xxx"}
                ]}
            ]
        });

        inject_prompt_cache(&mut body, &config("1h"));

        assert!(body["messages"][0]["content"][1]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][0]["content"][2]
            .get("cache_control")
            .is_none());
    }
}
