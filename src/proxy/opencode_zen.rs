//! OpenCode Zen free-lane wire, aligned with zen-gate `internal/lane/upstream.go`.
//!
//! The lane has no user secret. `Bearer public` is the pooled free credential.
//! A session id must be minted: the gate decodes the 12-hex prefix as the
//! truncated inverted `~(ms<<12|seq)` of a real client and rejects a hashed prefix.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

pub const ORIGIN: &str = "https://opencode.ai";
pub const CLIENT_UA: &str = "opencode/latest/2.1.3/cli";
pub const PUBLIC_ACCOUNT_ID: &str = "opencode-zen-public";
/// Local default when a request names no model. The id is a free-lane id;
/// a paid id is never substituted.
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash-free";
const SESSION_CAP: usize = 4096;
const SESSION_REMINT_MS: i64 = 6 * 24 * 60 * 60 * 1000;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const READ_DESC: &str = "Read the contents of a file or directory. Supports text files, images, and PDFs. Images and PDFs are presented directly to the model. Each text line is prefixed by its 1-based line number as <line>: <content>. The prefix is for reference and is not part of the file content. Directory entries are returned one per line. Use offset and limit to read large files or directories in sections. Prefer one larger read over many small slices, and use grep to find specific content in large files.";
const SHELL_DESC: &str = "Execute a shell command and return its output. Commands run on Linux using bash. Quote file paths containing spaces or special characters. Prefer dedicated tools over shell commands when possible. When output is large, the full result is saved to a file and a truncated preview is returned. Rely on automatic truncation unless filtering the output is more useful. Commands accept an optional timeout, background commands have no timeout by default. Background commands return immediately, and you will be notified when they complete.";

struct MintClock {
    last_stamp: i64,
    seq: i64,
}

static MINT: Mutex<MintClock> = Mutex::new(MintClock {
    last_stamp: 0,
    seq: 0,
});
static SESSIONS: Mutex<Option<BTreeMap<String, (String, i64)>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenWire {
    Chat,
    Responses,
    Messages,
}

pub fn base_model_id(model: &str) -> &str {
    let trimmed = model.trim();
    if let Some(open) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            return trimmed[..open].trim();
        }
    }
    trimmed
}

pub fn wire_for(model: &str) -> ZenWire {
    let base = base_model_id(model);
    let leaf = base.rsplit('/').next().unwrap_or(base);
    if leaf.eq_ignore_ascii_case("muse-spark")
        || leaf.to_ascii_lowercase().starts_with("muse-spark-")
        || leaf.to_ascii_lowercase().starts_with("muse_spark")
    {
        return ZenWire::Responses;
    }
    if base == "union-alpha" {
        return ZenWire::Messages;
    }
    ZenWire::Chat
}

pub fn endpoint_for(model: &str) -> &'static str {
    match wire_for(model) {
        ZenWire::Responses => "/inference/v1/responses",
        ZenWire::Messages => "/inference/v1/messages",
        ZenWire::Chat => "/inference/v1/chat/completions",
    }
}

pub fn is_free_model(id: &str) -> bool {
    let base = base_model_id(id);
    static FREE_TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?:^|[-_])free(?:$|[-_.])").expect("static free-lane pattern")
    });
    !base.rsplit('/').next().unwrap_or(base).starts_with("jev")
        && (FREE_TOKEN.is_match(base) || base == "union-alpha")
}

/// Decode the low 36 bits of the millisecond stamp from the 12-hex prefix.
///
/// The mint keeps the low 48 bits of `~(ms<<12|seq)`. Inverting those 48 bits
/// and shifting recovers only the low 36 bits of `ms`, which is the window the
/// gate checks. A sha256 prefix decodes to a stamp that is not fresh.
pub fn decode_session_stamp(session: &str) -> Option<i64> {
    let rest = session.strip_prefix("ses_")?;
    if rest.len() < 12 || !rest[..12].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let prefix = u64::from_str_radix(&rest[..12], 16).ok()?;
    let value = (!prefix) & 0x0000_FFFF_FFFF_FFFF;
    Some((value >> 12) as i64)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn base62_from(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| BASE62[(*byte as usize) % 62] as char)
        .collect()
}

pub fn mint_session_id(stamp_ms: i64) -> String {
    let now_ms = if stamp_ms <= 0 { now_ms() } else { stamp_ms };
    // The stamp reset and the counter step are one critical section. Two
    // mints in the same millisecond must not both observe a zero counter.
    let seq = {
        let mut clock = MINT.lock().unwrap_or_else(|poison| poison.into_inner());
        if clock.last_stamp != now_ms {
            clock.last_stamp = now_ms;
            clock.seq = 0;
        }
        clock.seq = clock.seq.wrapping_add(1);
        clock.seq
    };
    let value = !(((now_ms as u64) << 12) | ((seq as u64) & 0xFFF));
    let mut raw = [0u8; 6];
    for (index, slot) in raw.iter_mut().enumerate() {
        *slot = ((value >> (40 - 8 * index)) & 0xFF) as u8;
    }
    let mut rnd = [0u8; 14];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut rnd);
    format!("ses_{}{}", hex_encode(&raw), base62_from(&rnd))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

/// One downstream conversation keeps one upstream session until the freshness
/// window (six days) expires. Capacity evicts the oldest minted entry only.
pub fn session_for_conversation(seed: &str) -> String {
    // Fixed-size, opaque keys prevent caller-controlled conversation lengths
    // from expanding the process cache. Namespacing is supplied by the driver.
    use sha2::{Digest, Sha256};
    let seed = hex::encode(Sha256::digest(seed.trim().as_bytes()));
    let now = now_ms();
    let mut guard = SESSIONS.lock().unwrap_or_else(|poison| poison.into_inner());
    let table = guard.get_or_insert_with(BTreeMap::new);
    if let Some((id, at)) = table.get(&seed) {
        if now.saturating_sub(*at) < SESSION_REMINT_MS {
            return id.clone();
        }
    }
    if table.len() >= SESSION_CAP {
        if let Some(oldest) = table
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(key, _)| key.clone())
        {
            table.remove(&oldest);
        }
    }
    let id = mint_session_id(now);
    table.insert(seed, (id.clone(), now));
    id
}

pub fn gateway_headers(session: &str, stream: bool) -> Vec<(&'static str, String)> {
    let accept = if stream { "text/event-stream" } else { "*/*" };
    vec![
        ("content-type", "application/json".to_string()),
        ("authorization", "Bearer public".to_string()),
        ("user-agent", CLIENT_UA.to_string()),
        ("x-opencode-client", "cli".to_string()),
        ("x-opencode-session", session.to_string()),
        ("x-opencode-session-id", session.to_string()),
        ("x-session-affinity", session.to_string()),
        ("x-session-id", session.to_string()),
        ("x-opencode-project", "global".to_string()),
        ("accept", accept.to_string()),
    ]
}

