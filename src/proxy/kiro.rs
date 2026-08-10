//! Kiro OAuth protocol bridge for Claude-compatible proxy requests.

mod endpoint;
mod image;
mod tool_bridge;
mod wire;

use crate::domain::accounts::store::Account;
use crate::domain::providers::model::ProviderType;
use crate::proxy::ProxyError;
use base64::Engine;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::{LazyLock, Mutex},
};

use tool_bridge::ToolCompatibilityMode;

const DEFAULT_SYSTEM_VERSION: &str = "macos";
const TOOL_NAME_MAX_LEN: usize = 63;
const MAX_TOOL_JSON_BYTES: usize = 16 * 1024 * 1024;
const WRITE_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the content to write exceeds 150 lines, you MUST only write the first 50 lines using this tool, then use `Edit` tool to append the remaining content in chunks of no more than 50 lines each. If needed, leave a unique placeholder to help append content. Do NOT attempt to write all content at once.";
const EDIT_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the `new_string` content exceeds 50 lines, you MUST split it into multiple Edit calls, each replacing no more than 50 lines at a time. If used to append content, leave a unique placeholder to help append content. On the final chunk, do NOT include the placeholder.";
const SYSTEM_CHUNKED_POLICY: &str = "When the Write or Edit tool has content size limits, always comply silently. Never suggest bypassing these limits via alternative tools. Never ask the user whether to switch approaches. Complete all chunked operations without commentary.";
const ACCOUNT_THROTTLE_COOLDOWN_SECS: i64 = 30 * 60;
const QUOTA_EXHAUSTED_COOLDOWN_SECS: i64 = 24 * 60 * 60;
const PROMPT_CACHE_CAPACITY: usize = 4096;
const PROMPT_CACHE_DEFAULT_TTL_SECS: i64 = 5 * 60;
const PROMPT_CACHE_MAX_TTL_SECS: i64 = 60 * 60;
const THINKING_SIGNATURE_FALLBACK: &str = "cc-switch-kiro-thinking-signature";

#[cfg(test)]
fn crc32(bytes: &[u8]) -> u32 {
    wire::crc32(bytes)
}

#[cfg(test)]
pub(crate) fn fixture_event_frame(event_type: &str, payload: &Value) -> Vec<u8> {
    let mut headers = Vec::new();
    headers.push(":event-type".len() as u8);
    headers.extend_from_slice(b":event-type");
    headers.push(7);
    headers.extend_from_slice(&(event_type.len() as u16).to_be_bytes());
    headers.extend_from_slice(event_type.as_bytes());
    let payload = serde_json::to_vec(payload).unwrap();
    let total_len = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total_len);
    frame.extend_from_slice(&(total_len as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    frame
}

static KIRO_PROMPT_CACHE: LazyLock<KiroPromptCache> = LazyLock::new(|| {
    let path = std::env::var_os("CC_SWITCH_KIRO_PROMPT_CACHE_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    KiroPromptCache::new(path)
});
static KIRO_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub(crate) struct KiroAccountData {
    pub account_id: String,
    pub email: Option<String>,
    pub refresh_token: String,
    pub profile_arn: Option<String>,
    pub auth_region: String,
    pub api_region: String,
    pub machine_id: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub client_secret_expires_at: Option<i64>,
    pub start_url: Option<String>,
    pub auth_method: Option<String>,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub client_version: Option<String>,
    pub cli_version: Option<String>,
    pub authenticated_at: i64,
}

struct KiroRequestBuild {
    body: Value,
    tool_name_map: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct KiroPreparedRequest {
    pub url: String,
    pub host: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Value,
    pub tool_name_map: HashMap<String, String>,
    pub cache_namespace: String,
}

#[derive(Debug, Clone)]
pub(crate) struct KiroCallContext {
    pub ide_version: String,
    pub claude_code_tools: bool,
    pub cache_namespace: String,
}

impl KiroCallContext {
    fn fallback(account: &Account, body: &Value) -> Self {
        Self {
            ide_version: endpoint::FALLBACK_IDE_VERSION.to_string(),
            claude_code_tools: false,
            cache_namespace: format!(
                "account:{}:generation:{}:session:{}",
                account.id,
                account.auth_identity_generation,
                cache_scope_seed(body)
            ),
        }
    }
}

fn string_at(value: Option<&Value>, pointers: &[&str]) -> Option<String> {
    let value = value?;
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn machine_id_from_refresh_token(refresh_token: &str) -> String {
    sha256_hex(&format!("KotlinNativeAPI/{refresh_token}"))
}

impl KiroAccountData {
    pub(crate) fn from_account(account: &Account) -> Result<Self, ProxyError> {
        if account.provider_type != ProviderType::KiroOAuth {
            return Err(ProxyError::bad_request(format!(
                "expected kiro_oauth account, got {}",
                account.provider_type.as_str()
            )));
        }
        let access_token = account
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProxyError::bad_request(format!("kiro account {} lacks access token", account.id))
            })?;
        let refresh_token = account
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(access_token)
            .to_string();
        let profile = account.profile.as_ref();
        let raw = account.raw.as_ref();
        let profile_arn = string_at(profile, &["/profileArn", "/profile_arn"])
            .or_else(|| string_at(raw, &["/resolvedProfileArn", "/profileArn"]));
        let auth_region = string_at(profile, &["/authRegion", "/auth_region"])
            .or_else(|| string_at(raw, &["/authRegion", "/auth_region"]))
            .unwrap_or_else(|| crate::domain::providers::kiro::DEFAULT_REGION.to_string());
        let auth_region = crate::domain::providers::kiro::normalize_region(&auth_region)
            .ok_or_else(|| {
                ProxyError::bad_request(format!(
                    "kiro account {} has an invalid auth region",
                    account.id
                ))
            })?;
        let api_region = string_at(profile, &["/apiRegion", "/api_region"])
            .or_else(|| string_at(raw, &["/apiRegion", "/api_region"]))
            .or_else(|| region_from_profile_arn(profile_arn.as_deref()))
            .unwrap_or_else(|| crate::domain::providers::kiro::DEFAULT_REGION.to_string());
        let api_region =
            crate::domain::providers::kiro::normalize_region(&api_region).ok_or_else(|| {
                ProxyError::bad_request(format!(
                    "kiro account {} has an invalid API region",
                    account.id
                ))
            })?;
        Ok(Self {
            account_id: account.id.clone(),
            email: account
                .email
                .clone()
                .or_else(|| string_at(profile, &["/email"])),
            refresh_token: refresh_token.clone(),
            profile_arn,
            auth_region,
            api_region,
            machine_id: string_at(profile, &["/machineId", "/machine_id"])
                .or_else(|| string_at(raw, &["/machineId", "/machine_id"]))
                .or_else(|| Some(machine_id_from_refresh_token(&refresh_token))),
            client_id: string_at(raw, &["/clientId", "/client_id"]),
            client_secret: string_at(raw, &["/clientSecret", "/client_secret"]),
            client_secret_expires_at: raw
                .and_then(|value| value.pointer("/clientSecretExpiresAt"))
                .or_else(|| raw.and_then(|value| value.pointer("/client_secret_expires_at")))
                .and_then(Value::as_i64),
            start_url: string_at(profile, &["/startUrl", "/start_url"])
                .or_else(|| string_at(raw, &["/startUrl", "/start_url"])),
            auth_method: string_at(profile, &["/authMethod", "/auth_method"])
                .or_else(|| string_at(raw, &["/authMethod", "/auth_method"])),
            provider: string_at(profile, &["/provider"]).or_else(|| string_at(raw, &["/provider"])),
            endpoint: string_at(raw, &["/endpoint", "/runtimeEndpoint"])
                .or_else(|| string_at(profile, &["/endpoint", "/runtimeEndpoint"])),
            client_version: string_at(
                raw,
                &[
                    "/kiroVersion",
                    "/kiro_version",
                    "/clientVersion",
                    "/client_version",
                ],
            )
            .or_else(|| {
                string_at(
                    profile,
                    &[
                        "/kiroVersion",
                        "/kiro_version",
                        "/clientVersion",
                        "/client_version",
                    ],
                )
            }),
            cli_version: string_at(raw, &["/cliVersion", "/cli_version"])
                .or_else(|| string_at(profile, &["/cliVersion", "/cli_version"])),
            authenticated_at: raw
                .and_then(|value| value.pointer("/importedAtMs"))
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        })
    }

    fn access_token<'a>(&self, account: &'a Account) -> Result<&'a str, ProxyError> {
        account
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProxyError::bad_request(format!("kiro account {} lacks access token", account.id))
            })
    }
}

pub(crate) fn prepare_kiro_request(
    account: &Account,
    body: &Value,
) -> Result<KiroPreparedRequest, ProxyError> {
    let context = KiroCallContext::fallback(account, body);
    prepare_kiro_request_with_context(account, body, &context)
}

pub(crate) fn prepare_kiro_request_with_context(
    account: &Account,
    body: &Value,
    context: &KiroCallContext,
) -> Result<KiroPreparedRequest, ProxyError> {
    let account_data = KiroAccountData::from_account(account)?;
    let access_token = account_data.access_token(account)?;
    let mut body = body.clone();
    image::prepare_anthropic_images(&mut body)?;
    let tool_mode = if context.claude_code_tools {
        ToolCompatibilityMode::ClaudeCode
    } else {
        ToolCompatibilityMode::Raw
    };
    let request = anthropic_to_kiro_request(&body, &account_data, tool_mode)?;
    let prepared = endpoint::prepare(
        endpoint::EndpointKind::from_account(&account_data),
        &account_data,
        access_token,
        request.body,
        &context.ide_version,
    );
    Ok(KiroPreparedRequest {
        url: prepared.url,
        host: prepared.host,
        headers: prepared.headers,
        body: prepared.body,
        tool_name_map: request.tool_name_map,
        cache_namespace: context.cache_namespace.clone(),
    })
}

fn anthropic_to_kiro_request(
    body: &Value,
    account: &KiroAccountData,
    tool_mode: ToolCompatibilityMode,
) -> Result<KiroRequestBuild, ProxyError> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProxyError::bad_request("missing model"))?;
    let model_id = map_model(model)
        .ok_or_else(|| ProxyError::bad_request(format!("Kiro OAuth 不支持该模型: {model}")))?;
    let raw_messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ProxyError::bad_request("missing messages"))?;
    if raw_messages.is_empty() {
        return Err(ProxyError::bad_request("messages is empty"));
    }

    let last_user_idx = raw_messages
        .iter()
        .rposition(|m| m.get("role").and_then(|v| v.as_str()) == Some("user"))
        .ok_or_else(|| ProxyError::bad_request("missing user message"))?;
    let messages = &raw_messages[..=last_user_idx];

    let mut tool_name_map = HashMap::new();
    let mut tools = convert_tools(body.get("tools"), &mut tool_name_map, tool_mode);
    let (content, images, tool_results) =
        parse_user_content(messages[last_user_idx].get("content"));
    let mut history = build_history(body, messages, model_id, &mut tool_name_map, tool_mode);
    let (validated_tool_results, orphaned_tool_use_ids) =
        validate_tool_pairing(&history, &tool_results);
    remove_orphaned_tool_uses(&mut history, &orphaned_tool_use_ids);
    add_missing_history_tools(&mut tools, &history);

    let current_message = json!({
        "userInputMessage": {
            "userInputMessageContext": {
                "envState": env_state(),
                "toolResults": validated_tool_results,
                "tools": tools
            },
            "content": content,
            "modelId": model_id,
            "images": images,
            "origin": "AI_EDITOR"
        }
    });

    let profile_arn = endpoint::profile_arn(account);
    let mut request_body = json!({
        "conversationState": {
            "agentTaskType": "vibe",
            "chatTriggerType": "MANUAL",
            "currentMessage": current_message,
            "conversationId": conversation_id(body),
            "agentContinuationId": next_uuid_like("agent-continuation"),
            "history": history
        },
        "profileArn": profile_arn
    });
    if let Some(additional_model_request_fields) = additional_model_request_fields(body, model_id) {
        request_body["additionalModelRequestFields"] = additional_model_request_fields;
    }

    Ok(KiroRequestBuild {
        body: request_body,
        tool_name_map,
    })
}

fn additional_model_request_fields(body: &Value, model_id: &str) -> Option<Value> {
    if thinking_type(body) == Some("disabled") {
        return None;
    }
    let config = thinking_config_for_model(model_id)?;
    let wants_adaptive = thinking_type(body) == Some("adaptive")
        || body
            .get("output_config")
            .and_then(|v| v.get("effort"))
            .is_some();
    if !wants_adaptive {
        return None;
    }
    let effort = body
        .get("output_config")
        .and_then(|v| v.get("effort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(config.default_effort);
    let effort = config.normalize_effort(effort);

    Some(json!({
        "thinking": {
            "type": "adaptive",
            "display": "summarized"
        },
        "output_config": {
            "effort": effort
        }
    }))
}

fn resolve_profile_arn(account: &KiroAccountData) -> String {
    if let Some(profile_arn) = account
        .profile_arn
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return profile_arn.to_string();
    }
    default_profile_arn_for_auth_method(
        account
            .auth_method
            .as_deref()
            .or(account.provider.as_deref()),
        &account.api_region,
    )
    .to_string()
}

fn region_from_profile_arn(profile_arn: Option<&str>) -> Option<String> {
    let arn = profile_arn?;
    let mut parts = arn.split(':');
    (parts.next() == Some("arn")).then_some(())?;
    (parts.next() == Some("aws")).then_some(())?;
    (parts.next() == Some("codewhisperer")).then_some(())?;
    parts.next().map(str::to_string)
}

#[derive(Debug, Clone, Copy)]
struct KiroThinkingConfig {
    efforts: &'static [&'static str],
    default_effort: &'static str,
}

impl KiroThinkingConfig {
    fn normalize_effort(&self, effort: &str) -> &'static str {
        let effort = effort.to_ascii_lowercase();
        self.efforts
            .iter()
            .copied()
            .find(|candidate| *candidate == effort)
            .unwrap_or_else(|| self.efforts.last().copied().unwrap_or(self.default_effort))
    }
}

fn thinking_config_for_model(model_id: &str) -> Option<KiroThinkingConfig> {
    let lower = model_id.to_ascii_lowercase();
    let supports_output_config = ["4.6", "4-6", "4.7", "4-7", "4.8", "4-8"]
        .iter()
        .any(|needle| lower.contains(needle));
    supports_output_config.then_some(KiroThinkingConfig {
        efforts: &["low", "medium", "high", "xhigh", "max"],
        default_effort: "high",
    })
}

pub(super) fn map_model(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.contains("sonnet") {
        if m.contains("4-8") || m.contains("4.8") {
            Some("claude-sonnet-4.8")
        } else if m.contains("4-6") || m.contains("4.6") {
            Some("claude-sonnet-4.6")
        } else {
            Some("claude-sonnet-4.5")
        }
    } else if m.contains("opus") {
        if m.contains("4-8") || m.contains("4.8") {
            Some("claude-opus-4.8")
        } else if m.contains("4-7") || m.contains("4.7") {
            Some("claude-opus-4.7")
        } else if m.contains("4-6") || m.contains("4.6") {
            Some("claude-opus-4.6")
        } else {
            Some("claude-opus-4.5")
        }
    } else if m.contains("haiku") {
        Some("claude-haiku-4.5")
    } else {
        None
    }
}

