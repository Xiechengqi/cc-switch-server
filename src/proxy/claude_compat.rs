//! Claude OAuth-only history repairs. Never fabricate signatures or tool results.
use super::ProxyError;
use serde_json::{json, Value};

fn is_unsigned_blank_thinking(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("thinking")
        && block
            .get("thinking")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty())
        && block.get("signature").is_none_or(|signature| {
            signature.is_null()
                || signature
                    .as_str()
                    .is_some_and(|value| value.trim().is_empty())
        })
}

/// Remove foreign, unsigned empty reasoning placeholders at the official OAuth
/// boundary only. Native redaction can legitimately carry empty text with an
/// opaque signature; neither those blocks nor nested business data are touched.
/// Validate every affected turn before editing so failure is atomic and cannot
/// silently discard per-turn controls, assistant prefill or tool boundaries.
pub(crate) fn strip_unsigned_blank_thinking(body: &mut Value) -> Result<usize, ProxyError> {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Ok(0);
    };
    let mut removed = 0;
    for (index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        let count = blocks
            .iter()
            .filter(|block| is_unsigned_blank_thinking(block))
            .count();
        if count > 0 && count == blocks.len() {
            return Err(ProxyError::bad_request(format!(
                "Claude OAuth messages.{index}.content would be empty after removing unsigned blank thinking; remove the empty assistant turn or rebuild the session history"
            )));
        }
        removed += count;
    }
    if removed == 0 {
        return Ok(0);
    }
    for message in body["messages"].as_array_mut().expect("validated messages") {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
            blocks.retain(|block| !is_unsigned_blank_thinking(block));
        }
    }
    Ok(removed)
}

pub(crate) fn apply_preserved_thinking_binding(body: &mut Value) -> bool {
    let supported = body
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(crate::domain::claude_models::supports_preserved_thinking);
    if !supported
        || matches!(
            body.pointer("/tool_choice/type").and_then(Value::as_str),
            Some("any" | "tool")
        )
    {
        return false;
    }
    if let Some(thinking) = body.get("thinking") {
        if !matches!(
            thinking.get("type").and_then(Value::as_str),
            Some("adaptive" | "enabled")
        ) || thinking
            .pointer("/block_binding/prefix_mismatch_behavior")
            .is_some()
            || thinking
                .get("block_binding")
                .is_some_and(|value| !value.is_object())
        {
            return false;
        }
    }
    let signed = body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .any(|block| match block.get("type").and_then(Value::as_str) {
            Some("thinking") => block
                .get("signature")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty()),
            Some("redacted_thinking") => block
                .get("data")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty()),
            _ => false,
        });
    if !signed {
        return false;
    }
    if body.get("thinking").is_none() {
        body["thinking"] = json!({"type":"adaptive"});
    }
    body["thinking"]
        .as_object_mut()
        .expect("validated thinking")
        .entry("block_binding")
        .or_insert_with(|| json!({}))["prefix_mismatch_behavior"] = json!("drop_block");
    true
}

/// Drop only search citations whose source is being removed or is empty.
/// A real user turn ends the scope; tool-result turns and document citations do not.
pub(crate) fn strip_orphaned_search_citations(body: &mut Value, removing_results: bool) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut missing = false;
    let mut changed = false;
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("user")
            && message
                .get("content")
                .and_then(Value::as_array)
                .is_none_or(|blocks| {
                    blocks.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) != Some("tool_result")
                    })
                })
        {
            missing = false;
        }
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        missing |= blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("web_search_tool_result")
                && (removing_results
                    || block.get("content").is_none_or(|content| {
                        content.is_null()
                            || content.as_array().is_some_and(Vec::is_empty)
                            || content.as_str() == Some("")
                    }))
        });
        if !missing {
            continue;
        }
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            if let Some(citations) = block.get_mut("citations").and_then(Value::as_array_mut) {
                let before = citations.len();
                citations.retain(|citation| {
                    citation.get("type").and_then(Value::as_str)
                        != Some("web_search_result_location")
                });
                changed |= before != citations.len();
                if citations.is_empty() {
                    block
                        .as_object_mut()
                        .expect("text block")
                        .remove("citations");
                }
            }
        }
    }
    changed
}

fn rejected_schema_key(schema: &Value, depth: usize) -> bool {
    if depth > 64 {
        return true;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        if properties.keys().any(|name| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'.' | b'-'))
        }) {
            return true;
        }
    }
    // Traverse schema locations only, not enum/default/examples or business values.
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
        "dependencies",
    ] {
        if object
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|map| {
                map.values()
                    .any(|value| rejected_schema_key(value, depth + 1))
            })
        {
            return true;
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "additionalItems",
        "unevaluatedItems",
        "contentSchema",
        "unevaluatedProperties",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
    ] {
        if let Some(value) = object.get(key) {
            if rejected_schema_key(value, depth + 1)
                || value
                    .as_array()
                    .is_some_and(|values| values.iter().any(|v| rejected_schema_key(v, depth + 1)))
            {
                return true;
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if object
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(|v| rejected_schema_key(v, depth + 1)))
        {
            return true;
        }
    }
    false
}

