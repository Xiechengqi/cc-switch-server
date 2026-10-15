use std::collections::{BTreeMap, BTreeSet, VecDeque};

use bytes::Bytes;
use serde_json::Value;

use super::ProxyError;

const MAX_ENTRIES: usize = 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 64 * 1024;
const TTL_MS: i64 = 10 * 60 * 1_000;
const MAX_MESSAGE_ID_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ClaudeThreadAliasScope {
    pub share_id: String,
    pub principal: String,
    pub session_id: String,
    pub provider_id: String,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub account_id: String,
    pub account_incarnation: u64,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
    pub credential_fingerprint: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    scope: ClaudeThreadAliasScope,
    message_id: String,
}

impl Key {
    fn retained_bytes(&self) -> usize {
        let text_bytes = [
            &self.scope.share_id,
            &self.scope.principal,
            &self.scope.session_id,
            &self.scope.provider_id,
            &self.scope.runtime_fingerprint,
            &self.scope.account_id,
            &self.message_id,
        ]
        .iter()
        .fold(0usize, |bytes, text| bytes.saturating_add(text.capacity()));
        // The map and FIFO queue each own a copy of the key.
        text_bytes
            .saturating_add(std::mem::size_of::<Key>())
            .saturating_mul(2)
    }
}

#[derive(Debug)]
struct Entry {
    aliases: BTreeMap<String, String>,
    conflicted: bool,
    bytes: usize,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaudeThreadAliasLookup {
    Missing,
    Ready(BTreeMap<String, String>),
    Conflicted,
}

#[derive(Debug, Default)]
pub(crate) struct ClaudeThreadAliasCache {
    entries: BTreeMap<Key, Entry>,
    order: VecDeque<Key>,
    bytes: usize,
}

impl ClaudeThreadAliasCache {
    pub(crate) fn lookup(
        &mut self,
        scope: &ClaudeThreadAliasScope,
        message_id: &str,
        now_ms: i64,
    ) -> ClaudeThreadAliasLookup {
        self.expire(now_ms);
        let Some(message_id) = valid_message_id(message_id) else {
            return ClaudeThreadAliasLookup::Missing;
        };
        let key = Key {
            scope: scope.clone(),
            message_id: message_id.to_string(),
        };
        match self.entries.get(&key) {
            Some(entry) if entry.conflicted => ClaudeThreadAliasLookup::Conflicted,
            Some(entry) => ClaudeThreadAliasLookup::Ready(entry.aliases.clone()),
            None => ClaudeThreadAliasLookup::Missing,
        }
    }

    #[cfg(test)]
    fn load(
        &mut self,
        scope: &ClaudeThreadAliasScope,
        message_id: &str,
        now_ms: i64,
    ) -> Option<BTreeMap<String, String>> {
        match self.lookup(scope, message_id, now_ms) {
            ClaudeThreadAliasLookup::Ready(aliases) => Some(aliases),
            ClaudeThreadAliasLookup::Missing | ClaudeThreadAliasLookup::Conflicted => None,
        }
    }

    pub(crate) fn save(
        &mut self,
        scope: ClaudeThreadAliasScope,
        message_id: &str,
        aliases: BTreeMap<String, String>,
        now_ms: i64,
    ) -> bool {
        self.expire(now_ms);
        let Some(message_id) = valid_message_id(message_id) else {
            return false;
        };
        let key = Key {
            scope,
            message_id: message_id.to_string(),
        };
        if let Some(existing) = self.entries.get_mut(&key) {
            if existing.conflicted {
                return false;
            }
            if existing.aliases != aliases {
                existing.conflicted = true;
                return false;
            }
        }
        let bytes = aliases.iter().fold(
            key.retained_bytes()
                .saturating_add(std::mem::size_of::<Entry>()),
            |total, (wire, original)| {
                total
                    .saturating_add(wire.len())
                    .saturating_add(original.len())
                    .saturating_add(std::mem::size_of::<(String, String)>())
            },
        );
        if aliases.len() > 128 || bytes > MAX_ENTRY_BYTES {
            return false;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            self.order.retain(|existing| existing != &key);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key.clone(),
            Entry {
                aliases,
                conflicted: false,
                bytes,
                expires_at_ms: now_ms.saturating_add(TTL_MS),
            },
        );
        self.order.push_back(key);
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
        true
    }