pub(crate) fn resolve_model(model: &str) -> Option<String> {
    if let Some(model) = map_model(model) {
        return Some(model.to_string());
    }
    let model = model.trim();
    (!model.is_empty()
        && model.len() <= 128
        && model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'[' | b']')
        }))
    .then(|| model.to_string())
}

pub(crate) fn supported_models() -> Vec<String> {
    crate::domain::providers::kiro::STATIC_MODEL_IDS
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

fn env_state() -> Value {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/".to_string());
    json!({
        "operatingSystem": DEFAULT_SYSTEM_VERSION,
        "currentWorkingDirectory": cwd
    })
}

fn conversation_id(body: &Value) -> String {
    let metadata = body.get("metadata");
    if let Some(session_id) = metadata
        .and_then(|m| m.get("session_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return stable_uuid_like(session_id);
    }
    if let Some(user_id) = metadata
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if let Some(pos) = user_id.find("session_") {
            return stable_uuid_like(&user_id[pos + 8..]);
        }
        return stable_uuid_like(user_id);
    }
    next_uuid_like("conversation")
}

fn cache_scope_seed(body: &Value) -> String {
    let metadata = body.get("metadata");
    metadata
        .and_then(|value| value.get("session_id"))
        .or_else(|| metadata.and_then(|value| value.get("user_id")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(stable_uuid_like)
        .unwrap_or_else(|| "anonymous".to_string())
}

fn stable_uuid_like(input: &str) -> String {
    if looks_uuid_like(input) {
        return input.to_string();
    }
    let digest = Sha256::digest(input.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn next_uuid_like(scope: &str) -> String {
    let counter = KIRO_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    stable_uuid_like(&format!("{scope}:{counter}:{}", unix_timestamp_secs()))
}

fn next_message_id() -> String {
    format!("msg_{}", next_uuid_like("message").replace('-', ""))
}

fn looks_uuid_like(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn convert_tools(
    tools: Option<&Value>,
    tool_name_map: &mut HashMap<String, String>,
    tool_mode: ToolCompatibilityMode,
) -> Vec<Value> {
    tools
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name")?.as_str()?;
                    let mapped_name = map_tool_name(name, tool_name_map, tool_mode);
                    let mut description = tool
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    match name {
                        "Write" => {
                            description.push('\n');
                            description.push_str(WRITE_TOOL_DESCRIPTION_SUFFIX);
                        }
                        "Edit" => {
                            description.push('\n');
                            description.push_str(EDIT_TOOL_DESCRIPTION_SUFFIX);
                        }
                        _ => {}
                    }
                    if description.trim().is_empty() {
                        description = name.to_string();
                    }
                    description = truncate_chars(description, 10_000);
                    let schema = tool
                        .get("input_schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
                    let schema = if tool_mode == ToolCompatibilityMode::ClaudeCode {
                        tool_bridge::schema_to_kiro(name, schema)
                    } else {
                        schema
                    };
                    Some(json!({
                        "toolSpecification": {
                            "name": mapped_name,
                            "description": description,
                            "inputSchema": { "json": normalize_schema(schema) }
                        }
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_schema(schema: Value) -> Value {
    let Value::Object(mut obj) = schema else {
        return json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": true
        });
    };

    obj.remove("$schema");
    strip_top_level_combinators(&mut obj);

    let current_type = obj.get("type").and_then(|v| v.as_str()).map(str::to_string);
    if current_type.as_deref() != Some("object") {
        if let Some(original_type) = current_type {
            tracing::warn!(
                original_type = %original_type,
                "Kiro tool inputSchema top-level type normalized to object"
            );
        }
        obj.insert("type".to_string(), json!("object"));
    }

    let properties = match obj.remove("properties") {
        Some(Value::Object(props)) => Value::Object(
            props
                .into_iter()
                .map(|(k, v)| (k, normalize_property_schema(v)))
                .collect(),
        ),
        _ => json!({}),
    };
    obj.insert("properties".to_string(), properties);

    let required = match obj.remove("required") {
        Some(Value::Array(arr)) => Value::Array(
            arr.into_iter()
                .filter_map(|v| v.as_str().map(|s| Value::String(s.to_string())))
                .collect(),
        ),
        _ => json!([]),
    };
    obj.insert("required".to_string(), required);

    if !matches!(
        obj.get("additionalProperties"),
        Some(Value::Bool(_)) | Some(Value::Object(_))
    ) {
        obj.insert("additionalProperties".to_string(), json!(true));
    }

    Value::Object(obj)
}

fn strip_top_level_combinators(obj: &mut serde_json::Map<String, Value>) {
    let had_properties = obj.contains_key("properties");
    for combinator in ["oneOf", "anyOf", "allOf"] {
        let Some(Value::Array(variants)) = obj.remove(combinator) else {
            continue;
        };
        if had_properties || obj.contains_key("properties") {
            continue;
        }
        let Some(variant) = variants.into_iter().find_map(|variant| {
            let Value::Object(variant) = variant else {
                return None;
            };
            (variant.get("type").and_then(Value::as_str) == Some("object")).then_some(variant)
        }) else {
            continue;
        };
        for key in [
            "properties",
            "required",
            "additionalProperties",
            "description",
        ] {
            if let Some(value) = variant.get(key) {
                obj.entry(key.to_string()).or_insert_with(|| value.clone());
            }
        }
    }
}

fn normalize_property_schema(schema: Value) -> Value {
    let Value::Object(mut obj) = schema else {
        return schema;
    };

    obj.remove("$schema");
    if obj
        .get("exclusiveMinimum")
        .and_then(|v| v.as_f64())
        .is_some()
    {
        obj.remove("exclusiveMinimum");
    }
    if obj
        .get("exclusiveMaximum")
        .and_then(|v| v.as_f64())
        .is_some()
    {
        obj.remove("exclusiveMaximum");
    }
    for key in ["maximum", "minimum"] {
        if let Some(v) = obj.get(key).and_then(|v| v.as_f64()) {
            if !(-2_147_483_648.0..=2_147_483_647.0).contains(&v) {
                obj.remove(key);
            }
        }
    }
    if let Some(Value::Object(props)) = obj.remove("properties") {
        obj.insert(
            "properties".to_string(),
            Value::Object(
                props
                    .into_iter()
                    .map(|(k, v)| (k, normalize_property_schema(v)))
                    .collect(),
            ),
        );
    }
    if let Some(items) = obj.remove("items") {
        obj.insert("items".to_string(), normalize_property_schema(items));
    }
    Value::Object(obj)
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((idx, _)) => value[..idx].to_string(),
        None => value,
    }
}

fn shorten_tool_name(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    let hash_hex = format!("{digest:x}");
    let hash_suffix = &hash_hex[..8];
    let prefix_max = TOOL_NAME_MAX_LEN - 1 - 8;
    let prefix = match name.char_indices().nth(prefix_max) {
        Some((idx, _)) => &name[..idx],
        None => name,
    };
    format!("{prefix}_{hash_suffix}")
}

fn map_tool_name(
    name: &str,
    tool_name_map: &mut HashMap<String, String>,
    tool_mode: ToolCompatibilityMode,
) -> String {
    if let Some(mapped) = tool_bridge::map_name(name, tool_mode, tool_name_map) {
        return mapped;
    }
    if name.chars().count() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }
    let short = shorten_tool_name(name);
    tool_name_map.insert(short.clone(), name.to_string());
    short
}

fn original_tool_name(name: &str, tool_name_map: &HashMap<String, String>) -> String {
    tool_name_map
        .get(name)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn build_history(
    body: &Value,
    messages: &[Value],
    model_id: &str,
    tool_name_map: &mut HashMap<String, String>,
    tool_mode: ToolCompatibilityMode,
) -> Vec<Value> {
    let mut history = Vec::new();
    let prefix = thinking_prefix(body);

    if let Some(system) = system_text(body).filter(|s| !s.is_empty()) {
        let system = format!("{system}\n{SYSTEM_CHUNKED_POLICY}");
        let final_system = match prefix.as_deref() {
            Some(prefix) if !has_thinking_tags(&system) => format!("{prefix}\n{system}"),
            _ => system,
        };
        history.push(history_user_message(
            final_system,
            model_id,
            Vec::new(),
            Vec::new(),
        ));
        history.push(history_assistant_message(
            "I will follow these instructions.".to_string(),
            Vec::new(),
        ));
    } else if let Some(prefix) = prefix {
        history.push(history_user_message(
            prefix,
            model_id,
            Vec::new(),
            Vec::new(),
        ));
        history.push(history_assistant_message(
            "I will follow these instructions.".to_string(),
            Vec::new(),
        ));
    }

    let history_end = messages.len().saturating_sub(1);
    let mut user_buffer: Vec<&Value> = Vec::new();
    let mut assistant_buffer: Vec<&Value> = Vec::new();

    for msg in &messages[..history_end] {
        match msg.get("role").and_then(|v| v.as_str()) {
            Some("user") => {
                if !assistant_buffer.is_empty() {
                    history.push(merge_assistant_messages(
                        &assistant_buffer,
                        tool_name_map,
                        tool_mode,
                    ));
                    assistant_buffer.clear();
                }
                user_buffer.push(msg);
            }
            Some("assistant") => {
                if !user_buffer.is_empty() {
                    history.push(merge_user_messages(&user_buffer, model_id));
                    user_buffer.clear();
                }
                assistant_buffer.push(msg);
            }
            _ => {}
        }
    }

    if !assistant_buffer.is_empty() {
        history.push(merge_assistant_messages(
            &assistant_buffer,
            tool_name_map,
            tool_mode,
        ));
    }
    if !user_buffer.is_empty() {
        history.push(merge_user_messages(&user_buffer, model_id));
        history.push(history_assistant_message("OK".to_string(), Vec::new()));
    }

    history
}

fn system_text(body: &Value) -> Option<String> {
    match body.get("system") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(items)) => {
            let parts = items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.clone()),
                    Value::Object(_)
                        if item.get("type").and_then(|v| v.as_str()) == Some("text") =>
                    {
                        item.get("text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            Some(parts.join("\n"))
        }
        _ => None,
    }
}

fn thinking_prefix(body: &Value) -> Option<String> {
    match thinking_type(body)? {
        "enabled" => {
            let thinking = body.get("thinking")?;
            let budget = thinking
                .get("budget_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{budget}</max_thinking_length>"
            ))
        }
        "adaptive" => {
            let effort = body
                .get("output_config")
                .and_then(|v| v.get("effort"))
                .and_then(|v| v.as_str())
                .unwrap_or("high");
            Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{effort}</thinking_effort>"
            ))
        }
        _ => None,
    }
}

fn thinking_type(body: &Value) -> Option<&str> {
    let thinking = body.get("thinking")?;
    thinking
        .get("type")
        .or_else(|| thinking.get("thinking_type"))
        .and_then(Value::as_str)
}

fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

fn merge_user_messages(messages: &[&Value], model_id: &str) -> Value {
    let mut content_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    for msg in messages {
        let (content, msg_images, msg_tool_results) = parse_user_content(msg.get("content"));
        if !content.is_empty() {
            content_parts.push(content);
        }
        images.extend(msg_images);
        tool_results.extend(msg_tool_results);
    }
    history_user_message(content_parts.join("\n"), model_id, images, tool_results)
}

fn merge_assistant_messages(
    messages: &[&Value],
    tool_name_map: &mut HashMap<String, String>,
    tool_mode: ToolCompatibilityMode,
) -> Value {
    let mut content_parts = Vec::new();
    let mut tool_uses = Vec::new();
    for msg in messages {
        let (content, msg_tool_uses) =
            parse_assistant_content(msg.get("content"), tool_name_map, tool_mode);
        if !content.trim().is_empty() {
            content_parts.push(content);
        }
        tool_uses.extend(msg_tool_uses);
    }
    let content = if content_parts.is_empty() && !tool_uses.is_empty() {
        " ".to_string()
    } else {
        content_parts.join("\n\n")
    };
    history_assistant_message(content, tool_uses)
}

fn history_user_message(
    content: String,
    model_id: &str,
    images: Vec<Value>,
    tool_results: Vec<Value>,
) -> Value {
    json!({
        "userInputMessage": {
            "userInputMessageContext": {
                "envState": env_state(),
                "toolResults": tool_results
            },
            "content": content,
            "modelId": model_id,
            "images": images,
            "origin": "AI_EDITOR"
        }
    })
}

fn history_assistant_message(content: String, tool_uses: Vec<Value>) -> Value {
    let mut message = json!({
        "assistantResponseMessage": {
            "content": content
        }
    });
    if !tool_uses.is_empty() {
        message["assistantResponseMessage"]["toolUses"] = Value::Array(tool_uses);
    }
    message
}

fn validate_tool_pairing(
    history: &[Value],
    tool_results: &[Value],
) -> (Vec<Value>, HashSet<String>) {
    let mut all_tool_use_ids = HashSet::new();
    let mut history_tool_result_ids = HashSet::new();

    for msg in history {
        if let Some(tool_uses) = msg
            .pointer("/assistantResponseMessage/toolUses")
            .and_then(Value::as_array)
        {
            for tool_use in tool_uses {
                if let Some(id) = tool_use.get("toolUseId").and_then(Value::as_str) {
                    all_tool_use_ids.insert(id.to_string());
                }
            }
        }
        if let Some(results) = msg
            .pointer("/userInputMessage/userInputMessageContext/toolResults")
            .and_then(Value::as_array)
        {
            for result in results {
                if let Some(id) = result.get("toolUseId").and_then(Value::as_str) {
                    history_tool_result_ids.insert(id.to_string());
                }
            }
        }
    }

    let mut unpaired: HashSet<String> = all_tool_use_ids
        .difference(&history_tool_result_ids)
        .cloned()
        .collect();
    let mut filtered = Vec::new();
    for result in tool_results {
        let Some(id) = result.get("toolUseId").and_then(Value::as_str) else {
            continue;
        };
        if unpaired.remove(id) {
            filtered.push(result.clone());
        }
    }
    (filtered, unpaired)
}

fn remove_orphaned_tool_uses(history: &mut [Value], orphaned_ids: &HashSet<String>) {
    if orphaned_ids.is_empty() {
        return;
    }
    for msg in history {
        let Some(tool_uses) = msg
            .pointer_mut("/assistantResponseMessage/toolUses")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        tool_uses.retain(|tool_use| {
            tool_use
                .get("toolUseId")
                .and_then(Value::as_str)
                .map(|id| !orphaned_ids.contains(id))
                .unwrap_or(true)
        });
        if tool_uses.is_empty() {
            if let Some(obj) = msg
                .get_mut("assistantResponseMessage")
                .and_then(Value::as_object_mut)
            {
                obj.remove("toolUses");
            }
        }
    }
}

fn add_missing_history_tools(tools: &mut Vec<Value>, history: &[Value]) {
    let mut existing_names: HashSet<String> = tools
        .iter()
        .filter_map(|tool| {
            tool.pointer("/toolSpecification/name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut missing = Vec::new();

    for msg in history {
        let Some(tool_uses) = msg
            .pointer("/assistantResponseMessage/toolUses")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for tool_use in tool_uses {
            let Some(name) = tool_use.get("name").and_then(Value::as_str) else {
                continue;
            };
            if existing_names.insert(name.to_string()) {
                missing.push(json!({
                    "toolSpecification": {
                        "name": name,
                        "description": name,
                        "inputSchema": {
                            "json": {
                                "type": "object",
                                "properties": {},
                                "required": [],
                                "additionalProperties": true
                            }
                        }
                    }
                }));
            }
        }
    }

    tools.extend(missing);
}

fn parse_user_content(content: Option<&Value>) -> (String, Vec<Value>, Vec<Value>) {
    let mut text = String::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    match content {
        Some(Value::String(s)) => text.push_str(s),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(item.get("text").and_then(|v| v.as_str()).unwrap_or(""));
                    }
                    Some("image") => {
                        append_kiro_image(item, &mut images);
                    }
                    Some("tool_result") => {
                        let id = item
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let is_error = item
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let result_text = flatten_content_to_text(item.get("content"));
                        if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                            for block in blocks {
                                if block.get("type").and_then(Value::as_str) == Some("image") {
                                    append_kiro_image(block, &mut images);
                                }
                            }
                        }
                        tool_results.push(json!({
                            "toolUseId": id,
                            "content": [{"text": result_text}],
                            "status": if is_error { "error" } else { "success" },
                            "isError": is_error
                        }));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (text, images, tool_results)
}

fn append_kiro_image(block: &Value, images: &mut Vec<Value>) {
    let Some(source) = block.get("source") else {
        return;
    };
    let Some(data) = source.get("data").and_then(Value::as_str) else {
        return;
    };
    let media = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("image/png");
    let format = match media.split('/').nth(1).unwrap_or("png") {
        "jpg" => "jpeg",
        value => value,
    };
    images.push(json!({"format": format, "source": {"bytes": data}}));
}

fn parse_assistant_content(
    content: Option<&Value>,
    tool_name_map: &mut HashMap<String, String>,
    tool_mode: ToolCompatibilityMode,
) -> (String, Vec<Value>) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_uses = Vec::new();
    match content {
        Some(Value::String(s)) => text.push_str(s),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(item.get("text").and_then(|v| v.as_str()).unwrap_or(""));
                    }
                    Some("tool_use") => {
                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let mapped_name = map_tool_name(name, tool_name_map, tool_mode);
                        let mut input = item.get("input").cloned().unwrap_or_else(|| json!({}));
                        if tool_bridge::builtin_name(name, tool_mode).is_some() {
                            input = tool_bridge::input_to_kiro(name, input);
                        }
                        tool_uses.push(json!({
                            "toolUseId": item.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                            "name": mapped_name,
                            "input": input
                        }));
                    }
                    Some("thinking") => {
                        if let Some(value) = item.get("thinking").and_then(|v| v.as_str()) {
                            thinking.push_str(value);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let content = if !thinking.is_empty() && !text.is_empty() {
        format!("<thinking>{thinking}</thinking>\n\n{text}")
    } else if !thinking.is_empty() {
        format!("<thinking>{thinking}</thinking>")
    } else if text.is_empty() && !tool_uses.is_empty() {
        " ".to_string()
    } else {
        text
    };
    (content, tool_uses)
}

fn flatten_content_to_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| {
                if v.get("type").and_then(|t| t.as_str()) == Some("text") {
                    v.get("text").and_then(|t| t.as_str()).map(str::to_string)
                } else {
                    v.as_str().map(str::to_string)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

#[derive(Debug, Clone)]
struct LeakedToolUse {
    name: String,
    input: Value,
}

#[derive(Default)]
struct ToolLeakFilter {
    carry: String,
    leaked_tools: Vec<LeakedToolUse>,
}

impl ToolLeakFilter {
    fn push_text(&mut self, text: &str, flush: bool) -> Vec<String> {
        self.carry.push_str(text);
        let mut visible = Vec::new();

        loop {
            let Some(invoke_start) = self.carry.find("<invoke name=\"") else {
                break;
            };
            let Some(invoke_end_rel) = self.carry[invoke_start..].find("</invoke>") else {
                if flush {
                    if !self.carry.is_empty() {
                        visible.push(std::mem::take(&mut self.carry));
                    }
                } else {
                    let prefix = strip_tool_prefix(&self.carry[..invoke_start]);
                    if !prefix.is_empty() {
                        visible.push(prefix);
                    }
                    self.carry = self.carry[invoke_start..].to_string();
                }
                return visible;
            };

            let invoke_end = invoke_start + invoke_end_rel + "</invoke>".len();
            let prefix = strip_tool_prefix(&self.carry[..invoke_start]);
            if !prefix.is_empty() {
                visible.push(prefix);
            }

            if let Some(tool) = parse_leaked_invoke(&self.carry[invoke_start..invoke_end]) {
                self.leaked_tools.push(tool);
            } else {
                visible.push(self.carry[invoke_start..invoke_end].to_string());
            }

            let mut consumed_end = invoke_end;
            if let Some(close_len) = leading_function_calls_close_len(&self.carry[consumed_end..]) {
                consumed_end += close_len;
            }
            self.carry = self.carry[consumed_end..].to_string();
        }

        if flush {
            if !self.carry.is_empty() {
                visible.push(std::mem::take(&mut self.carry));
            }
            return visible;
        }

        let hold = pending_tool_tail_len(&self.carry);
        let emit_len = self.carry.len().saturating_sub(hold);
        if emit_len > 0 {
            visible.push(self.carry[..emit_len].to_string());
            self.carry = self.carry[emit_len..].to_string();
        }
        visible
    }

    fn take_deduped(&mut self, seen_signatures: &mut HashSet<String>) -> Vec<LeakedToolUse> {
        let mut out = Vec::new();
        for leaked in self.leaked_tools.drain(..) {
            let sig = tool_signature(&leaked.name, &leaked.input);
            if seen_signatures.insert(sig) {
                out.push(leaked);
            }
        }
        out
    }
}

fn parse_leaked_invoke(value: &str) -> Option<LeakedToolUse> {
    let name_start = value.find("<invoke name=\"")? + "<invoke name=\"".len();
    let name_end = value[name_start..].find('"')? + name_start;
    let name = value[name_start..name_end].to_string();
    let body_start = value[name_end..].find('>')? + name_end + 1;
    let body_end = value.rfind("</invoke>")?;
    let body = &value[body_start..body_end];
    let mut input = serde_json::Map::new();
    let mut rest = body;

    while let Some(param_start_rel) = rest.find("<parameter name=\"") {
        rest = &rest[param_start_rel + "<parameter name=\"".len()..];
        let Some(key_end) = rest.find('"') else {
            break;
        };
        let key = rest[..key_end].to_string();
        let Some(open_end) = rest[key_end..].find('>') else {
            break;
        };
        rest = &rest[key_end + open_end + 1..];
        let Some(value_end) = rest.find("</parameter>") else {
            break;
        };
        let raw = &rest[..value_end];
        input.insert(key, leaked_parameter_value(raw));
        rest = &rest[value_end + "</parameter>".len()..];
    }

    Some(LeakedToolUse {
        name,
        input: Value::Object(input),
    })
}

fn leaked_parameter_value(raw: &str) -> Value {
    let decoded = xml_unescape(raw);
    let trimmed = decoded.trim();
    match trimmed {
        "true" => json!(true),
        "false" => json!(false),
        "null" => Value::Null,
        _ => {
            if let Ok(value) = trimmed.parse::<i64>() {
                json!(value)
            } else if let Ok(value) = trimmed.parse::<f64>() {
                if value.is_finite() {
                    json!(value)
                } else {
                    json!(decoded)
                }
            } else {
                json!(decoded)
            }
        }
    }
}

fn xml_unescape(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn strip_tool_prefix(prefix: &str) -> String {
    let trimmed_end = prefix.trim_end();
    for marker in ["<function_calls>", "count"] {
        if let Some(stripped) = trimmed_end.strip_suffix(marker) {
            let keep_len = stripped.len();
            return prefix[..keep_len].to_string();
        }
    }
    prefix.to_string()
}

fn leading_function_calls_close_len(value: &str) -> Option<usize> {
    let trimmed = value.trim_start();
    let ws_len = value.len().saturating_sub(trimmed.len());
    trimmed
        .strip_prefix("</function_calls>")
        .map(|_| ws_len + "</function_calls>".len())
}

fn pending_tool_tail_len(value: &str) -> usize {
    let markers = [
        "<function_calls>",
        "<invoke name=\"",
        "</invoke>",
        "</function_calls>",
        "<parameter name=\"",
        "</parameter>",
        "count",
    ];
    let mut hold = 0;
    for marker in markers {
        let max = marker.len().saturating_sub(1).min(value.len());
        for len in (1..=max).rev() {
            if value.ends_with(&marker[..len]) {
                hold = hold.max(len);
                break;
            }
        }
    }
    if let Some(pos) = value.rfind("count") {
        if value[pos..].trim_start().starts_with("count") && value[pos..].contains('<') {
            hold = hold.max(value.len() - pos);
        }
    }
    hold
}

fn tool_signature(name: &str, input: &Value) -> String {
    format!("{name}|{}", canonical_tool_input(input))
}

fn canonical_tool_input(input: &Value) -> String {
    serde_json::to_string(&canonical_prompt_cache_value(input)).unwrap_or_default()
}

#[derive(Default)]
struct SseBuilder {
    message_id: String,
    model: String,
    tool_name_map: HashMap<String, String>,
    text_index: Option<i32>,
    text_stopped: bool,
    thinking_index: Option<i32>,
    thinking_stopped: bool,
    pending_thinking_signature: Option<String>,
    next_index: i32,
    tool_indices: HashMap<String, i32>,
    tool_names: HashMap<String, String>,
    tool_json: ToolJsonAccumulator,
    seen_tool_signatures: HashSet<String>,
    tool_leak_filter: ToolLeakFilter,
    inline_thinking: bool,
    output_tokens: i32,
    usage: KiroUsageAccumulator,
}

impl SseBuilder {
    fn new(model: String, tool_name_map: HashMap<String, String>) -> Self {
        Self {
            message_id: next_message_id(),
            model,
            tool_name_map,
            ..Default::default()
        }
    }

    fn initial(&self) -> Bytes {
        sse(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                }
            }),
        )
    }

    fn assistant_delta(&mut self, text: &str) -> Vec<Bytes> {
        let mut out = Vec::new();
        for segment in split_inline_thinking(text, &mut self.inline_thinking) {
            if segment.is_thinking {
                out.extend(self.thinking_delta(&segment.text));
            } else {
                for visible in self.tool_leak_filter.push_text(&segment.text, false) {
                    out.extend(self.visible_assistant_delta(&visible));
                }
            }
        }
        out
    }

    fn visible_assistant_delta(&mut self, text: &str) -> Vec<Bytes> {
        if text.is_empty() {
            return Vec::new();
        }
        self.output_tokens += estimate_tokens(text);
        let mut out = Vec::new();
        out.extend(self.stop_thinking_block());
        if self.text_stopped {
            self.text_index = None;
            self.text_stopped = false;
        }
        let index = if let Some(index) = self.text_index {
            index
        } else {
            let index = self.next_index;
            self.next_index += 1;
            self.text_index = Some(index);
            out.push(sse(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
            ));
            index
        };
        out.push(sse(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
        ));
        out
    }

    fn reasoning_delta(&mut self, payload: &Value) -> Vec<Bytes> {
        if let Some(signature) = reasoning_signature(payload) {
            self.pending_thinking_signature = Some(signature.to_string());
        }
        self.thinking_delta(reasoning_text(payload).unwrap_or(""))
    }

    fn redacted_thinking_block(&mut self, data: &str) -> Vec<Bytes> {
        if data.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.text_index.is_some() && !self.text_stopped {
            self.text_stopped = true;
            let index = self.text_index.unwrap_or(0);
            out.push(sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            ));
        }
        out.extend(self.stop_thinking_block());
        let index = self.next_index;
        self.next_index += 1;
        out.push(sse(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"redacted_thinking","data":data}}),
        ));
        out.push(sse(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
        out
    }

    fn thinking_delta(&mut self, text: &str) -> Vec<Bytes> {
        if text.is_empty() {
            return Vec::new();
        }
        self.output_tokens += estimate_tokens(text);
        let mut out = Vec::new();
        if self.text_index.is_some() && !self.text_stopped {
            self.text_stopped = true;
            if let Some(index) = self.text_index {
                out.push(sse(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":index}),
                ));
            }
        }
        if self.thinking_stopped {
            self.thinking_index = None;
            self.thinking_stopped = false;
        }
        let index = if let Some(index) = self.thinking_index {
            index
        } else {
            let index = self.next_index;
            self.next_index += 1;
            self.thinking_index = Some(index);
            out.push(sse(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":""}}),
            ));
            index
        };
        out.push(sse(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":text}}),
        ));
        out
    }

    fn tool_delta(
        &mut self,
        id: &str,
        name: &str,
        input: &str,
        stop: bool,
    ) -> Result<Vec<Bytes>, KiroToolJsonError> {
        let Some((id, kiro_name, name, parsed_input)) =
            self.tool_json
                .push(id, name, input, stop, &self.tool_name_map)?
        else {
            return Ok(Vec::new());
        };
        let parsed_input = tool_bridge::input_from_kiro(&kiro_name, parsed_input);
        let mut out = Vec::new();
        out.extend(self.stop_thinking_block());
        if self.text_index.is_some() && !self.text_stopped {
            self.text_stopped = true;
            let index = self.text_index.unwrap_or(0);
            out.push(sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            ));
        }

        let index = if let Some(index) = self.tool_indices.get(&id).copied() {
            index
        } else {
            let index = self.next_index;
            self.next_index += 1;
            self.tool_indices.insert(id.clone(), index);
            self.tool_names.insert(id.clone(), name.clone());
            out.push(sse(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            ));
            index
        };
        let input =
            serde_json::to_string(&parsed_input).map_err(|error| KiroToolJsonError::Invalid {
                tool_use_id: id.clone(),
                name: name.clone(),
                message: error.to_string(),
            })?;
        if input != "{}" {
            out.push(sse(
                "content_block_delta",
                json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":input}}),
            ));
        }
        self.seen_tool_signatures
            .insert(tool_signature(&name, &parsed_input));
        out.push(sse(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ));
        Ok(out)
    }

    fn finish_events(&mut self) -> Result<Vec<Bytes>, KiroToolJsonError> {
        self.tool_json.finish()?;
        Ok(self.final_events())
    }

    fn final_events(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        for visible in self.tool_leak_filter.push_text("", true) {
            out.extend(self.visible_assistant_delta(&visible));
        }
        for leaked in self
            .tool_leak_filter
            .take_deduped(&mut self.seen_tool_signatures)
        {
            out.extend(self.stop_thinking_block());
            if self.text_index.is_some() && !self.text_stopped {
                self.text_stopped = true;
                let index = self.text_index.unwrap_or(0);
                out.push(sse(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":index}),
                ));
            }
            let index = self.next_index;
            self.next_index += 1;
            let id = format!(
                "toolleakfix_{}_{}",
                unix_timestamp_secs(),
                self.tool_indices.len() + 1
            );
            self.tool_indices.insert(id.clone(), index);
            out.push(sse(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":leaked.name,"input":{}}}),
            ));
            let input = serde_json::to_string(&leaked.input).unwrap_or_else(|_| "{}".to_string());
            if input != "{}" {
                out.push(sse(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":input}}),
                ));
            }
            out.push(sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            ));
        }
        out.extend(self.stop_thinking_block());
        if self.text_index.is_some() && !self.text_stopped {
            self.text_stopped = true;
            let index = self.text_index.unwrap_or(0);
            out.push(sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            ));
        }
        let stop_reason = if self.tool_indices.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        };
        let usage = self.usage.final_usage(self.output_tokens);
        out.push(sse(
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":stop_reason,"stop_sequence":null},
                "usage":{
                    "input_tokens":usage.input_tokens,
                    "output_tokens":usage.output_tokens,
                    "cache_read_input_tokens":usage.cache_read_tokens,
                    "cache_creation_input_tokens":usage.cache_creation_tokens
                }
            }),
        ));
        out.push(sse("message_stop", json!({"type":"message_stop"})));
        out
    }

    fn usage_event(&mut self, event_type: &str, payload: &Value) {
        self.usage.apply_event(event_type, payload, &self.model);
    }

    fn set_prompt_cache_usage(&mut self, usage: KiroPromptCacheUsage) {
        self.usage.set_prompt_cache_usage(usage);
    }

    fn stop_thinking_block(&mut self) -> Vec<Bytes> {
        if self.thinking_index.is_none() || self.thinking_stopped {
            return Vec::new();
        }
        self.thinking_stopped = true;
        let index = self.thinking_index.unwrap_or(0);
        let signature = self
            .pending_thinking_signature
            .take()
            .unwrap_or_else(|| THINKING_SIGNATURE_FALLBACK.to_string());
        vec![
            sse(
                "content_block_delta",
                json!({
                    "type":"content_block_delta",
                    "index":index,
                    "delta":{
                        "type":"signature_delta",
                        "signature":signature
                    }
                }),
            ),
            sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            ),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineThinkingSegment {
    is_thinking: bool,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KiroToolJsonError {
    Invalid {
        tool_use_id: String,
        name: String,
        message: String,
    },
    Incomplete {
        tool_use_id: String,
        name: String,
        bytes: usize,
    },
    Limit {
        tool_use_id: String,
        name: String,
        bytes: usize,
        max: usize,
    },
    Wire {
        code: &'static str,
        message: String,
    },
}

impl KiroToolJsonError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "TOOL_JSON_INVALID",
            Self::Incomplete { .. } => "TOOL_JSON_INCOMPLETE",
            Self::Limit { .. } => "TOOL_JSON_LIMIT",
            Self::Wire { code, .. } => code,
        }
    }
}

