use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::infra::resident_memory::{ResidentMemoryBudget, ResidentMemoryReservation};
use bytes::Bytes;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

mod resident;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KimiReplayWriteOutcome {
    Stored,
    Conflict,
    CapacityRejected,
    InvalidContent,
}

use crate::domain::kimi_cli::KimiDeviceIdentity;

const MAX_MODEL_CATALOG_ENTRIES: usize = 64;
const MAX_MODEL_RESPONSE_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_REPLAY_ENTRIES: usize = 1024;
const MAX_REPLAY_BYTES_PER_ENTRY: usize = 8 * 1024 * 1024;
const MAX_REPLAY_BLOCKS_PER_ENTRY: usize = 512;
const MAX_REPLAY_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub const KIMI_MODEL_CATALOG_TTL_MS: i64 = 5 * 60 * 1000;
pub const KIMI_STALE_MODEL_CATALOG_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const KIMI_THINKING_REPLAY_TTL_MS: i64 = 60 * 60 * 1000;

pub fn kimi_thinking_replay_user_namespace(user_identity: &str) -> Option<String> {
    let normalized = user_identity.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let digest = Sha256::digest(normalized.as_bytes());
    Some(format!("principal_{}", hex::encode(&digest[..16])))
}

pub fn kimi_thinking_replay_model_family(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "k3" | "kimi-k3" | "k3-256k" | "kimi-k3-256k" => Some("k3"),
        crate::domain::kimi_cli::KIMI_DEFAULT_MODEL | "kimi-k2.7-code" => {
            Some(crate::domain::kimi_cli::KIMI_DEFAULT_MODEL)
        }
        crate::domain::kimi_cli::KIMI_HIGHSPEED_MODEL | "kimi-k2.7-code-highspeed" => {
            Some(crate::domain::kimi_cli::KIMI_HIGHSPEED_MODEL)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KimiModelCatalogScope(String);

impl KimiModelCatalogScope {
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
    ) -> Self {
        Self(scoped_digest(
            b"cc-switch-server:kimi-model-catalog:v1\0",
            [
                app,
                provider_id,
                &provider_revision.to_string(),
                runtime_fingerprint,
                account_id,
                &auth_identity_generation.to_string(),
                &token_refresh_generation.to_string(),
            ],
            "kimi-model-catalog-v1",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KimiThinkingReplayScope(String);

impl KimiThinkingReplayScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
        share_id: &str,
        user_namespace: &str,
        session_id: &str,
        model_family: &str,
    ) -> Option<Self> {
        let app = app.trim();
        let provider_id = provider_id.trim();
        let runtime_fingerprint = runtime_fingerprint.trim();
        let account_id = account_id.trim();
        let share_id = share_id.trim();
        let user_namespace = user_namespace.trim();
        let session_id = session_id.trim();
        let model_family = model_family.trim();
        if app.is_empty()
            || provider_id.is_empty()
            || runtime_fingerprint.is_empty()
            || account_id.is_empty()
            || share_id.is_empty()
            || user_namespace.is_empty()
            || session_id.is_empty()
            || model_family.is_empty()
        {
            return None;
        }
        Some(Self(scoped_digest(
            b"cc-switch-server:kimi-thinking-replay:v2\0",
            [
                app,
                provider_id,
                &provider_revision.to_string(),
                runtime_fingerprint,
                account_id,
                &auth_identity_generation.to_string(),
                &token_refresh_generation.to_string(),
                share_id,
                user_namespace,
                session_id,
                model_family,
            ],
            "kimi-thinking-replay-v2",
        )))
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

#[derive(Debug)]
pub struct KimiModelCatalogData {
    pub models: Vec<String>,
    _memory: Option<ResidentMemoryReservation>,
    _payload_memory: Option<ResidentMemoryReservation>,
}

#[derive(Debug, Clone)]
pub struct KimiModelCatalog {
    data: Arc<KimiModelCatalogData>,
    pub source: &'static str,
    pub fetched_at_ms: i64,
    pub stale: bool,
    expires_at_ms: i64,
}

impl std::ops::Deref for KimiModelCatalog {
    type Target = KimiModelCatalogData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

#[derive(Debug)]
pub struct KimiModelCatalogCache {
    catalogs: RwLock<BTreeMap<KimiModelCatalogScope, KimiModelCatalog>>,
    flights: crate::infra::resident_flights::ResidentFlights<KimiModelCatalogScope>,
    parent: ResidentMemoryBudget,
}

#[derive(Debug, Clone)]
pub struct KimiModelFetchError {
    pub capacity_rejected: bool,
    pub status: Option<StatusCode>,
    pub message: String,
}

#[derive(Debug)]
pub struct FetchedKimiModelCatalog {
    pub models: Vec<String>,
    pub upstream_non_empty: bool,
    _memory: Option<ResidentMemoryReservation>,
}

impl PartialEq for FetchedKimiModelCatalog {
    fn eq(&self, other: &Self) -> bool {
        self.models == other.models && self.upstream_non_empty == other.upstream_non_empty
    }
}
impl Eq for FetchedKimiModelCatalog {}

impl KimiModelFetchError {
    fn capacity() -> Self {
        crate::metrics::record_resident_retention_refused("kimi_catalog_fetch");
        Self {
            capacity_rejected: true,
            status: Some(StatusCode::SERVICE_UNAVAILABLE),
            message: "Kimi model discovery capacity is exhausted".into(),
        }
    }
    pub fn retryable(&self) -> bool {
        !self.capacity_rejected
            && self.status.is_none_or(|status| {
                status == StatusCode::REQUEST_TIMEOUT
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            })
    }
}

impl std::fmt::Display for KimiModelFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for KimiModelFetchError {}

pub async fn fetch_kimi_models(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    identity: &KimiDeviceIdentity,
    timeout: Duration,
) -> Result<FetchedKimiModelCatalog, KimiModelFetchError> {
    fetch_kimi_models_with_parent(
        http,
        url,
        access_token,
        identity,
        timeout,
        crate::infra::resident_memory::process_resident_memory(),
    )
    .await
}

async fn fetch_kimi_models_with_parent(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    identity: &KimiDeviceIdentity,
    timeout: Duration,
    parent: ResidentMemoryBudget,
) -> Result<FetchedKimiModelCatalog, KimiModelFetchError> {
    let memory = crate::infra::resident_flights::CatalogFetchMemory::new(
        parent,
        MAX_MODEL_RESPONSE_BODY_BYTES,
    )
    .ok_or_else(KimiModelFetchError::capacity)?;
    let mut request = http
        .get(url)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .timeout(timeout);
    for (name, value) in identity.headers() {
        request = request.header(name, value);
    }
    let mut response = request.send().await.map_err(|error| KimiModelFetchError {
        capacity_rejected: false,
        status: None,
        message: format!("Kimi model request failed: {error}"),
    })?;
    let status = response.status();
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_MODEL_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| KimiModelFetchError {
        capacity_rejected: false,
        status: Some(status),
        message: format!("Kimi model response could not be read: {error}"),
    })?;
    if !status.is_success() {
        return Err(KimiModelFetchError {
            capacity_rejected: false,
            status: Some(status),
            message: format!("Kimi model request returned HTTP {status}"),
        });
    }
    let _parsing = memory
        .parsing(body.len())
        .ok_or_else(KimiModelFetchError::capacity)?;
    let value = serde_json::from_slice::<Value>(&body).map_err(|error| KimiModelFetchError {
        capacity_rejected: false,
        status: Some(status),
        message: format!("Kimi model response is not valid JSON: {error}"),
    })?;
    let mut fetched = parse_kimi_models(&value).map_err(|message| KimiModelFetchError {
        capacity_rejected: false,
        status: Some(status),
        message,
    })?;
    fetched._memory = Some(
        memory
            .payload(
                resident::models_bytes(&fetched.models)
                    .saturating_add(std::mem::size_of::<FetchedKimiModelCatalog>()),
            )
            .ok_or_else(KimiModelFetchError::capacity)?,
    );
    Ok(fetched)
}

pub fn parse_kimi_models(value: &Value) -> Result<FetchedKimiModelCatalog, String> {
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .unwrap_or(value)
        .as_array()
        .ok_or_else(|| "Kimi model response does not contain an array catalog".to_string())?;
    let upstream_non_empty = entries.iter().any(|entry| {
        entry
            .as_str()
            .or_else(|| {
                entry
                    .get("id")
                    .or_else(|| entry.get("name"))
                    .and_then(Value::as_str)
            })
            .is_some_and(|model| !model.trim().is_empty())
    });
    let mut models = entries
        .iter()
        .filter_map(|entry| {
            entry.as_str().or_else(|| {
                entry
                    .get("id")
                    .or_else(|| entry.get("name"))
                    .and_then(Value::as_str)
            })
        })
        .map(str::trim)
        .filter(|model| crate::domain::kimi_cli::KIMI_REVIEWED_WIRE_MODELS.contains(model))
        .map(str::to_string)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if upstream_non_empty && models.is_empty() {
        return Err(
            "Kimi model response contains no reviewed model identifiers; catalog contract may have drifted"
                .to_string(),
        );
    }
    Ok(FetchedKimiModelCatalog {
        models,
        upstream_non_empty,
        _memory: None,
    })
}

pub fn unavailable_catalog() -> KimiModelCatalog {
    KimiModelCatalog {
        data: Arc::new(KimiModelCatalogData {
            models: Vec::new(),
            _memory: None,
            _payload_memory: None,
        }),
        source: "kimi_models_unavailable",
        fetched_at_ms: 0,
        stale: false,
        expires_at_ms: 0,
    }
}

#[derive(Debug, Clone)]
struct ThinkingReplayEntry {
    content: Bytes,
    recorded_at_ms: i64,
    expires_at_ms: i64,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KimiThinkingReplaySnapshot {
    generation: Option<u64>,
}

#[derive(Debug, Default)]
struct ThinkingReplayStore {
    entries: BTreeMap<KimiThinkingReplayScope, ThinkingReplayEntry>,
    next_generation: u64,
    total_bytes: usize,
}

#[derive(Debug)]
pub struct KimiThinkingReplayCache {
    store: Mutex<ThinkingReplayStore>,
    parent: ResidentMemoryBudget,
}

fn prune_replay_store(store: &mut ThinkingReplayStore, now_ms: i64) -> usize {
    let expired = store
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at_ms <= now_ms)
        .map(|(scope, _)| scope.clone())
        .collect::<Vec<_>>();
    let expired_count = expired.len();
    for scope in expired {
        if let Some(entry) = store.entries.remove(&scope) {
            store.total_bytes = store.total_bytes.saturating_sub(entry.content.len());
        }
    }
    expired_count
}

pub fn valid_replay_content(content: &[u8]) -> bool {
    if content.len() > MAX_REPLAY_BYTES_PER_ENTRY {
        return false;
    }
    let Ok(Value::Array(blocks)) = serde_json::from_slice::<Value>(content) else {
        return false;
    };
    if blocks.len() > MAX_REPLAY_BLOCKS_PER_ENTRY {
        return false;
    }
    let signed_thinking = blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("thinking")
            && block
                .get("signature")
                .and_then(Value::as_str)
                .is_some_and(|signature| !signature.trim().is_empty())
            && block
                .get("thinking")
                .and_then(Value::as_str)
                .is_none_or(|thinking| !super::kimi::is_kimi_reasoning_unavailable(thinking))
    });
    let tool_use = blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("tool_use")
            && block
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.trim().is_empty())
    });
    signed_thinking && tool_use
}