fn fingerprint_params(name: &str) -> Option<(&'static str, Value)> {
    let (desc, required, props): (&str, &str, &[(&str, &str, &str)]) = match name {
        "read" => (
            READ_DESC,
            "path",
            &[
                ("path", "string", "File or directory to read"),
                (
                    "offset",
                    "integer",
                    "The line or directory entry to start reading from (1-based)",
                ),
                (
                    "limit",
                    "integer",
                    "The maximum number of lines or directory entries to read (defaults to and capped at 2000)",
                ),
            ],
        ),
        "shell" => (
            SHELL_DESC,
            "command",
            &[
                ("command", "string", "Shell command string to execute"),
                (
                    "workdir",
                    "string",
                    "Working directory to execute the command in. Defaults to the current working directory. When possible, avoid changing directories in the command and set the working directory here instead.",
                ),
                (
                    "timeout",
                    "integer",
                    "Timeout in milliseconds. Set to 0 to disable the timeout. Defaults to 120000 for foreground commands. Background commands have no timeout by default.",
                ),
                (
                    "background",
                    "boolean",
                    "Run the command in the background and return immediately (useful for dev servers and long-running builds). You do not need to use '&' at the end of the command when using this parameter. You will be notified when it completes. DO NOT poll for completion.",
                ),
            ],
        ),
        _ => return None,
    };
    let mut properties = Map::new();
    for (key, kind, description) in props {
        let mut field = Map::new();
        field.insert("type".to_string(), Value::String((*kind).to_string()));
        field.insert(
            "description".to_string(),
            Value::String((*description).to_string()),
        );
        if *kind == "integer" {
            field.insert("minimum".to_string(), Value::from(0));
        }
        properties.insert((*key).to_string(), Value::Object(field));
    }
    let mut params = Map::new();
    params.insert("type".to_string(), Value::String("object".to_string()));
    params.insert("properties".to_string(), Value::Object(properties));
    params.insert(
        "required".to_string(),
        Value::Array(vec![Value::String(required.to_string())]),
    );
    params.insert("additionalProperties".to_string(), Value::Bool(false));
    Some((desc, Value::Object(params)))
}

fn tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")
        .and_then(|function| function.get("name"))
        .or_else(|| tool.get("name"))
        .and_then(Value::as_str)
}

fn fingerprint_key(name: &str) -> Option<&'static str> {
    match name.trim() {
        "read" => Some("read"),
        "shell" => Some("shell"),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintStyle {
    /// OpenAI chat: `{type:function, function:{name, parameters, strict}}`.
    Chat,
    /// OpenAI Responses: a flat function tool.
    Flat,
    /// Anthropic Messages: `{name, description, input_schema}`.
    Claude,
}

/// Append the official `read` and `shell` definitions. A caller tool that
/// already uses one of those names with a different schema is renamed to
/// `<name>_client`. The returned map is sent spelling to caller spelling.
///
/// `tool_choice: "none"` is a gate miss: the caller must not send it. Chat
/// leaves an absent choice unset. Responses defaults an absent choice to
/// `auto`.
pub fn apply_fingerprint(body: &mut Value) -> BTreeMap<String, String> {
    apply_fingerprint_style(body, FingerprintStyle::Chat)
}

pub fn apply_fingerprint_style(
    body: &mut Value,
    style: FingerprintStyle,
) -> BTreeMap<String, String> {
    let mut rename = BTreeMap::new();
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut taken: Vec<String> = tools
        .iter()
        .filter_map(|tool| tool_name(tool).map(str::to_string))
        .collect();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for mut tool in tools {
        let Some(name) = tool_name(&tool).map(str::to_string) else {
            out.push(tool);
            continue;
        };
        let Some(key) = fingerprint_key(&name) else {
            if style == FingerprintStyle::Chat {
                if let Some(function) = tool.get_mut("function") {
                    if function.get("strict").is_none() {
                        function
                            .as_object_mut()
                            .map(|map| map.insert("strict".to_string(), Value::Bool(false)));
                    }
                }
            }
            out.push(tool);
            continue;
        };
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let exact = fingerprint_params(key).is_some_and(|(desc, params)| {
            let holder = tool.get("function").unwrap_or(&tool);
            let schema = holder
                .get("parameters")
                .or_else(|| holder.get("input_schema"));
            holder.get("description").and_then(Value::as_str) == Some(desc)
                && schema == Some(&params)
        });
        if exact {
            out.push(tool);
            continue;
        }
        let mut alias = format!("{key}_client");
        let mut index = 2;
        while taken.iter().any(|item| item == &alias) {
            alias = format!("{key}_client{index}");
            index += 1;
        }
        taken.push(alias.clone());
        rename.insert(alias.clone(), name);
        if let Some(function) = tool.get_mut("function").and_then(Value::as_object_mut) {
            function.insert("name".to_string(), Value::String(alias));
        } else if let Some(map) = tool.as_object_mut() {
            map.insert("name".to_string(), Value::String(alias));
        }
        out.push(tool);
    }
    let mut appended = Vec::new();
    for name in ["read", "shell"] {
        if out.iter().any(|tool| tool_name(tool) == Some(name)) {
            continue;
        }
        let Some((desc, params)) = fingerprint_params(name) else {
            continue;
        };
        out.push(fingerprint_tool(name, desc, params, style));
        appended.push(name);
    }
    if let Some(map) = body.as_object_mut() {
        map.insert("tools".to_string(), Value::Array(out));
        if style == FingerprintStyle::Chat
            && !map.contains_key("max_completion_tokens")
            && !map.contains_key("max_tokens")
        {
            map.insert("max_completion_tokens".to_string(), Value::from(32_000));
        }
        retarget_tool_choice(map, &rename);
        alias_history_tool_calls(map, &rename);
        if !appended.is_empty() {
            note_unavailable_tools(map, style, &appended);
        }
        if style == FingerprintStyle::Flat && !map.contains_key("tool_choice") {
            map.insert("tool_choice".to_string(), Value::String("auto".to_string()));
        }
    }
    rename
}

fn fingerprint_tool(name: &str, desc: &str, params: Value, style: FingerprintStyle) -> Value {
    match style {
        FingerprintStyle::Claude => serde_json::json!({
            "name": name,
            "description": desc,
            "input_schema": params,
        }),
        FingerprintStyle::Flat => serde_json::json!({
            "type": "function",
            "name": name,
            "description": desc,
            "parameters": params,
            "strict": false,
        }),
        FingerprintStyle::Chat => serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": desc,
                "parameters": params,
                "strict": false,
            }
        }),
    }
}

fn alias_for(rename: &BTreeMap<String, String>, original: &str) -> Option<String> {
    rename
        .iter()
        .find(|(_, caller)| caller.as_str() == original)
        .map(|(alias, _)| alias.clone())
}

fn retarget_tool_choice(body: &mut Map<String, Value>, rename: &BTreeMap<String, String>) {
    if rename.is_empty() {
        return;
    }
    let Some(choice) = body.get_mut("tool_choice").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(name) = choice
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        if let Some(alias) = alias_for(rename, name.trim()) {
            choice.insert("name".to_string(), Value::String(alias));
        }
        return;
    }
    if let Some(function) = choice.get_mut("function").and_then(Value::as_object_mut) {
        if let Some(name) = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            if let Some(alias) = alias_for(rename, name.trim()) {
                function.insert("name".to_string(), Value::String(alias));
            }
        }
    }
}

fn alias_history_tool_calls(body: &mut Map<String, Value>, rename: &BTreeMap<String, String>) {
    if rename.is_empty() {
        return;
    }
    if let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            let Some(map) = item.as_object_mut() else {
                continue;
            };
            if map.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            if let Some(name) = map.get("name").and_then(Value::as_str).map(str::to_string) {
                if let Some(alias) = alias_for(rename, &name) {
                    map.insert("name".to_string(), Value::String(alias));
                }
            }
        }
    }
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        let Some(calls) = message
            .as_object_mut()
            .and_then(|map| map.get_mut("tool_calls"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for call in calls {
            let Some(function) = call
                .as_object_mut()
                .and_then(|map| map.get_mut("function"))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            if let Some(name) = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                if let Some(alias) = alias_for(rename, &name) {
                    function.insert("name".to_string(), Value::String(alias));
                }
            }
        }
    }
}