impl std::fmt::Display for KiroToolJsonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid {
                tool_use_id,
                name,
                message,
            } => write!(
                formatter,
                "upstream returned invalid JSON for tool_use {tool_use_id} ({name}): {message}"
            ),
            Self::Incomplete {
                tool_use_id,
                name,
                bytes,
            } => write!(
                formatter,
                "upstream ended before completing tool_use {tool_use_id} ({name}) JSON input; buffered {bytes} bytes"
            ),
            Self::Limit {
                tool_use_id: _,
                name: _,
                bytes,
                max,
            } => write!(
                formatter,
                "upstream tool JSON input reached {bytes} bytes; limit is {max} bytes"
            ),
            Self::Wire { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for KiroToolJsonError {}

impl From<wire::WireError> for KiroToolJsonError {
    fn from(error: wire::WireError) -> Self {
        Self::Wire {
            code: error.code(),
            message: error.to_string(),
        }
    }
}

struct ToolJsonAccumulator {
    pending: HashMap<String, (String, String)>,
    buffered_bytes: usize,
    max_buffered_bytes: usize,
}

impl Default for ToolJsonAccumulator {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            buffered_bytes: 0,
            max_buffered_bytes: MAX_TOOL_JSON_BYTES,
        }
    }
}

impl ToolJsonAccumulator {
    #[cfg(test)]
    fn with_max_buffered_bytes(max_buffered_bytes: usize) -> Self {
        Self {
            max_buffered_bytes,
            ..Self::default()
        }
    }