pub fn restore_kimi_thinking_replay_content(body: &[u8], cached_content: &[u8]) -> Option<Bytes> {
    if !valid_replay_content(cached_content) {
        return None;
    }
    let cached = serde_json::from_slice::<Value>(cached_content).ok()?;
    let cached_parts = non_thinking_content_parts(&cached)?;
    let mut request = serde_json::from_slice::<Value>(body).ok()?;
    let messages = request.get_mut("messages")?.as_array_mut()?;
    for message in messages.iter_mut().rev() {
        let Some(message) = message.as_object_mut() else {
            continue;
        };
        if !message
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role.trim().eq_ignore_ascii_case("assistant"))
        {
            continue;
        }
        let Some(content) = message.get("content") else {
            continue;
        };
        if content == &cached || content_has_usable_thinking(content) {
            continue;
        }
        let Some(current_parts) = non_thinking_content_parts(content) else {
            continue;
        };
        if current_parts != cached_parts {
            continue;
        }
        message.insert("content".to_string(), cached.clone());
        return serde_json::to_vec(&request).ok().map(Bytes::from);
    }
    None
}

fn content_has_usable_thinking(content: &Value) -> bool {
    content.as_array().is_some_and(|parts| {
        parts
            .iter()
            .any(|part| match part.get("type").and_then(Value::as_str) {
                Some("redacted_thinking") => true,
                Some("thinking") => {
                    part.get("signature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.trim().is_empty())
                        && part
                            .get("thinking")
                            .and_then(Value::as_str)
                            .is_none_or(|thinking| {
                                !super::kimi::is_kimi_reasoning_unavailable(thinking)
                            })
                }
                _ => false,
            })
    })
}

fn non_thinking_content_parts(content: &Value) -> Option<Vec<Value>> {
    let parts = content.as_array()?;
    let mut has_tool_use = false;
    let mut normalized = Vec::with_capacity(parts.len());
    for part in parts {
        let part_type = part.get("type").and_then(Value::as_str)?;
        if matches!(part_type, "thinking" | "redacted_thinking") {
            continue;
        }
        if part_type == "tool_use" {
            if part
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|id| id.trim().is_empty())
            {
                return None;
            }
            has_tool_use = true;
        }
        normalized.push(part.clone());
    }
    has_tool_use.then_some(normalized)
}