fn note_unavailable_tools(body: &mut Map<String, Value>, style: FingerprintStyle, names: &[&str]) {
    let quoted = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" or ");
    let note = format!(
        "Do not call the {quoted} tools; they are placeholders that do nothing. Use the other tools you have been given instead."
    );
    if style == FingerprintStyle::Flat {
        match body.get("instructions").and_then(Value::as_str) {
            Some(existing) if !existing.trim().is_empty() => {
                body.insert(
                    "instructions".to_string(),
                    Value::String(format!("{existing}\n\n{note}")),
                );
            }
            _ => {
                body.insert("instructions".to_string(), Value::String(note));
            }
        }
        return;
    }
    if style != FingerprintStyle::Chat || body.get("system").is_some() {
        return;
    }
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    if messages.is_empty() {
        return;
    }
    if messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("system")
    {
        let content = messages
            .first()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(content) = content {
            if let Some(message) = messages.first_mut().and_then(Value::as_object_mut) {
                message.insert(
                    "content".to_string(),
                    Value::String(format!("{content}\n\n{note}")),
                );
            }
            return;
        }
    }
    messages.insert(0, serde_json::json!({"role": "system", "content": note}));
}

/// `tool_choice: "none"` is a free-tier gate miss. Any other choice is left
/// for the fingerprint step.
pub fn tool_choice_rejected(body: &Value) -> bool {
    match body.get("tool_choice") {
        Some(Value::String(choice)) => choice == "none",
        Some(Value::Object(choice)) => choice.get("type").and_then(Value::as_str) == Some("none"),
        _ => false,
    }
}

const CHAT_KEY_ORDER: &[&str] = &[
    "model",
    "messages",
    "tools",
    "stream",
    "stream_options",
    "max_completion_tokens",
];

/// Encode a chat body with HTML characters left intact and the CLI key order.
/// An extra top-level field is kept after the known keys; the gate 403s a
/// reordered or HTML-escaped body.
pub fn marshal_chat_body(body: &Value) -> Result<Vec<u8>, String> {
    let Some(map) = body.as_object() else {
        return Err("opencode zen chat body must be an object".to_string());
    };
    let mut out = Vec::from(&b"{"[..]);
    let mut first = true;
    let mut write_key = |key: &str, out: &mut Vec<u8>| -> Result<(), String> {
        let Some(value) = map.get(key) else {
            return Ok(());
        };
        if !first {
            out.push(b',');
        }
        first = false;
        out.extend(serde_json::to_vec(&Value::String(key.to_string())).map_err(|e| e.to_string())?);
        out.push(b':');
        out.extend(marshal_no_escape(value)?);
        Ok(())
    };
    for key in CHAT_KEY_ORDER {
        write_key(key, &mut out)?;
    }
    let mut rest: Vec<&String> = map
        .keys()
        .filter(|key| !CHAT_KEY_ORDER.contains(&key.as_str()))
        .collect();
    rest.sort();
    for key in rest {
        write_key(key, &mut out)?;
    }
    out.push(b'}');
    Ok(out)
}

fn marshal_no_escape(value: &Value) -> Result<Vec<u8>, String> {
    let mut raw = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let text = String::from_utf8(raw.clone()).map_err(|error| error.to_string())?;
    if text.contains("\\u003c") || text.contains("\\u003e") || text.contains("\\u0026") {
        raw = text
            .replace("\\u003c", "<")
            .replace("\\u003e", ">")
            .replace("\\u0026", "&")
            .into_bytes();
    }
    Ok(raw)
}

/// Build one chat completion body for a free-lane model. `rename` maps a
/// sent tool name back to the caller name.
pub fn chat_body(model: &str, messages: Value, stream: bool) -> (Value, BTreeMap<String, String>) {
    let mut body = serde_json::json!({
        "model": base_model_id(model),
        "messages": messages,
        "stream": stream,
    });
    let rename = apply_fingerprint(&mut body);
    (body, rename)
}

/// One prepared upstream call: the path, the encoded body, and the tool-name
/// map that a response must restore.
pub struct PreparedUpstream {
    pub path: &'static str,
    pub body: Vec<u8>,
    pub rename: BTreeMap<String, String>,
    pub stream: bool,
}

/// Translate a Claude Messages or OpenAI Chat/Responses body onto the wire
/// that `model` selects. Effort suffixes stay local. `tool_choice: "none"`
/// is refused before any fingerprint is applied.
pub fn prepare_upstream(requested: &Value) -> Result<PreparedUpstream, PrepareError> {
    let model = requested
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MODEL);
    if !is_free_model(model) {
        return Err(PrepareError::PaidModel);
    }
    if tool_choice_rejected(requested) {
        return Err(PrepareError::ToolChoiceNone);
    }
    let stream = requested
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let wire = wire_for(model);
    let (mut body, style) = match wire {
        ZenWire::Responses => (
            responses_shaped(model, requested, stream),
            FingerprintStyle::Flat,
        ),
        ZenWire::Messages => (
            messages_shaped(model, requested, stream),
            FingerprintStyle::Claude,
        ),
        ZenWire::Chat => (
            chat_shaped(model, requested, stream),
            FingerprintStyle::Chat,
        ),
    };
    let rename = apply_fingerprint_style(&mut body, style);
    let bytes = marshal_upstream(&body).map_err(PrepareError::Encode)?;
    Ok(PreparedUpstream {
        path: endpoint_for(model),
        body: bytes,
        rename,
        stream,
    })
}

/// This reviewed lane publishes text/tools only. Reject unsupported payloads
/// before generic converters could drop or reshape them into empty text.
pub(super) fn validate_source_request(request: &Value) -> Result<(), super::ProxyError> {
    if request.get("n").is_some_and(|n| n.as_u64() != Some(1)) {
        return Err(super::ProxyError::bad_request(
            "OpenCode Zen supports one completion per request",
        ));
    }
    for key in ["max_tokens", "max_output_tokens", "max_completion_tokens"] {
        if request
            .get(key)
            .is_some_and(|value| value.as_u64().is_none_or(|n| n == 0 || n > i32::MAX as u64))
        {
            return Err(super::ProxyError::bad_request(
                "OpenCode Zen output ceiling must be a positive integer",
            ));
        }
    }
    fn media(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.iter().any(media),
            Value::Object(map) => {
                map.get("type").and_then(Value::as_str).is_some_and(|kind| {
                    matches!(
                        kind,
                        "image"
                            | "image_url"
                            | "input_image"
                            | "audio"
                            | "input_audio"
                            | "file"
                            | "input_file"
                            | "document"
                            | "video"
                            | "input_video"
                    )
                }) || ["messages", "input", "content", "system"]
                    .iter()
                    .any(|key| map.get(*key).is_some_and(media))
            }
            _ => false,
        }
    }
    if media(request) {
        return Err(super::ProxyError::bad_request(
            "OpenCode Zen media input has no reviewed capability receipt",
        ));
    }
    Ok(())
}