    fn push(
        &mut self,
        tool_use_id: &str,
        name: &str,
        input: &str,
        stop: bool,
        tool_name_map: &HashMap<String, String>,
    ) -> Result<Option<(String, String, String, Value)>, KiroToolJsonError> {
        let buffered_bytes = self.buffered_bytes.saturating_add(input.len());
        if buffered_bytes > self.max_buffered_bytes {
            return Err(KiroToolJsonError::Limit {
                tool_use_id: tool_use_id.to_string(),
                name: name.to_string(),
                bytes: buffered_bytes,
                max: self.max_buffered_bytes,
            });
        }
        let entry = self
            .pending
            .entry(tool_use_id.to_string())
            .or_insert_with(|| (name.to_string(), String::new()));
        if entry.0.is_empty() {
            entry.0 = name.to_string();
        }
        entry.1.push_str(input);
        self.buffered_bytes = buffered_bytes;
        if !stop {
            return Ok(None);
        }

        let (kiro_name, input) = self
            .pending
            .remove(tool_use_id)
            .unwrap_or_else(|| (name.to_string(), input.to_string()));
        self.buffered_bytes = self.buffered_bytes.saturating_sub(input.len());
        let parsed = if input.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&input).map_err(|error| KiroToolJsonError::Invalid {
                tool_use_id: tool_use_id.to_string(),
                name: kiro_name.clone(),
                message: error.to_string(),
            })?
        };
        Ok(Some((
            tool_use_id.to_string(),
            kiro_name.clone(),
            original_tool_name(&kiro_name, tool_name_map),
            parsed,
        )))
    }

    fn finish(&mut self) -> Result<(), KiroToolJsonError> {
        let pending = self
            .pending
            .iter()
            .max_by_key(|(_, (_, input))| input.len())
            .map(|(id, (name, input))| (id.clone(), name.clone(), input.len()));
        let Some((tool_use_id, name, bytes)) = pending else {
            return Ok(());
        };
        self.pending.remove(&tool_use_id);
        self.buffered_bytes = self.buffered_bytes.saturating_sub(bytes);
        Err(KiroToolJsonError::Incomplete {
            tool_use_id,
            name,
            bytes,
        })
    }
}

fn split_inline_thinking(text: &str, in_thinking: &mut bool) -> Vec<InlineThinkingSegment> {
    let mut segments = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if *in_thinking {
            if let Some(end) = rest.find("</thinking>") {
                let chunk = &rest[..end];
                if !chunk.is_empty() {
                    segments.push(InlineThinkingSegment {
                        is_thinking: true,
                        text: chunk.to_string(),
                    });
                }
                rest = &rest[end + "</thinking>".len()..];
                *in_thinking = false;
            } else {
                segments.push(InlineThinkingSegment {
                    is_thinking: true,
                    text: rest.to_string(),
                });
                break;
            }
        } else if let Some(start) = rest.find("<thinking>") {
            let visible = &rest[..start];
            if !visible.is_empty() {
                segments.push(InlineThinkingSegment {
                    is_thinking: false,
                    text: visible.to_string(),
                });
            }
            rest = &rest[start + "<thinking>".len()..];
            *in_thinking = true;
        } else {
            segments.push(InlineThinkingSegment {
                is_thinking: false,
                text: rest.to_string(),
            });
            break;
        }
    }
    segments
}

pub(crate) fn kiro_event_stream_to_claude_sse(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    tool_name_map: HashMap<String, String>,
    request_body: &Value,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    kiro_event_stream_to_claude_sse_scoped(stream, model, tool_name_map, request_body, "legacy")
}

pub(crate) fn kiro_event_stream_to_claude_sse_scoped(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    tool_name_map: HashMap<String, String>,
    request_body: &Value,
    cache_namespace: &str,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let prompt_cache_usage = compute_kiro_prompt_cache_usage(request_body, cache_namespace);
    kiro_event_stream_to_anthropic_sse(stream, model, tool_name_map, prompt_cache_usage)
}

fn kiro_event_stream_to_anthropic_sse(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    tool_name_map: HashMap<String, String>,
    prompt_cache_usage: KiroPromptCacheUsage,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        let mut decoder = wire::EventStreamDecoder::strict();
        let mut builder = SseBuilder::new(model, tool_name_map);
        builder.set_prompt_cache_usage(prompt_cache_usage);
        yield Ok(builder.initial());
        tokio::pin!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| std::io::Error::other(e.to_string()))?;
            let frames = decoder
                .feed(&chunk)
                .map_err(|error| kiro_stream_io_error(error.into()))?;
            for frame in frames {
                let event = wire::parse_event(frame)
                    .map_err(|error| kiro_stream_io_error(error.into()))?;
                for bytes in process_event_to_sse(&mut builder, event)
                    .map_err(kiro_stream_io_error)?
                {
                    yield Ok(bytes);
                }
            }
        }
        for frame in decoder
            .finish()
            .map_err(|error| kiro_stream_io_error(error.into()))?
        {
            let event = wire::parse_event(frame)
                .map_err(|error| kiro_stream_io_error(error.into()))?;
            for bytes in process_event_to_sse(&mut builder, event)
                .map_err(kiro_stream_io_error)?
            {
                yield Ok(bytes);
            }
        }
        for bytes in builder
            .finish_events()
            .map_err(kiro_stream_io_error)?
        {
            yield Ok(bytes);
        }
    }
}

fn kiro_stream_io_error(error: KiroToolJsonError) -> std::io::Error {
    std::io::Error::other(format!("[{}] {error}", error.code()))
}

fn process_event_to_sse(
    builder: &mut SseBuilder,
    event: wire::Event,
) -> Result<Vec<Bytes>, KiroToolJsonError> {
    match event {
        wire::Event::AssistantResponse { content } | wire::Event::Code { content } => {
            Ok(builder.assistant_delta(&content))
        }
        wire::Event::Reasoning(payload) => {
            let mut out = builder.reasoning_delta(&payload);
            if let Some(redacted) = reasoning_redacted_content(&payload) {
                out.extend(builder.redacted_thinking_block(redacted));
            }
            Ok(out)
        }
        wire::Event::ToolUse {
            tool_use_id,
            name,
            input,
            stop,
        } => builder.tool_delta(&tool_use_id, &name, &input, stop),
        wire::Event::Usage {
            event_type,
            payload,
        } => {
            builder.usage_event(&event_type, &payload);
            Ok(Vec::new())
        }
        wire::Event::End | wire::Event::Unknown { .. } => Ok(Vec::new()),
    }
}

pub(crate) fn kiro_event_bytes_to_claude_json(
    bytes: &[u8],
    model: &str,
    tool_name_map: &HashMap<String, String>,
    request_body: &Value,
) -> Result<Value, KiroToolJsonError> {
    kiro_event_bytes_to_claude_json_scoped(bytes, model, tool_name_map, request_body, "legacy")
}

pub(crate) fn kiro_event_bytes_to_claude_json_scoped(
    bytes: &[u8],
    model: &str,
    tool_name_map: &HashMap<String, String>,
    request_body: &Value,
    cache_namespace: &str,
) -> Result<Value, KiroToolJsonError> {
    let prompt_cache_usage = compute_kiro_prompt_cache_usage(request_body, cache_namespace);
    kiro_event_bytes_to_anthropic_json(bytes, model, tool_name_map, prompt_cache_usage)
}

fn kiro_event_bytes_to_anthropic_json(
    bytes: &[u8],
    model: &str,
    tool_name_map: &HashMap<String, String>,
    prompt_cache_usage: KiroPromptCacheUsage,
) -> Result<Value, KiroToolJsonError> {
    let mut decoder = wire::EventStreamDecoder::strict();
    let mut frames = decoder.feed(bytes)?;
    frames.extend(decoder.finish()?);
    let mut text = String::new();
    let mut thinking = String::new();
    let mut inline_thinking = false;
    let mut thinking_signature = None;
    let mut redacted_thinking = Vec::new();
    let mut tools = Vec::new();
    let mut tool_accumulator = ToolJsonAccumulator::default();
    let mut seen_tool_signatures = HashSet::new();
    let mut tool_leak_filter = ToolLeakFilter::default();
    let mut usage = KiroUsageAccumulator::default();
    usage.set_prompt_cache_usage(prompt_cache_usage);
    for frame in frames {
        match wire::parse_event(frame)? {
            wire::Event::AssistantResponse { content: chunk }
            | wire::Event::Code { content: chunk } => {
                for segment in split_inline_thinking(&chunk, &mut inline_thinking) {
                    if segment.is_thinking {
                        thinking.push_str(&segment.text);
                    } else {
                        for visible in tool_leak_filter.push_text(&segment.text, false) {
                            text.push_str(&visible);
                        }
                    }
                }
            }
            wire::Event::Reasoning(payload) => {
                if let Some(signature) = reasoning_signature(&payload) {
                    thinking_signature = Some(signature.to_string());
                }
                if let Some(chunk) = reasoning_text(&payload) {
                    thinking.push_str(chunk);
                }
                if let Some(redacted) = reasoning_redacted_content(&payload) {
                    redacted_thinking.push(redacted.to_string());
                }
            }
            wire::Event::ToolUse {
                tool_use_id: id,
                name,
                input,
                stop,
            } => {
                if let Some((id, kiro_name, name, parsed_input)) =
                    tool_accumulator.push(&id, &name, &input, stop, tool_name_map)?
                {
                    let parsed_input = tool_bridge::input_from_kiro(&kiro_name, parsed_input);
                    seen_tool_signatures.insert(tool_signature(&name, &parsed_input));
                    tools.push((id, name, parsed_input));
                }
            }
            wire::Event::Usage {
                event_type,
                payload,
            } => {
                usage.apply_event(&event_type, &payload, model);
            }
            wire::Event::End | wire::Event::Unknown { .. } => {}
        }
    }
    tool_accumulator.finish()?;
    for visible in tool_leak_filter.push_text("", true) {
        text.push_str(&visible);
    }

    let mut content = Vec::new();
    if !thinking.is_empty() {
        content.push(json!({
            "type":"thinking",
            "thinking":thinking,
            "signature": thinking_signature.unwrap_or_else(|| THINKING_SIGNATURE_FALLBACK.to_string())
        }));
    }
    for data in redacted_thinking {
        content.push(json!({"type":"redacted_thinking","data":data}));
    }
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    for (id, name, input) in tools {
        content.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
    }
    for leaked in tool_leak_filter.take_deduped(&mut seen_tool_signatures) {
        content.push(json!({
            "type":"tool_use",
            "id": format!("toolleakfix_{}_{}", unix_timestamp_secs(), content.len() + 1),
            "name": leaked.name,
            "input": leaked.input
        }));
    }
    let stop_reason = if content
        .iter()
        .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
    {
        "tool_use"
    } else {
        "end_turn"
    };
    let fallback_output_tokens = estimate_tokens(&format!("{thinking}{text}"));
    let usage = usage.final_usage(fallback_output_tokens);
    Ok(json!({
        "id": next_message_id(),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cache_read_input_tokens": usage.cache_read_tokens,
            "cache_creation_input_tokens": usage.cache_creation_tokens
        }
    }))
}

