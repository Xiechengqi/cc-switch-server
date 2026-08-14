use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::domain::qoder::{QoderCosySession, QoderCredentialRail, QoderSite};

const MAX_RUNTIME_ENTRIES: usize = 128;
const MAX_MODEL_CATALOG_ENTRIES: usize = 256;
const MAX_MODEL_KEY_BYTES: usize = 256;
const SESSION_EXPIRY_BUFFER_MS: i64 = 5 * 60 * 1000;
pub const QODER_MODEL_CATALOG_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QoderRuntimeScope(String);

impl QoderRuntimeScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        site: QoderSite,
        credential_rail: QoderCredentialRail,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
    ) -> Result<Self, String> {
        let values = [
            app.trim(),
            provider_id.trim(),
            runtime_fingerprint.trim(),
            account_id.trim(),
        ];
        if values.iter().any(|value| value.is_empty()) {
            return Err("Qoder runtime scope contains an empty identity component".to_string());
        }
        Ok(Self(scoped_digest(
            b"cc-switch-server:qoder-runtime:v1\0",
            [
                values[0],
                values[1],
                &provider_revision.to_string(),
                values[2],
                values[3],
                site.as_str(),
                credential_rail.as_str(),
                &auth_identity_generation.to_string(),
                &token_refresh_generation.to_string(),
            ],
            "qoder-runtime-v1",
        )))
    }

    fn key(&self) -> &str {
        &self.0
    }
}

fn scoped_digest<const N: usize>(domain: &[u8], values: [&str; N], prefix: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for value in values {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{prefix}:{:x}", hasher.finalize())
}

#[derive(Clone)]
pub struct QoderCachedSession {
    pub session: Arc<QoderCosySession>,
    pub gateway_base_url: String,
    pub expires_at_ms: Option<i64>,
    cached_at_ms: i64,
}

impl QoderCachedSession {
    pub fn new(
        session: QoderCosySession,
        gateway_base_url: String,
        expires_at_ms: Option<i64>,
        cached_at_ms: i64,
    ) -> Result<Self, String> {
        let gateway_base_url = gateway_base_url.trim().trim_end_matches('/').to_string();
        if gateway_base_url.is_empty() {
            return Err("Qoder session gateway URL is empty".to_string());
        }
        Ok(Self {
            session: Arc::new(session),
            gateway_base_url,
            expires_at_ms,
            cached_at_ms,
        })
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms.is_none_or(|expires_at_ms| {
            expires_at_ms > now_ms.saturating_add(SESSION_EXPIRY_BUFFER_MS)
        })
    }
}

impl std::fmt::Debug for QoderCachedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QoderCachedSession")
            .field("site", &self.session.site)
            .field("gateway_base_url", &self.gateway_base_url)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("cached_at_ms", &self.cached_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QoderModelCatalog {
    pub enabled_models: Vec<String>,
    pub raw_configs: BTreeMap<String, Value>,
    pub fetched_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct PreparedQoderRuntime {
    pub scope: QoderRuntimeScope,
    pub session: QoderCachedSession,
    pub catalog: QoderModelCatalog,
    pub credential_rail: QoderCredentialRail,
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
}

impl PreparedQoderRuntime {
    pub fn exact_model_config(&self, model_key: &str) -> Option<&Value> {
        self.catalog.exact_config(model_key)
    }
}

impl QoderModelCatalog {
    pub fn exact_config(&self, model_key: &str) -> Option<&Value> {
        self.raw_configs.get(model_key.trim())
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchedQoderModelCatalog {
    pub enabled_models: Vec<String>,
    pub raw_configs: BTreeMap<String, Value>,
}

#[derive(Default)]
pub struct QoderRuntimeCache {
    sessions: RwLock<HashMap<QoderRuntimeScope, QoderCachedSession>>,
    session_flights: Mutex<HashMap<QoderRuntimeScope, Arc<Mutex<()>>>>,
    catalogs: RwLock<HashMap<QoderRuntimeScope, QoderModelCatalog>>,
    catalog_flights: Mutex<HashMap<QoderRuntimeScope, Arc<Mutex<()>>>>,
}

impl std::fmt::Debug for QoderRuntimeCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QoderRuntimeCache")
            .finish_non_exhaustive()
    }
}

impl QoderRuntimeCache {
    pub async fn session(
        &self,
        scope: &QoderRuntimeScope,
        now_ms: i64,
    ) -> Option<QoderCachedSession> {
        self.sessions
            .read()
            .await
            .get(scope)
            .filter(|entry| entry.is_fresh(now_ms))
            .cloned()
    }

    pub async fn insert_session(&self, scope: QoderRuntimeScope, session: QoderCachedSession) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(scope, session);
        while sessions.len() > MAX_RUNTIME_ENTRIES {
            let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, session)| session.cached_at_ms)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            sessions.remove(&oldest);
        }
    }

    pub async fn invalidate_session(&self, scope: &QoderRuntimeScope) {
        self.sessions.write().await.remove(scope);
    }

    pub async fn session_lock(&self, scope: &QoderRuntimeScope) -> OwnedMutexGuard<()> {
        flight_lock(&self.session_flights, scope).await
    }

    pub async fn catalog(
        &self,
        scope: &QoderRuntimeScope,
        now_ms: i64,
    ) -> Option<QoderModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.is_fresh(now_ms))
            .cloned()
    }

    pub async fn insert_catalog(
        &self,
        scope: QoderRuntimeScope,
        fetched: FetchedQoderModelCatalog,
        fetched_at_ms: i64,
    ) -> QoderModelCatalog {
        let catalog = QoderModelCatalog {
            enabled_models: fetched.enabled_models,
            raw_configs: fetched.raw_configs,
            fetched_at_ms,
            expires_at_ms: fetched_at_ms.saturating_add(QODER_MODEL_CATALOG_TTL_MS),
        };
        let mut catalogs = self.catalogs.write().await;
        catalogs.insert(scope, catalog.clone());
        while catalogs.len() > MAX_RUNTIME_ENTRIES {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.fetched_at_ms)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            catalogs.remove(&oldest);
        }
        catalog
    }

    pub async fn invalidate_catalog(&self, scope: &QoderRuntimeScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub async fn invalidate_scope(&self, scope: &QoderRuntimeScope) {
        self.invalidate_session(scope).await;
        self.invalidate_catalog(scope).await;
    }

    pub async fn catalog_lock(&self, scope: &QoderRuntimeScope) -> OwnedMutexGuard<()> {
        flight_lock(&self.catalog_flights, scope).await
    }
}