/// Convert from the actual ingress protocol, never infer it from optional fields.
pub(super) fn prepare_for_route(
    requested: &Value,
    route: super::ProxyRoute,
) -> Result<PreparedUpstream, super::ProxyError> {
    validate_source_request(requested)?;
    if tool_choice_rejected(requested) {
        return Err(super::ProxyError {
            status: http::StatusCode::FORBIDDEN,
            message: "OpenCode Zen rejects tool_choice none".to_string(),
        });
    }
    use super::{adapters::UpstreamFormat as F, transforms as t};
    let source = super::adapters::downstream_format_for_route(route);
    let target = wire_for(
        requested
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_MODEL),
    )
    .format();
    let mut converted = match (source, target) {
        (F::AnthropicMessages, F::OpenAiChat) => t::anthropic_to_openai_chat(requested),
        (F::AnthropicMessages, F::OpenAiResponses) => t::anthropic_to_openai_responses(requested),
        (F::OpenAiChat, F::AnthropicMessages) => t::openai_chat_to_anthropic(requested),
        (F::OpenAiChat, F::OpenAiResponses) => t::openai_chat_to_responses(requested),
        (F::OpenAiResponses, F::OpenAiChat) => t::openai_responses_to_chat(requested),
        (F::OpenAiResponses, F::AnthropicMessages) => t::openai_responses_to_anthropic(requested),
        _ => Ok(requested.clone()),
    }
    .map_err(|error| super::ProxyError::bad_request(error.to_string()))?;
    // Shared transforms have their own defaults. The caller's explicit ceiling
    // wins, otherwise use the reviewed free-lane default for all protocols.
    let limit = requested_limit(requested).unwrap_or(32_000);
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        converted
            .as_object_mut()
            .ok_or_else(|| {
                super::ProxyError::bad_request("OpenCode Zen request must be an object")
            })?
            .remove(key);
    }
    let key = match target {
        F::OpenAiChat => "max_completion_tokens",
        F::OpenAiResponses => "max_output_tokens",
        _ => "max_tokens",
    };
    converted[key] = Value::from(limit);
    // Zen fingerprints body bytes: no active cache hints from generic converters.
    strip_cache_hints(&mut converted);
    prepare_upstream(&converted).map_err(|error| super::ProxyError {
        status: if error == PrepareError::ToolChoiceNone {
            http::StatusCode::FORBIDDEN
        } else {
            http::StatusCode::BAD_REQUEST
        },
        message: error.to_string(),
    })
}

fn strip_cache_hints(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.remove("cache_control");
    map.remove("prompt_cache_key");
    for key in ["system", "tools"] {
        if let Some(items) = map.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                if let Some(item) = item.as_object_mut() {
                    item.remove("cache_control");
                }
            }
        }
    }
    for key in ["messages", "input"] {
        if let Some(items) = map.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                if let Some(parts) = item.get_mut("content").and_then(Value::as_array_mut) {
                    for part in parts {
                        if let Some(part) = part.as_object_mut() {
                            part.remove("cache_control");
                        }
                    }
                }
            }
        }
    }
}

impl ZenWire {
    pub(super) fn format(self) -> super::adapters::UpstreamFormat {
        use super::adapters::UpstreamFormat as F;
        match self {
            Self::Chat => F::OpenAiChat,
            Self::Responses => F::OpenAiResponses,
            Self::Messages => F::AnthropicMessages,
        }
    }
    pub(super) fn semantics(self) -> crate::domain::usage::store::InputTokenSemantics {
        use crate::domain::usage::store::InputTokenSemantics as S;
        if self == Self::Messages {
            S::Exclusive
        } else {
            S::Inclusive
        }
    }
}

pub(super) fn convert_response(
    payload: &Value,
    wire: ZenWire,
    route: super::ProxyRoute,
    context: &super::transforms::ResponsesToolContext,
) -> Result<Value, super::ProxyError> {
    use super::{adapters::UpstreamFormat as F, transforms as t};
    let result = match (
        wire.format(),
        super::adapters::downstream_format_for_route(route),
    ) {
        (F::OpenAiChat, F::AnthropicMessages) => t::openai_chat_response_to_anthropic(payload),
        (F::OpenAiChat, F::OpenAiResponses) => {
            t::openai_chat_response_to_responses_with_tool_context(payload, context)
        }
        (F::OpenAiResponses, F::AnthropicMessages) => {
            t::openai_responses_response_to_anthropic(payload)
        }
        (F::OpenAiResponses, F::OpenAiChat) => t::openai_responses_response_to_chat(payload),
        (F::AnthropicMessages, F::OpenAiChat) => t::anthropic_response_to_openai_chat(payload),
        (F::AnthropicMessages, F::OpenAiResponses) => {
            t::anthropic_response_to_openai_responses_with_tool_context(payload, context)
        }
        _ => Ok(payload.clone()),
    };
    let mut value = result.map_err(|error| super::ProxyError::bad_gateway(error.to_string()))?;
    if super::adapters::downstream_format_for_route(route) == F::OpenAiChat
        && value
            .get("created")
            .and_then(super::openai_chat_compat::valid_created)
            .is_none()
    {
        value["created"] = Value::from(super::openai_chat_compat::unix_timestamp_seconds());
    }
    Ok(value)
}

#[derive(Debug, PartialEq, Eq)]
pub enum PrepareError {
    PaidModel,
    ToolChoiceNone,
    Encode(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PaidModel => formatter.write_str("OpenCode Zen only forwards free-lane models"),
            Self::ToolChoiceNone => formatter.write_str("OpenCode Zen rejects tool_choice none"),
            Self::Encode(message) => formatter.write_str(message),
        }
    }
}

fn chat_shaped(model: &str, requested: &Value, stream: bool) -> Value {
    let messages = if requested.get("messages").is_some() {
        requested
            .get("messages")
            .cloned()
            .unwrap_or(Value::Array(vec![]))
    } else if requested.get("input").is_some() {
        responses_input_to_messages(requested)
    } else {
        serde_json::json!([{"role":"user","content":""}])
    };
    let mut body = serde_json::json!({
        "model": base_model_id(model),
        "messages": messages,
        "stream": stream,
    });
    copy_limit(&mut body, requested);
    if stream {
        if let Some(map) = body.as_object_mut() {
            map.insert(
                "stream_options".to_string(),
                serde_json::json!({"include_usage": true}),
            );
        }
    }
    if let Some(choice) = requested.get("tool_choice") {
        body["tool_choice"] = choice.clone();
    }
    if let Some(tools) = requested.get("tools") {
        body["tools"] = chat_tools(tools);
    }
    body
}

fn responses_shaped(model: &str, requested: &Value, stream: bool) -> Value {
    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        Value::String(base_model_id(model).to_string()),
    );
    body.insert("stream".to_string(), Value::Bool(stream));
    body.insert("store".to_string(), Value::Bool(false));
    if let Some(input) = requested.get("input") {
        body.insert("input".to_string(), input.clone());
        if let Some(instructions) = requested.get("instructions") {
            body.insert("instructions".to_string(), instructions.clone());
        }
    } else {
        let (instructions, input) = messages_to_response_input(requested);
        body.insert("input".to_string(), input);
        if !instructions.is_empty() {
            body.insert("instructions".to_string(), Value::String(instructions));
        }
    }
    let max_tokens = requested_limit(requested).unwrap_or(32_000);
    body.insert("max_output_tokens".to_string(), Value::from(max_tokens));
    if let Some(choice) = requested.get("tool_choice") {
        body.insert("tool_choice".to_string(), choice.clone());
    }
    if let Some(tools) = requested.get("tools") {
        body.insert("tools".to_string(), flat_tools(tools));
    }
    Value::Object(body)
}