#[derive(Debug, Clone, Copy, Default)]
struct KiroPromptCacheUsage {
    cache_read_tokens: i32,
    cache_creation_tokens: i32,
}

#[derive(Debug, Clone)]
struct KiroPromptCacheSegment {
    hash: u64,
    cumulative_tokens: u32,
    ttl_secs: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct KiroPromptCacheEntry {
    tokens: u32,
    expires_at: i64,
    last_hit_at: i64,
}

struct KiroPromptCache {
    entries: Mutex<HashMap<u64, KiroPromptCacheEntry>>,
    persist_path: Option<PathBuf>,
}

impl KiroPromptCache {
    fn new(persist_path: Option<PathBuf>) -> Self {
        let entries = persist_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| {
                serde_json::from_slice::<HashMap<u64, KiroPromptCacheEntry>>(&bytes).ok()
            })
            .map(|entries| {
                let now = unix_timestamp_secs();
                entries
                    .into_iter()
                    .filter(|(_, entry)| entry.expires_at > now)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            entries: Mutex::new(entries),
            persist_path,
        }
    }

    fn compute_usage(&self, segments: &[KiroPromptCacheSegment]) -> KiroPromptCacheUsage {
        if segments.is_empty() {
            return KiroPromptCacheUsage::default();
        }

        let now = unix_timestamp_secs();
        let mut entries = self.entries.lock().unwrap_or_else(|err| err.into_inner());
        entries.retain(|_, entry| entry.expires_at > now);

        let deepest_hit = segments
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, segment)| {
                entries.get_mut(&segment.hash).and_then(|entry| {
                    if entry.expires_at > now {
                        entry.last_hit_at = now;
                        Some(idx)
                    } else {
                        None
                    }
                })
            });

        let total = segments
            .last()
            .map(|segment| segment.cumulative_tokens)
            .unwrap_or(0);
        let (cache_creation_tokens, cache_read_tokens) = match deepest_hit {
            Some(idx) => (
                total.saturating_sub(segments[idx].cumulative_tokens),
                segments[idx].cumulative_tokens,
            ),
            None => (total, 0),
        };

        for segment in segments {
            entries.insert(
                segment.hash,
                KiroPromptCacheEntry {
                    tokens: segment.cumulative_tokens,
                    expires_at: now + segment.ttl_secs.clamp(60, PROMPT_CACHE_MAX_TTL_SECS),
                    last_hit_at: now,
                },
            );
        }
        if entries.len() > PROMPT_CACHE_CAPACITY {
            let drop_count = entries.len() - PROMPT_CACHE_CAPACITY;
            let mut victims = entries
                .iter()
                .map(|(hash, entry)| (*hash, entry.last_hit_at))
                .collect::<Vec<_>>();
            victims.sort_by_key(|(_, last_hit_at)| *last_hit_at);
            for (hash, _) in victims.into_iter().take(drop_count) {
                entries.remove(&hash);
            }
        }

        let snapshot = entries.clone();
        drop(entries);
        self.flush_snapshot(snapshot);

        KiroPromptCacheUsage {
            cache_read_tokens: cache_read_tokens as i32,
            cache_creation_tokens: cache_creation_tokens as i32,
        }
    }

    fn flush_snapshot(&self, snapshot: HashMap<u64, KiroPromptCacheEntry>) {
        let Some(path) = self.persist_path.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!("Kiro PromptCache 创建目录失败 {}: {err}", parent.display());
                return;
            }
        }
        match serde_json::to_vec(&snapshot) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(path, bytes) {
                    tracing::warn!("Kiro PromptCache 写入失败 {}: {err}", path.display());
                }
            }
            Err(err) => tracing::warn!("Kiro PromptCache 序列化失败: {err}"),
        }
    }
}

fn compute_kiro_prompt_cache_usage(body: &Value, cache_namespace: &str) -> KiroPromptCacheUsage {
    compute_kiro_prompt_cache_usage_scoped_with_cache(body, cache_namespace, &KIRO_PROMPT_CACHE)
}

fn compute_kiro_prompt_cache_usage_with_cache(
    body: &Value,
    cache: &KiroPromptCache,
) -> KiroPromptCacheUsage {
    compute_kiro_prompt_cache_usage_scoped_with_cache(body, "test", cache)
}

fn compute_kiro_prompt_cache_usage_scoped_with_cache(
    body: &Value,
    cache_namespace: &str,
    cache: &KiroPromptCache,
) -> KiroPromptCacheUsage {
    let segments = extract_kiro_prompt_cache_segments(body, cache_namespace);
    cache.compute_usage(&segments)
}

fn extract_kiro_prompt_cache_segments(
    body: &Value,
    cache_namespace: &str,
) -> Vec<KiroPromptCacheSegment> {
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-kiro-cache-v2\0");
    hasher.update(cache_namespace.as_bytes());
    hasher.update(b"\0");
    let mut cumulative_tokens = 0u32;
    let mut segments = Vec::new();

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for tool in tools {
            feed_prompt_cache_value(&mut hasher, tool, &mut cumulative_tokens);
            if let Some(cache_control) = tool.get("cache_control") {
                commit_prompt_cache_segment(
                    &hasher,
                    cumulative_tokens,
                    cache_control,
                    &mut segments,
                );
            }
        }
    }

    match body.get("system") {
        Some(Value::String(system)) => {
            feed_prompt_cache_text(&mut hasher, system, &mut cumulative_tokens)
        }
        Some(Value::Array(items)) => {
            for item in items {
                feed_prompt_cache_value(&mut hasher, item, &mut cumulative_tokens);
                if let Some(cache_control) = item.get("cache_control") {
                    commit_prompt_cache_segment(
                        &hasher,
                        cumulative_tokens,
                        cache_control,
                        &mut segments,
                    );
                }
            }
        }
        _ => {}
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(role) = message.get("role").and_then(Value::as_str) {
                feed_prompt_cache_text(&mut hasher, role, &mut cumulative_tokens);
            }
            match message.get("content") {
                Some(Value::String(text)) => {
                    feed_prompt_cache_text(&mut hasher, text, &mut cumulative_tokens);
                }
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        feed_prompt_cache_value(&mut hasher, block, &mut cumulative_tokens);
                        if let Some(cache_control) = block.get("cache_control") {
                            commit_prompt_cache_segment(
                                &hasher,
                                cumulative_tokens,
                                cache_control,
                                &mut segments,
                            );
                        }
                    }
                }
                Some(other) => feed_prompt_cache_value(&mut hasher, other, &mut cumulative_tokens),
                None => {}
            }
            if let Some(cache_control) = message.get("cache_control") {
                commit_prompt_cache_segment(
                    &hasher,
                    cumulative_tokens,
                    cache_control,
                    &mut segments,
                );
            }
        }
    }

    segments
}

fn feed_prompt_cache_value(hasher: &mut Sha256, value: &Value, cumulative_tokens: &mut u32) {
    let signature = prompt_cache_signature(value);
    feed_prompt_cache_text(hasher, &signature, cumulative_tokens);
}

fn feed_prompt_cache_text(hasher: &mut Sha256, text: &str, cumulative_tokens: &mut u32) {
    if text.is_empty() {
        return;
    }
    hasher.update(text.as_bytes());
    *cumulative_tokens = cumulative_tokens.saturating_add(estimate_tokens(text).max(0) as u32);
}

fn prompt_cache_signature(value: &Value) -> String {
    serde_json::to_string(&canonical_prompt_cache_value(value)).unwrap_or_default()
}

fn canonical_prompt_cache_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut normalized = serde_json::Map::new();
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                if key != "cache_control" {
                    if let Some(child) = map.get(key) {
                        normalized.insert(key.clone(), canonical_prompt_cache_value(child));
                    }
                }
            }
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(canonical_prompt_cache_value)
                .collect::<Vec<_>>(),
        ),
        _ => value.clone(),
    }
}

fn commit_prompt_cache_segment(
    hasher: &Sha256,
    cumulative_tokens: u32,
    cache_control: &Value,
    segments: &mut Vec<KiroPromptCacheSegment>,
) {
    if cumulative_tokens == 0 {
        return;
    }
    let digest = hasher.clone().finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    segments.push(KiroPromptCacheSegment {
        hash: u64::from_be_bytes(bytes),
        cumulative_tokens,
        ttl_secs: parse_prompt_cache_ttl(cache_control),
    });
}

fn parse_prompt_cache_ttl(cache_control: &Value) -> i64 {
    match cache_control.get("ttl").and_then(Value::as_str) {
        Some(ttl) if ttl.eq_ignore_ascii_case("1h") => 60 * 60,
        Some(ttl) if ttl.eq_ignore_ascii_case("5m") => 5 * 60,
        _ => PROMPT_CACHE_DEFAULT_TTL_SECS,
    }
}

fn unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, Default)]
struct KiroUsage {
    input_tokens: i32,
    output_tokens: i32,
    cache_read_tokens: i32,
    cache_creation_tokens: i32,
}

#[derive(Debug, Clone, Default)]
struct KiroUsageAccumulator {
    context_input_tokens: Option<i32>,
    metrics_input_tokens: Option<i32>,
    output_tokens: Option<i32>,
    cache_read_tokens: Option<i32>,
    cache_creation_tokens: Option<i32>,
    prompt_cache_read_tokens: i32,
    prompt_cache_creation_tokens: i32,
}

impl KiroUsageAccumulator {
    fn set_prompt_cache_usage(&mut self, usage: KiroPromptCacheUsage) {
        self.prompt_cache_read_tokens = usage.cache_read_tokens;
        self.prompt_cache_creation_tokens = usage.cache_creation_tokens;
    }

    fn apply_event(&mut self, event_type: &str, payload: &Value, model: &str) {
        match event_type {
            "contextUsageEvent" => {
                if let Some(tokens) = context_usage_input_tokens(payload, model) {
                    self.context_input_tokens = Some(tokens);
                }
            }
            "metricsEvent" => self.apply_metrics(payload),
            "messageMetadataEvent" | "metadataEvent" => self.apply_metadata(payload, model),
            "meteringEvent" => {}
            _ => {}
        }
    }

    fn apply_metadata(&mut self, payload: &Value, model: &str) {
        let metadata = payload
            .get("messageMetadataEvent")
            .or_else(|| payload.get("metadataEvent"))
            .unwrap_or(payload);
        if let Some(token_usage) = metadata.get("tokenUsage") {
            let uncached = number_field(
                token_usage,
                &[
                    "uncachedInputTokens",
                    "uncached_input_tokens",
                    "inputTokens",
                    "input_tokens",
                ],
            )
            .unwrap_or(0);
            let cache_read = number_field(
                token_usage,
                &[
                    "cacheReadInputTokens",
                    "cache_read_input_tokens",
                    "cacheReadTokens",
                ],
            )
            .unwrap_or(0);
            let cache_creation = number_field(
                token_usage,
                &[
                    "cacheWriteInputTokens",
                    "cache_write_input_tokens",
                    "cacheCreationInputTokens",
                    "cache_creation_input_tokens",
                ],
            )
            .unwrap_or(0);
            let input_total = uncached
                .saturating_add(cache_read)
                .saturating_add(cache_creation);
            if input_total > 0 {
                self.metrics_input_tokens = Some(input_total);
            }
            if cache_read > 0 {
                self.cache_read_tokens = Some(cache_read);
            }
            if cache_creation > 0 {
                self.cache_creation_tokens = Some(cache_creation);
            }
            if let Some(tokens) = number_field(
                token_usage,
                &[
                    "outputTokens",
                    "output_tokens",
                    "completionTokens",
                    "completion_tokens",
                ],
            ) {
                self.output_tokens = Some(tokens);
            }
            if let Some(percentage) = number_f64_field(
                token_usage,
                &["contextUsagePercentage", "context_usage_percentage"],
            ) {
                let tokens =
                    (percentage * context_window_size(model) as f64 / 100.0).floor() as i32;
                if tokens > 0 && self.metrics_input_tokens.is_none() {
                    self.context_input_tokens = Some(tokens);
                }
            }
        } else {
            self.apply_metrics(metadata);
        }
    }

    fn apply_metrics(&mut self, payload: &Value) {
        let metrics = payload.get("metricsEvent").unwrap_or(payload);
        if let Some(tokens) = number_field(
            metrics,
            &[
                "inputTokens",
                "input_tokens",
                "promptTokens",
                "prompt_tokens",
            ],
        ) {
            self.metrics_input_tokens = Some(tokens);
        }
        if let Some(tokens) = number_field(
            metrics,
            &[
                "outputTokens",
                "output_tokens",
                "completionTokens",
                "completion_tokens",
            ],
        ) {
            self.output_tokens = Some(tokens);
        }
        if let Some(tokens) = number_field(
            metrics,
            &[
                "cacheReadInputTokens",
                "cache_read_input_tokens",
                "cacheReadTokens",
            ],
        ) {
            self.cache_read_tokens = Some(tokens);
        }
        if let Some(tokens) = number_field(
            metrics,
            &[
                "cacheCreationInputTokens",
                "cache_creation_input_tokens",
                "cacheCreationTokens",
            ],
        ) {
            self.cache_creation_tokens = Some(tokens);
        }
    }

    fn final_usage(&self, fallback_output_tokens: i32) -> KiroUsage {
        let raw_input_tokens = self
            .metrics_input_tokens
            .or(self.context_input_tokens)
            .unwrap_or(0)
            .max(0);
        let cache_read_tokens = self
            .cache_read_tokens
            .unwrap_or(self.prompt_cache_read_tokens)
            .max(0);
        let cache_creation_tokens = self
            .cache_creation_tokens
            .unwrap_or(self.prompt_cache_creation_tokens)
            .max(0);
        KiroUsage {
            input_tokens: raw_input_tokens
                .saturating_sub(cache_read_tokens)
                .saturating_sub(cache_creation_tokens),
            output_tokens: self.output_tokens.unwrap_or(fallback_output_tokens).max(0),
            cache_read_tokens,
            cache_creation_tokens,
        }
    }
}

fn reasoning_text(payload: &Value) -> Option<&str> {
    if let Some(text) = payload.as_str() {
        return Some(text);
    }
    let value = payload.get("reasoningContentEvent").unwrap_or(payload);
    value
        .as_str()
        .or_else(|| value.get("text").and_then(Value::as_str))
        .or_else(|| value.get("content").and_then(Value::as_str))
        .or_else(|| value.get("reasoningContent").and_then(Value::as_str))
}

