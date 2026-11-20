//! Zen-only request-local aliases. Client tool declarations and arguments stay
//! intact; historical-only names never acquire response/declaration authority.
use super::{alias_history_tool_calls, retarget_tool_choice, tool_name};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn reviewed_name(name: &str) -> bool {
    // Also covers the existing namespace/collision spelling `ns__name__hash`.
    // Apply independently of the other tool's presence so deferred tools and
    // resumed conversations use the same translation.
    name.split("__")
        .any(|part| matches!(part, "spawn_agent" | "wait_agent"))
}

fn candidate(name: &str, attempt: u64) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cc-switch/zen/tool-name/v1\0");
    hash.update(name.as_bytes());
    hash.update(attempt.to_be_bytes());
    format!("zen_fn_{}", hex::encode(&hash.finalize()[..16]))
}

fn declaration_name(tool: &Value) -> Option<&str> {
    // Native Responses namespaces are identity containers, not functions.
    if tool["type"] == "namespace" {
        None
    } else {
        tool_name(tool)
    }
}

pub(super) fn apply(body: &mut Value) -> BTreeMap<String, String> {
    let declared: BTreeSet<String> = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(declaration_name)
        .map(str::to_owned)
        .collect();
    let mut names = declared.clone();
    if let Some(input) = body.get("input").and_then(Value::as_array) {
        for item in input {
            if matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            ) {
                if let Some(name) = item["name"]
                    .as_str()
                    .filter(|_| item["namespace"].as_str().is_none())
                {
                    names.insert(name.to_owned());
                }
            }
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(name) = message
                .pointer("/function_call/name")
                .and_then(Value::as_str)
            {
                names.insert(name.to_owned());
            }
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                        names.insert(name.to_owned());
                    }
                }
            }
            if let Some(parts) = message.get("content").and_then(Value::as_array) {
                for part in parts {
                    if part["type"] == "tool_use" {
                        if let Some(name) = part["name"].as_str() {
                            names.insert(name.to_owned());
                        }
                    }
                }
            }
        }
    }
    let mut taken = names.clone();
    let mut aliases = BTreeMap::new();
    for name in names.iter().filter(|name| reviewed_name(name)) {
        let mut attempt = 0;
        let alias = loop {
            let alias = candidate(name, attempt);
            if taken.insert(alias.clone()) {
                break alias;
            }
            attempt += 1;
        };
        aliases.insert(alias, name.clone());
    }
    if aliases.is_empty() {
        return aliases;
    }
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let Some(alias) =
                declaration_name(tool).and_then(|name| super::alias_for(&aliases, name))
            else {
                continue;
            };
            if let Some(function) = tool.get_mut("function") {
                function["name"] = alias.into();
            } else {
                tool["name"] = alias.into();
            }
        }
    }
    if let Some(map) = body.as_object_mut() {
        retarget_tool_choice(map, &aliases);
        alias_history_tool_calls(map, &aliases);
    }
    aliases.retain(|_, name| declared.contains(name));
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::opencode_zen::{
        apply_fingerprint_style, restore_tool_names, FingerprintStyle,
    };
    use serde_json::json;

    const SPAWN: &str = "collaboration__spawn_agent";
    const WAIT: &str = "collaboration__wait_agent";

    fn declaration(name: &str, style: FingerprintStyle) -> Value {
        let schema = json!({"type":"object","properties":{"name":{"type":"string","description":SPAWN}},"required":["name"]});
        match style {
            FingerprintStyle::Chat => {
                json!({"type":"function","function":{"name":name,"description":"retained","parameters":schema,"strict":false}})
            }
            FingerprintStyle::Flat => {
                json!({"type":"function","name":name,"description":"retained","parameters":schema,"strict":false})
            }
            FingerprintStyle::Claude => {
                json!({"name":name,"description":"retained","input_schema":schema})
            }
        }
    }

    #[test]
    fn zen_multiagent_aliases_preserve_all_three_tool_wires_and_history() {
        for style in [
            FingerprintStyle::Chat,
            FingerprintStyle::Flat,
            FingerprintStyle::Claude,
        ] {
            let opaque = json!({"name":SPAWN,"nested":{"type":"tool_use","name":WAIT}});
            let mut body = json!({
                "tools":[declaration(SPAWN,style),declaration(WAIT,style)],
                "tool_choice": if style == FingerprintStyle::Chat { json!({"type":"function","function":{"name":SPAWN}}) } else { json!({"type":"tool","name":SPAWN}) },
                "messages":[{"role":"assistant","tool_calls":[{"id":"call_keep","type":"function","function":{"name":SPAWN,"arguments":opaque.to_string()}}],
                    "content":[{"type":"tool_use","id":"use_keep","name":WAIT,"input":opaque}]},
                    {"role":"tool","tool_call_id":"call_keep","content":opaque.to_string()}],
                "input":[{"type":"function_call","call_id":"flat_keep","name":SPAWN,"arguments":opaque.to_string()},
                    {"type":"function_call_output","call_id":"flat_keep","output":opaque}]
            });
            let before = body.clone();
            let aliases = apply_fingerprint_style(&mut body, style);
            let assistant = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "assistant")
                .unwrap();
            let before_assistant = &before["messages"][0];
            let tool_result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "tool")
                .unwrap();
            let before_tool_result = &before["messages"][1];
            assert_eq!(aliases.len(), 2);
            let spawn = super::super::alias_for(&aliases, SPAWN).unwrap();
            let wait = super::super::alias_for(&aliases, WAIT).unwrap();
            assert!(spawn.len() <= 64 && wait.len() <= 64);
            assert!(!spawn.contains("spawn_agent") && !wait.contains("wait_agent"));
            assert_eq!(assistant["tool_calls"][0]["function"]["name"], spawn);
            assert_eq!(assistant["content"][0]["name"], wait);
            assert_eq!(body["input"][0]["name"], spawn);
            assert_eq!(
                assistant["tool_calls"][0]["function"]["arguments"],
                before_assistant["tool_calls"][0]["function"]["arguments"]
            );
            assert_eq!(assistant["content"][0]["input"], opaque);
            assert_eq!(tool_result, before_tool_result);
            assert_eq!(body["input"][1], before["input"][1]);
            let choice = if style == FingerprintStyle::Chat {
                &body["tool_choice"]["function"]["name"]
            } else {
                &body["tool_choice"]["name"]
            };
            assert_eq!(choice, &spawn);
            for (index, alias) in [(0, &spawn), (1, &wait)] {
                let tool = &body["tools"][index];
                let mut holder = tool.get("function").unwrap_or(tool).clone();
                holder["name"] = before["tools"][index]
                    .get("function")
                    .unwrap_or(&before["tools"][index])["name"]
                    .clone();
                assert_eq!(
                    &holder,
                    before["tools"][index]
                        .get("function")
                        .unwrap_or(&before["tools"][index])
                );
                assert_eq!(tool_name(tool), Some(alias.as_str()));
            }
            let mut result = json!({"choices":[{"message":{"tool_calls":[{"id":"call_keep","function":{"name":spawn,"arguments":opaque.to_string()}}]}}],
                "output":[{"type":"function_call","call_id":"flat_keep","name":spawn,"arguments":opaque.to_string()}],
                "content":[{"type":"tool_use","id":"use_keep","name":wait,"input":opaque}]});
            restore_tool_names(&mut result, &aliases);
            assert_eq!(
                result["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
                SPAWN
            );
            assert_eq!(result["output"][0]["name"], SPAWN);
            assert_eq!(result["content"][0]["name"], WAIT);
            assert_eq!(result["content"][0]["input"], opaque);
        }
    }

    #[test]
    fn zen_tool_aliases_are_collision_safe_order_independent_and_cover_retired_history() {
        let collision = candidate(SPAWN, 0);
        let tools = vec![
            declaration(SPAWN, FingerprintStyle::Chat),
            declaration(WAIT, FingerprintStyle::Chat),
            declaration(&collision, FingerprintStyle::Chat),
        ];
        let mut first = json!({"tools":tools});
        let mut reordered = first.clone();
        reordered["tools"].as_array_mut().unwrap().reverse();
        let aliases = apply(&mut first);
        assert_eq!(aliases, apply(&mut reordered));
        let alias = super::super::alias_for(&aliases, SPAWN).unwrap();
        assert_ne!(alias, collision);
        assert_eq!(first["tools"][2]["function"]["name"], collision);
        let mut history = json!({"messages":[{"role":"assistant","tool_calls":[{"id":"old","function":{"name":SPAWN,"arguments":"{}"}}]}]});
        assert!(apply(&mut history).is_empty());
        let name = history["messages"][0]["tool_calls"][0]["function"]["name"]
            .as_str()
            .unwrap();
        assert_eq!(name, candidate(SPAWN, 0));
        // Plain names and collision-qualified namespace identities are covered.
        for name in [
            "spawn_agent",
            "wait_agent",
            "ns__spawn_agent__a012",
            SPAWN,
            WAIT,
        ] {
            assert!(reviewed_name(name));
        }
        for name in [
            "spawn_agent_result",
            "wait_agent_log",
            "functions__exec",
            "read",
            "shell",
        ] {
            assert!(!reviewed_name(name));
        }
    }
    #[test]
    fn zen_aliases_do_not_reinterpret_native_namespace_containers_or_selectors() {
        let mut body = json!({"tools":[{"type":"namespace","name":"spawn_agent","tools":[{"type":"function","name":"wait_agent"}]}],
            "tool_choice":{"type":"function","namespace":"spawn_agent","name":"wait_agent"},
            "input":[{"type":"function_call","namespace":"spawn_agent","name":"wait_agent","call_id":"native"}]});
        let original = body.clone();
        assert!(apply(&mut body).is_empty());
        assert_eq!(body, original);
        body["tools"]
            .as_array_mut()
            .unwrap()
            .push(declaration("wait_agent", FingerprintStyle::Flat));
        let aliases = apply(&mut body);
        assert_eq!(aliases.len(), 1);
        assert_eq!(body["tool_choice"], original["tool_choice"]);
        assert_eq!(body["input"], original["input"]);
        let mut response = json!({"output":[{"type":"function_call","namespace":"spawn_agent","name":aliases.keys().next().unwrap()}]});
        let unchanged = response.clone();
        restore_tool_names(&mut response, &aliases);
        assert_eq!(response, unchanged);
    }
}
