use std::collections::HashSet;

use serde_json::{json, Value};

const MAX_CACHE_BREAKPOINTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CachePath {
    Tool(usize),
    System(usize),
    Message(usize, usize),
}

pub(crate) fn reconcile_forced_tool_choice(body: &mut Value) {
    let forced = body
        .pointer("/tool_choice/type")
        .and_then(Value::as_str)
        .is_some_and(|kind| matches!(kind, "any" | "tool"));
    if !forced {
        return;
    }
    if let Some(object) = body.as_object_mut() {
        object.remove("thinking");
        object.remove("context_management");
    }
}

pub(crate) fn normalize_anthropic_cache_control(body: &mut Value, inject_defaults: bool) {
    if inject_defaults {
        inject_default_breakpoints(body);
    }
    enforce_cache_control_limit(body, MAX_CACHE_BREAKPOINTS);
    normalize_cache_control_ttls(body);
}

fn inject_default_breakpoints(body: &mut Value) {
    let paths = collect_cache_paths(body);
    if !paths
        .iter()
        .any(|path| matches!(path, CachePath::System(_)))
    {
        if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
            if let Some(block) = system.last_mut().and_then(Value::as_object_mut) {
                block.insert("cache_control".to_string(), ephemeral_cache_control());
            }
        }
    }

    let paths = collect_cache_paths(body);
    let has_system = body
        .get("system")
        .and_then(Value::as_array)
        .is_some_and(|system| !system.is_empty());
    if !has_system && !paths.iter().any(|path| matches!(path, CachePath::Tool(_))) {
        if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
            if let Some(tool) = tools.last_mut().and_then(Value::as_object_mut) {
                tool.insert("cache_control".to_string(), ephemeral_cache_control());
            }
        }
    }

    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut().rev() {
        let role = message.get("role").and_then(Value::as_str);
        if role == Some("assistant") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        if let Some(text) = content.as_str().map(str::to_string) {
            *content = json!([{
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }]);
            return;
        }
        if let Some(blocks) = content.as_array_mut() {
            if let Some(block) = blocks
                .iter_mut()
                .rev()
                .find_map(|block| block.as_object_mut())
            {
                if !block.contains_key("cache_control") {
                    block.insert("cache_control".to_string(), ephemeral_cache_control());
                }
                return;
            }
        }
    }
}

fn ephemeral_cache_control() -> Value {
    json!({"type": "ephemeral"})
}

fn normalize_cache_control_ttls(body: &mut Value) {
    let paths = collect_cache_paths(body);
    let mut seen_five_minute = false;
    for path in paths {
        let Some(cache) = cache_control_mut(body, path) else {
            seen_five_minute = true;
            continue;
        };
        let Some(object) = cache.as_object_mut() else {
            seen_five_minute = true;
            continue;
        };
        if object.get("type").and_then(Value::as_str) != Some("ephemeral") {
            seen_five_minute = true;
            continue;
        }
        match object.get("ttl").and_then(Value::as_str) {
            Some("1h") if !seen_five_minute => {}
            Some("1h") => {
                object.remove("ttl");
                seen_five_minute = true;
            }
            Some("5m") => {
                object.remove("ttl");
                seen_five_minute = true;
            }
            Some(_) => {
                object.remove("ttl");
                seen_five_minute = true;
            }
            None => seen_five_minute = true,
        }
    }
}

fn enforce_cache_control_limit(body: &mut Value, limit: usize) {
    let paths = collect_cache_paths(body);
    if paths.len() <= limit {
        return;
    }

    let last_tool = paths
        .iter()
        .rev()
        .find(|path| matches!(path, CachePath::Tool(_)))
        .copied();
    let last_system = paths
        .iter()
        .rev()
        .find(|path| matches!(path, CachePath::System(_)))
        .copied();
    let mut keep = Vec::with_capacity(limit);
    if let Some(path) = last_tool {
        keep.push(path);
    }
    if let Some(path) = last_system.filter(|path| !keep.contains(path)) {
        keep.push(path);
    }
    for path in paths.iter().rev().copied() {
        if keep.len() >= limit {
            break;
        }
        if matches!(path, CachePath::Message(_, _)) && !keep.contains(&path) {
            keep.push(path);
        }
    }
    for path in paths.iter().rev().copied() {
        if keep.len() >= limit {
            break;
        }
        if !keep.contains(&path) {
            keep.push(path);
        }
    }
    let keep = keep.into_iter().collect::<HashSet<_>>();
    for path in paths {
        if !keep.contains(&path) {
            remove_cache_control(body, path);
        }
    }
}

fn collect_cache_paths(body: &Value) -> Vec<CachePath> {
    let mut paths = Vec::new();
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for (index, tool) in tools.iter().enumerate() {
            if tool.get("cache_control").is_some() {
                paths.push(CachePath::Tool(index));
            }
        }
    }
    if let Some(system) = body.get("system").and_then(Value::as_array) {
        for (index, block) in system.iter().enumerate() {
            if block.get("cache_control").is_some() {
                paths.push(CachePath::System(index));
            }
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for (message_index, message) in messages.iter().enumerate() {
            if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                for (block_index, block) in blocks.iter().enumerate() {
                    if block.get("cache_control").is_some() {
                        paths.push(CachePath::Message(message_index, block_index));
                    }
                }
            }
        }
    }
    paths
}