#[derive(Debug, Clone, PartialEq)]
pub enum KimiThinkingReplayStreamOutcome {
    Replayable(Bytes),
    NonReplayable,
    Incomplete,
}

#[derive(Debug)]
struct KimiThinkingReplayStreamBlock {
    value: Value,
    partial_input: Option<String>,
    finished: bool,
}

#[derive(Debug)]
pub struct KimiThinkingReplayStreamAccumulator {
    pending: Vec<u8>,
    blocks: BTreeMap<u64, KimiThinkingReplayStreamBlock>,
    saw_message_start: bool,
    complete: bool,
    abandoned: bool,
    bytes_used: usize,
    memory: resident::ReplayStreamMemory,
}

impl KimiThinkingReplayStreamAccumulator {
    pub fn is_complete(&self) -> bool {
        self.complete && !self.abandoned
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if self.abandoned || chunk.is_empty() {
            return;
        }
        if self.complete && chunk.iter().any(|byte| !byte.is_ascii_whitespace()) {
            self.abandon();
            return;
        }
        if !self.reserve(chunk.len()) {
            return;
        }
        self.pending.extend_from_slice(chunk);
        while let Some((end, delimiter_len)) = replay_sse_event_boundary(&self.pending) {
            let event = self.pending[..end].to_vec();
            self.pending.drain(..end + delimiter_len);
            if !self.inspect_event(&event) {
                self.abandon();
                return;
            }
        }
        if !self.memory.resize(self.retained_bytes()) {
            self.abandon_capacity();
        }
    }