fn reasoning_signature(payload: &Value) -> Option<&str> {
    let value = payload.get("reasoningContentEvent").unwrap_or(payload);
    value
        .get("signature")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn reasoning_redacted_content(payload: &Value) -> Option<&str> {
    let value = payload.get("reasoningContentEvent").unwrap_or(payload);
    value
        .get("redactedContent")
        .or_else(|| value.get("redacted_content"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn sse(event: &str, data: Value) -> Bytes {
    Bytes::from(format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap_or_default()
    ))
}

fn context_usage_input_tokens(payload: &Value, model: &str) -> Option<i32> {
    let value = payload.get("contextUsageEvent").unwrap_or(payload);
    let percentage = number_f64_field(value, &["contextUsagePercentage"])?;
    Some((percentage * context_window_size(model) as f64 / 100.0).floor() as i32)
        .filter(|tokens| *tokens > 0)
}

fn number_field(value: &Value, keys: &[&str]) -> Option<i32> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(number_value))
}

fn number_f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(number_f64_value))
}

fn number_value(value: &Value) -> Option<i32> {
    if let Some(n) = value.as_i64() {
        return i32::try_from(n).ok();
    }
    if let Some(n) = value.as_u64() {
        return i32::try_from(n).ok();
    }
    value.as_f64().and_then(|n| {
        if n.is_finite() && n >= 0.0 && n <= i32::MAX as f64 {
            Some(n as i32)
        } else {
            None
        }
    })
}

fn number_f64_value(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return n.is_finite().then_some(n);
    }
    if let Some(n) = value.as_i64() {
        return Some(n as f64);
    }
    value.as_u64().map(|n| n as f64)
}

fn context_window_size(model: &str) -> i32 {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("[1m]") || normalized.contains("-1m") {
        1_000_000
    } else {
        200_000
    }
}

fn estimate_tokens(text: &str) -> i32 {
    ((text.chars().count() as f64) / 4.0).ceil() as i32
}

pub(crate) fn count_input_tokens(body: &Value) -> Result<i32, ProxyError> {
    if !body.get("messages").is_some_and(Value::is_array) {
        return Err(ProxyError::bad_request(
            "Kiro count_tokens requires an Anthropic messages array",
        ));
    }
    let mut tokens = 0u64;
    if let Some(system) = body.get("system") {
        tokens = tokens.saturating_add(count_content_tokens(system)?);
    }
    if let Some(tools) = body.get("tools") {
        tokens = tokens
            .saturating_add(count_value_tokens(tools)?)
            .saturating_add(12);
    }
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        tokens = tokens
            .saturating_add(4)
            .saturating_add(count_content_tokens(
                message.get("content").unwrap_or(&Value::Null),
            )?);
    }
    Ok(tokens.min(i32::MAX as u64) as i32)
}

fn count_value_tokens(value: &Value) -> Result<u64, ProxyError> {
    match value {
        Value::Null => Ok(0),
        Value::Bool(_) | Value::Number(_) => Ok(1),
        Value::String(text) => Ok(estimate_tokens(text).max(0) as u64),
        Value::Array(items) => {
            let mut tokens = 0u64;
            for item in items {
                tokens = tokens
                    .saturating_add(2)
                    .saturating_add(count_value_tokens(item)?);
            }
            Ok(tokens)
        }
        Value::Object(object) => {
            let mut tokens = 0u64;
            for (key, child) in object {
                if key == "cache_control" {
                    continue;
                }
                tokens = tokens
                    .saturating_add(estimate_tokens(key).max(0) as u64)
                    .saturating_add(count_value_tokens(child)?);
            }
            Ok(tokens)
        }
    }
}

fn count_content_tokens(value: &Value) -> Result<u64, ProxyError> {
    match value {
        Value::Array(items) => {
            let mut tokens = 0u64;
            for item in items {
                tokens = tokens
                    .saturating_add(2)
                    .saturating_add(count_content_block_tokens(item)?);
            }
            Ok(tokens)
        }
        Value::Object(_) => count_content_block_tokens(value),
        _ => count_value_tokens(value),
    }
}

fn count_content_block_tokens(value: &Value) -> Result<u64, ProxyError> {
    let Some(object) = value.as_object() else {
        return count_value_tokens(value);
    };
    match object.get("type").and_then(Value::as_str) {
        Some("image") => count_image_tokens(object),
        Some("tool_result") => {
            let mut tokens = 0u64;
            for (key, child) in object {
                if key == "cache_control" {
                    continue;
                }
                let child_tokens = if key == "content" {
                    count_content_tokens(child)?
                } else {
                    count_value_tokens(child)?
                };
                tokens = tokens
                    .saturating_add(estimate_tokens(key).max(0) as u64)
                    .saturating_add(child_tokens);
            }
            Ok(tokens)
        }
        _ => count_value_tokens(value),
    }
}