async fn flight_lock(
    flights: &Mutex<HashMap<QoderRuntimeScope, Arc<Mutex<()>>>>,
    scope: &QoderRuntimeScope,
) -> OwnedMutexGuard<()> {
    let flight = {
        let mut flights = flights.lock().await;
        flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
        flights
            .entry(scope.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    flight.lock_owned().await
}

pub fn parse_qoder_model_catalog(value: &Value) -> Result<FetchedQoderModelCatalog, String> {
    let entries = value
        .get("chat")
        .and_then(Value::as_array)
        .ok_or_else(|| "Qoder model catalog must contain a chat array".to_string())?;
    if entries.len() > MAX_MODEL_CATALOG_ENTRIES {
        return Err(format!(
            "Qoder model catalog exceeds {MAX_MODEL_CATALOG_ENTRIES} entries"
        ));
    }

    let mut raw_configs = BTreeMap::new();
    let mut enabled_models = Vec::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(key) = object
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        if key.len() > MAX_MODEL_KEY_BYTES {
            return Err("Qoder model key exceeds the size limit".to_string());
        }
        if raw_configs.contains_key(key) {
            return Err(format!(
                "Qoder model catalog contains duplicate key {key:?}"
            ));
        }
        raw_configs.insert(key.to_string(), entry.clone());
        if object.get("enable").and_then(Value::as_bool) != Some(false) {
            enabled_models.push(key.to_string());
        }
    }
    enabled_models.sort();
    enabled_models.dedup();
    Ok(FetchedQoderModelCatalog {
        enabled_models,
        raw_configs,
    })
}

pub fn resolve_qoder_model_key(site: QoderSite, requested: &str) -> Result<String, String> {
    let requested = requested
        .trim()
        .strip_prefix("qoder/")
        .unwrap_or(requested.trim())
        .trim()
        .to_ascii_lowercase();
    if requested.is_empty() {
        return Err("Qoder request model is empty".to_string());
    }
    let global = [
        ("claude-opus-4-6", "ultimate"),
        ("auto", "auto"),
        ("performance", "performance"),
        ("efficient", "efficient"),
        ("lite", "lite"),
        ("qwen3.8-max", "qmodel_38max"),
        ("qwen3.7-max", "qmodel_latest"),
        ("qwen3.7-plus", "qmodel"),
        ("kimi-k3", "kmodel_latest"),
        ("kimi-k2.7-code", "kmodel"),
        ("glm-5.2", "gm51model"),
        ("deepseek-v4-pro", "dmodel"),
        ("deepseek-v4-flash", "dfmodel"),
        ("minimax-m3", "mmodel"),
    ];
    let cn = [
        ("auto", "auto"),
        ("qwen3.8-max", "qmodel_38max"),
        ("qwen3.7-max", "qmodel_latest"),
        ("qwen3.7-plus", "qmodel"),
        ("qwen3.6-flash", "q36fmodel"),
        ("deepseek-v4-pro", "dmodel"),
        ("deepseek-v4-flash", "dfmodel"),
        ("glm-5.2", "gm51model"),
        ("kimi-k2.7-code", "kmodel"),
        ("minimax-m2.7", "mmodel"),
    ];
    let aliases = match site {
        QoderSite::Global => global.as_slice(),
        QoderSite::Cn => cn.as_slice(),
    };
    Ok(aliases
        .iter()
        .find_map(|(alias, key)| (*alias == requested).then_some(*key))
        .unwrap_or(requested.as_str())
        .to_string())
}

pub fn derive_qoder_conversation_session_id(
    scope: &QoderRuntimeScope,
    share_id: &str,
    user_namespace: &str,
    downstream_session_id: &str,
    model_key: &str,
) -> Result<String, String> {
    let values = [
        share_id.trim(),
        user_namespace.trim(),
        downstream_session_id.trim(),
        model_key.trim(),
    ];
    if values.iter().any(|value| value.is_empty()) {
        return Err("Qoder conversation scope contains an empty component".to_string());
    }
    let digest = scoped_digest(
        b"cc-switch-server:qoder-conversation:v1\0",
        [scope.key(), values[0], values[1], values[2], values[3]],
        "",
    );
    let hex = digest.trim_start_matches(':');
    Ok(format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedQoderPayload {
    pub body: Value,
    pub request_id: String,
    pub session_id: String,
    pub model_key: String,
}

pub fn build_qoder_payload(
    canonical_chat_request: &Value,
    exact_model_config: &Value,
    site: QoderSite,
    model_key: &str,
    session_id: &str,
    user_type: &str,
    now_ms: i64,
) -> Result<PreparedQoderPayload, String> {
    let request = canonical_chat_request
        .as_object()
        .ok_or_else(|| "Qoder canonical Chat request must be an object".to_string())?;
    let model_key = model_key.trim();
    let session_id = session_id.trim();
    if model_key.is_empty() || session_id.is_empty() {
        return Err("Qoder model and session identifiers are required".to_string());
    }
    let mut model_config = exact_model_config
        .as_object()
        .cloned()
        .ok_or_else(|| "Qoder live model_config must be an object".to_string())?;
    let config_key = model_config
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if config_key != model_key {
        return Err(format!(
            "Qoder live model_config key {config_key:?} does not match requested key {model_key:?}"
        ));
    }

    let (system, mut messages, last_user_text) = normalize_chat_messages(request.get("messages"))?;
    if !system.is_empty() {
        messages.insert(0, qoder_message("system", &system, None, None));
    }
    add_ephemeral_cache_control(&mut messages);
    let tools = normalized_tools(request)?;
    let max_tokens = qoder_max_tokens(request, &model_config);
    let reasoning = qoder_reasoning_directive(request);
    apply_reasoning_directive(site, model_key, reasoning, &mut model_config);

    let request_id = crate::domain::qoder::random_qoder_uuid();
    let request_set_id = crate::domain::qoder::random_qoder_uuid();
    let business_id = crate::domain::qoder::random_qoder_uuid();
    let mut compact_model_config = Map::new();
    for field in ["key", "source", "is_reasoning", "reasoning_effort"] {
        if let Some(value) = model_config.get(field) {
            compact_model_config.insert(field.to_string(), value.clone());
        }
    }
    compact_model_config
        .entry("key".to_string())
        .or_insert_with(|| Value::String(model_key.to_string()));

    let display_prompt = truncate_chars(&last_user_text, 30);
    let effective_user_type = if user_type.trim().is_empty() {
        "personal_standard"
    } else {
        user_type.trim()
    };
    let body = json!({
        "request_id": request_id,
        "request_set_id": request_set_id,
        "chat_record_id": request_id,
        "stream": true,
        "chat_task": "FREE_INPUT",
        "image_urls": null,
        "is_reply": true,
        "is_retry": false,
        "session_id": session_id,
        "code_language": "",
        "source": 1,
        "version": "3",
        "chat_prompt": "",
        "parameters": {"max_tokens": max_tokens},
        "aliyun_user_type": effective_user_type,
        "session_type": "qodercli",
        "agent_id": "agent_common",
        "task_id": "common",
        "chat_context": {
            "chatPrompt": "",
            "features": [],
            "imageUrls": null,
            "text": {"type": "text", "text": last_user_text},
            "extra": {
                "context": [],
                "modelConfig": compact_model_config,
                "originalContent": {"type": "text", "text": last_user_text}
            }
        },
        "model_config": model_config,
        "messages": messages,
        "tools": tools,
        "business": {
            "product": "cli",
            "version": site.profile().client_version,
            "type": "agent",
            "stage": "init",
            "id": business_id,
            "name": display_prompt,
            "begin_at": now_ms
        }
    });
    Ok(PreparedQoderPayload {
        body,
        request_id,
        session_id: session_id.to_string(),
        model_key: model_key.to_string(),
    })
}

fn normalize_chat_messages(value: Option<&Value>) -> Result<(String, Vec<Value>, String), String> {
    let messages = value
        .and_then(Value::as_array)
        .ok_or_else(|| "Qoder canonical Chat request must contain messages".to_string())?;
    let mut system_parts = Vec::new();
    let mut output = Vec::new();
    let mut last_user_text = String::new();
    for message in messages {
        let object = message
            .as_object()
            .ok_or_else(|| "Qoder Chat message must be an object".to_string())?;
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("user");
        let text = chat_content_text(object.get("content"))?;
        match role {
            "system" | "developer" => {
                if !text.trim().is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => {
                last_user_text = text.clone();
                output.push(qoder_message("user", &text, None, None));
            }
            "assistant" => {
                let tool_calls = normalized_tool_calls(object.get("tool_calls"))?;
                output.push(qoder_message(
                    "assistant",
                    &text,
                    (!tool_calls.is_empty()).then_some(tool_calls),
                    None,
                ));
            }
            "tool" => {
                let call_id = object
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "Qoder tool message is missing tool_call_id".to_string())?;
                let mut message =
                    qoder_message("tool", empty_placeholder(&text), None, Some(call_id));
                if let Some(name) = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    message
                        .as_object_mut()
                        .expect("Qoder message is an object")
                        .insert("name".to_string(), Value::String(name.to_string()));
                }
                output.push(message);
            }
            "function" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "Qoder legacy function result is missing name".to_string())?;
                let mut message = qoder_message("tool", empty_placeholder(&text), None, Some(name));
                message
                    .as_object_mut()
                    .expect("Qoder message is an object")
                    .insert("name".to_string(), Value::String(name.to_string()));
                output.push(message);
            }
            other => return Err(format!("unsupported Qoder Chat message role {other:?}")),
        }
    }
    Ok((system_parts.join("\n\n"), output, last_user_text))
}