    pub fn finish(&mut self) -> KimiThinkingReplayStreamOutcome {
        if self.abandoned
            || self.pending.iter().any(|byte| !byte.is_ascii_whitespace())
            || !self.saw_message_start
            || !self.complete
            || self.blocks.values().any(|block| !block.finished)
        {
            return KimiThinkingReplayStreamOutcome::Incomplete;
        }
        if !self.memory.grow(
            self.retained_bytes()
                .saturating_mul(2)
                .saturating_add(64 * 1024),
        ) {
            self.abandon_capacity();
            return KimiThinkingReplayStreamOutcome::Incomplete;
        }
        let content = Value::Array(
            self.blocks
                .values()
                .map(|block| block.value.clone())
                .collect(),
        );
        let Some(encoded_bytes) = crate::infra::resident_memory::serialized_json_bytes(
            &content,
            MAX_REPLAY_BYTES_PER_ENTRY,
        ) else {
            return KimiThinkingReplayStreamOutcome::Incomplete;
        };
        let Some(memory) = self.memory.output(encoded_bytes) else {
            self.abandon_capacity();
            return KimiThinkingReplayStreamOutcome::Incomplete;
        };
        let Ok(content) = serde_json::to_vec(&content) else {
            return KimiThinkingReplayStreamOutcome::Incomplete;
        };
        let content = Bytes::from_owner(resident::ReplayBytes {
            content,
            _memory: memory,
        });
        if content.len() > MAX_REPLAY_BYTES_PER_ENTRY {
            return KimiThinkingReplayStreamOutcome::Incomplete;
        }
        if valid_replay_content(&content) {
            KimiThinkingReplayStreamOutcome::Replayable(content)
        } else {
            KimiThinkingReplayStreamOutcome::NonReplayable
        }
    }

    fn inspect_event(&mut self, event: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(event) else {
            return false;
        };
        let mut declared_event = None;
        let mut data = Vec::new();
        for raw_line in text.split('\n') {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(value) = replay_sse_field(line, "event") {
                declared_event = Some(value);
            } else if let Some(value) = replay_sse_field(line, "data") {
                data.push(value);
            }
        }
        if data.is_empty() {
            return true;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data.join("\n")) else {
            return false;
        };
        let Some(event_type) = value.get("type").and_then(Value::as_str) else {
            return false;
        };
        if declared_event.is_some_and(|declared| declared != event_type) {
            return false;
        }
        if self.complete {
            return false;
        }
        match event_type {
            "message_start" => {
                if self.saw_message_start {
                    return false;
                }
                self.saw_message_start = true;
                true
            }
            "ping" | "message_delta" => self.saw_message_start,
            "content_block_start" => self.start_block(&value),
            "content_block_delta" => self.apply_delta(&value),
            "content_block_stop" => self.stop_block(&value),
            "message_stop" => {
                if !self.saw_message_start || self.blocks.values().any(|block| !block.finished) {
                    return false;
                }
                self.complete = true;
                true
            }
            "error" => false,
            _ => false,
        }
    }