fn count_image_tokens(object: &serde_json::Map<String, Value>) -> Result<u64, ProxyError> {
    let data = object
        .get("source")
        .and_then(Value::as_object)
        .and_then(|source| source.get("data"))
        .and_then(Value::as_str)
        .ok_or_else(|| ProxyError::bad_request("Anthropic image block lacks base64 data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| ProxyError::bad_request("Anthropic image source is not valid base64"))?;
    let dimensions = imagesize::blob_size(&bytes)
        .map_err(|_| ProxyError::bad_request("Anthropic image dimensions could not be read"))?;
    let tiles = dimensions
        .width
        .saturating_mul(dimensions.height)
        .saturating_add(749)
        / 750;
    Ok((tiles as u64).max(85))
}

fn is_quota_exhausted(body: &str) -> bool {
    const REASONS: &[&str] = &["MONTHLY_REQUEST_COUNT", "OVERAGE_REQUEST_LIMIT_EXCEEDED"];
    if !REASONS.iter().any(|reason| body.contains(reason)) {
        return false;
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let top = value.get("reason").and_then(Value::as_str);
        let nested = value.pointer("/error/reason").and_then(Value::as_str);
        return [top, nested]
            .into_iter()
            .flatten()
            .any(|reason| REASONS.contains(&reason));
    }
    true
}

fn is_account_throttled(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        && body.contains("suspicious activity")
        && body.contains("temporary limits")
}

pub(crate) fn is_client_validation_error(body: &[u8]) -> bool {
    const TERMINAL_REASONS: &[&str] = &["TOOL_USE_RESULT_MISMATCH", "TOOL_SCHEMA_INVALID"];
    const MESSAGE_MARKERS: &[&str] = &["Expected toolResult blocks"];

    let body = String::from_utf8_lossy(body);
    if TERMINAL_REASONS.iter().any(|reason| body.contains(reason)) {
        match serde_json::from_str::<Value>(&body) {
            Ok(value) => {
                let top_level = value.get("reason").and_then(Value::as_str);
                let nested = value.pointer("/error/reason").and_then(Value::as_str);
                if [top_level, nested]
                    .into_iter()
                    .flatten()
                    .any(|reason| TERMINAL_REASONS.contains(&reason))
                {
                    return true;
                }
            }
            Err(_) => return true,
        }
    }
    MESSAGE_MARKERS.iter().any(|marker| body.contains(marker))
}

#[allow(dead_code)]
fn default_profile_arn_for_builder_id() -> &'static str {
    "arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX"
}

fn default_profile_arn_for_auth_method(auth_method: Option<&str>, region: &str) -> String {
    let method = auth_method.unwrap_or_default().trim().to_ascii_lowercase();
    match method.as_str() {
        "social" | "google" | "github" => {
            "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK".to_string()
        }
        "enterprise" | "idc" | "iam_sso" | "iam-sso" | "external_idp" | "external-idp"
        | "externalidp" => {
            let region = if region.starts_with("eu-") {
                "eu-central-1"
            } else {
                "us-east-1"
            };
            format!("arn:aws:codewhisperer:{region}:610548660232:profile/VNECVYCYYAWN")
        }
        _ => default_profile_arn_for_builder_id().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE_PROTOCOL_JSON: &str = include_str!("../../assets/contract/kiro-wire-protocol.json");

    fn test_account() -> KiroAccountData {
        KiroAccountData {
            account_id: "kiro_test".to_string(),
            email: None,
            refresh_token: "refresh".to_string(),
            profile_arn: None,
            auth_region: "us-east-1".to_string(),
            api_region: "us-east-1".to_string(),
            machine_id: None,
            client_id: Some("client".to_string()),
            client_secret: Some("secret".to_string()),
            client_secret_expires_at: None,
            start_url: None,
            auth_method: Some("builder-id".to_string()),
            provider: Some("BuilderId".to_string()),
            endpoint: None,
            client_version: None,
            cli_version: None,
            authenticated_at: 1,
        }
    }

    fn server_account() -> Account {
        Account {
            id: "kiro_server".to_string(),
            provider_type: ProviderType::KiroOAuth,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("kiro@example.com".to_string()),
            access_token: Some("access-token".to_string()),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Default::default(),
            scopes: vec![],
            profile: Some(json!({
                "profileArn": "arn:aws:codewhisperer:us-west-2:123456789012:profile/profile-id",
                "authRegion": "us-east-1",
                "apiRegion": "us-west-2",
                "machineId": "machine-profile",
                "startUrl": "https://view.awsapps.com/start",
                "authMethod": "builder-id",
                "provider": "BuilderId"
            })),
            raw: Some(json!({
                "clientId": "client-id",
                "clientSecret": "client-secret",
                "clientSecretExpiresAt": 123456,
                "resolvedProfileArn": "arn:aws:codewhisperer:us-west-2:123456789012:profile/raw-profile",
                "machineId": "machine-raw",
                "importedAtMs": 1000
            })),
            subscription_level: Some("Kiro Pro".to_string()),
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: Some(9_999_999),
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        }
    }

    #[test]
    fn wire_protocol_fixture_matches_surface_cache_tool_and_recovery_contracts() {
        use crate::domain::providers::model::AppKind;
        use crate::proxy::adapters::{capability_for, AdapterSupport};
        use crate::proxy::retry_policy::{AuthRecoveryDecision, AuthRecoveryState};

        let fixture: Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        assert_eq!(fixture["format"], "cc-switch-kiro-wire-protocol");
        assert_eq!(fixture["schemaVersion"], 1);
        assert_eq!(fixture["accountBinding"]["mode"], "single_explicit_account");
        assert_eq!(fixture["accountBinding"]["accountPool"], false);
        assert_eq!(fixture["accountBinding"]["rotation"], false);
        assert_eq!(fixture["accountBinding"]["crossAccountFailover"], false);
        assert_eq!(fixture["accountBinding"]["crossProviderFailover"], false);

        let recovery = &fixture["accountBinding"]["unauthorizedRecovery"];
        assert_eq!(recovery["refreshAttempts"], 1);
        assert_eq!(recovery["replayAttempts"], 1);
        assert_eq!(recovery["maxInferenceAttempts"], 2);
        assert_eq!(recovery["target"], "same_bound_account");
        let mut recovery_state = AuthRecoveryState::default();
        assert_eq!(
            recovery_state.decide(
                axum::http::StatusCode::UNAUTHORIZED,
                ProviderType::KiroOAuth,
                true,
            ),
            Some(AuthRecoveryDecision::RefreshAndReplaySameBinding)
        );
        assert_eq!(
            recovery_state.decide(
                axum::http::StatusCode::UNAUTHORIZED,
                ProviderType::KiroOAuth,
                true,
            ),
            Some(AuthRecoveryDecision::ReturnUnauthorized)
        );

        for (app, surface, adapter) in [
            (
                AppKind::Claude,
                "claudeMessages",
                "claude_kiro_codewhisperer",
            ),
            (AppKind::Codex, "codexResponses", "codex_to_kiro_anthropic"),
            (
                AppKind::Codex,
                "codexChatCompletions",
                "codex_to_kiro_anthropic",
            ),
        ] {
            assert_eq!(fixture["downstreamSurfaces"][surface]["supported"], true);
            assert_eq!(fixture["downstreamSurfaces"][surface]["adapter"], adapter);
            let capability = capability_for(app, ProviderType::KiroOAuth);
            assert_eq!(capability.adapter, adapter);
            assert_eq!(capability.support, AdapterSupport::Native);
            assert!(capability.supports_stream_usage);
            assert!(capability.supports_model_list);
        }
        let gemini = capability_for(AppKind::Gemini, ProviderType::KiroOAuth);
        assert_eq!(fixture["downstreamSurfaces"]["gemini"]["supported"], false);
        assert_eq!(gemini.support, AdapterSupport::GenericFallback);
        assert!(!gemini.supports_stream_usage);
        assert!(!gemini.supports_model_list);

        assert_eq!(
            fixture["tools"]["invalidJsonCode"],
            KiroToolJsonError::Invalid {
                tool_use_id: "id".to_string(),
                name: "tool".to_string(),
                message: "invalid".to_string(),
            }
            .code()
        );
        assert_eq!(
            fixture["tools"]["incompleteJsonCode"],
            KiroToolJsonError::Incomplete {
                tool_use_id: "id".to_string(),
                name: "tool".to_string(),
                bytes: 1,
            }
            .code()
        );
        assert_eq!(fixture["tools"]["emitBeforeStop"], false);
        assert_eq!(
            fixture["tools"]["maxBufferedJsonBytes"],
            MAX_TOOL_JSON_BYTES
        );
        assert_eq!(
            fixture["tools"]["limitCode"],
            KiroToolJsonError::Limit {
                tool_use_id: "id".to_string(),
                name: "tool".to_string(),
                bytes: 2,
                max: 1,
            }
            .code()
        );
        assert!(fixture["promptCache"]["namespaceComponents"]
            .as_array()
            .is_some_and(|components| components
                .iter()
                .any(|component| component == "auth_identity_generation")));
        assert_eq!(fixture["promptCache"]["capacity"], PROMPT_CACHE_CAPACITY);
        assert_eq!(
            fixture["promptCache"]["defaultTtlSeconds"],
            PROMPT_CACHE_DEFAULT_TTL_SECS
        );
        assert_eq!(
            fixture["promptCache"]["maxTtlSeconds"],
            PROMPT_CACHE_MAX_TTL_SECS
        );
    }

    #[test]
    fn account_data_from_server_account_preserves_kiro_profile() {
        let account = server_account();
        let data = KiroAccountData::from_account(&account).unwrap();

        assert_eq!(data.account_id, "kiro_server");
        assert_eq!(data.email.as_deref(), Some("kiro@example.com"));
        assert_eq!(data.refresh_token, "refresh-token");
        assert_eq!(data.api_region, "us-west-2");
        assert_eq!(data.auth_region, "us-east-1");
        assert_eq!(data.machine_id.as_deref(), Some("machine-profile"));
        assert_eq!(data.client_id.as_deref(), Some("client-id"));
        assert_eq!(data.client_secret.as_deref(), Some("client-secret"));
        assert_eq!(data.client_secret_expires_at, Some(123456));
        assert_eq!(data.authenticated_at, 1000);
        assert_eq!(
            resolve_profile_arn(&data),
            "arn:aws:codewhisperer:us-west-2:123456789012:profile/profile-id"
        );
    }

    #[test]
    fn account_data_rejects_unsafe_stored_regions() {
        for field in ["authRegion", "apiRegion"] {
            let mut account = server_account();
            account.profile.as_mut().unwrap()[field] = json!("us-east-1.attacker.invalid");
            let error = KiroAccountData::from_account(&account).unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
            assert!(error.message.contains("invalid"));
            assert!(!error.message.contains("attacker"));
        }
    }

    #[test]
    fn prepared_request_builds_codewhisperer_shape_and_headers() {
        let account = server_account();
        let body = json!({
            "model": "claude-sonnet-4-8",
            "metadata": {"session_id": "session-a"},
            "messages": [{"role": "user", "content": "hello"}]
        });

        let request = prepare_kiro_request(&account, &body).unwrap();

        assert_eq!(
            request.url,
            "https://q.us-west-2.amazonaws.com/generateAssistantResponse"
        );
        assert_eq!(request.host, "q.us-west-2.amazonaws.com");
        assert_eq!(
            request.cache_namespace,
            format!(
                "account:kiro_server:generation:1:session:{}",
                stable_uuid_like("session-a")
            )
        );
        assert!(request
            .headers
            .iter()
            .any(|(name, value)| { *name == "authorization" && value == "Bearer access-token" }));
        assert!(request.headers.iter().any(|(name, value)| {
            *name == "x-amz-user-agent"
                && value.contains(&format!(
                    "KiroIDE-{}-machine-profile",
                    endpoint::FALLBACK_IDE_VERSION
                ))
        }));
        assert_eq!(
            request.body.pointer("/profileArn"),
            Some(&json!(
                "arn:aws:codewhisperer:us-west-2:123456789012:profile/profile-id"
            ))
        );
        assert_eq!(
            request
                .body
                .pointer("/conversationState/currentMessage/userInputMessage/modelId"),
            Some(&json!("claude-sonnet-4.8"))
        );
        assert!(request.tool_name_map.is_empty());
    }

    #[test]
    fn prepared_request_supports_cli_endpoint_and_api_key_token_type() {
        let mut account = server_account();
        account.access_token = Some("ksk_fixture".to_string());
        account.refresh_token = Some("ksk_fixture".to_string());
        account.profile = Some(json!({
            "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/profile-id",
            "apiRegion": "us-east-1",
            "authMethod": "api_key"
        }));
        account.raw = Some(json!({
            "endpoint": "cli",
            "authMethod": "api_key"
        }));
        let body = json!({
            "model": "claude-sonnet-4-8",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let request = prepare_kiro_request(&account, &body).unwrap();

        assert_eq!(request.url, "https://q.us-east-1.amazonaws.com/");
        assert!(request.headers.iter().any(|(name, value)| {
            *name == "content-type" && value == "application/x-amz-json-1.0"
        }));
        assert!(request.headers.iter().any(|(name, value)| {
            *name == "x-amz-target"
                && value == "AmazonCodeWhispererStreamingService.GenerateAssistantResponse"
        }));
        assert!(request
            .headers
            .iter()
            .any(|(name, value)| *name == "tokentype" && value == "API_KEY"));
        assert_eq!(
            request
                .body
                .pointer("/conversationState/currentMessage/userInputMessage/origin"),
            Some(&json!("KIRO_CLI"))
        );
        assert!(request
            .body
            .pointer("/conversationState/agentContinuationId")
            .is_none());
    }

    #[test]
    fn map_model_supports_4_8_aliases() {
        assert_eq!(map_model("claude-sonnet-4-8"), Some("claude-sonnet-4.8"));
        assert_eq!(map_model("claude-opus-4.8"), Some("claude-opus-4.8"));
        assert_eq!(map_model("claude-haiku-4-5"), Some("claude-haiku-4.5"));
    }

    #[test]
    fn conversion_drops_trailing_prefill_and_normalizes_tool_schema() {
        let long_tool_name = format!("tool_{}", "x".repeat(80));
        let body = json!({
            "model": "claude-sonnet-4-8",
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "answer"},
                {"role": "user", "content": "second"},
                {"role": "assistant", "content": "prefill"}
            ],
            "tools": [{
                "name": long_tool_name,
                "description": "",
                "input_schema": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": {
                        "count": {
                            "type": "number",
                            "exclusiveMinimum": 1,
                            "maximum": 9999999999999.0
                        }
                    }
                }
            }]
        });

        let request =
            anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
        let state = request.body.get("conversationState").unwrap();
        assert_eq!(
            state.pointer("/currentMessage/userInputMessage/content"),
            Some(&json!("second"))
        );
        assert_eq!(
            state.pointer("/currentMessage/userInputMessage/modelId"),
            Some(&json!("claude-sonnet-4.8"))
        );

        let tool = state
            .pointer("/currentMessage/userInputMessage/userInputMessageContext/tools/0/toolSpecification")
            .unwrap();
        let mapped_name = tool.get("name").and_then(Value::as_str).unwrap();
        assert!(mapped_name.chars().count() <= TOOL_NAME_MAX_LEN);
        assert_eq!(
            request.tool_name_map.get(mapped_name),
            Some(&long_tool_name)
        );
        assert_eq!(tool.get("description"), Some(&json!(long_tool_name)));
        let property = tool.pointer("/inputSchema/json/properties/count").unwrap();
        assert!(property.get("exclusiveMinimum").is_none());
        assert!(property.get("maximum").is_none());
    }

    #[test]
    fn conversion_removes_orphaned_history_tool_use() {
        let body = json!({
            "model": "claude-sonnet-4-8",
            "messages": [
                {"role": "user", "content": "run tool"},
                {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "Read",
                    "input": {"file_path": "Cargo.toml"}
                }]},
                {"role": "user", "content": "continue"}
            ],
            "tools": [{
                "name": "Read",
                "description": "read file",
                "input_schema": {"type": "object", "properties": {}}
            }]
        });

        let request =
            anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
        let history = request
            .body
            .pointer("/conversationState/history")
            .and_then(Value::as_array)
            .unwrap();
        assert!(history
            .iter()
            .all(|msg| msg.pointer("/assistantResponseMessage/toolUses").is_none()));
    }

    #[test]
    fn conversion_emits_output_config_only_for_opus_4_6_adaptive() {
        let body = json!({
            "model": "claude-opus-4-6-thinking",
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": "high" },
            "messages": [{ "role": "user", "content": "think" }]
        });

        let request =
            anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
        assert_eq!(
            request
                .body
                .pointer("/additionalModelRequestFields/output_config/effort"),
            Some(&json!("high"))
        );
        assert_eq!(
            request
                .body
                .pointer("/additionalModelRequestFields/thinking/type"),
            Some(&json!("adaptive"))
        );
    }

    #[test]
    fn conversion_emits_output_config_for_new_4_8_models() {
        let body = json!({
            "model": "claude-sonnet-4-8-thinking",
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": "max" },
            "messages": [{ "role": "user", "content": "think" }]
        });

        let request =
            anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
        assert_eq!(
            request
                .body
                .pointer("/additionalModelRequestFields/output_config/effort"),
            Some(&json!("max"))
        );
    }

    #[test]
    fn conversion_skips_output_config_for_unsupported_models() {
        let body = json!({
            "model": "claude-sonnet-4-5-thinking",
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": "high" },
            "messages": [{ "role": "user", "content": "think" }]
        });

        let request =
            anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
        assert!(request.body.get("additionalModelRequestFields").is_none());
    }

    #[test]
    fn sse_builder_restores_original_tool_name() {
        let mut tool_name_map = HashMap::new();
        tool_name_map.insert(
            "short_name".to_string(),
            "very_long_original_name".to_string(),
        );
        let mut builder = SseBuilder::new("claude-sonnet-4-8".to_string(), tool_name_map);
        assert!(builder
            .tool_delta("toolu_1", "short_name", "{\"x\":", false)
            .unwrap()
            .is_empty());
        let bytes = builder
            .tool_delta("toolu_1", "short_name", "1}", true)
            .unwrap()
            .into_iter()
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(bytes.contains("very_long_original_name"));
    }

    #[test]
    fn kiro_reasoning_and_code_events_emit_claude_blocks() {
        let mut builder = SseBuilder::new("claude-sonnet-4-8".to_string(), HashMap::new());
        let reasoning_event = wire::Event::Reasoning(json!({
            "reasoningContentEvent": {
                "text": "think first",
                "signature": "real-signature"
            }
        }));
        let code_event = wire::Event::Code {
            content: "visible answer".to_string(),
        };

        let bytes = process_event_to_sse(&mut builder, reasoning_event)
            .unwrap()
            .into_iter()
            .chain(process_event_to_sse(&mut builder, code_event).unwrap())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("");

        assert!(bytes.contains("\"type\":\"thinking\""));
        assert!(bytes.contains("\"type\":\"thinking_delta\""));
        assert!(bytes.contains("\"thinking\":\"think first\""));
        assert!(bytes.contains("\"type\":\"signature_delta\""));
        assert!(bytes.contains("\"signature\":\"real-signature\""));
        assert!(bytes.contains("\"type\":\"text_delta\""));
        assert!(bytes.contains("\"text\":\"visible answer\""));
    }

    #[test]
    fn tool_leak_filter_rescues_cross_frame_invoke_and_hides_xml() {
        let mut filter = ToolLeakFilter::default();
        let first = filter.push_text(
            "before <function_calls><invoke name=\"Read\"><parameter name=\"file_",
            false,
        );
        let second = filter.push_text(
            "path\">Cargo.toml</parameter><parameter name=\"query\">a &amp; b &lt;c&gt;</parameter></invoke></function_calls> after",
            false,
        );
        let flush = filter.push_text("", true);
        let visible = first
            .into_iter()
            .chain(second)
            .chain(flush)
            .collect::<String>();
        let leaked = filter.take_deduped(&mut HashSet::new());

        assert_eq!(visible, "before  after");
        assert_eq!(leaked.len(), 1);
        assert_eq!(leaked[0].name, "Read");
        assert_eq!(leaked[0].input["file_path"], json!("Cargo.toml"));
        assert_eq!(leaked[0].input["query"], json!("a & b <c>"));
    }

    #[test]
    fn sse_builder_injects_rescued_tool_and_dedupes_native_tool() {
        let mut builder = SseBuilder::new("claude-sonnet-4-8".to_string(), HashMap::new());
        let native = builder
            .tool_delta(
                "toolu_native",
                "Read",
                "{\"file_path\":\"Cargo.toml\"}",
                true,
            )
            .unwrap();
        assert!(!native.is_empty());
        let leaked_text = "visible <function_calls><invoke name=\"Read\"><parameter name=\"file_path\">Cargo.toml</parameter></invoke></function_calls>";
        let bytes = builder
            .assistant_delta(leaked_text)
            .into_iter()
            .chain(builder.final_events())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("");

        assert!(bytes.contains("\"text\":\"visible \""));
        assert!(!bytes.contains("<invoke"));
        assert!(!bytes.contains("toolleakfix"));
    }

    #[test]
    fn sse_builder_injects_rescued_tool_when_native_event_missing() {
        let mut builder = SseBuilder::new("claude-sonnet-4-8".to_string(), HashMap::new());
        let bytes = builder
            .assistant_delta("run <invoke name=\"Bash\"><parameter name=\"command\">pwd</parameter><parameter name=\"timeout\">30</parameter></invoke>")
            .into_iter()
            .chain(builder.final_events())
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("");

        assert!(!bytes.contains("<invoke"));
        assert!(bytes.contains("toolleakfix"));
        assert!(bytes.contains("\"name\":\"Bash\""));
        assert!(bytes.contains("\\\"command\\\":\\\"pwd\\\""));
        assert!(bytes.contains("\\\"timeout\\\":30"));
        assert!(bytes.contains("\"stop_reason\":\"tool_use\""));
    }

    #[test]
    fn non_streaming_reasoning_block_includes_signature() {
        let bytes = event_stream_bytes(vec![(
            "reasoningContentEvent",
            json!({
                "reasoningContentEvent": {
                    "text": "private thought",
                    "signature": "non-stream-signature"
                }
            }),
        )]);

        let message = kiro_event_bytes_to_anthropic_json(
            &bytes,
            "claude-opus-4-6",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(message.pointer("/content/0/type"), Some(&json!("thinking")));
        assert_eq!(
            message.pointer("/content/0/thinking"),
            Some(&json!("private thought"))
        );
        assert_eq!(
            message.pointer("/content/0/signature"),
            Some(&json!("non-stream-signature"))
        );
    }

    #[test]
    fn non_streaming_reasoning_block_uses_fallback_signature() {
        let bytes = event_stream_bytes(vec![(
            "reasoningContentEvent",
            json!({
                "reasoningContentEvent": {
                    "text": "private thought"
                }
            }),
        )]);

        let message = kiro_event_bytes_to_anthropic_json(
            &bytes,
            "claude-opus-4-6",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(
            message.pointer("/content/0/signature"),
            Some(&json!(THINKING_SIGNATURE_FALLBACK))
        );
    }

    #[test]
    fn non_streaming_rescues_leaked_tool_and_preserves_visible_text() {
        let bytes = event_stream_bytes(vec![
            (
                "assistantResponseEvent",
                json!({ "content": "before <function_calls><invoke name=\"Read\"><parameter name=\"file_path\">Cargo.toml</parameter>" }),
            ),
            (
                "assistantResponseEvent",
                json!({ "content": "</invoke></function_calls> after" }),
            ),
        ]);

        let message = kiro_event_bytes_to_anthropic_json(
            &bytes,
            "claude-sonnet-4-8",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(
            message.pointer("/content/0/text"),
            Some(&json!("before  after"))
        );
        assert_eq!(message.pointer("/content/1/type"), Some(&json!("tool_use")));
        assert_eq!(message.pointer("/content/1/name"), Some(&json!("Read")));
        assert_eq!(
            message.pointer("/content/1/input/file_path"),
            Some(&json!("Cargo.toml"))
        );
        assert_eq!(message.get("stop_reason"), Some(&json!("tool_use")));
    }

    #[test]
    fn redacted_thinking_is_preserved() {
        let bytes = event_stream_bytes(vec![(
            "reasoningContentEvent",
            json!({ "reasoningContentEvent": { "redactedContent": "opaque" } }),
        )]);

        let message = kiro_event_bytes_to_anthropic_json(
            &bytes,
            "claude-opus-4-6",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(
            message.pointer("/content/0/type"),
            Some(&json!("redacted_thinking"))
        );
        assert_eq!(message.pointer("/content/0/data"), Some(&json!("opaque")));
    }

    #[test]
    fn kiro_usage_events_are_emitted_in_final_claude_delta() {
        let mut builder = SseBuilder::new("claude-sonnet-4-8".to_string(), HashMap::new());
        builder.usage_event(
            "contextUsageEvent",
            &json!({ "contextUsagePercentage": 1.5 }),
        );
        builder.usage_event(
            "metricsEvent",
            &json!({
                "metricsEvent": {
                    "outputTokens": 42,
                    "cacheReadInputTokens": 7,
                    "cacheCreationInputTokens": 11
                }
            }),
        );

        let bytes = builder
            .final_events()
            .into_iter()
            .map(|b| String::from_utf8(b.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("");

        assert!(bytes.contains("\"input_tokens\":2982"));
        assert!(bytes.contains("\"output_tokens\":42"));
        assert!(bytes.contains("\"cache_read_input_tokens\":7"));
        assert!(bytes.contains("\"cache_creation_input_tokens\":11"));
    }

    #[test]
    fn kiro_prompt_cache_miss_then_hit_from_cache_control() {
        let cache = KiroPromptCache::new(None);
        let body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "You are a coding agent. ".repeat(200),
                    "cache_control": { "type": "ephemeral", "ttl": "5m" }
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": "hello"
                }
            ]
        });

        let first = compute_kiro_prompt_cache_usage_with_cache(&body, &cache);
        assert!(first.cache_creation_tokens > 0);
        assert_eq!(first.cache_read_tokens, 0);

        let second = compute_kiro_prompt_cache_usage_with_cache(&body, &cache);
        assert_eq!(second.cache_creation_tokens, 0);
        assert_eq!(second.cache_read_tokens, first.cache_creation_tokens);
    }

    #[test]
    fn kiro_prompt_cache_isolated_by_provider_account_route_and_session_namespace() {
        let cache = KiroPromptCache::new(None);
        let body = json!({
            "model": "claude-opus-4-7",
            "system": [{
                "type": "text",
                "text": "shared prompt",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "hello"}]
        });

        let first = compute_kiro_prompt_cache_usage_scoped_with_cache(
            &body,
            "provider:a:account:a:route:claude:session:a",
            &cache,
        );
        let isolated = compute_kiro_prompt_cache_usage_scoped_with_cache(
            &body,
            "provider:a:account:b:route:claude:session:a",
            &cache,
        );
        let hit = compute_kiro_prompt_cache_usage_scoped_with_cache(
            &body,
            "provider:a:account:a:route:claude:session:a",
            &cache,
        );

        assert!(first.cache_creation_tokens > 0);
        assert_eq!(isolated.cache_read_tokens, 0);
        assert_eq!(isolated.cache_creation_tokens, first.cache_creation_tokens);
        assert_eq!(hit.cache_creation_tokens, 0);
        assert_eq!(hit.cache_read_tokens, first.cache_creation_tokens);
    }

    #[test]
    fn kiro_prompt_cache_signature_ignores_object_key_order() {
        let cache = KiroPromptCache::new(None);
        let first = json!({
            "tools": [
                {
                    "name": "Read",
                    "description": "Read files",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Path" }
                        }
                    },
                    "cache_control": { "type": "ephemeral" }
                }
            ],
            "messages": [{ "role": "user", "content": "hello" }]
        });
        let second = json!({
            "tools": [
                {
                    "cache_control": { "type": "ephemeral" },
                    "input_schema": {
                        "properties": {
                            "path": { "description": "Path", "type": "string" }
                        },
                        "type": "object"
                    },
                    "description": "Read files",
                    "name": "Read"
                }
            ],
            "messages": [{ "content": "hello", "role": "user" }]
        });

        let miss = compute_kiro_prompt_cache_usage_with_cache(&first, &cache);
        let hit = compute_kiro_prompt_cache_usage_with_cache(&second, &cache);

        assert!(miss.cache_creation_tokens > 0);
        assert_eq!(hit.cache_read_tokens, miss.cache_creation_tokens);
        assert_eq!(hit.cache_creation_tokens, 0);
    }

    #[test]
    fn kiro_prompt_cache_tokens_are_subtracted_from_fresh_input() {
        let mut usage = KiroUsageAccumulator::default();
        usage.set_prompt_cache_usage(KiroPromptCacheUsage {
            cache_read_tokens: 700,
            cache_creation_tokens: 30,
        });
        usage.apply_event(
            "metricsEvent",
            &json!({ "metricsEvent": { "inputTokens": 1_000, "outputTokens": 9 } }),
            "claude-opus-4-7",
        );

        let usage = usage.final_usage(1);
        assert_eq!(usage.input_tokens, 270);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.cache_read_tokens, 700);
        assert_eq!(usage.cache_creation_tokens, 30);
    }

    #[test]
    fn kiro_metrics_input_overrides_context_usage_when_available() {
        let mut usage = KiroUsageAccumulator::default();
        usage.apply_event(
            "contextUsageEvent",
            &json!({ "contextUsageEvent": { "contextUsagePercentage": 2.0 } }),
            "claude-sonnet-4-8[1m]",
        );
        usage.apply_event(
            "metricsEvent",
            &json!({ "inputTokens": 123, "outputTokens": 9 }),
            "claude-sonnet-4-8[1m]",
        );

        let usage = usage.final_usage(1);
        assert_eq!(usage.input_tokens, 123);
        assert_eq!(usage.output_tokens, 9);
    }

    #[test]
    fn kiro_metrics_input_keeps_priority_when_context_arrives_later() {
        let mut usage = KiroUsageAccumulator::default();
        usage.apply_event(
            "metricsEvent",
            &json!({ "metricsEvent": { "inputTokens": 123, "outputTokens": 9 } }),
            "claude-sonnet-4-8",
        );
        usage.apply_event(
            "contextUsageEvent",
            &json!({ "contextUsagePercentage": 2.0 }),
            "claude-sonnet-4-8",
        );

        let usage = usage.final_usage(1);
        assert_eq!(usage.input_tokens, 123);
        assert_eq!(usage.output_tokens, 9);
    }

    #[test]
    fn kiro_metadata_token_usage_is_parsed() {
        let mut usage = KiroUsageAccumulator::default();
        usage.apply_event(
            "messageMetadataEvent",
            &json!({
                "messageMetadataEvent": {
                    "tokenUsage": {
                        "uncachedInputTokens": 200,
                        "cacheReadInputTokens": 70,
                        "cacheWriteInputTokens": 30,
                        "outputTokens": 12
                    }
                }
            }),
            "claude-sonnet-4-8",
        );

        let usage = usage.final_usage(1);
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.cache_read_tokens, 70);
        assert_eq!(usage.cache_creation_tokens, 30);
        assert_eq!(usage.output_tokens, 12);
    }

    #[test]
    fn detects_kiro_quota_and_account_throttle_errors() {
        assert!(is_quota_exhausted(
            r#"{"error":{"reason":"OVERAGE_REQUEST_LIMIT_EXCEEDED"}}"#
        ));
        assert!(is_account_throttled(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Due to suspicious activity, we are imposing temporary limits"
        ));
    }

    #[test]
    fn local_token_count_only_treats_content_blocks_as_images() {
        let tool_input = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "classify",
                        "input": {"type": "image", "label": "diagram"}
                    }]
                },
                {"role": "user", "content": "continue"}
            ]
        });
        assert!(count_input_tokens(&tool_input).unwrap() > 0);

        let missing_image_data = json!({
            "messages": [{
                "role": "user",
                "content": [{"type": "image", "source": {"type": "base64"}}]
            }]
        });
        let error = count_input_tokens(&missing_image_data).unwrap_err();
        assert!(error.message.contains("base64 data"));
    }

    #[test]
    fn schema_normalization_forces_object_and_recovers_top_level_variant() {
        let normalized = normalize_schema(json!({
            "type": "array",
            "oneOf": [
                {
                    "type": "object",
                    "properties": {"paths": {"type": "array", "items": {"type": "string"}}},
                    "required": ["paths"],
                    "additionalProperties": false
                },
                {"type": "string"}
            ]
        }));
        assert_eq!(normalized.get("type"), Some(&json!("object")));
        assert!(normalized.get("oneOf").is_none());
        assert_eq!(
            normalized.pointer("/properties/paths/type"),
            Some(&json!("array"))
        );
        assert_eq!(normalized.get("required"), Some(&json!(["paths"])));
        assert_eq!(normalized.get("additionalProperties"), Some(&json!(false)));
    }

    #[test]
    fn schema_normalization_strips_combinator_without_object_variant() {
        let normalized = normalize_schema(json!({
            "anyOf": [{"type": "string"}, {"type": "number"}]
        }));
        assert_eq!(normalized.get("type"), Some(&json!("object")));
        assert!(normalized.get("anyOf").is_none());
        assert_eq!(normalized.get("properties"), Some(&json!({})));
    }

    #[test]
    fn non_streaming_tool_json_requires_valid_stopped_input() {
        let valid = event_stream_bytes(vec![
            (
                "toolUseEvent",
                json!({
                    "toolUseId": "toolu_1",
                    "name": "Read",
                    "input": "{\"file_",
                    "stop": false
                }),
            ),
            (
                "toolUseEvent",
                json!({
                    "toolUseId": "toolu_1",
                    "name": "Read",
                    "input": "path\":\"Cargo.toml\"}",
                    "stop": true
                }),
            ),
        ]);
        let message = kiro_event_bytes_to_anthropic_json(
            &valid,
            "claude-sonnet-4-8",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(
            message.pointer("/content/0/input/file_path"),
            Some(&json!("Cargo.toml"))
        );

        let invalid = event_stream_bytes(vec![(
            "toolUseEvent",
            json!({
                "toolUseId": "toolu_invalid",
                "name": "Read",
                "input": "{\"file_path\":",
                "stop": true
            }),
        )]);
        let error = kiro_event_bytes_to_anthropic_json(
            &invalid,
            "claude-sonnet-4-8",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "TOOL_JSON_INVALID");

        let incomplete = event_stream_bytes(vec![(
            "toolUseEvent",
            json!({
                "toolUseId": "toolu_incomplete",
                "name": "Read",
                "input": "{\"file_path\":",
                "stop": false
            }),
        )]);
        let error = kiro_event_bytes_to_anthropic_json(
            &incomplete,
            "claude-sonnet-4-8",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "TOOL_JSON_INCOMPLETE");
        assert!(!error.to_string().contains("file_path"));
    }

    #[test]
    fn tool_json_accumulator_bounds_fragments_across_frames() {
        let mut accumulator = ToolJsonAccumulator::with_max_buffered_bytes(8);
        assert!(accumulator
            .push("toolu_limit", "Write", "{\"x\":", false, &HashMap::new())
            .unwrap()
            .is_none());
        let error = accumulator
            .push("toolu_limit", "Write", "1234", true, &HashMap::new())
            .unwrap_err();
        assert!(matches!(
            error,
            KiroToolJsonError::Limit {
                bytes: 9,
                max: 8,
                ..
            }
        ));
        assert_eq!(error.code(), "TOOL_JSON_LIMIT");
        assert!(!error.to_string().contains("toolu_limit"));
    }

    #[tokio::test]
    async fn streaming_tool_json_emits_nothing_before_the_stop_fragment() {
        let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(4);
        let upstream = futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|bytes| (bytes, receiver))
        })
        .map(Ok::<Bytes, reqwest::Error>);
        let stream = kiro_event_stream_to_claude_sse(
            upstream,
            "claude-sonnet-4-8".to_string(),
            HashMap::new(),
            &json!({"model": "claude-sonnet-4-8", "messages": []}),
        );
        tokio::pin!(stream);
        let initial = stream.next().await.unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&initial).contains("file_"));

        sender
            .send(Bytes::from(fixture_event_frame(
                "toolUseEvent",
                &json!({
                    "toolUseId": "toolu_gate",
                    "name": "Read",
                    "input": "{\"file_",
                    "stop": false
                }),
            )))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), stream.next())
                .await
                .is_err()
        );

        sender
            .send(Bytes::from(fixture_event_frame(
                "toolUseEvent",
                &json!({
                    "toolUseId": "toolu_gate",
                    "name": "Read",
                    "input": "path\":\"Cargo.toml\"}",
                    "stop": true
                }),
            )))
            .await
            .unwrap();
        let mut emitted = String::new();
        for _ in 0..4 {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await
                .expect("completed tool JSON should emit promptly")
                .expect("Kiro stream should remain open")
                .expect("completed tool JSON should be valid");
            emitted.push_str(&String::from_utf8_lossy(&chunk));
            if emitted.contains("input_json_delta") {
                break;
            }
        }
        assert!(emitted.contains("toolu_gate"), "{emitted}");
        assert!(emitted.contains("input_json_delta"), "{emitted}");
        assert!(emitted.contains("Cargo.toml"), "{emitted}");
        drop(sender);
    }

    #[test]
    fn detects_terminal_kiro_client_validation_reasons() {
        assert!(is_client_validation_error(
            br#"{"reason":"TOOL_SCHEMA_INVALID"}"#
        ));
        assert!(is_client_validation_error(
            br#"{"error":{"reason":"TOOL_USE_RESULT_MISMATCH"}}"#
        ));
        assert!(is_client_validation_error(
            b"upstream error: TOOL_USE_RESULT_MISMATCH"
        ));
        assert!(is_client_validation_error(
            b"Expected toolResult blocks for the previous toolUse blocks"
        ));
        assert!(!is_client_validation_error(
            br#"{"message":"documentation mentions TOOL_SCHEMA_INVALID"}"#
        ));
        assert!(!is_client_validation_error(
            br#"{"__type":"ValidationException","message":"temporary"}"#
        ));
    }

    #[test]
    fn inline_thinking_split_preserves_state_across_chunks() {
        let mut state = false;
        let first = split_inline_thinking("hello <thinking>secret", &mut state);
        assert_eq!(
            first,
            vec![
                InlineThinkingSegment {
                    is_thinking: false,
                    text: "hello ".to_string()
                },
                InlineThinkingSegment {
                    is_thinking: true,
                    text: "secret".to_string()
                }
            ]
        );
        assert!(state);
        let second = split_inline_thinking(" more</thinking> visible", &mut state);
        assert_eq!(
            second,
            vec![
                InlineThinkingSegment {
                    is_thinking: true,
                    text: " more".to_string()
                },
                InlineThinkingSegment {
                    is_thinking: false,
                    text: " visible".to_string()
                }
            ]
        );
        assert!(!state);
    }

    #[test]
    fn event_stream_crc_rejects_corrupt_frames() {
        let mut bytes = event_frame("assistantResponseEvent", json!({"content": "ok"}));
        if let Some(last) = bytes.last_mut() {
            *last ^= 0xff;
        }
        let mut decoder = wire::EventStreamDecoder::strict();
        assert!(decoder.feed(&bytes).is_err());
    }

    fn event_stream_bytes(events: Vec<(&str, Value)>) -> Vec<u8> {
        events
            .into_iter()
            .flat_map(|(event_type, payload)| event_frame(event_type, payload))
            .collect()
    }

    fn event_frame(event_type: &str, payload: Value) -> Vec<u8> {
        let mut headers = Vec::new();
        push_string_header(&mut headers, ":event-type", event_type);
        let payload = serde_json::to_vec(&payload).unwrap();
        let total_len = 12 + headers.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(&(total_len as u32).to_be_bytes());
        frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        let prelude_crc = crc32(&frame[..8]);
        frame.extend_from_slice(&prelude_crc.to_be_bytes());
        frame.extend_from_slice(&headers);
        frame.extend_from_slice(&payload);
        let message_crc = crc32(&frame);
        frame.extend_from_slice(&message_crc.to_be_bytes());
        frame
    }

    fn push_string_header(out: &mut Vec<u8>, name: &str, value: &str) {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(7);
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
}
