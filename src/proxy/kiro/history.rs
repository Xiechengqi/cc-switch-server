use super::*;

pub(super) fn normalize_messages(messages: &[Value]) -> Vec<Value> {
    let mut normalized: Vec<Value> = Vec::new();
    for message in messages {
        let mut message = message.clone();
        if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("server_tool_use") if block["name"] == "web_search" => {
                        *block = json!({"type":"text","text":format!("Previous web search query\n{}", block["input"])});
                    }
                    Some("web_search_tool_result") => {
                        *block = json!({"type":"text","text":format!("Previous web search results\n{}", block["content"])});
                    }
                    _ => {}
                }
            }
        }
        if message.get("role").and_then(Value::as_str) == Some("system") {
            let text = flatten_content_to_text(message.get("content"));
            if text.trim().is_empty() {
                continue;
            }
            message = json!({"role":"user","content":[{
                "type":"text","text":format!("<system-reminder>\n{text}\n</system-reminder>")
            }]});
        }
        if !matches!(
            message.get("role").and_then(Value::as_str),
            Some("user" | "assistant")
        ) {
            continue;
        }
        if message["role"] == "user" && normalized.last().is_some_and(|last| last["role"] == "user")
        {
            let last = normalized.last_mut().expect("adjacent user message");
            let mut blocks = content_blocks(last.get("content"));
            blocks.extend(content_blocks(message.get("content")));
            last["content"] = Value::Array(blocks);
        } else {
            normalized.push(message);
        }
    }
    normalized
}

fn content_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![json!({"type":"text","text":text})],
        Some(Value::Array(blocks)) => blocks.clone(),
        _ => Vec::new(),
    }
}

pub(super) fn history_reasoning_content(messages: &[&Value]) -> Option<Value> {
    let mut blocks = messages
        .iter()
        .filter_map(|message| message.get("content")?.as_array())
        .flatten()
        .filter(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking")
            )
        });
    let block = blocks.next()?;
    if blocks.next().is_some() {
        return None;
    }
    if block["type"] == "redacted_thinking" {
        let data = block.get("data")?.as_str()?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()?;
        return (!decoded.is_empty()).then(|| json!({"redactedContent":data}));
    }
    let text = block.get("thinking")?.as_str()?;
    let signature = block.get("signature")?.as_str()?;
    if text.is_empty()
        || signature.trim().is_empty()
        || matches!(
            signature,
            THINKING_SIGNATURE_FALLBACK | "kiro-rs-thinking-signature"
        )
    {
        return None;
    }
    Some(json!({"reasoningText":{"text":text,"signature":signature}}))
}

pub(super) fn pair_orphaned_tool_uses(
    history: &mut [Value],
    orphaned: &HashSet<String>,
) -> Vec<Value> {
    let mut pending = Vec::new();
    for message in history {
        if let Some(results) = message
            .pointer_mut("/userInputMessage/userInputMessageContext/toolResults")
            .and_then(Value::as_array_mut)
        {
            results.append(&mut pending);
        }
        if let Some(tools) = message
            .pointer("/assistantResponseMessage/toolUses")
            .and_then(Value::as_array)
        {
            for tool in tools {
                if let Some(id) = tool
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .filter(|id| orphaned.contains(*id))
                {
                    pending.push(json!({"toolUseId":id,"status":"error","isError":true,
                        "content":[{"text":"The tool call was interrupted before a result was available."}]}));
                }
            }
        }
    }
    pending
}

pub(super) fn preserve_resent_tool_results(
    history: &[Value],
    results: &[Value],
    text: &mut String,
) {
    let prior: HashMap<&str, &Value> = history
        .iter()
        .filter_map(|message| {
            message
                .pointer("/userInputMessage/userInputMessageContext/toolResults")?
                .as_array()
        })
        .flatten()
        .filter_map(|result| Some((result.get("toolUseId")?.as_str()?, result)))
        .collect();
    for result in results {
        let Some(id) = result.get("toolUseId").and_then(Value::as_str) else {
            continue;
        };
        if prior.get(id).is_some_and(|old| **old != *result) {
            let updated = flatten_content_to_text(result.get("content"));
            text.push_str(&format!("\nUpdated result for tool call {id}\n{updated}"));
        }
    }
}

pub(super) fn retain_model_reasoning(history: &mut [Value], messages: &[Value], model: &str) {
    let normalized = model.to_ascii_lowercase();
    let keep_prior = normalized.strip_prefix("claude-").is_some_and(|name| {
        let (family, version) = name.split_once('-').unwrap_or((name, ""));
        let mut version = version
            .split('.')
            .filter_map(|part| part.parse::<u32>().ok());
        let version = (version.next().unwrap_or(0), version.next().unwrap_or(0));
        match family {
            "opus" => version >= (4, 5),
            "sonnet" => version >= (4, 6),
            "haiku" => version >= (5, 0),
            "fable" | "mythos" => true,
            _ => family
                .parse::<u32>()
                .is_ok_and(|generation| generation >= 5),
        }
    });
    if keep_prior {
        return;
    }
    let tool_continuation = messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            !blocks.is_empty() && blocks.iter().all(|block| block["type"] == "tool_result")
        });
    let last_assistant = history
        .iter()
        .rposition(|message| message.get("assistantResponseMessage").is_some());
    for (index, message) in history.iter_mut().enumerate() {
        if !tool_continuation || Some(index) != last_assistant {
            if let Some(assistant) = message
                .get_mut("assistantResponseMessage")
                .and_then(Value::as_object_mut)
            {
                assistant.remove("reasoningContent");
            }
        }
    }
}