    fn start_block(&mut self, event: &Value) -> bool {
        if !self.saw_message_start || self.blocks.len() >= MAX_REPLAY_BLOCKS_PER_ENTRY {
            return false;
        }
        let Some(index) = event.get("index").and_then(Value::as_u64) else {
            return false;
        };
        let Some(block) = event.get("content_block").filter(|block| block.is_object()) else {
            return false;
        };
        if self.blocks.contains_key(&index) {
            return false;
        }
        let Some(block_type) = block.get("type").and_then(Value::as_str) else {
            return false;
        };
        if !matches!(
            block_type,
            "text" | "thinking" | "redacted_thinking" | "tool_use"
        ) {
            return false;
        }
        if block_type == "tool_use"
            && (block
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|id| id.trim().is_empty())
                || block
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|name| name.trim().is_empty())
                || !block.get("input").is_some_and(Value::is_object))
        {
            return false;
        }
        let Ok(encoded) = serde_json::to_vec(block) else {
            return false;
        };
        if !self.reserve(encoded.len()) {
            return false;
        }
        self.blocks.insert(
            index,
            KimiThinkingReplayStreamBlock {
                value: block.clone(),
                partial_input: None,
                finished: false,
            },
        );
        true
    }

    fn apply_delta(&mut self, event: &Value) -> bool {
        let Some(index) = event.get("index").and_then(Value::as_u64) else {
            return false;
        };
        let Some(delta) = event.get("delta").and_then(Value::as_object) else {
            return false;
        };
        let Some(delta_type) = delta.get("type").and_then(Value::as_str) else {
            return false;
        };
        let Some(block_type) = self
            .blocks
            .get(&index)
            .and_then(|block| block.value.get("type"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        let field = match (block_type, delta_type) {
            ("text", "text_delta") => "text",
            ("thinking", "thinking_delta") => "thinking",
            ("thinking", "signature_delta") => "signature",
            ("tool_use", "input_json_delta") => {
                let Some(suffix) = delta.get("partial_json").and_then(Value::as_str) else {
                    return false;
                };
                if !self.reserve(suffix.len()) {
                    return false;
                }
                let block = self.blocks.get_mut(&index).expect("block checked above");
                block
                    .partial_input
                    .get_or_insert_with(String::new)
                    .push_str(suffix);
                return true;
            }
            _ => return false,
        };
        let Some(suffix) = delta.get(field).and_then(Value::as_str) else {
            return false;
        };
        if !self.reserve(suffix.len()) {
            return false;
        }
        let block = self.blocks.get_mut(&index).expect("block checked above");
        let Some(object) = block.value.as_object_mut() else {
            return false;
        };
        let existing = object
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default();
        object.insert(
            field.to_string(),
            Value::String(format!("{existing}{suffix}")),
        );
        true
    }

    fn stop_block(&mut self, event: &Value) -> bool {
        let Some(index) = event.get("index").and_then(Value::as_u64) else {
            return false;
        };
        let Some(block) = self.blocks.get_mut(&index) else {
            return false;
        };
        if block.finished {
            return false;
        }
        if let Some(input) = block.partial_input.take() {
            let Ok(input) = serde_json::from_str::<Value>(&input) else {
                return false;
            };
            if !input.is_object() {
                return false;
            }
            let Some(object) = block.value.as_object_mut() else {
                return false;
            };
            object.insert("input".to_string(), input);
        }
        block.finished = true;
        true
    }

    fn reserve(&mut self, additional: usize) -> bool {
        let Some(total) = self.bytes_used.checked_add(additional) else {
            self.abandon();
            return false;
        };
        if total > MAX_REPLAY_BYTES_PER_ENTRY {
            self.abandon();
            return false;
        }
        // Includes the pending event copy, JSON parsing, collection nodes and
        // String growth. Shrink to the retained estimate after this push.
        let transient = self
            .retained_bytes()
            .saturating_add(additional.saturating_mul(64))
            .saturating_add(64 * 1024);
        if !self.memory.grow(transient) {
            self.abandon_capacity();
            return false;
        }
        self.bytes_used = total;
        true
    }

    fn abandon(&mut self) {
        self.abandoned = true;
        self.pending = Vec::new();
        self.blocks = BTreeMap::new();
        self.bytes_used = 0;
        self.memory.resize(0);
    }
}

fn replay_sse_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) if crlf <= lf => Some((crlf, 4)),
        (Some(lf), Some(_)) | (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn replay_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let value = line.strip_prefix(field)?.strip_prefix(':')?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay_scope(suffix: &str) -> KimiThinkingReplayScope {
        KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 3, "share", "user", suffix, "k3",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn replay_is_generation_fenced_and_scope_isolated() {
        let cache = KimiThinkingReplayCache::default();
        let first = replay_scope("session-a");
        let other = replay_scope("session-b");
        let content = Bytes::from_static(
            br#"[{"type":"thinking","thinking":"x","signature":"sig"},{"type":"tool_use","id":"tool-1","name":"read","input":{}}]"#,
        );
        let (_, missing) = cache.get(&first, 1).await;
        assert!(
            cache
                .replace_if_unchanged(first.clone(), missing, content.clone(), 1)
                .await
                == KimiReplayWriteOutcome::Stored
        );
        let (loaded, current) = cache.get(&first, 2).await;
        assert_eq!(loaded.as_deref(), Some(content.as_ref()));
        assert!(cache.get(&other, 2).await.0.is_none());
        assert!(
            cache
                .replace_if_unchanged(first.clone(), missing, content.clone(), 3)
                .await
                == KimiReplayWriteOutcome::Conflict
        );
        assert!(cache.delete_if_unchanged(&first, current).await);
        assert!(cache.get(&first, 4).await.0.is_none());
    }

    #[test]
    fn replay_scope_requires_every_tenant_component_and_normalizes_families() {
        assert!(KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 3, "", "user", "session", "k3"
        )
        .is_none());
        assert!(KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 3, "share", "", "session", "k3"
        )
        .is_none());
        assert!(KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 3, "share", "user", "", "k3"
        )
        .is_none());
        assert_eq!(kimi_thinking_replay_model_family("kimi-k3"), Some("k3"));
        assert_eq!(kimi_thinking_replay_model_family("k3-256k"), Some("k3"));
        assert_eq!(
            kimi_thinking_replay_model_family("kimi-k2.7-code-highspeed"),
            Some(crate::domain::kimi_cli::KIMI_HIGHSPEED_MODEL)
        );
        assert!(kimi_thinking_replay_model_family("unknown").is_none());
        assert_eq!(
            kimi_thinking_replay_user_namespace(" User@Example.COM "),
            kimi_thinking_replay_user_namespace("user@example.com")
        );
        assert!(kimi_thinking_replay_user_namespace(" ").is_none());

        let token_generation_three = KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 3, "share", "user", "session", "k3",
        )
        .unwrap();
        let token_generation_four = KimiThinkingReplayScope::derive(
            "claude", "provider", 1, "runtime", "account", 2, 4, "share", "user", "session", "k3",
        )
        .unwrap();
        assert_ne!(token_generation_three, token_generation_four);
    }

    #[test]
    fn replay_restores_only_an_exact_non_thinking_assistant_tool_turn() {
        let cached = br#"[
            {"type":"thinking","thinking":"signed","signature":"sig"},
            {"type":"text","text":"inspect"},
            {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"README.md"}}
        ]"#;
        let body = br#"{"messages":[
            {"role":"user","content":"inspect"},
            {"role":"assistant","content":[
                {"type":"text","text":"inspect"},
                {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"README.md"}}
            ]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}
        ]}"#;
        let restored = restore_kimi_thinking_replay_content(body, cached).unwrap();
        let restored = serde_json::from_slice::<Value>(&restored).unwrap();
        assert_eq!(
            restored.pointer("/messages/1/content").unwrap(),
            &serde_json::from_slice::<Value>(cached).unwrap()
        );

        let changed = body
            .windows(b"README.md".len())
            .position(|window| window == b"README.md")
            .map(|offset| {
                let mut changed = body.to_vec();
                changed.splice(
                    offset..offset + b"README.md".len(),
                    b"CHANGELOG".iter().copied(),
                );
                changed
            })
            .unwrap();
        assert!(restore_kimi_thinking_replay_content(&changed, cached).is_none());

        let already_signed = br#"{"messages":[{"role":"assistant","content":[
            {"type":"thinking","thinking":"caller","signature":"caller-sig"},
            {"type":"text","text":"inspect"},
            {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"README.md"}}
        ]}]}"#;
        assert!(restore_kimi_thinking_replay_content(already_signed, cached).is_none());

        let signature_only = br#"{"messages":[{"role":"assistant","content":[
            {"type":"thinking","signature":"caller-sig"},
            {"type":"text","text":"inspect"},
            {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"README.md"}}
        ]}]}"#;
        assert!(restore_kimi_thinking_replay_content(signature_only, cached).is_none());

        for placeholder in [
            "",
            crate::proxy::kimi::KIMI_REASONING_UNAVAILABLE,
            "unsigned caller reasoning",
        ] {
            let body = serde_json::to_vec(
                &serde_json::json!({"messages":[{"role":"assistant","content":[
                    {"type":"thinking","thinking":placeholder},
                    {"type":"text","text":"inspect"},
                    {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"README.md"}}
                ]}]}),
            )
            .unwrap();
            let restored = restore_kimi_thinking_replay_content(&body, cached).unwrap();
            let restored = serde_json::from_slice::<Value>(&restored).unwrap();
            assert_eq!(
                restored.pointer("/messages/0/content").unwrap(),
                &serde_json::from_slice::<Value>(cached).unwrap()
            );
        }
    }

    #[test]
    fn replay_rejects_unsigned_or_tool_free_cached_content() {
        let body = br#"{"messages":[]}"#;
        assert!(restore_kimi_thinking_replay_content(
            body,
            br#"[{"type":"thinking","thinking":"x"},{"type":"tool_use","id":"t"}]"#
        )
        .is_none());
        assert!(restore_kimi_thinking_replay_content(
            body,
            br#"[{"type":"thinking","thinking":"x","signature":"sig"}]"#
        )
        .is_none());
    }

    #[test]
    fn replay_preserves_parallel_tools_and_matches_canonical_turns_across_history() {
        let cached = br#"[
            {"type":"thinking","thinking":"signed parallel plan","signature":"parallel-sig"},
            {"type":"text","text":"inspect both"},
            {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"a.rs"}},
            {"type":"tool_use","id":"tool-2","name":"Read","input":{"path":"b.rs"}}
        ]"#;
        let unsigned = serde_json::json!([
            {"type":"text","text":"inspect both"},
            {"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"a.rs"}},
            {"type":"tool_use","id":"tool-2","name":"Read","input":{"path":"b.rs"}}
        ]);
        let body = serde_json::to_vec(&serde_json::json!({
            "messages": [
                {"role":"assistant","content":unsigned.clone()},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"tool-1","content":"a"},
                    {"type":"tool_result","tool_use_id":"tool-2","content":"b"}
                ]},
                {"role":"assistant","content":unsigned},
                {"role":"user","content":"continue"}
            ]
        }))
        .unwrap();

        let restored = restore_kimi_thinking_replay_content(&body, cached).unwrap();
        let restored = serde_json::from_slice::<Value>(&restored).unwrap();
        assert_eq!(
            restored.pointer("/messages/0/content/0/type"),
            Some(&Value::String("text".to_string()))
        );
        assert_eq!(
            restored.pointer("/messages/2/content"),
            Some(&serde_json::from_slice::<Value>(cached).unwrap())
        );

        let mut reordered = serde_json::from_slice::<Value>(&body).unwrap();
        reordered["messages"][2]["content"]
            .as_array_mut()
            .unwrap()
            .swap(1, 2);
        let reordered_content = reordered["messages"][2]["content"].clone();
        let reordered = serde_json::to_vec(&reordered).unwrap();
        assert!(restore_kimi_thinking_replay_content(&reordered, cached).is_some());
        let only_reordered = serde_json::to_vec(&serde_json::json!({
            "messages": [{"role":"assistant","content":reordered_content}]
        }))
        .unwrap();
        assert!(restore_kimi_thinking_replay_content(&only_reordered, cached).is_none());
    }

    #[test]
    fn stream_accumulator_reconstructs_signed_thinking_and_tool_use() {
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"reason\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"Read\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        let split = stream.len() / 3;
        let mut accumulator = KimiThinkingReplayStreamAccumulator::default();
        accumulator.push(&stream.as_bytes()[..split]);
        accumulator.push(&stream.as_bytes()[split..split * 2]);
        accumulator.push(&stream.as_bytes()[split * 2..]);
        let KimiThinkingReplayStreamOutcome::Replayable(content) = accumulator.finish() else {
            panic!("expected replayable stream content");
        };
        let content = serde_json::from_slice::<Value>(&content).unwrap();
        assert_eq!(content[0]["thinking"], Value::String("reason".to_string()));
        assert_eq!(content[0]["signature"], Value::String("sig".to_string()));
        assert_eq!(
            content[1]["input"]["path"],
            Value::String("README.md".to_string())
        );
    }

    #[test]
    fn stream_accumulator_does_not_commit_incomplete_error_or_unknown_delta() {
        for stream in [
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n"
            ),
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"bad_request\"}}\n\n"
            ),
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"future_delta\",\"value\":\"x\"}}\n\n"
            ),
        ] {
            let mut accumulator = KimiThinkingReplayStreamAccumulator::default();
            accumulator.push(stream.as_bytes());
            assert!(matches!(
                accumulator.finish(),
                KimiThinkingReplayStreamOutcome::Incomplete
            ));
        }
    }

    #[tokio::test]
    async fn authoritative_empty_catalog_is_cached_and_scopes_fence_generations() {
        let cache = KimiModelCatalogCache::default();
        let first = KimiModelCatalogScope::derive("claude", "p", 1, "r", "a", 2, 3);
        let changed_auth = KimiModelCatalogScope::derive("claude", "p", 1, "r", "a", 3, 3);
        let changed_token = KimiModelCatalogScope::derive("claude", "p", 1, "r", "a", 2, 4);
        let changed_runtime = KimiModelCatalogScope::derive("claude", "p", 1, "r2", "a", 2, 3);
        cache.insert(first.clone(), Vec::new(), 1_000).await;
        assert_eq!(
            cache.fresh(&first, 1_001).await.unwrap().models,
            Vec::<String>::new()
        );
        assert!(cache.fresh(&changed_auth, 1_001).await.is_none());
        assert!(cache.fresh(&changed_token, 1_001).await.is_none());
        assert!(cache.fresh(&changed_runtime, 1_001).await.is_none());
    }

    #[test]
    fn replay_validation_requires_signed_thinking_and_tool_use() {
        assert!(!valid_replay_content(br#"[{"type":"text","text":"x"}]"#));
        assert!(valid_replay_content(
            br#"[{"type":"thinking","signature":"sig"},{"type":"tool_use","id":"tool"}]"#
        ));
        assert!(!valid_replay_content(
            br#"[{"type":"thinking","thinking":"[reasoning unavailable]","signature":"sig"},{"type":"tool_use","id":"tool"}]"#
        ));
    }

    #[test]
    fn model_parser_accepts_reviewed_shapes_and_preserves_empty_authority() {
        assert_eq!(
            parse_kimi_models(&serde_json::json!({
                "data": [
                    {"id": "kimi-for-coding"},
                    {"id": "future-unreviewed-model"},
                    {"name": "k3"},
                    {"id": "kimi-for-coding"}
                ]
            }))
            .unwrap(),
            FetchedKimiModelCatalog {
                models: vec!["k3".to_string(), "kimi-for-coding".to_string()],
                upstream_non_empty: true,
                _memory: None,
            }
        );
        assert_eq!(
            parse_kimi_models(&serde_json::json!({"models": []})).unwrap(),
            FetchedKimiModelCatalog {
                models: Vec::new(),
                upstream_non_empty: false,
                _memory: None,
            }
        );
        assert!(parse_kimi_models(&serde_json::json!({
            "data": [{"id": "future-unreviewed-model"}]
        }))
        .unwrap_err()
        .contains("contract may have drifted"));
        assert!(parse_kimi_models(&serde_json::json!({"data": {}})).is_err());
    }

    #[tokio::test]
    async fn model_fetch_uses_bound_identity_and_classifies_statuses() {
        use axum::http::HeaderMap;
        use axum::routing::get;
        use axum::Router;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer bound-token")
                    );
                    assert_eq!(
                        headers
                            .get("x-msh-device-id")
                            .and_then(|value| value.to_str().ok()),
                        Some("bound-device")
                    );
                    axum::Json(serde_json::json!({
                        "data": [{"id": "kimi-for-coding-highspeed"}]
                    }))
                }),
            )
            .route(
                "/limited",
                get(|| async { (axum::http::StatusCode::TOO_MANY_REQUESTS, "limited") }),
            )
            .route(
                "/timeout",
                get(|| async { (axum::http::StatusCode::REQUEST_TIMEOUT, "timeout") }),
            )
            .route(
                "/denied",
                get(|| async { (axum::http::StatusCode::FORBIDDEN, "denied") }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let identity = KimiDeviceIdentity {
            device_id: "bound-device".to_string(),
            device_name: "device".to_string(),
            device_model: "model".to_string(),
            os_version: "os".to_string(),
        };
        let fetched = fetch_kimi_models(
            &reqwest::Client::new(),
            &format!("http://{address}/models"),
            "bound-token",
            &identity,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(fetched.models, ["kimi-for-coding-highspeed"]);

        let limited = fetch_kimi_models(
            &reqwest::Client::new(),
            &format!("http://{address}/limited"),
            "bound-token",
            &identity,
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(limited.retryable());
        let timeout = fetch_kimi_models(
            &reqwest::Client::new(),
            &format!("http://{address}/timeout"),
            "bound-token",
            &identity,
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(timeout.retryable());
        let denied = fetch_kimi_models(
            &reqwest::Client::new(),
            &format!("http://{address}/denied"),
            "bound-token",
            &identity,
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(!denied.retryable());
        server.abort();
    }
}

#[cfg(test)]
mod r5_resident_regression_tests {
    use super::*;
    include!("kimi_runtime/r5_resident_tests.rs");
}