fn chat_content_text(value: Option<&Value>) -> Result<String, String> {
    match value.unwrap_or(&Value::Null) {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value.clone()),
        Value::Array(parts) => {
            let mut output = Vec::new();
            for part in parts {
                let object = part
                    .as_object()
                    .ok_or_else(|| "Qoder Chat content part must be an object".to_string())?;
                let kind = object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match kind {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = object.get("text").and_then(Value::as_str) {
                            output.push(text.to_string());
                        }
                    }
                    "image" | "image_url" | "input_image" => {
                        return Err(
                            "Qoder image input is not supported by the verified COSY wire contract"
                                .to_string(),
                        )
                    }
                    other => {
                        return Err(format!(
                            "unsupported Qoder Chat content part type {other:?}"
                        ))
                    }
                }
            }
            Ok(output.join("\n"))
        }
        _ => Err("Qoder Chat content must be text, an array, or null".to_string()),
    }
}

fn qoder_message(
    role: &str,
    text: &str,
    tool_calls: Option<Vec<Value>>,
    tool_call_id: Option<&str>,
) -> Value {
    let content = if role == "user" { "" } else { text };
    let contents = if text.is_empty() {
        Vec::new()
    } else {
        vec![json!({"type": "text", "text": text})]
    };
    let mut message = json!({
        "role": role,
        "content": content,
        "contents": contents,
        "reasoning_content_signature": "",
        "response_meta": blank_response_meta()
    });
    let object = message.as_object_mut().expect("Qoder message is an object");
    if let Some(tool_calls) = tool_calls {
        object.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    if let Some(call_id) = tool_call_id {
        object.insert(
            "tool_call_id".to_string(),
            Value::String(call_id.to_string()),
        );
        object.insert(
            "tool_call_call_id".to_string(),
            Value::String(call_id.to_string()),
        );
    }
    message
}

fn blank_response_meta() -> Value {
    json!({
        "id": "",
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "completion_tokens_details": {"reasoning_tokens": 0},
            "prompt_tokens_details": {"cached_tokens": 0}
        }
    })
}

