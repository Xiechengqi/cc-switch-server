use super::*;

const BEGIN: &str = "<|FunctionCallBegin|>";
const END: &str = "<|FunctionCallEnd|>";
const MAX_MARKER_BYTES: usize = 256 * 1024;

/// Explicit opt-in prompt emulation. Never advertised as native function calling.
pub(crate) fn build_raw_payload(
    canonical: &Value,
    model: &str,
    capability: &TraeModelCapability,
    simulate_tools: bool,
) -> Result<(Value, TraeToolContract), String> {
    let mut request = canonical
        .as_object()
        .cloned()
        .ok_or("Trae Chat request must be an object")?;
    normalize_messages_with_images(&mut request, true)?;
    normalize_tool_choice(&mut request)?;
    normalize_tools(&mut request)?;
    normalize_generation_controls(&mut request, capability)?;
    apply_reasoning_fields(&mut request, capability)?;
    let contract = TraeToolContract::from_payload(&Value::Object(request.clone()))?;
    let mut tools = request.remove("tools").unwrap_or(json!([]));
    let has_tools = tools.as_array().is_some_and(|tools| !tools.is_empty());
    if has_tools && !simulate_tools {
        return Err("Trae raw_chat tools require explicit traeToolSimulationEnabled; native tools are unavailable".into());
    }
    let choice = request.remove("tool_choice").unwrap_or(json!("auto"));
    let mut messages = request
        .remove("messages")
        .and_then(|messages| messages.as_array().cloned())
        .expect("validated messages");
    if has_tools {
        for tool in tools.as_array_mut().unwrap() {
            let schema = &mut tool["function"]["parameters"];
            if let Some(text) = schema.as_str() {
                *schema = serde_json::from_str(text).map_err(|_| "invalid tool schema")?;
            }
        }
        let prompt = format!("Available tools (JSON Schemas): {tools}\nWhen calling tools, output only {BEGIN}[{{\"name\":\"tool name\",\"parameters\":{{}}}}]{END}. Parameters must be a complete JSON object matching the declared schema. Tool choice: {choice}. Parallel calls allowed: {}. Do not invent tool names. Otherwise respond normally.",contract.parallel);
        messages.insert(
            0,
            json!({"role":"system","content":[{"type":"text","text":prompt}]}),
        );
    }
    // Preserve every system message, call id, result, and image. The wire format
    // is textual because raw_chat has no reviewed native tool contract.
    for message in &mut messages {
        let role = message["role"].as_str().unwrap_or_default().to_string();
        if let Some(calls) = message
            .as_object_mut()
            .and_then(|message| message.remove("tool_calls"))
        {
            let calls = calls.as_array().unwrap().iter().map(|call| json!({"id":call["id"],"name":call["function_call"]["name"],"parameters":serde_json::from_str::<Value>(call["function_call"]["arguments"].as_str().unwrap()).unwrap()})).collect::<Vec<_>>();
            message["content"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"text","text":format!("{BEGIN}{}{END}",json!(calls))}));
        }
        if role == "tool" {
            let id = message
                .as_object_mut()
                .unwrap()
                .remove("tool_call_id")
                .unwrap();
            let name = message.as_object_mut().unwrap().remove("name").unwrap();
            message["role"] = json!("user");
            message["content"].as_array_mut().unwrap().insert(0,json!({"type":"text","text":format!("Tool result for call {id}, function {name}:\n")}));
        }
    }
    let mut parameters = Map::new();
    for key in [
        "max_tokens",
        "temperature",
        "top_p",
        "stop",
        "reasoning_effort_level",
    ] {
        if let Some(value) = request.remove(key) {
            parameters.insert(key.into(), value);
        }
    }
    if request.get("is_max_mode").is_some() {
        return Err("Trae raw_chat does not support SOLO max mode".into());
    }
    Ok((
        json!({"model_name":model,"messages":messages,"parameters":parameters,"stream":true}),
        contract,
    ))
}

#[derive(Debug, Default)]
pub(super) struct ToolMarkers {
    pending: String,
    inside: bool,
    count: usize,
}
impl ToolMarkers {
    pub(super) fn retained_bytes(&self) -> usize {
        self.pending.capacity()
    }
    pub(super) fn feed(&mut self, text: &str, terminal: bool) -> Result<Vec<Value>, ProxyError> {
        self.pending.push_str(text);
        let mut deltas = Vec::new();
        loop {
            if self.inside {
                if let Some(end) = self.pending.find(END) {
                    if end > MAX_MARKER_BYTES {
                        return Err(ProxyError::bad_gateway("Trae tool marker exceeds limit"));
                    }
                    let calls: Value =
                        serde_json::from_str(&self.pending[..end]).map_err(|_| {
                            ProxyError::bad_gateway("Trae tool marker contains invalid JSON")
                        })?;
                    let calls = calls
                        .as_array()
                        .filter(|calls| !calls.is_empty())
                        .ok_or_else(|| {
                            ProxyError::bad_gateway(
                                "Trae tool marker must contain a nonempty array",
                            )
                        })?;
                    let mut wire_calls = Vec::new();
                    for call in calls {
                        if self.count >= MAX_TOOL_CALLS {
                            return Err(ProxyError::bad_gateway(
                                "Trae tool marker has too many calls",
                            ));
                        }
                        let name = call
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty())
                            .ok_or_else(|| {
                                ProxyError::bad_gateway("Trae tool marker is missing name")
                            })?;
                        let args = call
                            .get("parameters")
                            .filter(|args| args.is_object())
                            .ok_or_else(|| {
                                ProxyError::bad_gateway(
                                    "Trae tool marker parameters must be an object",
                                )
                            })?;
                        wire_calls.push(json!({"index":self.count,"id":format!("call_{}",random_trae_request_id()),"type":"function","function_call":{"name":name,"arguments":serde_json::to_string(args).map_err(ProxyError::bad_gateway)?}}));
                        self.count += 1;
                    }
                    deltas.push(json!({"tool_calls":wire_calls}));
                    self.pending.drain(..end + END.len());
                    self.inside = false;
                } else {
                    if self.pending.len() > MAX_MARKER_BYTES {
                        return Err(ProxyError::bad_gateway("Trae tool marker exceeds limit"));
                    }
                    break;
                }
            } else if let Some(begin) = self.pending.find(BEGIN) {
                if begin > 0 {
                    deltas.push(json!({"response":&self.pending[..begin]}));
                }
                self.pending.drain(..begin + BEGIN.len());
                self.inside = true;
            } else {
                let held = if terminal {
                    0
                } else {
                    (1..BEGIN.len())
                        .rev()
                        .find(|len| self.pending.ends_with(&BEGIN[..*len]))
                        .unwrap_or(0)
                };
                let emit = self.pending.len() - held;
                if emit > 0 {
                    deltas.push(json!({"response":&self.pending[..emit]}));
                    self.pending.drain(..emit);
                }
                break;
            }
        }
        if terminal && self.inside {
            return Err(ProxyError::bad_gateway(
                "Trae stream ended inside a tool marker",
            ));
        }
        Ok(deltas)
    }
}