fn validate_tool(tool: &mut Value, fallback: bool) -> Result<(), ProxyError> {
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(super::claude_oauth::claude_server_tool_type)
    {
        return Ok(());
    }
    let Some(schema) = tool.get("input_schema") else {
        return Ok(());
    };
    if !rejected_schema_key(schema, 0) {
        return Ok(());
    }
    if !fallback || tool.get("strict") == Some(&Value::Bool(true)) {
        return Err(ProxyError::bad_request("Claude tool schema contains an unsupported property name or exceeds schema depth; strict schemas are never silently weakened"));
    }
    let description = format!(
        "{}\n\nOriginal parameter schema (preserve its property names and constraints):\n{}",
        tool.get("description")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        schema
    );
    tool["description"] = json!(description);
    tool["input_schema"] = json!({"type":"object","additionalProperties":true});
    Ok(())
}

pub(crate) fn validate_tool_schemas(body: &mut Value) -> Result<(), ProxyError> {
    let fallback = std::env::var("CC_SWITCH_CLAUDE_SCHEMA_DESCRIPTION_FALLBACK")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    validate_tool_schemas_with_policy(body, fallback)
}

fn validate_tool_schemas_with_policy(body: &mut Value, fallback: bool) -> Result<(), ProxyError> {
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            validate_tool(tool, fallback)?;
        }
    }
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_addition")
                        && block.pointer("/tool/type").and_then(Value::as_str)
                            == Some("tool_definition")
                    {
                        if let Some(tool) = block.pointer_mut("/tool/definition") {
                            validate_tool(tool, fallback)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_blank_thinking_repair_is_narrow_atomic_and_idempotent() {
        for text in ["", " ", "\t\r\n", "\u{a0}\u{2003}\u{3000}"] {
            for signature in [
                None,
                Some(Value::Null),
                Some(json!("")),
                Some(json!(" \t\n")),
            ] {
                let mut placeholder = json!({"type":"thinking","thinking":text});
                if let Some(signature) = signature {
                    placeholder["signature"] = signature;
                }
                let retained = json!([
                    {"type":"thinking","thinking":"","signature":"opaque-future-native"},
                    {"type":"thinking","thinking":" \t\n","signature":"CAQS-synthetic"},
                    {"type":"thinking","thinking":" nonblank "},
                    {"type":"thinking","thinking":"\u{200b}","signature":""},
                    {"type":"thinking","thinking":" ","signature":false},
                    {"type":"redacted_thinking","data":"opaque-data"},
                    {"type":"tool_use","id":"toolu_fixture","name":"Read","input":{"nested":placeholder}},
                    {"type":"text","text":"visible"},
                    {"type":"future_content","thinking":" ","signature":""}
                ]);
                let mut content = retained.as_array().unwrap().clone();
                content.insert(0, placeholder.clone());
                let mut body = json!({"thinking":{"type":"adaptive","display":"updates"},"messages":[
                    {"role":"assistant","output_config":{"effort":"low"},"content":content},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_fixture","content":[placeholder.clone()]}]},
                    {"role":"system","content":[placeholder]}
                ]});
                let mut expected = body.clone();
                expected["messages"][0]["content"] = retained;
                assert_eq!(strip_unsigned_blank_thinking(&mut body).unwrap(), 1);
                assert_eq!(body, expected);
                assert_eq!(strip_unsigned_blank_thinking(&mut body).unwrap(), 0);
                assert_eq!(body, expected);

                // A later unrepairable turn must prevent earlier edits too.
                body["messages"][0]["content"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, json!({"type":"thinking","thinking":" ","signature":""}));
                body["messages"].as_array_mut().unwrap().push(json!({
                    "role":"assistant","output_config":{"effort":"high"},
                    "content":[{"type":"thinking","thinking":" ","signature":""}]
                }));
                let original = body.clone();
                let error = strip_unsigned_blank_thinking(&mut body).unwrap_err();
                assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
                assert!(error.message.contains("messages.3.content"));
                assert_eq!(body, original);
            }
        }
        for mut body in [
            json!({}),
            json!({"messages":null}),
            json!({"messages":[
                {"role":"assistant","content":"ordinary text"},
                {"role":"assistant","content":[]}
            ]}),
        ] {
            let original = body.clone();
            assert_eq!(strip_unsigned_blank_thinking(&mut body).unwrap(), 0);
            assert_eq!(body, original);
        }
    }

    #[test]
    fn fable52_binding_is_narrow_and_respects_explicit_policy() {
        let original = json!({"model":"claude-fable-5-2", "messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"opaque","signature":"CAQSopaque"}]}]});
        let mut body = original.clone();
        assert!(apply_preserved_thinking_binding(&mut body));
        assert_eq!(
            body.pointer("/thinking/block_binding/prefix_mismatch_behavior"),
            Some(&json!("drop_block"))
        );
        assert_eq!(body["messages"], original["messages"]);
        for patch in [
            json!({"thinking":{"type":"adaptive","block_binding":{"prefix_mismatch_behavior":"error"}}}),
            json!({"thinking":{"type":"disabled"}}),
            json!({"tool_choice":{"type":"any"}}),
            json!({"model":"unknown"}),
        ] {
            let mut body = original.clone();
            for (key, value) in patch.as_object().unwrap() {
                body[key] = value.clone();
            }
            let before = body.clone();
            assert!(!apply_preserved_thinking_binding(&mut body));
            assert_eq!(body, before);
        }
    }
    #[test]
    fn search_repair_preserves_valid_results_and_non_search_citations() {
        let mut body = json!({"messages":[{"role":"assistant","content":[{"type":"web_search_tool_result","content":[]},{"type":"text","citations":[{"type":"web_search_result_location"},{"type":"char_location"}]}]},{"role":"user","content":"next"},{"role":"assistant","content":[{"type":"text","citations":[{"type":"web_search_result_location"}]}]}]});
        assert!(strip_orphaned_search_citations(&mut body, false));
        assert_eq!(
            body["messages"][0]["content"][1]["citations"],
            json!([{"type":"char_location"}])
        );
        assert_eq!(
            body["messages"][2]["content"][0]["citations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(!strip_orphaned_search_citations(&mut body, false));
    }
    #[test]
    fn schema_fallback_is_explicit_never_weakens_strict_or_touches_defaults() {
        let original = json!({"tools":[{"name":"test","input_schema":{"type":"object","properties":{"bad name":{"type":"string"}}}}]});
        assert!(validate_tool_schemas_with_policy(&mut original.clone(), false).is_err());
        let mut strict = original.clone();
        strict["tools"][0]["strict"] = json!(true);
        assert!(validate_tool_schemas_with_policy(&mut strict, true).is_err());
        let mut compatible = original.clone();
        validate_tool_schemas_with_policy(&mut compatible, true).unwrap();
        assert_eq!(
            compatible["tools"][0]["input_schema"],
            json!({"type":"object","additionalProperties":true})
        );
        let mut valid = json!({"tools":[{"input_schema":{"type":"object","properties":{"good":{"default":{"bad name":1}}}}}]});
        let before = valid.clone();
        validate_tool_schemas_with_policy(&mut valid, false).unwrap();
        assert_eq!(valid, before);
    }

    #[test]
    fn search_dependencies_survive_tool_results_until_a_real_user_turn() {
        let citation = json!({"type":"text","text":"cited","citations":[{"type":"web_search_result_location"}]});
        let mut body = json!({"messages":[
            {"role":"assistant","content":[{"type":"web_search_tool_result","content":[]}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"result"}]},
            {"role":"assistant","content":[citation.clone()]},
            {"role":"user","content":"new turn"},
            {"role":"assistant","content":[{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://example.test"}]},citation]}
        ]});
        assert!(strip_orphaned_search_citations(&mut body, false));
        assert!(body["messages"][2]["content"][0].get("citations").is_none());
        assert!(body["messages"][4]["content"][1].get("citations").is_some());
    }

    #[test]
    fn schema_walk_covers_applicators_but_not_keyword_named_business_data() {
        for key in [
            "additionalItems",
            "unevaluatedItems",
            "contentSchema",
            "not",
            "if",
            "then",
            "else",
        ] {
            let mut schema = json!({"type":"object"});
            schema[key] = json!({"properties":{"bad name":{"type":"string"}}});
            assert!(rejected_schema_key(&schema, 0), "{key}");
        }
        let schema = json!({"properties":{"default":{"default":{"properties":{"bad name":1}}}},"enum":[{"properties":{"bad name":1}}]});
        assert!(!rejected_schema_key(&schema, 0));
        let mut body = json!({"messages":[{"role":"system","content":[{"type":"tool_addition","tool":{"type":"tool_definition","definition":{"strict":true,"input_schema":{"dependencies":{"x":{"properties":{"bad name":{}}}}}}}}]}]});
        assert!(validate_tool_schemas_with_policy(&mut body, true).is_err());
    }
}