fn messages_shaped(model: &str, requested: &Value, stream: bool) -> Value {
    let mut body = Map::new();
    body.insert(
        "model".to_string(),
        Value::String(base_model_id(model).to_string()),
    );
    body.insert("stream".to_string(), Value::Bool(stream));
    body.insert(
        "anthropic_version".to_string(),
        Value::String("2023-06-01".to_string()),
    );
    let max_tokens = requested_limit(requested).unwrap_or(32_000);
    body.insert("max_tokens".to_string(), Value::from(max_tokens));
    if requested.get("messages").is_some() {
        body.insert(
            "messages".to_string(),
            requested
                .get("messages")
                .cloned()
                .unwrap_or(Value::Array(vec![])),
        );
        if let Some(system) = requested.get("system") {
            body.insert("system".to_string(), system.clone());
        }
    } else {
        let (system, messages) = chat_to_claude_messages(requested);
        body.insert("messages".to_string(), messages);
        if !system.is_empty() {
            body.insert("system".to_string(), Value::String(system));
        }
    }
    if let Some(choice) = requested.get("tool_choice") {
        body.insert("tool_choice".to_string(), choice.clone());
    }
    if let Some(tools) = requested.get("tools") {
        body.insert("tools".to_string(), claude_tools(tools));
    }
    Value::Object(body)
}

fn copy_limit(body: &mut Value, requested: &Value) {
    let Some(map) = body.as_object_mut() else {
        return;
    };
    if let Some(value) = requested.get("max_completion_tokens") {
        map.insert("max_completion_tokens".to_string(), value.clone());
    } else if let Some(value) = requested.get("max_tokens") {
        map.insert("max_tokens".to_string(), value.clone());
    } else if let Some(value) = requested.get("max_output_tokens") {
        map.insert("max_completion_tokens".to_string(), value.clone());
    }
}

fn requested_limit(requested: &Value) -> Option<u64> {
    ["max_completion_tokens", "max_tokens", "max_output_tokens"]
        .iter()
        .find_map(|key| requested.get(*key).and_then(Value::as_u64))
}

fn chat_tools(tools: &Value) -> Value {
    let Some(list) = tools.as_array() else {
        return tools.clone();
    };
    Value::Array(
        list.iter()
            .map(|tool| {
                if tool.get("function").is_some() {
                    return tool.clone();
                }
                let name = tool.get("name").cloned().unwrap_or(Value::Null);
                let description = tool.get("description").cloned().unwrap_or(Value::Null);
                let parameters = tool
                    .get("parameters")
                    .or_else(|| tool.get("input_schema"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}}));
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": description,
                        "parameters": parameters,
                    }
                })
            })
            .collect(),
    )
}

fn flat_tools(tools: &Value) -> Value {
    let Some(list) = tools.as_array() else {
        return tools.clone();
    };
    Value::Array(
        list.iter()
            .map(|tool| {
                if tool.get("name").is_some() && tool.get("function").is_none() {
                    let mut flat = tool.clone();
                    if flat.get("parameters").is_none() {
                        if let Some(schema) = tool.get("input_schema") {
                            flat["parameters"] = schema.clone();
                        }
                    }
                    return flat;
                }
                let function = tool.get("function").unwrap_or(tool);
                serde_json::json!({
                    "type": "function",
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "parameters": function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                })
            })
            .collect(),
    )
}

fn claude_tools(tools: &Value) -> Value {
    let Some(list) = tools.as_array() else {
        return tools.clone();
    };
    Value::Array(
        list.iter()
            .map(|tool| {
                if tool.get("input_schema").is_some() && tool.get("function").is_none() {
                    return tool.clone();
                }
                let function = tool.get("function").unwrap_or(tool);
                serde_json::json!({
                    "name": function.get("name").cloned().unwrap_or(Value::Null),
                    "description": function.get("description").cloned().unwrap_or(Value::String(String::new())),
                    "input_schema": function.get("parameters").or_else(|| function.get("input_schema")).cloned().unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                })
            })
            .collect(),
    )
}

fn responses_input_to_messages(requested: &Value) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) = requested.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            messages.push(serde_json::json!({"role":"system","content": instructions}));
        }
    }
    match requested.get("input") {
        Some(Value::String(text)) => {
            messages.push(serde_json::json!({"role":"user","content": text}));
        }
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    messages.push(item.clone());
                    let _ = role;
                    continue;
                }
                let text = item
                    .get("content")
                    .or_else(|| item.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                messages.push(serde_json::json!({"role":"user","content": text}));
            }
        }
        _ => {}
    }
    Value::Array(messages)
}

fn messages_to_response_input(requested: &Value) -> (String, Value) {
    let mut instructions = requested
        .get("instructions")
        .or_else(|| requested.get("system"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut input = Vec::new();
    if let Some(messages) = requested.get("messages").and_then(Value::as_array) {
        for message in messages {
            if message.get("role").and_then(Value::as_str) == Some("system") {
                if let Some(text) = message_text(message) {
                    if instructions.is_empty() {
                        instructions = text;
                    } else {
                        instructions = format!("{instructions}\n\n{text}");
                    }
                }
                continue;
            }
            input.push(message.clone());
        }
    }
    (instructions, Value::Array(input))
}

fn chat_to_claude_messages(requested: &Value) -> (String, Value) {
    let mut system = requested
        .get("system")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut messages = Vec::new();
    if let Some(list) = requested.get("messages").and_then(Value::as_array) {
        for message in list {
            if message.get("role").and_then(Value::as_str) == Some("system") {
                if let Some(text) = message_text(message) {
                    if system.is_empty() {
                        system = text;
                    } else {
                        system = format!("{system}\n\n{text}");
                    }
                }
                continue;
            }
            messages.push(message.clone());
        }
    } else if let Some(text) = requested.get("input").and_then(Value::as_str) {
        messages.push(serde_json::json!({"role":"user","content": text}));
    }
    (system, Value::Array(messages))
}

fn message_text(message: &Value) -> Option<String> {
    match message.get("content") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

/// Encode a free-lane body. Chat payloads keep the CLI key order. Responses
/// and Messages payloads keep JSON object order and do not HTML-escape.
pub fn marshal_upstream(body: &Value) -> Result<Vec<u8>, String> {
    if body.get("messages").is_some() && body.get("input").is_none() && body.get("system").is_none()
    {
        return marshal_chat_body(body);
    }
    marshal_no_escape(body)
}

/// Map a sent tool name back to the name the caller declared.
pub fn restore_tool_name(name: &str, rename: &BTreeMap<String, String>) -> String {
    rename
        .get(name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

/// Walk a buffered JSON response or one SSE `data:` payload and restore
/// aliased tool names. Unrelated fields stay as the upstream wrote them.
pub fn restore_tool_names(value: &mut Value, rename: &BTreeMap<String, String>) {
    if rename.is_empty() {
        return;
    }
    restore_tool_names_inner(value, rename);
}

fn restore_tool_names_inner(value: &mut Value, rename: &BTreeMap<String, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                restore_tool_names_inner(item, rename);
            }
        }
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str).map(str::to_string);
            if let Some(name) = map.get("name").and_then(Value::as_str).map(str::to_string) {
                let toolish = kind.as_deref().is_some_and(|kind| {
                    kind.contains("tool") || kind == "function_call" || kind == "function"
                }) || map.contains_key("arguments")
                    || map.contains_key("input")
                    || map.contains_key("function");
                if toolish {
                    if let Some(original) = rename.get(&name) {
                        map.insert("name".to_string(), Value::String(original.clone()));
                    }
                }
            }
            if let Some(function) = map.get_mut("function") {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    if let Some(original) = rename.get(name) {
                        function.as_object_mut().map(|object| {
                            object.insert("name".to_string(), Value::String(original.clone()))
                        });
                    }
                }
            }
            for child in map.values_mut() {
                restore_tool_names_inner(child, rename);
            }
        }
        _ => {}
    }
}