fn normalized_tool_calls(value: Option<&Value>) -> Result<Vec<Value>, String> {
    let Some(calls) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut output = Vec::with_capacity(calls.len());
    for call in calls {
        let object = call
            .as_object()
            .ok_or_else(|| "Qoder tool call must be an object".to_string())?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Qoder tool call is missing id".to_string())?;
        let function = object
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| "Qoder tool call is missing function".to_string())?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Qoder tool call function is missing name".to_string())?;
        let arguments = match function.get("arguments") {
            None | Some(Value::Null) => "{}".to_string(),
            Some(Value::String(value)) if value.trim().is_empty() => "{}".to_string(),
            Some(Value::String(value)) => value.clone(),
            Some(value) => serde_json::to_string(value)
                .map_err(|error| format!("encode Qoder tool arguments: {error}"))?,
        };
        output.push(json!({
            "id": id,
            "type": object.get("type").and_then(Value::as_str).unwrap_or("function"),
            "function": {"name": name, "arguments": arguments}
        }));
    }
    Ok(output)
}

fn normalized_tools(request: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let mut tools = request
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if request.get("tool_choice").and_then(Value::as_str) == Some("none") {
        return Ok(Vec::new());
    }
    let selected_name = request
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|choice| choice.get("function"))
        .and_then(Value::as_object)
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    for tool in &tools {
        let function = tool
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| "Qoder tool must contain a function object".to_string())?;
        if function
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            return Err("Qoder tool function is missing name".to_string());
        }
    }
    if let Some(selected_name) = selected_name {
        let selected = tools
            .iter()
            .filter(|tool| {
                tool.pointer("/function/name").and_then(Value::as_str) == Some(selected_name)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !selected.is_empty() {
            tools = selected;
        }
    }
    Ok(tools)
}

fn qoder_max_tokens(request: &Map<String, Value>, config: &Map<String, Value>) -> u64 {
    let configured = config
        .get("max_output_tokens")
        .and_then(positive_u64)
        .unwrap_or(32_768);
    ["max_completion_tokens", "max_tokens"]
        .into_iter()
        .find_map(|field| request.get(field).and_then(positive_u64))
        .map(|requested| requested.min(configured))
        .unwrap_or(configured)
}

fn positive_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .filter(|value| *value > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningDirective {
    Unspecified,
    Disabled,
    Enabled(&'static str),
}

fn qoder_reasoning_directive(request: &Map<String, Value>) -> ReasoningDirective {
    let raw = request
        .get("reasoning_effort")
        .or_else(|| {
            request
                .get("reasoning")
                .and_then(Value::as_object)
                .and_then(|reasoning| reasoning.get("effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    match raw.as_deref() {
        None | Some("") => ReasoningDirective::Unspecified,
        Some("none" | "disabled" | "off") => ReasoningDirective::Disabled,
        Some("minimal" | "low" | "medium") => ReasoningDirective::Enabled("high"),
        Some("high" | "xhigh" | "very_high" | "very-high" | "max") => {
            ReasoningDirective::Enabled("max")
        }
        Some(_) => ReasoningDirective::Unspecified,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingCapability {
    Unsupported,
    ToggleOnly,
    HighMax,
}

fn thinking_capability(site: QoderSite, model_key: &str) -> ThinkingCapability {
    match model_key {
        "qmodel_38max" | "qmodel_latest" | "qmodel" => ThinkingCapability::ToggleOnly,
        "dmodel" | "dfmodel" | "gm51model" => ThinkingCapability::HighMax,
        "q36fmodel" if site == QoderSite::Cn => ThinkingCapability::Unsupported,
        _ => ThinkingCapability::Unsupported,
    }
}

fn apply_reasoning_directive(
    site: QoderSite,
    model_key: &str,
    directive: ReasoningDirective,
    model_config: &mut Map<String, Value>,
) {
    let capability = thinking_capability(site, model_key);
    if capability == ThinkingCapability::Unsupported || directive == ReasoningDirective::Unspecified
    {
        return;
    }
    let enabled = !matches!(directive, ReasoningDirective::Disabled);
    model_config.insert("is_reasoning".to_string(), Value::Bool(enabled));
    if capability == ThinkingCapability::HighMax || !enabled {
        let effort = match directive {
            ReasoningDirective::Enabled(effort) => effort,
            ReasoningDirective::Disabled => "none",
            ReasoningDirective::Unspecified => return,
        };
        model_config.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
    }
}

fn add_ephemeral_cache_control(messages: &mut [Value]) {
    for message in messages.iter_mut().rev() {
        let Some(contents) = message.get_mut("contents").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in contents.iter_mut().rev() {
            if block.get("type").and_then(Value::as_str) != Some("text")
                || block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                continue;
            }
            block
                .as_object_mut()
                .expect("Qoder content block is an object")
                .entry("cache_control".to_string())
                .or_insert_with(|| json!({"type": "ephemeral"}));
            return;
        }
    }
}

fn empty_placeholder(value: &str) -> &str {
    if value.is_empty() {
        "(empty)"
    } else {
        value
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut output = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::qoder::{QoderIdentity, QoderMachineIdentity};

    fn scope(
        token_generation: u64,
        site: QoderSite,
        rail: QoderCredentialRail,
    ) -> QoderRuntimeScope {
        QoderRuntimeScope::derive(
            "claude",
            "provider-a",
            3,
            "runtime-a",
            "account-a",
            site,
            rail,
            7,
            token_generation,
        )
        .unwrap()
    }

    fn session() -> QoderCosySession {
        QoderCosySession::new_with_key(
            QoderSite::Global,
            QoderIdentity {
                name: String::new(),
                aid: "aid-a".to_string(),
                uid: "uid-a".to_string(),
                organization_id: String::new(),
                organization_name: String::new(),
                user_type: "personal_standard".to_string(),
                security_oauth_token: "secret-job-token".to_string(),
                refresh_token: String::new(),
            },
            QoderMachineIdentity {
                machine_id: "machine-a".to_string(),
                machine_token: "machine-token-a".to_string(),
                machine_type: "5".to_string(),
            },
            b"0123456789abcdef",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cache_fences_site_rail_and_both_account_generations() {
        let cache = QoderRuntimeCache::default();
        let current = scope(4, QoderSite::Global, QoderCredentialRail::GlobalOauth);
        let rotated = scope(5, QoderSite::Global, QoderCredentialRail::GlobalOauth);
        let pat = scope(4, QoderSite::Global, QoderCredentialRail::PatJobToken);
        let cn = scope(4, QoderSite::Cn, QoderCredentialRail::CnOauth);
        cache
            .insert_session(
                current.clone(),
                QoderCachedSession::new(
                    session(),
                    "https://api1.qoder.sh".to_string(),
                    Some(10_000_000),
                    1_000,
                )
                .unwrap(),
            )
            .await;
        assert!(cache.session(&current, 2_000).await.is_some());
        assert!(cache.session(&rotated, 2_000).await.is_none());
        assert!(cache.session(&pat, 2_000).await.is_none());
        assert!(cache.session(&cn, 2_000).await.is_none());
    }

    #[tokio::test]
    async fn authoritative_empty_catalog_is_cached() {
        let cache = QoderRuntimeCache::default();
        let scope = scope(4, QoderSite::Global, QoderCredentialRail::GlobalOauth);
        cache
            .insert_catalog(
                scope.clone(),
                FetchedQoderModelCatalog {
                    enabled_models: Vec::new(),
                    raw_configs: BTreeMap::new(),
                },
                1_000,
            )
            .await;
        let catalog = cache.catalog(&scope, 1_001).await.unwrap();
        assert!(catalog.enabled_models.is_empty());
        assert!(catalog.raw_configs.is_empty());
    }

    #[test]
    fn catalog_keeps_hidden_raw_config_but_only_exposes_enabled_models() {
        let catalog = parse_qoder_model_catalog(&json!({
            "chat": [
                {"key": "auto", "display_name": "Auto", "enable": true},
                {"key": "hidden", "display_name": "Hidden", "enable": false}
            ]
        }))
        .unwrap();
        assert_eq!(catalog.enabled_models, vec!["auto"]);
        assert!(catalog.raw_configs.contains_key("hidden"));
        assert_eq!(
            parse_qoder_model_catalog(&json!({"chat": []})).unwrap(),
            FetchedQoderModelCatalog {
                enabled_models: Vec::new(),
                raw_configs: BTreeMap::new()
            }
        );
        assert!(parse_qoder_model_catalog(&json!({"models": []})).is_err());
    }

    #[test]
    fn aliases_are_site_specific_and_unknown_keys_remain_exact() {
        assert_eq!(
            resolve_qoder_model_key(QoderSite::Global, "qoder/claude-opus-4-6").unwrap(),
            "ultimate"
        );
        assert_eq!(
            resolve_qoder_model_key(QoderSite::Cn, "qwen3.6-flash").unwrap(),
            "q36fmodel"
        );
        assert_eq!(
            resolve_qoder_model_key(QoderSite::Global, "future-route").unwrap(),
            "future-route"
        );
    }

    #[test]
    fn payload_hoists_control_roles_and_preserves_tools_ids_and_reasoning() {
        let request = json!({
            "model": "glm-5.2",
            "messages": [
                {"role": "system", "content": "system-a"},
                {"role": "developer", "content": [{"type":"text","text":"system-b"}]},
                {"role": "user", "content": "run"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call-a", "type": "function",
                    "function": {"name": "shell", "arguments": {"cmd":"pwd"}}
                }]},
                {"role": "tool", "tool_call_id": "call-a", "name": "shell", "content": "ok"}
            ],
            "tools": [{"type":"function","function":{"name":"shell","parameters":{"type":"object"}}}],
            "reasoning_effort": "high",
            "max_completion_tokens": 2000
        });
        let prepared = build_qoder_payload(
            &request,
            &json!({
                "key":"gm51model", "display_name":"GLM", "enable":true,
                "is_reasoning":false, "max_output_tokens":4096, "vendor":"preserved"
            }),
            QoderSite::Global,
            "gm51model",
            "session-a",
            "personal_standard",
            1_700_000_000_000,
        )
        .unwrap();
        assert_eq!(prepared.body["messages"][0]["role"], "system");
        assert_eq!(
            prepared.body["messages"][0]["contents"][0]["text"],
            "system-a\n\nsystem-b"
        );
        assert_eq!(
            prepared.body["messages"][2]["tool_calls"][0]["id"],
            "call-a"
        );
        assert_eq!(prepared.body["messages"][3]["tool_call_call_id"], "call-a");
        assert_eq!(prepared.body["parameters"]["max_tokens"], 2000);
        assert_eq!(prepared.body["model_config"]["vendor"], "preserved");
        assert_eq!(prepared.body["model_config"]["is_reasoning"], true);
        assert_eq!(prepared.body["model_config"]["reasoning_effort"], "max");
        assert_eq!(prepared.body["stream"], true);
    }

    #[test]
    fn payload_rejects_images_and_non_exact_model_config() {
        let image = json!({
            "messages": [{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]
        });
        assert!(build_qoder_payload(
            &image,
            &json!({"key":"auto"}),
            QoderSite::Global,
            "auto",
            "session-a",
            "",
            1,
        )
        .unwrap_err()
        .contains("image input"));
        let text = json!({"messages":[{"role":"user","content":"hi"}]});
        assert!(build_qoder_payload(
            &text,
            &json!({"key":"other"}),
            QoderSite::Global,
            "auto",
            "session-a",
            "",
            1,
        )
        .is_err());
    }

    #[test]
    fn conversation_session_is_stable_and_scope_isolating() {
        let first = scope(4, QoderSite::Global, QoderCredentialRail::GlobalOauth);
        let rotated = scope(5, QoderSite::Global, QoderCredentialRail::GlobalOauth);
        let a = derive_qoder_conversation_session_id(&first, "share", "user", "session", "auto")
            .unwrap();
        let b = derive_qoder_conversation_session_id(&first, "share", "user", "session", "auto")
            .unwrap();
        let c = derive_qoder_conversation_session_id(&rotated, "share", "user", "session", "auto")
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