    fn expire(&mut self, now_ms: i64) {
        self.order.retain(|key| {
            if self
                .entries
                .get(key)
                .is_some_and(|entry| entry.expires_at_ms <= now_ms)
            {
                if let Some(entry) = self.entries.remove(key) {
                    self.bytes = self.bytes.saturating_sub(entry.bytes);
                }
                false
            } else {
                true
            }
        });
    }
}

fn valid_message_id(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= MAX_MESSAGE_ID_BYTES
        && !value.chars().any(char::is_control))
    .then_some(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaudeThreadMode {
    Create,
    Continue {
        previous_message_id: String,
        needs_state: bool,
    },
}

pub(crate) fn thread_mode(body: &[u8]) -> Result<Option<ClaudeThreadMode>, ProxyError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| ProxyError::bad_request("invalid Claude thread request body"))?;
    thread_mode_from_value(&value)
}

pub(crate) fn thread_mode_from_value(
    value: &Value,
) -> Result<Option<ClaudeThreadMode>, ProxyError> {
    match value.pointer("/thread/type").and_then(Value::as_str) {
        Some("create") => Ok(Some(ClaudeThreadMode::Create)),
        Some("continue") => {
            let previous_message_id = value
                .pointer("/thread/previous_message_id")
                .and_then(Value::as_str)
                .and_then(valid_message_id)
                .ok_or_else(|| {
                    ProxyError::bad_request(
                        "Claude thread continuation requires a valid previous_message_id",
                    )
                })?
                .to_string();
            let needs_state = needs_prior_alias_state(value)?;
            Ok(Some(ClaudeThreadMode::Continue {
                previous_message_id,
                needs_state,
            }))
        }
        _ => Ok(None),
    }
}

fn needs_prior_alias_state(value: &Value) -> Result<bool, ProxyError> {
    let Some(tools) = value.get("tools").and_then(Value::as_array) else {
        return Ok(true);
    };
    if tools.is_empty() {
        return Ok(true);
    }
    if tools.len() > 128 {
        return Err(ProxyError::bad_request(
            "Claude custom tool alias map exceeds 128 declarations",
        ));
    }
    let declared = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    let needs_alias = |name: Option<&str>| {
        name.is_some_and(|name| {
            super::claude_oauth::claude_tool_name_needs_prior_alias(name)
                && !declared.contains(&name.to_ascii_lowercase())
        })
    };
    if needs_alias(value.pointer("/tool_choice/name").and_then(Value::as_str)) {
        return Ok(true);
    }
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return Ok(false);
    };
    for block in messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use" | "server_tool_use")
                if needs_alias(block.get("name").and_then(Value::as_str)) =>
            {
                return Ok(true);
            }
            Some("tool_reference")
                if needs_alias(block.get("tool_name").and_then(Value::as_str)) =>
            {
                return Ok(true);
            }
            Some("tool_result" | "tool_search_tool_result") => {
                let references = if block.get("type").and_then(Value::as_str) == Some("tool_result")
                {
                    block.get("content").and_then(Value::as_array)
                } else {
                    block
                        .pointer("/content/tool_references")
                        .and_then(Value::as_array)
                };
                if references.is_some_and(|references| {
                    references.iter().any(|reference| {
                        reference.get("type").and_then(Value::as_str) == Some("tool_reference")
                            && needs_alias(reference.get("tool_name").and_then(Value::as_str))
                    })
                }) {
                    return Ok(true);
                }
            }
            Some("tool_addition" | "tool_removal")
                if needs_alias(super::claude_oauth::claude_tool_change_alias_target(block)) =>
            {
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}

pub(crate) fn restore_request_names(
    body: &mut Bytes,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ProxyError> {
    if aliases.is_empty() {
        return Ok(());
    }
    let mut value: Value = serde_json::from_slice(body)
        .map_err(|_| ProxyError::bad_request("invalid Claude thread continuation body"))?;
    let names = aliases
        .iter()
        .map(|(wire, original)| (original.to_ascii_lowercase(), wire.clone()))
        .collect::<BTreeMap<_, _>>();
    if let Some(choice) = value.get_mut("tool_choice") {
        rewrite_name(choice, "name", &names);
    }
    if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use" | "server_tool_use") => rewrite_name(block, "name", &names),
                        Some("tool_reference") => rewrite_name(block, "tool_name", &names),
                        _ => {}
                    }
                    super::claude_oauth::rewrite_claude_nested_tool_references(block, &names);
                    super::claude_oauth::rewrite_claude_tool_change_name(block, &names);
                }
            }
        }
    }
    *body =
        Bytes::from(serde_json::to_vec(&value).map_err(|_| {
            ProxyError::bad_request("cannot encode Claude thread continuation body")
        })?);
    Ok(())
}