/// Rewrite tool names inside an SSE body. Non-JSON data lines pass through.
pub fn restore_sse(body: &str, rename: &BTreeMap<String, String>) -> String {
    if rename.is_empty() {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let (payload, ending) = line
            .strip_suffix('\n')
            .map(|text| (text, "\n"))
            .unwrap_or((line, ""));
        let Some(data) = payload.strip_prefix("data:") else {
            out.push_str(payload);
            out.push_str(ending);
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            out.push_str(payload);
            out.push_str(ending);
            continue;
        }
        match serde_json::from_str::<Value>(data) {
            Ok(mut value) => {
                restore_tool_names(&mut value, rename);
                out.push_str("data: ");
                out.push_str(&value.to_string());
                out.push_str(ending);
            }
            Err(_) => {
                out.push_str(payload);
                out.push_str(ending);
            }
        }
    }
    out
}

/// Why a finished turn may take one continuation, if it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationKind {
    /// The stream ended during reasoning and never produced an answer.
    PureReasoning,
    /// The stream finished cleanly but carried only reasoning.
    ReasoningOnly,
    /// The answer was cut at the output ceiling.
    MaxTokens,
}

/// Inspect a buffered chat completion. Tool calls and a normal stop do not
/// continue. At most one of these kinds is returned.
pub fn continuation_kind(payload: &Value) -> Option<ContinuationKind> {
    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())?;
    let message = choice.get("message").unwrap_or(choice);
    let has_tools = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty())
        || choice.get("finish_reason").and_then(Value::as_str) == Some("tool_calls");
    if has_tools {
        return None;
    }
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let reasoning = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let finish = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if finish == "length" && !text.is_empty() {
        return Some(ContinuationKind::MaxTokens);
    }
    if text.is_empty() && !reasoning.is_empty() {
        return if finish.is_empty() {
            Some(ContinuationKind::PureReasoning)
        } else if matches!(finish, "stop" | "end_turn") {
            Some(ContinuationKind::ReasoningOnly)
        } else {
            None
        };
    }
    None
}

const CONTINUATION_INSTRUCTION: &str = "You reached the output token limit and your answer was cut off mid-way. Continue exactly from where you stopped. Do not repeat text you already wrote. Do not call tools for this continuation.";
const RECOVERY_INSTRUCTION: &str = "The previous turn stopped during reasoning and produced no answer. Continue from this reasoning checkpoint and write the answer now. Do not call tools. Checkpoint: ";

/// Append the one allowed continuation turn onto a chat body.
pub fn continuation_messages(
    body: &Value,
    kind: ContinuationKind,
    payload: &Value,
) -> Option<Value> {
    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())?;
    let message = choice.get("message").unwrap_or(choice);
    let mut messages = body.get("messages")?.as_array()?.clone();
    match kind {
        ContinuationKind::MaxTokens => {
            let text = message.get("content").and_then(Value::as_str)?;
            messages.push(serde_json::json!({"role":"assistant","content": text}));
            messages.push(serde_json::json!({"role":"user","content": CONTINUATION_INSTRUCTION}));
        }
        ContinuationKind::PureReasoning | ContinuationKind::ReasoningOnly => {
            let reasoning = message
                .get("reasoning_content")
                .or_else(|| message.get("reasoning"))
                .and_then(Value::as_str)?;
            messages.push(serde_json::json!({
                "role": "user",
                "content": format!("{RECOVERY_INSTRUCTION}{reasoning}"),
            }));
        }
    }
    Some(Value::Array(messages))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CatalogSnapshot {
    ids: Vec<String>,
    fetched_at_ms: i64,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}

static CATALOG: Mutex<Option<CatalogSnapshot>> = Mutex::new(None);

/// Keep ids the free lane can serve. `union-alpha` has no `free` token.
pub fn filter_free_ids(ids: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut kept = Vec::new();
    for id in ids {
        let id = id.as_ref().trim();
        if kept.len() >= 512 {
            break;
        }
        if !id.is_empty()
            && id.len() <= 256
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
            })
            && is_free_model(id)
            && !kept.iter().any(|seen| seen == id)
        {
            kept.push(id.to_string());
        }
    }
    kept
}

pub fn parse_model_ids(payload: &Value) -> Vec<String> {
    let rows = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| payload.as_array().cloned())
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|row| match row {
            Value::String(id) => Some(id),
            Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Remember a successful free-lane listing. A later failed fetch keeps it.
pub fn store_catalog(ids: Vec<String>, fetched_at_ms: i64) -> Vec<String> {
    let ids = filter_free_ids(ids);
    if ids.is_empty() {
        return current_catalog().unwrap_or_default();
    }
    let mut guard = CATALOG.lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(CatalogSnapshot {
        ids: ids.clone(),
        fetched_at_ms,
        metadata: BTreeMap::new(),
    });
    ids
}

pub fn current_catalog() -> Option<Vec<String>> {
    CATALOG
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()
        .map(|snapshot| snapshot.ids.clone())
}

pub fn catalog_fetched_at_ms() -> Option<i64> {
    CATALOG
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()
        .map(|snapshot| snapshot.fetched_at_ms)
}

/// A generation may use `model` only after a catalog fetch has succeeded and
/// lists it. A cold start that never fetched refuses instead of inventing ids.
pub fn generation_allowed(model: &str) -> Result<(), &'static str> {
    if !is_free_model(model) {
        return Err("OpenCode Zen only forwards free-lane models");
    }
    if catalog_fetched_at_ms().is_some_and(|at| now_ms().saturating_sub(at) > CATALOG_MAX_STALE_MS)
    {
        return Err("OpenCode Zen catalog has expired");
    }
    match current_catalog() {
        Some(ids)
            if ids
                .iter()
                .any(|id| id == base_model_id(model) || id == model) =>
        {
            Ok(())
        }
        Some(_) => Err("OpenCode Zen catalog does not list this free-lane model"),
        None => Err("OpenCode Zen catalog is unavailable"),
    }
}

pub const CATALOG_PATH: &str = "/zen/v1/models";

const CATALOG_FRESH_MS: i64 = 8 * 60 * 1000;
const CATALOG_MAX_STALE_MS: i64 = 24 * 60 * 60 * 1000;
static CATALOG_FETCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CATALOG_RETRY_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn catalog_is_stale() -> bool {
    catalog_fetched_at_ms().is_none_or(|at| now_ms().saturating_sub(at) >= CATALOG_FRESH_MS)
}

/// Fresh cached reads and concurrent callers share one bounded refresh. Explicit
/// discovery also uses the freshness window, so a UI refresh cannot amplify IO.
pub async fn refresh_catalog(
    client: &reqwest::Client,
    config_dir: &std::path::Path,
) -> Result<Vec<String>, String> {
    let _flight = CATALOG_FETCH.lock().await;
    let cache_path = config_dir.join("opencode-zen-catalog.json");
    if current_catalog().is_none() {
        let path = cache_path.clone();
        if let Ok(Some(snapshot)) =
            tokio::task::spawn_blocking(move || load_catalog_cache(&path)).await
        {
            *CATALOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);
        }
    }
    if !catalog_is_stale() {
        return Ok(current_catalog().unwrap_or_default());
    }
    let stale = || {
        catalog_fetched_at_ms()
            .filter(|at| now_ms().saturating_sub(*at) <= CATALOG_MAX_STALE_MS)
            .and_then(|_| current_catalog())
    };
    if now_ms() < CATALOG_RETRY_AT.load(std::sync::atomic::Ordering::Relaxed) {
        return stale().ok_or_else(|| "OpenCode Zen catalog refresh is cooling down".to_string());
    }
    let result = fetch_catalog(client).await;
    match result {
        Ok(snapshot) => {
            CATALOG_RETRY_AT.store(0, std::sync::atomic::Ordering::Relaxed);
            let ids = snapshot.ids.clone();
            *CATALOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot.clone());
            // A cache write failure cannot invalidate an accepted upstream listing.
            let _ = tokio::task::spawn_blocking(move || {
                crate::infra::storage::write_json_pretty(&cache_path, &snapshot)
            })
            .await;
            Ok(ids)
        }
        Err(error) => {
            CATALOG_RETRY_AT.store(now_ms() + 30_000, std::sync::atomic::Ordering::Relaxed);
            stale().ok_or(error)
        }
    }
}

