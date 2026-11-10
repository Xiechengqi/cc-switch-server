// Request-local validation at the shared Buddy wire boundary. No argument values
// or user-controlled tool/schema names enter diagnostics.
#[derive(Debug, Default)]
struct ToolContractGuard {
    enabled: bool,
    schemas: Value,
    request_id: String,
    calls: BTreeMap<usize, GuardedToolCall>,
    pending: Vec<u8>,
}

#[derive(Debug, Default)]
struct GuardedToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolContractGuard {
    fn retained_bytes(&self) -> usize {
        retained_json_bytes(&self.schemas)
            + self.request_id.capacity()
            + self.pending.capacity()
            + self
                .calls
                .values()
                .map(|call| {
                    std::mem::size_of::<GuardedToolCall>()
                        + 64
                        + call.id.capacity()
                        + call.name.capacity()
                        + call.arguments.capacity()
                })
                .sum::<usize>()
    }

    fn observe(&mut self, value: &mut Value) -> Result<(), ProxyError> {
        if !self.enabled {
            return Ok(());
        }
        let Some(calls) = value
            .pointer_mut("/choices/0/delta/tool_calls")
            .and_then(Value::as_array_mut)
        else {
            return Ok(());
        };
        for call in calls {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let by_id =
                id.and_then(|id| self.calls.iter().find(|(_, c)| c.id == id).map(|(i, _)| *i));
            let explicit = validate_codebuddy_tool_call_index(call)?;
            let index = match (explicit, by_id) {
                (Some(index), Some(bound)) if index != bound => {
                    return Err(Self::error("conflicting_tool_identity"))
                }
                (Some(index), _) => index,
                (None, Some(index)) => index,
                (None, None) if self.calls.is_empty() => 0,
                (None, None) if id.is_none() && self.calls.len() == 1 => {
                    *self.calls.keys().next().unwrap()
                }
                _ => return Err(Self::error("ambiguous_tool_identity")),
            };
            if let Some(existing) = self.calls.get(&index) {
                if id.is_some_and(|id| !existing.id.is_empty() && id != existing.id) {
                    return Err(Self::error("conflicting_tool_identity"));
                }
            }
            if !self.calls.contains_key(&index) && self.calls.len() >= MAX_TOOL_CALLS {
                return Err(Self::error("too_many_tool_calls"));
            }
            let state = self.calls.entry(index).or_default();
            if let Some(id) = id {
                state.id = id.to_owned();
            }
            if let Some(function) = call.get("function") {
                if !function.is_object() {
                    return Err(Self::error("invalid_tool_function"));
                }
                for (field, target) in [
                    ("name", &mut state.name),
                    ("arguments", &mut state.arguments),
                ] {
                    if let Some(fragment) = function.get(field) {
                        let fragment = fragment
                            .as_str()
                            .ok_or_else(|| Self::error("non_string_tool_fragment"))?;
                        if target.len().saturating_add(fragment.len()) > MAX_TEXT_BYTES {
                            return Err(Self::error("tool_arguments_too_large"));
                        }
                        target.push_str(fragment);
                    }
                }
            }
            call["index"] = json!(index);
        }
        Ok(())
    }

    fn error(kind: &'static str) -> ProxyError {
        ProxyError::bad_gateway(format!("CodeBuddy tool contract violation: {kind}"))
    }

    fn validate(&self) -> Result<(), ProxyError> {
        for (index, call) in &self.calls {
            let parsed = serde_json::from_str::<Value>(&call.arguments);
            let result = (|| {
                if call.id.is_empty() || call.name.is_empty() {
                    return Err("missing_tool_identity");
                }
                let arguments = parsed.as_ref().map_err(|_| "invalid_tool_json")?;
                if !arguments.is_object() {
                    return Err("tool_arguments_not_object");
                }
                if let Some(tools) = self.schemas.as_array() {
                    let schema = tools
                        .iter()
                        .find(|tool| {
                            tool.pointer("/function/name").and_then(Value::as_str)
                                == Some(&call.name)
                        })
                        .ok_or("unknown_tool")?;
                    validate_tool_shape(arguments, &schema["function"]["parameters"], 0)?;
                }
                Ok(())
            })();
            if let Err(kind) = result {
                tracing::warn!(request_id = %self.request_id, tool_index = index, argument_bytes = call.arguments.len(), stage = "buddy_wire", violation = kind, "Buddy tool contract rejected");
                return Err(Self::error(kind));
            }
        }
        Ok(())
    }
}