pub(crate) fn merge_alias_maps(
    mut previous: BTreeMap<String, String>,
    current: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ProxyError> {
    let mut names = previous
        .iter()
        .map(|(wire, original)| (original.to_ascii_lowercase(), wire.clone()))
        .collect::<BTreeMap<_, _>>();
    for (wire, original) in current {
        if previous
            .get(&wire)
            .is_some_and(|existing| existing != &original)
            || names
                .get(&original.to_ascii_lowercase())
                .is_some_and(|existing| existing != &wire)
        {
            return Err(ProxyError::bad_request(
                "Claude thread tool aliases conflict with previous state",
            ));
        }
        names.insert(original.to_ascii_lowercase(), wire.clone());
        previous.insert(wire, original);
        if previous.len() > 128 {
            return Err(ProxyError::bad_request(
                "Claude thread tool aliases exceed 128 declarations",
            ));
        }
    }
    Ok(previous)
}

fn rewrite_name(value: &mut Value, field: &str, names: &BTreeMap<String, String>) {
    let Some(original) = value.get(field).and_then(Value::as_str) else {
        return;
    };
    if let Some(wire) = names.get(&original.to_ascii_lowercase()) {
        value[field] = Value::String(wire.clone());
    }
}

pub(crate) fn response_message_id(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    (value.get("type")?.as_str()? == "message").then(|| {
        value
            .get("id")?
            .as_str()
            .and_then(valid_message_id)
            .map(str::to_string)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(session: &str) -> ClaudeThreadAliasScope {
        ClaudeThreadAliasScope {
            share_id: "share".into(),
            principal: "user@example.com".into(),
            session_id: session.into(),
            provider_id: "provider".into(),
            provider_revision: 1,
            runtime_fingerprint: "runtime".into(),
            account_id: "account".into(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            credential_fingerprint: [1; 32],
        }
    }

    #[test]
    fn claude_thread_alias_cache_is_scoped_and_expires() {
        let mut cache = ClaudeThreadAliasCache::default();
        let aliases = BTreeMap::from([("cc_tool_123".into(), "mcp__bad/name".into())]);
        assert!(cache.save(scope("one"), "msg_1", aliases.clone(), 0));
        assert_eq!(cache.load(&scope("one"), "msg_1", 1), Some(aliases));
        assert_eq!(cache.load(&scope("two"), "msg_1", 1), None);
        let mut changed_provider = scope("one");
        changed_provider.provider_revision += 1;
        assert_eq!(cache.load(&changed_provider, "msg_1", 1), None);
        let mut changed_account = scope("one");
        changed_account.auth_identity_generation += 1;
        assert_eq!(cache.load(&changed_account, "msg_1", 1), None);
        let mut recreated_account = scope("one");
        recreated_account.account_incarnation += 1;
        assert_eq!(cache.load(&recreated_account, "msg_1", 1), None);
        let mut changed_credential = scope("one");
        changed_credential.credential_fingerprint[0] ^= 1;
        assert_eq!(cache.load(&changed_credential, "msg_1", 1), None);
        assert_eq!(cache.load(&scope("one"), "msg_1", TTL_MS), None);
    }

    #[test]
    fn claude_thread_alias_cache_evicts_oldest_message() {
        let mut cache = ClaudeThreadAliasCache::default();
        for index in 0..=MAX_ENTRIES {
            assert!(cache.save(scope("one"), &format!("msg_{index}"), BTreeMap::new(), 0));
        }
        assert_eq!(cache.load(&scope("one"), "msg_0", 1), None);
        assert_eq!(cache.load(&scope("one"), "msg_1", 1), Some(BTreeMap::new()));
    }

    #[test]
    fn claude_thread_alias_conflicting_message_id_never_uses_either_mapping() {
        let mut cache = ClaudeThreadAliasCache::default();
        let first = BTreeMap::from([("cc_tool_first".into(), "mcp__first/name".into())]);
        let second = BTreeMap::from([("cc_tool_second".into(), "mcp__second/name".into())]);
        assert!(cache.save(scope("one"), "msg_reused", first.clone(), 0));
        assert_eq!(cache.load(&scope("one"), "msg_reused", 1), Some(first));
        assert!(!cache.save(scope("one"), "msg_reused", second.clone(), 2));
        assert_eq!(cache.load(&scope("one"), "msg_reused", 3), None);
        assert_eq!(
            cache.lookup(&scope("one"), "msg_reused", 3),
            ClaudeThreadAliasLookup::Conflicted
        );
        assert!(!cache.save(scope("one"), "msg_reused", second, 4));
        assert_eq!(cache.load(&scope("one"), "msg_reused", 5), None);
        assert_eq!(
            cache.lookup(&scope("one"), "msg_reused", TTL_MS),
            ClaudeThreadAliasLookup::Missing
        );
    }

    #[test]
    fn claude_thread_alias_cache_bounds_scope_and_message_key_bytes() {
        let mut cache = ClaudeThreadAliasCache::default();
        let mut oversized = scope("one");
        oversized.session_id = "s".repeat(MAX_ENTRY_BYTES + 1);
        assert!(!cache.save(oversized, "msg_oversized", BTreeMap::new(), 0));
        let mut reserved = scope("one");
        reserved.session_id = String::with_capacity(MAX_ENTRY_BYTES + 1);
        reserved.session_id.push('s');
        assert!(!cache.save(reserved, "msg_reserved", BTreeMap::new(), 0));
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn claude_thread_alias_request_rewrites_history_without_declarations() {
        let mut body = Bytes::from_static(br#"{"thread":{"type":"continue","previous_message_id":"msg_1"},"tool_choice":{"type":"tool","name":"mcp__bad/name"},"messages":[{"role":"assistant","content":[{"type":"tool_use","name":"mcp__bad/name"},{"type":"tool_reference","tool_name":"mcp__bad/name"},{"type":"tool_addition","tool":{"type":"tool_reference","name":"mcp__bad/name"}},{"type":"tool_removal","tool":{"type":"tool_reference","name":"mcp__bad/name"}}]}]}"#);
        assert_eq!(
            thread_mode(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: true
            })
        );
        restore_request_names(
            &mut body,
            &BTreeMap::from([("cc_tool_123".into(), "mcp__bad/name".into())]),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/tool_choice/name").and_then(Value::as_str),
            Some("cc_tool_123")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/0/name")
                .and_then(Value::as_str),
            Some("cc_tool_123")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/1/tool_name")
                .and_then(Value::as_str),
            Some("cc_tool_123")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/2/tool/name")
                .and_then(Value::as_str),
            Some("cc_tool_123")
        );
        assert_eq!(
            value
                .pointer("/messages/0/content/3/tool/name")
                .and_then(Value::as_str),
            Some("cc_tool_123")
        );
    }

    #[test]
    fn claude_thread_alias_request_restores_nested_protocol_references() {
        let original = "mcp__bad/name";
        let mut body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "thread":{"type":"continue","previous_message_id":"msg_1"},
                "messages":[{"role":"user","content":[
                    {"type":"tool_result","content":[
                        {"type":"tool_reference","tool_name":original},
                        {"type":"text","tool_name":original}
                    ]},
                    {"type":"tool_search_tool_result","content":{"tool_references":[
                        {"type":"tool_reference","tool_name":original},
                        {"type":"text","tool_name":original}
                    ]}},
                    {"type":"tool_use","name":original,"input":{"tool_name":original}}
                ]}]
            }))
            .unwrap(),
        );
        restore_request_names(
            &mut body,
            &BTreeMap::from([("cc_tool_123".into(), original.into())]),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        for pointer in [
            "/messages/0/content/0/content/0/tool_name",
            "/messages/0/content/1/content/tool_references/0/tool_name",
            "/messages/0/content/2/name",
        ] {
            assert_eq!(
                value.pointer(pointer).and_then(Value::as_str),
                Some("cc_tool_123")
            );
        }
        for pointer in [
            "/messages/0/content/0/content/1/tool_name",
            "/messages/0/content/1/content/tool_references/1/tool_name",
            "/messages/0/content/2/input/tool_name",
        ] {
            assert_eq!(
                value.pointer(pointer).and_then(Value::as_str),
                Some(original)
            );
        }
    }

    #[test]
    fn claude_thread_alias_partial_declarations_need_prior_state() {
        let mut body = serde_json::json!({
            "thread":{"type":"continue","previous_message_id":"msg_1"},
            "tools":[{"name":"mcp__new/name"}],
            "messages":[{"role":"assistant","content":[
                {"type":"tool_use","name":"mcp__old/name"}
            ]}]
        });
        let expected = Some(ClaudeThreadMode::Continue {
            previous_message_id: "msg_1".into(),
            needs_state: true,
        });
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"tool_result","content":[
                {"type":"tool_reference","tool_name":"mcp__old/name"},
                {"type":"text","tool_name":"mcp__old/name"}
            ]
        });
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body["tools"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name":"mcp__old/name"
            }));
        assert_eq!(
            thread_mode_from_value(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: false,
            })
        );
        body["tools"].as_array_mut().unwrap().pop();
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"tool_use","name":"portable_old"
        });
        assert_eq!(
            thread_mode_from_value(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: false,
            })
        );
        body["messages"][0]["content"][0]["name"] = serde_json::json!("read");
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"text","tool_name":"mcp__old/name"
        });
        assert_eq!(
            thread_mode_from_value(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: false,
            })
        );
        body["tools"] = serde_json::json!((0..129)
            .map(|index| serde_json::json!({"name":format!("tool_{index}")}))
            .collect::<Vec<_>>());
        assert_eq!(
            thread_mode_from_value(&body).unwrap_err().status,
            axum::http::StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn claude_thread_alias_partial_choice_and_change_references_need_state() {
        let mut body = serde_json::json!({
            "thread":{"type":"continue","previous_message_id":"msg_1"},
            "tools":[{"name":"mcp__new/name"}],
            "messages":[{"role":"user","content":[{"type":"text","text":"next"}]}],
            "tool_choice":{"type":"tool","name":"mcp__old/name"}
        });
        let expected = Some(ClaudeThreadMode::Continue {
            previous_message_id: "msg_1".into(),
            needs_state: true,
        });
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body.as_object_mut().unwrap().remove("tool_choice");
        for change in ["tool_addition", "tool_removal"] {
            body["messages"][0]["content"][0] = serde_json::json!({
                "type":change,
                "tool":{"type":"tool_reference","name":"mcp__old/name"}
            });
            assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        }
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"server_tool_use","name":"mcp__old/name"
        });
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"tool_addition",
            "tool":{"type":"tool_definition","definition":{
                "type":"custom","name":"mcp__old/name"
            }}
        });
        assert_eq!(thread_mode_from_value(&body).unwrap(), expected);
        body["messages"][0]["content"][0]["tool"]["definition"]["type"] =
            serde_json::json!("web_search_20250305");
        assert_eq!(
            thread_mode_from_value(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: false,
            })
        );
        body["messages"][0]["content"][0] = serde_json::json!({
            "type":"text","text":"mcp__old/name"
        });
        assert_eq!(
            thread_mode_from_value(&body).unwrap(),
            Some(ClaudeThreadMode::Continue {
                previous_message_id: "msg_1".into(),
                needs_state: false,
            })
        );
    }

    #[test]
    fn claude_thread_alias_merge_rejects_wire_and_original_collisions() {
        let previous = BTreeMap::from([("cc_tool_old".into(), "mcp__old/name".into())]);
        let wire_collision = BTreeMap::from([("cc_tool_old".into(), "mcp__new/name".into())]);
        assert_eq!(
            merge_alias_maps(previous.clone(), wire_collision)
                .unwrap_err()
                .status,
            axum::http::StatusCode::BAD_REQUEST
        );
        let original_collision = BTreeMap::from([("cc_tool_new".into(), "MCP__OLD/NAME".into())]);
        assert_eq!(
            merge_alias_maps(previous.clone(), original_collision)
                .unwrap_err()
                .status,
            axum::http::StatusCode::BAD_REQUEST
        );
        let merged = merge_alias_maps(
            previous,
            BTreeMap::from([("cc_tool_new".into(), "mcp__new/name".into())]),
        )
        .unwrap();
        assert_eq!(merged.len(), 2);

        let full = (0..128)
            .map(|index| (format!("wire_{index}"), format!("original_{index}")))
            .collect();
        assert_eq!(
            merge_alias_maps(
                full,
                BTreeMap::from([("wire_129".into(), "original_129".into())]),
            )
            .unwrap_err()
            .status,
            axum::http::StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn claude_thread_alias_rejects_invalid_continuation_id() {
        for body in [
            br#"{"thread":{"type":"continue"}}"#.as_slice(),
            br#"{"thread":{"type":"continue","previous_message_id":""}}"#.as_slice(),
        ] {
            assert_eq!(
                thread_mode(body).unwrap_err().status,
                axum::http::StatusCode::BAD_REQUEST
            );
        }
    }
}