async fn fetch_catalog(client: &reqwest::Client) -> Result<CatalogSnapshot, String> {
    let session = session_for_conversation("catalog:opencode-zen");
    let mut request = client
        .get(format!("{ORIGIN}{CATALOG_PATH}"))
        .timeout(std::time::Duration::from_secs(15));
    for (name, value) in gateway_headers(&session, false) {
        if name != "content-type" {
            request = request.header(name, value);
        }
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| "OpenCode Zen catalog transport failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "OpenCode Zen catalog {}: HTTP {}",
            classify_upstream_status(response.status().as_u16()),
            response.status().as_u16()
        ));
    }
    let bytes = crate::infra::http::read_response_body_limited(&mut response, 1024 * 1024)
        .await
        .map_err(|_| "OpenCode Zen catalog body invalid or exceeds 1 MiB".to_string())?;
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "OpenCode Zen catalog invalid JSON".to_string())?;
    let ids = filter_free_ids(parse_model_ids(&payload));
    if ids.is_empty() {
        return Err("OpenCode Zen catalog listed no free-lane models".to_string());
    }
    let mut metadata = BTreeMap::new();
    if let Some(rows) = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
    {
        for row in rows {
            if let Some(id) = row
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| ids.iter().any(|item| item == id))
            {
                let mut safe = Map::new();
                for key in ["context_window", "max_output_tokens"] {
                    if let Some(value) = row
                        .get(key)
                        .and_then(Value::as_u64)
                        .filter(|n| *n > 0 && *n <= 100_000_000)
                    {
                        safe.insert(key.to_string(), Value::from(value));
                    }
                }
                if !safe.is_empty() {
                    metadata.insert(id.to_string(), Value::Object(safe));
                }
            }
        }
    }
    Ok(CatalogSnapshot {
        ids,
        metadata,
        fetched_at_ms: now_ms(),
    })
}

fn load_catalog_cache(path: &std::path::Path) -> Option<CatalogSnapshot> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let mut snapshot: CatalogSnapshot = serde_json::from_slice(&bytes).ok()?;
    let age = now_ms().checked_sub(snapshot.fetched_at_ms)?;
    if !(0..=CATALOG_MAX_STALE_MS).contains(&age) {
        return None;
    }
    snapshot.ids = filter_free_ids(snapshot.ids);
    snapshot.metadata.retain(|id, value| {
        snapshot.ids.contains(id)
            && value.as_object().is_some_and(|map| {
                map.len() <= 2
                    && map.iter().all(|(key, value)| {
                        matches!(key.as_str(), "context_window" | "max_output_tokens")
                            && value.as_u64().is_some_and(|n| n > 0 && n <= 100_000_000)
                    })
            })
    });
    (!snapshot.ids.is_empty()).then_some(snapshot)
}

pub fn catalog_metadata(id: &str) -> Value {
    CATALOG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|snapshot| snapshot.metadata.get(id))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

#[cfg(test)]
fn clear_catalog() {
    *CATALOG.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
}