fn cache_control_mut(body: &mut Value, path: CachePath) -> Option<&mut Value> {
    match path {
        CachePath::Tool(index) => body
            .get_mut("tools")?
            .as_array_mut()?
            .get_mut(index)?
            .get_mut("cache_control"),
        CachePath::System(index) => body
            .get_mut("system")?
            .as_array_mut()?
            .get_mut(index)?
            .get_mut("cache_control"),
        CachePath::Message(message, block) => body
            .get_mut("messages")?
            .as_array_mut()?
            .get_mut(message)?
            .get_mut("content")?
            .as_array_mut()?
            .get_mut(block)?
            .get_mut("cache_control"),
    }
}

fn remove_cache_control(body: &mut Value, path: CachePath) {
    let block = match path {
        CachePath::Tool(index) => body
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .and_then(|items| items.get_mut(index)),
        CachePath::System(index) => body
            .get_mut("system")
            .and_then(Value::as_array_mut)
            .and_then(|items| items.get_mut(index)),
        CachePath::Message(message, block) => body
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .and_then(|items| items.get_mut(message))
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
            .and_then(|items| items.get_mut(block)),
    };
    if let Some(block) = block.and_then(Value::as_object_mut) {
        block.remove("cache_control");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(body: &Value) -> usize {
        collect_cache_paths(body).len()
    }

    #[test]
    fn preserves_high_value_breakpoints_and_caps_at_four() {
        let mut body = json!({
            "tools": [
                {"name":"a","cache_control":{"type":"ephemeral","ttl":"1h"}},
                {"name":"b","cache_control":{"type":"ephemeral","ttl":"1h"}}
            ],
            "system": [
                {"type":"text","text":"a","cache_control":{"type":"ephemeral"}},
                {"type":"text","text":"b","cache_control":{"type":"ephemeral"}}
            ],
            "messages": [
                {"role":"user","content":[{"type":"text","text":"old","cache_control":{"type":"ephemeral"}}]},
                {"role":"user","content":[{"type":"text","text":"new","cache_control":{"type":"ephemeral"}}]}
            ]
        });
        normalize_anthropic_cache_control(&mut body, false);
        assert_eq!(count(&body), 4);
        assert!(body.pointer("/tools/1/cache_control").is_some());
        assert!(body.pointer("/system/1/cache_control").is_some());
        assert!(body
            .pointer("/messages/1/content/0/cache_control")
            .is_some());
    }

    #[test]
    fn downgrades_one_hour_marker_after_five_minute_marker() {
        let mut body = json!({
            "system": [
                {"type":"text","text":"a","cache_control":{"type":"ephemeral"}},
                {"type":"text","text":"b","cache_control":{"type":"ephemeral","ttl":"1h"}}
            ]
        });
        normalize_anthropic_cache_control(&mut body, false);
        assert!(body.pointer("/system/1/cache_control/ttl").is_none());
    }

    #[test]
    fn injects_rolling_message_breakpoint_and_is_idempotent() {
        let mut body = json!({
            "system": [{"type":"text","text":"system"}],
            "messages": [
                {"role":"user","content":[{"type":"text","text":"old"}]},
                {"role":"assistant","content":[{"type":"text","text":"answer"}]},
                {"role":"user","content":[{"type":"text","text":"new"}]}
            ]
        });
        normalize_anthropic_cache_control(&mut body, true);
        let once = body.clone();
        normalize_anthropic_cache_control(&mut body, true);
        assert_eq!(body, once);
        assert!(body.pointer("/system/0/cache_control").is_some());
        assert!(body
            .pointer("/messages/2/content/0/cache_control")
            .is_some());
    }

    #[test]
    fn reanchors_rolling_breakpoint_when_an_older_message_is_already_cached() {
        let mut body = json!({
            "system": [{"type":"text","text":"system"}],
            "messages": [
                {"role":"user","content":[{"type":"text","text":"old","cache_control":{"type":"ephemeral"}}]},
                {"role":"assistant","content":"answer"},
                {"role":"user","content":"new"}
            ]
        });
        normalize_anthropic_cache_control(&mut body, true);
        let once = body.clone();
        normalize_anthropic_cache_control(&mut body, true);
        assert_eq!(body, once);
        assert_eq!(
            body.pointer("/messages/2/content/0/text")
                .and_then(Value::as_str),
            Some("new")
        );
        assert!(body
            .pointer("/messages/2/content/0/cache_control")
            .is_some());
    }

    #[test]
    fn forced_tool_choice_removes_incompatible_thinking_state() {
        let mut body = json!({
            "tool_choice":{"type":"tool","name":"Read"},
            "thinking":{"type":"adaptive"},
            "context_management":{"edits":[]}
        });
        reconcile_forced_tool_choice(&mut body);
        assert!(body.get("thinking").is_none());
        assert!(body.get("context_management").is_none());
    }
}