// Deliberately bounded structural checks, not a full JSON Schema interpreter.
fn validate_tool_shape(value: &Value, schema: &Value, depth: usize) -> Result<(), &'static str> {
    if depth > 64 {
        return Err("tool_schema_depth");
    }
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "null" => value.is_null(),
        _ => true,
    };
    if let Some(kind) = schema.get("type") {
        let valid = match kind {
            Value::String(kind) => matches_type(kind),
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).any(matches_type),
            _ => true,
        };
        if !valid {
            return Err("tool_argument_type_mismatch");
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            if required
                .iter()
                .filter_map(Value::as_str)
                .any(|key| !object.contains_key(key))
            {
                return Err("missing_required_tool_argument");
            }
        }
        for (key, field) in object {
            if let Some(property) = schema.get("properties").and_then(|p| p.get(key)) {
                validate_tool_shape(field, property, depth + 1)?;
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for item in items {
            validate_tool_shape(item, item_schema, depth + 1)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tool_contract_tests {
    use super::*;

    fn decoder() -> CodeBuddySseDecoder {
        let mut decoder = CodeBuddySseDecoder::default();
        decoder.set_tool_contract(&json!({"tools":[{"type":"function","function":{
            "name":"Bash", "parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}
        }}]}), "fixture-request").unwrap();
        decoder
    }

    fn frame(value: Value) -> Bytes {
        Bytes::from(format!("data: {value}\n\n"))
    }

    fn call(arguments: Value) -> Value {
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"opaque_a","type":"function","function":{"name":"Bash","arguments":arguments}}]}}]})
    }

    #[test]
    fn bash_invalid_arguments_never_reach_execution_boundary() {
        for arguments in [
            "{}",
            "{\"cmd\":\"printf test\"}",
            "\"hello\"",
            "{\"command\":3}",
            "{\"command\":",
        ] {
            let mut decoder = decoder();
            assert!(decoder
                .push_classified(frame(call(json!(arguments))))
                .unwrap()
                .is_empty());
            assert!(decoder
                .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
                .unwrap()
                .is_empty());
            let error = decoder.finish_classified().unwrap_err();
            assert!(!format!("{error:?}").contains("printf test"));
        }
        let mut decoder = decoder();
        assert!(decoder
            .push_classified(frame(call(json!({"command":"printf test"}))))
            .is_err());
    }

    #[test]
    fn fragmented_unicode_arguments_wait_for_validated_eof() {
        let mut decoder = decoder();
        assert!(decoder
            .push_classified(frame(call(json!("{\"command\":"))))
            .unwrap()
            .is_empty());
        assert!(decoder.push_classified(frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"function":{"arguments":"\"printf 你好\\n\"}"}}]}}]}))).unwrap().is_empty());
        assert!(decoder
            .push_classified(frame(
                json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})
            ))
            .unwrap()
            .is_empty());
        decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        let output = decoder.finish_classified().unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        assert!(text.contains("你好"));
        assert!(text.contains("[DONE]"));
        assert!(text.contains("\"index\":0"));
    }

    #[test]
    fn parallel_identity_is_resolved_by_id_and_ambiguity_fails_closed() {
        let mut guard = ToolContractGuard {
            enabled: true,
            ..Default::default()
        };
        guard.observe(&mut call(json!("{}"))).unwrap();
        guard.observe(&mut json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"opaque_b","function":{"name":"Bash","arguments":"{}"}}]}}]})).unwrap();
        let mut by_id = json!({"choices":[{"delta":{"tool_calls":[{"id":"opaque_b","function":{"arguments":""}}]}}]});
        guard.observe(&mut by_id).unwrap();
        assert_eq!(
            by_id.pointer("/choices/0/delta/tool_calls/0/index"),
            Some(&json!(1))
        );
        assert!(guard
            .observe(
                &mut json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":""}}]}}]})
            )
            .is_err());
        assert!(guard
            .observe(
                &mut json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"opaque_b"}]}}]})
            )
            .is_err());
    }

    #[test]
    fn valid_tool_is_not_released_after_truncated_or_duplicate_terminal() {
        for suffix in ["", "data: [DONE]\n\ndata: [DONE]\n\n"] {
            let mut decoder = decoder();
            assert!(decoder
                .push_classified(frame(call(json!("{\"command\":\"printf test\"}"))))
                .unwrap()
                .is_empty());
            let result = decoder.push_classified(Bytes::from(suffix.to_owned()));
            if result.is_ok() {
                assert!(decoder.finish_classified().is_err());
            }
        }
    }
    #[test]
    fn nested_types_required_and_nullable_fields_are_checked() {
        let schema = json!({"type":"object","required":["items"],"properties":{
            "items":{"type":"array","items":{"type":"object","required":["command"],"properties":{
                "command":{"type":"string"},"timeout":{"type":["integer","null"]}
            }}}
        }});
        assert!(validate_tool_shape(
            &json!({"items":[{"command":"x","timeout":1.0},{"command":"y","timeout":null}]}),
            &schema,
            0
        )
        .is_ok());
        assert!(validate_tool_shape(&json!({"items":[{}]}), &schema, 0).is_err());
        assert!(validate_tool_shape(
            &json!({"items":[{"command":"x","timeout":"1"}]}),
            &schema,
            0
        )
        .is_err());
    }

    #[test]
    fn missing_index_before_identity_is_resolved_only_for_a_single_call() {
        let mut guard = ToolContractGuard {
            enabled: true,
            ..Default::default()
        };
        let mut first =
            json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"{"}}]}}]});
        guard.observe(&mut first).unwrap();
        let mut second = call(json!("\"command\":\"x\"}"));
        guard.observe(&mut second).unwrap();
        assert!(guard.validate().is_ok());
        assert_eq!(guard.calls.len(), 1);
    }

    #[test]
    fn undeclared_tools_are_rejected() {
        let mut decoder = CodeBuddySseDecoder::default();
        decoder.set_tool_contract(&json!({}), "fixture").unwrap();
        assert!(decoder
            .push_classified(frame(call(json!("{}"))))
            .unwrap()
            .is_empty());
        decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(decoder.finish_classified().is_err());
    }
    #[test]
    fn tool_buffer_memory_is_released_on_cancel_and_output_drop() {
        let budget = RequestMemoryBudget::new(2 * 1024 * 1024);
        let mut decoder = CodeBuddySseDecoder::with_request_memory(Some(budget.clone())).unwrap();
        decoder.set_tool_contract(&json!({"tools":[{"type":"function","function":{"name":"Bash","parameters":{"type":"object"}}}]}), "fixture").unwrap();
        assert!(decoder
            .push_classified(frame(call(json!("{}"))))
            .unwrap()
            .is_empty());
        assert!(budget.snapshot().used_bytes > 0);
        drop(decoder);
        assert_eq!(budget.snapshot().used_bytes, 0);

        let mut decoder = CodeBuddySseDecoder::with_request_memory(Some(budget.clone())).unwrap();
        decoder.set_tool_contract(&json!({"tools":[{"type":"function","function":{"name":"Bash","parameters":{"type":"object"}}}]}), "fixture").unwrap();
        decoder.push_classified(frame(call(json!("{}")))).unwrap();
        decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        let output = decoder.finish_classified().unwrap();
        drop(decoder);
        assert!(budget.snapshot().used_bytes > 0);
        drop(output);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }
}