/// Quota failures are not replayed. The reference observes free quota by
/// egress IP; neither reminting sessions nor retrying invents more allowance.
pub fn classify_upstream_status(status: u16) -> &'static str {
    match status {
        429 => "quota",
        403 => "forbidden",
        401 => "rejected",
        _ => "upstream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_prefix_decodes_to_a_fresh_stamp() {
        let now = 1_700_000_000_000_i64;
        let id = mint_session_id(now);
        let stamp = decode_session_stamp(&id).unwrap();
        let low = now & 0xF_FFFF_FFFF;
        assert!((stamp - low).abs() < 5, "{stamp} vs {low} id {id}");
        assert!(id.starts_with("ses_"));
        assert_eq!(id.len(), 4 + 12 + 14);
    }

    #[test]
    fn hashed_prefix_does_not_decode_to_a_fresh_stamp() {
        let hashed = "ses_deadbeef0000aaaaaaaaaaaaaa";
        let stamp = decode_session_stamp(hashed).unwrap();
        assert!(now_ms().saturating_sub(stamp).abs() > 86_400_000);
    }

    #[test]
    fn conversation_keeps_one_session() {
        let first = session_for_conversation("share-session-a");
        let second = session_for_conversation("share-session-a");
        assert_eq!(first, second);
        assert_ne!(first, session_for_conversation("share-session-b"));
    }

    #[test]
    fn headers_use_public_bearer_and_drop_request_id() {
        let headers = gateway_headers("ses_test", true);
        let names: Vec<_> = headers.iter().map(|(name, _)| *name).collect();
        assert!(names.contains(&"authorization"));
        assert!(names.contains(&"x-opencode-session"));
        assert!(!names.contains(&"x-opencode-request"));
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| *name == "authorization")
                .map(|(_, value)| value.as_str()),
            Some("Bearer public")
        );
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| *name == "user-agent")
                .map(|(_, value)| value.as_str()),
            Some(CLIENT_UA)
        );
    }

    #[test]
    fn muse_and_union_select_their_wires() {
        assert_eq!(
            endpoint_for("muse-spark-1.2-contributor-free"),
            "/inference/v1/responses"
        );
        assert_eq!(endpoint_for("union-alpha"), "/inference/v1/messages");
        assert_eq!(
            endpoint_for(DEFAULT_MODEL),
            "/inference/v1/chat/completions"
        );
    }

    #[test]
    fn chat_body_keeps_tool_text_and_field_order() {
        let (body, rename) = chat_body(
            "deepseek-v4-flash-free(deep)",
            serde_json::json!([{"role":"user","content":"hi"}]),
            true,
        );
        assert!(rename.is_empty());
        assert_eq!(body["model"], "deepseek-v4-flash-free");
        assert_eq!(body["max_completion_tokens"], 32_000);
        assert!(body.get("tool_choice").is_none());
        let bytes = marshal_chat_body(&body).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("<line>"));
        assert!(!text.contains("\\u003c"));
        let model_at = text.find("\"model\"").unwrap();
        let messages_at = text.find("\"messages\"").unwrap();
        let tools_at = text.find("\"tools\"").unwrap();
        assert!(model_at < messages_at && messages_at < tools_at);
        assert!(text.contains("\"name\":\"read\""));
        assert!(text.contains("\"name\":\"shell\""));
    }

    #[test]
    fn official_claude_read_is_not_aliased() {
        let (_desc, params) = fingerprint_params("read").unwrap();
        let mut body = serde_json::json!({
            "model": "union-alpha",
            "tools": [{
                "name": "read",
                "description": READ_DESC,
                "input_schema": params
            }]
        });
        let rename = apply_fingerprint_style(&mut body, FingerprintStyle::Claude);
        assert!(rename.is_empty());
        let names: Vec<_> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(tool_name)
            .collect();
        assert_eq!(names.iter().filter(|name| **name == "read").count(), 1);
    }

    #[test]
    fn conflicting_read_tool_is_aliased() {
        let mut body = serde_json::json!({
            "model": "deepseek-v4-flash-free",
            "tools": [{"type":"function","function":{"name":"read","description":"caller","parameters":{}}}]
        });
        let rename = apply_fingerprint(&mut body);
        assert_eq!(rename.get("read_client").map(String::as_str), Some("read"));
        let names: Vec<_> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(tool_name)
            .collect();
        assert!(names.contains(&"read_client"));
        assert!(names.contains(&"read"));
    }

    #[test]
    fn catalog_keeps_only_free_ids() {
        assert!(is_free_model("deepseek-v4-flash-free"));
        assert!(is_free_model("union-alpha"));
        assert!(!is_free_model("gpt-5.4"));
    }

    #[test]
    fn quota_is_not_retried() {
        assert_eq!(classify_upstream_status(429), "quota");
        assert_eq!(classify_upstream_status(403), "forbidden");
    }

    #[test]
    fn tool_choice_none_is_refused_before_send() {
        let requested = serde_json::json!({
            "model": "deepseek-v4-flash-free",
            "messages": [{"role":"user","content":"hi"}],
            "tool_choice": "none"
        });
        assert!(tool_choice_rejected(&requested));
        assert_eq!(
            prepare_upstream(&requested).err(),
            Some(PrepareError::ToolChoiceNone)
        );
    }

    #[test]
    fn three_wires_keep_their_shapes() {
        let chat = prepare_upstream(&serde_json::json!({
            "model": "deepseek-v4-flash-free",
            "messages": [{"role":"user","content":"hi"}]
        }))
        .unwrap();
        assert_eq!(chat.path, "/inference/v1/chat/completions");
        let chat_body: Value = serde_json::from_slice(&chat.body).unwrap();
        assert_eq!(chat_body["max_completion_tokens"], 32_000);
        assert!(chat_body.get("tool_choice").is_none());
        assert!(String::from_utf8(chat.body.clone())
            .unwrap()
            .contains("<line>"));

        let responses = prepare_upstream(&serde_json::json!({
            "model": "muse-spark-1.2-contributor-free",
            "input": "hi"
        }))
        .unwrap();
        assert_eq!(responses.path, "/inference/v1/responses");
        let responses_body: Value = serde_json::from_slice(&responses.body).unwrap();
        assert_eq!(responses_body["tool_choice"], "auto");
        assert!(responses_body.get("input").is_some());
        let names = responses_body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(tool_name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"read"));

        let messages = prepare_upstream(&serde_json::json!({
            "model": "union-alpha",
            "messages": [{"role":"user","content":"hi"}]
        }))
        .unwrap();
        assert_eq!(messages.path, "/inference/v1/messages");
        let messages_body: Value = serde_json::from_slice(&messages.body).unwrap();
        assert_eq!(messages_body["anthropic_version"], "2023-06-01");
        assert!(messages_body["tools"][0].get("input_schema").is_some());
    }

    #[test]
    fn response_restores_the_caller_tool_name() {
        let mut payload = serde_json::json!({
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "function": {"name": "read_client", "arguments": "{}"}
                    }]
                }
            }]
        });
        let mut rename = BTreeMap::new();
        rename.insert("read_client".to_string(), "read".to_string());
        restore_tool_names(&mut payload, &rename);
        assert_eq!(
            payload["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "read"
        );
        let sse = restore_sse(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"read_client\"}}]}}]}\n\n",
            &rename,
        );
        assert!(sse.contains("\"name\":\"read\""));
        assert!(!sse.contains("read_client"));
    }

    #[test]
    fn max_tokens_cut_continues_once() {
        let payload = serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": {"role":"assistant","content":"partial"}
            }]
        });
        assert_eq!(
            continuation_kind(&payload),
            Some(ContinuationKind::MaxTokens)
        );
        let body = serde_json::json!({"messages":[{"role":"user","content":"hi"}]});
        let next = continuation_messages(&body, ContinuationKind::MaxTokens, &payload).unwrap();
        assert_eq!(next.as_array().unwrap().len(), 3);
        let tool_cut = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {"content":"", "tool_calls":[{"id":"1"}]}
            }]
        });
        assert!(continuation_kind(&tool_cut).is_none());
    }

    #[test]
    fn catalog_keeps_the_previous_list_and_filters_paid_ids() {
        let _lock = CATALOG_TEST_LOCK.blocking_lock();
        clear_catalog();
        let first = store_catalog(
            vec![
                "deepseek-v4-flash-free".to_string(),
                "gpt-5.4".to_string(),
                "union-alpha".to_string(),
            ],
            now_ms(),
        );
        assert_eq!(
            first,
            vec![
                "deepseek-v4-flash-free".to_string(),
                "union-alpha".to_string()
            ]
        );
        let kept = store_catalog(Vec::<String>::new(), 20);
        assert_eq!(kept, first);
        assert!(generation_allowed("deepseek-v4-flash-free").is_ok());
        assert!(generation_allowed("gpt-5.4").is_err());
        clear_catalog();
        assert_eq!(
            generation_allowed("deepseek-v4-flash-free"),
            Err("OpenCode Zen catalog is unavailable")
        );
    }

    #[test]
    fn paid_model_is_refused() {
        let requested = serde_json::json!({
            "model": "gpt-5.4",
            "messages": [{"role":"user","content":"hi"}]
        });
        assert_eq!(
            prepare_upstream(&requested).err(),
            Some(PrepareError::PaidModel)
        );
    }
}

#[path = "opencode_zen/stream.rs"]
mod stream;
pub(super) use stream::ZenStream;

#[cfg(test)]
pub(crate) static CATALOG_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod catalog_lifecycle_tests {
    use super::*;
    #[test]
    fn opencode_zen_disk_cache_accepts_recent_success_only() {
        let path =
            std::env::temp_dir().join(format!("zen-catalog-{}.json", mint_session_id(now_ms())));
        let mut snapshot = CatalogSnapshot {
            ids: vec!["chat-free".into(), "paid".into(), "jev-free".into()],
            fetched_at_ms: now_ms(),
            metadata: BTreeMap::from([(
                "chat-free".into(),
                serde_json::json!({"context_window":32768}),
            )]),
        };
        crate::infra::storage::write_json_pretty(&path, &snapshot).unwrap();
        let loaded = load_catalog_cache(&path).unwrap();
        assert_eq!(loaded.ids, vec!["chat-free"]);
        assert_eq!(loaded.metadata["chat-free"]["context_window"], 32768);
        snapshot.fetched_at_ms = now_ms() - CATALOG_MAX_STALE_MS - 1;
        crate::infra::storage::write_json_pretty(&path, &snapshot).unwrap();
        assert!(load_catalog_cache(&path).is_none());
        snapshot.fetched_at_ms = now_ms() + 10000;
        crate::infra::storage::write_json_pretty(&path, &snapshot).unwrap();
        assert!(load_catalog_cache(&path).is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn opencode_zen_cold_catalog_refresh_is_single_flight_with_backoff() {
        use futures_util::StreamExt;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let _lock = CATALOG_TEST_LOCK.lock().await;
        clear_catalog();
        CATALOG_RETRY_AT.store(0, Ordering::Relaxed);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let router = axum::Router::new().fallback(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { http::StatusCode::BAD_GATEWAY }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = crate::infra::http::test_outbound_client_builder_with_proxy(&proxy)
            .unwrap()
            .build()
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("zen-catalog-flight-{}", mint_session_id(now_ms())));
        let mut requests = futures_util::stream::FuturesUnordered::new();
        for _ in 0..16 {
            requests.push(refresh_catalog(&client, &directory));
        }
        while let Some(result) = requests.next().await {
            assert!(result.is_err());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        CATALOG_RETRY_AT.store(0, Ordering::Relaxed);
        clear_catalog();
        server.abort();
    }
}
