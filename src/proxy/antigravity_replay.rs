use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

const MAX_ENTRIES: usize = 2_048;
const MAX_TURNS_PER_ENTRY: usize = 256;
const MAX_ITEMS_PER_ENTRY: usize = 4_096;
const MAX_BYTES_PER_ENTRY: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BUFFER_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const REPLAY_TTL_MS: i64 = 60 * 60 * 1_000;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct AntigravityReplayScope(String);

impl fmt::Debug for AntigravityReplayScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AntigravityReplayScope(<redacted>)")
    }
}

impl AntigravityReplayScope {
    pub(crate) fn ownership_digest(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn derive(
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
        upstream_plane: &str,
    ) -> Option<Self> {
        let values = [
            app,
            provider_id,
            runtime_fingerprint,
            account_id,
            share_id,
            user_namespace,
            session_id,
            model_family,
            upstream_plane,
        ];
        if values.iter().any(|value| value.trim().is_empty()) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-server:antigravity-reasoning-replay:v1\0");
        for value in [
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
            upstream_plane,
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        Some(Self(format!(
            "antigravity-replay-v1:{}",
            hex::encode(hasher.finalize())
        )))
    }
}

pub(crate) fn user_namespace(user_identity: &str) -> Option<String> {
    let normalized = user_identity.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let digest = Sha256::digest(normalized.as_bytes());
    Some(format!("principal_{}", hex::encode(&digest[..16])))
}

pub(crate) fn model_family(model: &str) -> Option<String> {
    let mut normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 160 || normalized.contains("claude") {
        return None;
    }
    if !(normalized.contains("gemini")
        || normalized.contains("flash")
        || normalized.contains("agent"))
    {
        return None;
    }
    if let Some((base, _)) = normalized.split_once('(') {
        normalized = base.trim_end().to_string();
    }
    for suffix in ["-minimal", "-low", "-medium", "-high", "-max"] {
        if let Some(base) = normalized.strip_suffix(suffix) {
            normalized = base.to_string();
            break;
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AntigravityReplayChain {
    turns: Vec<ReplayTurn>,
}

impl fmt::Debug for AntigravityReplayChain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AntigravityReplayChain")
            .field("turns", &self.turns.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ReplayTurn {
    base_history_hash: String,
    items: Vec<ReplayItem>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReplayItem {
    FunctionCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        args: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
        occurrence: usize,
    },
    SignedPart {
        target_kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_hash: Option<String>,
        thought_signature: String,
        occurrence: usize,
    },
}

impl ReplayItem {
    fn signature(&self) -> Option<&str> {
        match self {
            Self::FunctionCall {
                thought_signature, ..
            } => thought_signature.as_deref(),
            Self::SignedPart {
                thought_signature, ..
            } => Some(thought_signature),
        }
    }
}

#[derive(Clone)]
struct ReplayEntry {
    chain: Option<AntigravityReplayChain>,
    recorded_at_ms: i64,
    expires_at_ms: i64,
    generation: u64,
    bytes: usize,
}

impl fmt::Debug for ReplayEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayEntry")
            .field("present", &self.chain.is_some())
            .field("recorded_at_ms", &self.recorded_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("generation", &self.generation)
            .field("bytes", &self.bytes)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AntigravityReplaySnapshot {
    generation: u64,
}

impl AntigravityReplaySnapshot {
    pub(crate) fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Default)]
struct ReplayStore {
    entries: HashMap<AntigravityReplayScope, ReplayEntry>,
    next_generation: u64,
    total_bytes: usize,
}

pub(crate) struct AntigravityReplayCache {
    store: Mutex<ReplayStore>,
}

impl fmt::Debug for AntigravityReplayCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AntigravityReplayCache(<opaque>)")
    }
}

impl Default for AntigravityReplayCache {
    fn default() -> Self {
        Self {
            store: Mutex::new(ReplayStore::default()),
        }
    }
}

impl AntigravityReplayCache {
    pub(crate) async fn get(
        &self,
        scope: &AntigravityReplayScope,
        now_ms: i64,
    ) -> (Option<AntigravityReplayChain>, AntigravityReplaySnapshot) {
        let mut store = self.store.lock().await;
        prune_expired(&mut store, now_ms);
        if !store.entries.contains_key(scope) {
            reserve_tombstone(&mut store, scope.clone(), now_ms);
        }
        let entry = store
            .entries
            .get_mut(scope)
            .expect("replay miss must be fenced by a tombstone");
        entry.recorded_at_ms = now_ms;
        entry.expires_at_ms = now_ms.saturating_add(REPLAY_TTL_MS);
        (
            entry.chain.clone(),
            AntigravityReplaySnapshot {
                generation: entry.generation,
            },
        )
    }

    pub(crate) async fn replace_if_unchanged(
        &self,
        scope: AntigravityReplayScope,
        snapshot: AntigravityReplaySnapshot,
        chain: AntigravityReplayChain,
        now_ms: i64,
    ) -> bool {
        let Some(bytes) = valid_chain_bytes(&chain) else {
            return false;
        };
        let mut store = self.store.lock().await;
        prune_expired(&mut store, now_ms);
        if store.entries.get(&scope).map(|entry| entry.generation) != Some(snapshot.generation) {
            return false;
        }
        remove_entry(&mut store, &scope);
        let generation = next_generation(&mut store);
        store.total_bytes = store.total_bytes.saturating_add(bytes);
        store.entries.insert(
            scope,
            ReplayEntry {
                chain: Some(chain),
                recorded_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(REPLAY_TTL_MS),
                generation,
                bytes,
            },
        );
        enforce_limits(&mut store);
        true
    }

    pub(crate) async fn delete_if_unchanged(
        &self,
        scope: &AntigravityReplayScope,
        snapshot: AntigravityReplaySnapshot,
        now_ms: i64,
    ) -> bool {
        let mut store = self.store.lock().await;
        if store.entries.get(scope).map(|entry| entry.generation) != Some(snapshot.generation) {
            return false;
        }
        remove_entry(&mut store, scope);
        reserve_tombstone(&mut store, scope.clone(), now_ms);
        true
    }
}

fn next_generation(store: &mut ReplayStore) -> u64 {
    store.next_generation = store.next_generation.saturating_add(1).max(1);
    store.next_generation
}

fn reserve_tombstone(store: &mut ReplayStore, scope: AntigravityReplayScope, now_ms: i64) {
    let generation = next_generation(store);
    store.entries.insert(
        scope,
        ReplayEntry {
            chain: None,
            recorded_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(REPLAY_TTL_MS),
            generation,
            bytes: 0,
        },
    );
    enforce_limits(store);
}

fn remove_entry(store: &mut ReplayStore, scope: &AntigravityReplayScope) {
    if let Some(entry) = store.entries.remove(scope) {
        store.total_bytes = store.total_bytes.saturating_sub(entry.bytes);
    }
}

fn prune_expired(store: &mut ReplayStore, now_ms: i64) {
    let expired = store
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at_ms <= now_ms)
        .map(|(scope, _)| scope.clone())
        .collect::<Vec<_>>();
    for scope in expired {
        remove_entry(store, &scope);
    }
}

fn enforce_limits(store: &mut ReplayStore) {
    while store.entries.len() > MAX_ENTRIES || store.total_bytes > MAX_TOTAL_BYTES {
        let Some(oldest) = store
            .entries
            .iter()
            .min_by_key(|(_, entry)| (entry.recorded_at_ms, entry.generation))
            .map(|(scope, _)| scope.clone())
        else {
            break;
        };
        remove_entry(store, &oldest);
    }
}

fn valid_chain_bytes(chain: &AntigravityReplayChain) -> Option<usize> {
    if chain.turns.is_empty() || chain.turns.len() > MAX_TURNS_PER_ENTRY {
        return None;
    }
    let item_count = chain
        .turns
        .iter()
        .map(|turn| turn.items.len())
        .sum::<usize>();
    if item_count == 0 || item_count > MAX_ITEMS_PER_ENTRY {
        return None;
    }
    let bytes = serde_json::to_vec(chain).ok()?.len();
    (bytes <= MAX_BYTES_PER_ENTRY).then_some(bytes)
}

pub(crate) struct ReplayApplyResult {
    pub(crate) body: Bytes,
    pub(crate) applied: bool,
    pub(crate) context_mismatch: bool,
}

pub(crate) fn apply_replay(body: &[u8], chain: &AntigravityReplayChain) -> ReplayApplyResult {
    let Ok(mut document) = serde_json::from_slice::<Value>(body) else {
        return unchanged(body, false);
    };
    let Some(contents) = request_contents_mut(&mut document) else {
        return unchanged(body, false);
    };
    let mut applied = false;
    let mut context_mismatch = false;
    for turn in &chain.turns {
        let mut matched_context = false;
        let mut turn_applied = false;
        for index in 0..contents.len() {
            if history_hash(&contents[..index]) != turn.base_history_hash {
                continue;
            }
            matched_context = true;
            if contents[index].get("role").and_then(Value::as_str) == Some("model") {
                if let Some(patched) = patch_model_content(&contents[index], &turn.items) {
                    if patched != contents[index] {
                        contents[index] = patched;
                        turn_applied = true;
                    }
                    break;
                }
            }
            if content_has_function_responses(&contents[index])
                && response_matches_calls(&contents[index], &turn.items)
            {
                let model = minimal_model_content(&turn.items);
                if model
                    .get("parts")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| !parts.is_empty())
                {
                    patch_function_response_ids(&mut contents[index], &turn.items);
                    contents.insert(index, model);
                    turn_applied = true;
                }
                break;
            }
        }
        if !matched_context {
            context_mismatch = true;
        }
        applied |= turn_applied;
    }
    if !applied {
        return unchanged(body, context_mismatch);
    }
    match serde_json::to_vec(&document) {
        Ok(encoded) => ReplayApplyResult {
            body: Bytes::from(encoded),
            applied: true,
            context_mismatch,
        },
        Err(_) => unchanged(body, context_mismatch),
    }
}

fn unchanged(body: &[u8], context_mismatch: bool) -> ReplayApplyResult {
    ReplayApplyResult {
        body: Bytes::copy_from_slice(body),
        applied: false,
        context_mismatch,
    }
}

fn request_contents_mut(document: &mut Value) -> Option<&mut Vec<Value>> {
    if document.get("request").is_some() {
        document.pointer_mut("/request/contents")?.as_array_mut()
    } else {
        document.get_mut("contents")?.as_array_mut()
    }
}

fn request_contents(document: &Value) -> Option<&[Value]> {
    if document.get("request").is_some() {
        document.pointer("/request/contents")?.as_array()
    } else {
        document.get("contents")?.as_array()
    }
    .map(Vec::as_slice)
}

fn history_hash(contents: &[Value]) -> String {
    let canonical = Value::Array(contents.iter().map(canonical_history_value).collect());
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-server:antigravity-history:v1\0");
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn canonical_history_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sorted = BTreeMap::new();
            for (key, child) in object {
                if matches!(
                    key.as_str(),
                    "thoughtSignature" | "thought_signature" | "cache_control"
                ) {
                    continue;
                }
                sorted.insert(key.clone(), canonical_history_value(child));
            }
            Value::Object(Map::from_iter(sorted))
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical_history_value).collect()),
        _ => value.clone(),
    }
}

fn native_signature(part: &Value) -> Option<&str> {
    part.get("thoughtSignature")
        .or_else(|| part.get("thought_signature"))
        .or_else(|| part.pointer("/extra_content/google/thought_signature"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|signature| signature.len() >= 16 && signature.len() <= MAX_SIGNATURE_BYTES)
}

fn part_target(part: &Value) -> Option<(String, String)> {
    let text = part.get("text").and_then(Value::as_str)?;
    if text.is_empty() {
        return None;
    }
    let kind = if part.get("thought").and_then(Value::as_bool) == Some(true) {
        "thought"
    } else {
        "text"
    };
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-server:antigravity-part:v1\0");
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    Some((kind.to_string(), hex::encode(hasher.finalize())))
}

fn capture_turn(request_body: &[u8], content: &Value) -> Option<ReplayTurn> {
    if content.get("role").and_then(Value::as_str) != Some("model") {
        return None;
    }
    let request = serde_json::from_slice::<Value>(request_body).ok()?;
    let contents = request_contents(&request)?;
    let parts = content.get("parts")?.as_array()?;
    let mut items = Vec::new();
    let mut function_occurrences = HashMap::<String, usize>::new();
    let mut target_occurrences = HashMap::<(String, String), usize>::new();
    let mut seen_ids = HashSet::<String>::new();
    let mut saw_signature = false;
    for part in parts {
        let signature = native_signature(part).map(str::to_string);
        saw_signature |= signature.is_some();
        if let Some(call) = part.get("functionCall").and_then(Value::as_object) {
            let name = call.get("name")?.as_str()?.trim();
            if name.is_empty() || name.len() > 256 {
                return None;
            }
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string);
            if id.as_ref().is_some_and(|id| !seen_ids.insert(id.clone())) {
                return None;
            }
            let args = canonical_history_value(call.get("args").unwrap_or(&json!({})));
            let key = format!("{name}\0{}", serde_json::to_string(&args).ok()?);
            let occurrence = function_occurrences.entry(key).or_default();
            items.push(ReplayItem::FunctionCall {
                id,
                name: name.to_string(),
                args,
                thought_signature: signature,
                occurrence: *occurrence,
            });
            *occurrence = occurrence.saturating_add(1);
            continue;
        }
        let Some(signature) = signature else {
            continue;
        };
        if part
            .get("text")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            if let Some(ReplayItem::FunctionCall {
                thought_signature, ..
            }) = items.last_mut()
            {
                if thought_signature.is_none() {
                    *thought_signature = Some(signature);
                    continue;
                }
            }
        }
        let target = part_target(part);
        let (target_kind, target_hash) = target
            .map(|(kind, hash)| (kind, Some(hash)))
            .unwrap_or_else(|| ("carrier".to_string(), None));
        let occurrence = target_hash
            .as_ref()
            .map(|hash| {
                let entry = target_occurrences
                    .entry((target_kind.clone(), hash.clone()))
                    .or_default();
                let current = *entry;
                *entry = entry.saturating_add(1);
                current
            })
            .unwrap_or_default();
        items.push(ReplayItem::SignedPart {
            target_kind,
            target_hash,
            thought_signature: signature,
            occurrence,
        });
    }
    if !saw_signature || items.is_empty() {
        return None;
    }
    Some(ReplayTurn {
        base_history_hash: history_hash(contents),
        items,
    })
}

pub(crate) fn capture_response(
    request_body: &[u8],
    response_body: &[u8],
    previous: Option<&AntigravityReplayChain>,
) -> Option<AntigravityReplayChain> {
    let response = serde_json::from_slice::<Value>(response_body).ok()?;
    let response = response.get("response").unwrap_or(&response);
    let candidate = response.get("candidates")?.as_array()?.first()?;
    if candidate
        .get("finishReason")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return None;
    }
    append_turn(request_body, candidate.get("content")?, previous)
}

pub(crate) fn response_has_terminal(response_body: &[u8]) -> bool {
    let Ok(response) = serde_json::from_slice::<Value>(response_body) else {
        return false;
    };
    let response = response.get("response").unwrap_or(&response);
    response
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|candidates| {
            candidates.iter().any(|candidate| {
                candidate
                    .get("finishReason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.trim().is_empty())
            })
        })
}

pub(crate) fn is_signature_rejection(status: u16, body: &[u8]) -> bool {
    if !matches!(status, 400 | 422) {
        return false;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    ["thoughtsignature", "thought_signature", "thought signature"]
        .iter()
        .any(|marker| text.contains(marker))
        && ["invalid", "missing", "required", "rejected", "signature"]
            .iter()
            .any(|marker| text.contains(marker))
}

fn append_turn(
    request_body: &[u8],
    content: &Value,
    previous: Option<&AntigravityReplayChain>,
) -> Option<AntigravityReplayChain> {
    let turn = capture_turn(request_body, content)?;
    let mut turns = previous
        .map(|chain| chain.turns.clone())
        .unwrap_or_default();
    turns.retain(|existing| existing.base_history_hash != turn.base_history_hash);
    turns.push(turn);
    let chain = AntigravityReplayChain { turns };
    valid_chain_bytes(&chain).map(|_| chain)
}

fn function_call_items(items: &[ReplayItem]) -> Vec<&ReplayItem> {
    items
        .iter()
        .filter(|item| matches!(item, ReplayItem::FunctionCall { .. }))
        .collect()
}

fn patch_model_content(content: &Value, items: &[ReplayItem]) -> Option<Value> {
    let mut patched = content.clone();
    let parts = patched.get_mut("parts")?.as_array_mut()?;
    let mut matched_functions = 0usize;
    for item in items {
        match item {
            ReplayItem::FunctionCall {
                id,
                name,
                args,
                thought_signature,
                occurrence,
            } => {
                let mut seen = 0usize;
                let mut matched = None;
                for (index, part) in parts.iter().enumerate() {
                    let Some(call) = part.get("functionCall") else {
                        continue;
                    };
                    let call_id = call.get("id").and_then(Value::as_str).map(str::trim);
                    if id.as_deref().is_some_and(|id| call_id == Some(id)) {
                        matched = Some(index);
                        break;
                    }
                    if call.get("name").and_then(Value::as_str) == Some(name)
                        && canonical_history_value(call.get("args").unwrap_or(&json!({}))) == *args
                    {
                        if seen == *occurrence {
                            matched = Some(index);
                            break;
                        }
                        seen = seen.saturating_add(1);
                    }
                }
                let index = matched?;
                let part = &mut parts[index];
                if let Some(existing) = native_signature(part) {
                    if thought_signature
                        .as_deref()
                        .is_some_and(|cached| cached != existing)
                    {
                        return None;
                    }
                } else if let Some(signature) = thought_signature {
                    part["thoughtSignature"] = Value::String(signature.clone());
                }
                if let Some(id) = id {
                    let call = part.get_mut("functionCall")?.as_object_mut()?;
                    match call.get("id").and_then(Value::as_str) {
                        Some(existing) if existing != id => return None,
                        None => {
                            call.insert("id".to_string(), Value::String(id.clone()));
                        }
                        _ => {}
                    }
                }
                matched_functions = matched_functions.saturating_add(1);
            }
            ReplayItem::SignedPart {
                target_kind,
                target_hash: Some(target_hash),
                thought_signature,
                occurrence,
            } => {
                let mut seen = 0usize;
                for part in parts.iter_mut() {
                    let Some((kind, hash)) = part_target(part) else {
                        continue;
                    };
                    if &kind != target_kind || &hash != target_hash {
                        continue;
                    }
                    if seen != *occurrence {
                        seen = seen.saturating_add(1);
                        continue;
                    }
                    match native_signature(part) {
                        Some(existing) if existing != thought_signature => return None,
                        None => {
                            part["thoughtSignature"] = Value::String(thought_signature.clone());
                        }
                        _ => {}
                    }
                    break;
                }
            }
            ReplayItem::SignedPart {
                target_hash: None, ..
            } => {}
        }
    }
    let required_functions = function_call_items(items).len();
    (required_functions == 0 || matched_functions == required_functions).then_some(patched)
}

fn content_has_function_responses(content: &Value) -> bool {
    content
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part.get("functionResponse").is_some())
        })
}

fn response_matches_calls(content: &Value, items: &[ReplayItem]) -> bool {
    let calls = function_call_items(items);
    if calls.is_empty() {
        return false;
    }
    let responses = content
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("functionResponse"))
        .collect::<Vec<_>>();
    if responses.is_empty() {
        return false;
    }
    let mut used = HashSet::new();
    for response in responses {
        let response_id = response.get("id").and_then(Value::as_str).map(str::trim);
        let response_name = response.get("name").and_then(Value::as_str).map(str::trim);
        let candidates = calls
            .iter()
            .enumerate()
            .filter(|(index, _)| !used.contains(index))
            .filter(|(_, item)| match item {
                ReplayItem::FunctionCall { id, name, .. } => response_id
                    .filter(|id| !id.is_empty())
                    .zip(id.as_deref())
                    .map(|(left, right)| left == right)
                    .unwrap_or_else(|| response_name == Some(name.as_str())),
                _ => false,
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return false;
        }
        used.insert(candidates[0]);
    }
    used.len() == calls.len()
}

fn patch_function_response_ids(content: &mut Value, items: &[ReplayItem]) {
    let calls = function_call_items(items);
    let Some(parts) = content.get_mut("parts").and_then(Value::as_array_mut) else {
        return;
    };
    let mut used = HashSet::new();
    for part in parts {
        let Some(response) = part
            .get_mut("functionResponse")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if response
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        {
            continue;
        }
        let name = response.get("name").and_then(Value::as_str);
        let candidates = calls
            .iter()
            .enumerate()
            .filter(|(index, _)| !used.contains(index))
            .filter_map(|(index, item)| match item {
                ReplayItem::FunctionCall {
                    id: Some(id),
                    name: call_name,
                    ..
                } if name == Some(call_name.as_str()) => Some((index, id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if let [(index, id)] = candidates.as_slice() {
            response.insert("id".to_string(), Value::String((*id).clone()));
            used.insert(*index);
        }
    }
}

fn minimal_model_content(items: &[ReplayItem]) -> Value {
    let parts = items
        .iter()
        .map(|item| match item {
            ReplayItem::FunctionCall {
                id,
                name,
                args,
                thought_signature,
                ..
            } => {
                let mut call = Map::from_iter([
                    ("name".to_string(), Value::String(name.clone())),
                    ("args".to_string(), args.clone()),
                ]);
                if let Some(id) = id {
                    call.insert("id".to_string(), Value::String(id.clone()));
                }
                let mut part = json!({"functionCall": Value::Object(call)});
                if let Some(signature) = thought_signature {
                    part["thoughtSignature"] = Value::String(signature.clone());
                }
                part
            }
            ReplayItem::SignedPart {
                target_kind,
                thought_signature,
                ..
            } => json!({
                "text": "",
                "thought": target_kind == "thought",
                "thoughtSignature": thought_signature,
            }),
        })
        .collect::<Vec<_>>();
    json!({"role": "model", "parts": parts})
}

pub(crate) fn derive_session_id(
    account_id: &str,
    conversation_scope: &str,
    generation: u64,
) -> Option<String> {
    let account_id = account_id.trim();
    let conversation_scope = conversation_scope.trim();
    if account_id.is_empty() || conversation_scope.is_empty() {
        return None;
    }
    let source = format!("{account_id}|{conversation_scope}|{generation}");
    let mut hash = -3_750_763_034_362_895_579_i64;
    for byte in source.bytes() {
        hash = hash.wrapping_mul(1_099_511_628_211_i64);
        hash ^= i64::from(byte);
    }
    Some(hash.to_string())
}

pub(crate) fn apply_session_id(body: &mut Bytes, session_id: &str) -> bool {
    let Ok(mut document) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(request) = document.get_mut("request").and_then(Value::as_object_mut) else {
        return false;
    };
    request.insert(
        "sessionId".to_string(),
        Value::String(session_id.to_string()),
    );
    let Ok(encoded) = serde_json::to_vec(&document) else {
        return false;
    };
    *body = Bytes::from(encoded);
    true
}

pub(crate) fn is_session_accumulation_error(status: u16, body: &[u8]) -> bool {
    if status != 400 {
        return false;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    (text.contains("input token count exceeds")
        || text.contains("input tokens exceed")
        || text.contains("maximum number of tokens allowed"))
        && (text.contains("1048576") || text.contains("1,048,576"))
}

#[derive(Default)]
pub(crate) struct AntigravityReplayStreamAccumulator {
    buffer: Vec<u8>,
    content: Value,
    terminal: bool,
    overflow: bool,
}

impl fmt::Debug for AntigravityReplayStreamAccumulator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AntigravityReplayStreamAccumulator")
            .field("buffered_bytes", &self.buffer.len())
            .field("terminal", &self.terminal)
            .field("overflow", &self.overflow)
            .finish()
    }
}

impl AntigravityReplayStreamAccumulator {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.overflow || chunk.is_empty() {
            return;
        }
        if self.buffer.len().saturating_add(chunk.len()) > MAX_STREAM_BUFFER_BYTES {
            self.overflow = true;
            self.buffer.clear();
            return;
        }
        self.buffer.extend_from_slice(chunk);
        while let Some((end, delimiter)) = next_sse_event(&self.buffer) {
            let event = self.buffer.drain(..end).collect::<Vec<_>>();
            self.buffer.drain(..delimiter);
            self.observe_event(&event);
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.terminal && !self.overflow
    }

    pub(crate) fn finish(
        mut self,
        request_body: &[u8],
        previous: Option<&AntigravityReplayChain>,
    ) -> Option<AntigravityReplayChain> {
        if !self.buffer.is_empty() {
            let remaining = std::mem::take(&mut self.buffer);
            self.observe_event(&remaining);
        }
        if !self.terminal || self.overflow {
            return None;
        }
        append_turn(request_body, &self.content, previous)
    }

    fn observe_event(&mut self, event: &[u8]) {
        let mut data = Vec::new();
        for line in event.split(|byte| *byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(payload) = line.strip_prefix(b"data:") {
                if !data.is_empty() {
                    data.push(b'\n');
                }
                data.extend_from_slice(trim_ascii(payload));
            } else if data.is_empty() && line.first() == Some(&b'{') {
                data.extend_from_slice(line);
            }
        }
        if data.is_empty() || data == b"[DONE]" {
            return;
        }
        let Ok(document) = serde_json::from_slice::<Value>(&data) else {
            return;
        };
        let response = document.get("response").unwrap_or(&document);
        let Some(candidate) = response
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return;
        };
        if candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.trim().is_empty())
        {
            self.terminal = true;
        }
        let Some(content) = candidate.get("content") else {
            return;
        };
        merge_stream_content(&mut self.content, content);
    }
}

fn next_sse_event(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn merge_stream_content(target: &mut Value, source: &Value) {
    if !target.is_object() {
        *target = json!({"role": "model", "parts": []});
    }
    if let Some(role) = source.get("role").and_then(Value::as_str) {
        target["role"] = Value::String(role.to_string());
    }
    let Some(source_parts) = source.get("parts").and_then(Value::as_array) else {
        return;
    };
    let Some(target_parts) = target.get_mut("parts").and_then(Value::as_array_mut) else {
        return;
    };
    for part in source_parts {
        let merge_text = part.get("functionCall").is_none()
            && part.get("text").and_then(Value::as_str).is_some()
            && target_parts.last().is_some_and(|last| {
                last.get("functionCall").is_none()
                    && last.get("text").and_then(Value::as_str).is_some()
                    && last.get("thought").and_then(Value::as_bool)
                        == part.get("thought").and_then(Value::as_bool)
            });
        if merge_text {
            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
            let last = target_parts.last_mut().expect("last part exists");
            let combined = format!(
                "{}{}",
                last.get("text").and_then(Value::as_str).unwrap_or_default(),
                text
            );
            last["text"] = Value::String(combined);
            if let Some(signature) = native_signature(part) {
                last["thoughtSignature"] = Value::String(signature.to_string());
            }
        } else {
            target_parts.push(part.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIG_A: &str = "opaque-thought-signature-a";
    const SIG_B: &str = "opaque-thought-signature-b";

    fn scope(suffix: &str) -> AntigravityReplayScope {
        AntigravityReplayScope::derive(
            "claude",
            "provider",
            7,
            "runtime",
            "account",
            2,
            3,
            "share",
            "principal",
            suffix,
            "gemini-3-flash",
            "cloudcode-pa.googleapis.com",
        )
        .unwrap()
    }

    fn request(contents: Value) -> Bytes {
        Bytes::from(serde_json::to_vec(&json!({"request": {"contents": contents}})).unwrap())
    }

    fn response(content: Value) -> Bytes {
        Bytes::from(
            serde_json::to_vec(&json!({
                "response": {"candidates": [{"content": content, "finishReason": "STOP"}]}
            }))
            .unwrap(),
        )
    }

    #[test]
    fn scope_isolates_every_identity_dimension_and_redacts_debug() {
        let baseline = scope("session");
        assert_ne!(baseline, scope("other-session"));
        let derive = |app: &str,
                      provider: &str,
                      revision: u64,
                      runtime: &str,
                      account: &str,
                      auth_generation: u64,
                      token_generation: u64,
                      share: &str,
                      principal: &str,
                      model: &str,
                      plane: &str| {
            AntigravityReplayScope::derive(
                app,
                provider,
                revision,
                runtime,
                account,
                auth_generation,
                token_generation,
                share,
                principal,
                "session",
                model,
                plane,
            )
            .unwrap()
        };
        for changed in [
            derive(
                "gemini",
                "provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "other-provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                8,
                "runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "other-runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "other-account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                3,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                2,
                4,
                "share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "other-share",
                "principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "share",
                "other-principal",
                "gemini-3-flash",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-pro",
                "cloudcode-pa.googleapis.com",
            ),
            derive(
                "claude",
                "provider",
                7,
                "runtime",
                "account",
                2,
                3,
                "share",
                "principal",
                "gemini-3-flash",
                "daily-cloudcode-pa.googleapis.com",
            ),
        ] {
            assert_ne!(baseline, changed);
        }
        assert_eq!(
            format!("{baseline:?}"),
            "AntigravityReplayScope(<redacted>)"
        );
        assert_eq!(
            user_namespace(" User@Example.COM "),
            user_namespace("user@example.com")
        );
    }

    #[tokio::test]
    async fn miss_tombstone_and_snapshot_cas_fence_concurrent_writers() {
        let cache = AntigravityReplayCache::default();
        let scope = scope("cas");
        let (missing, first) = cache.get(&scope, 10).await;
        let (_, second) = cache.get(&scope, 11).await;
        assert!(missing.is_none());
        assert_eq!(first, second);
        let base = request(json!([{"role":"user","parts":[{"text":"hi"}]}]));
        let output = response(json!({
            "role":"model",
            "parts":[{"functionCall":{"id":"call-1","name":"lookup","args":{}},"thoughtSignature":SIG_A}]
        }));
        let chain = capture_response(&base, &output, None).unwrap();
        assert!(
            cache
                .replace_if_unchanged(scope.clone(), first, chain.clone(), 12)
                .await
        );
        assert!(!cache.replace_if_unchanged(scope, second, chain, 13).await);
    }

    #[test]
    fn inserts_parallel_signed_calls_before_tool_results() {
        let base_contents = json!([{"role":"user","parts":[{"text":"look up both"}]}]);
        let base = request(base_contents.clone());
        let output = response(json!({
            "role":"model",
            "parts":[
                {"functionCall":{"id":"call-a","name":"first","args":{"q":1}},"thoughtSignature":SIG_A},
                {"functionCall":{"id":"call-b","name":"second","args":{"q":2}},"thoughtSignature":SIG_B}
            ]
        }));
        let chain = capture_response(&base, &output, None).unwrap();
        let next = request(json!([
            {"role":"user","parts":[{"text":"look up both"}]},
            {"role":"user","parts":[
                {"functionResponse":{"id":"call-a","name":"first","response":{"ok":1}}},
                {"functionResponse":{"id":"call-b","name":"second","response":{"ok":2}}}
            ]}
        ]));
        let applied = apply_replay(&next, &chain);
        assert!(applied.applied);
        let body: Value = serde_json::from_slice(&applied.body).unwrap();
        assert_eq!(
            body.pointer("/request/contents/1/parts/0/thoughtSignature"),
            Some(&Value::String(SIG_A.to_string()))
        );
        assert_eq!(
            body.pointer("/request/contents/2/role"),
            Some(&json!("user"))
        );

        let out_of_order = request(json!([
            {"role":"user","parts":[{"text":"look up both"}]},
            {"role":"user","parts":[
                {"functionResponse":{"id":"call-b","name":"second","response":{"ok":2}}},
                {"functionResponse":{"id":"call-a","name":"first","response":{"ok":1}}}
            ]}
        ]));
        assert!(apply_replay(&out_of_order, &chain).applied);

        let missing_parallel_result = request(json!([
            {"role":"user","parts":[{"text":"look up both"}]},
            {"role":"user","parts":[
                {"functionResponse":{"id":"call-a","name":"first","response":{"ok":1}}}
            ]}
        ]));
        assert!(!apply_replay(&missing_parallel_result, &chain).applied);
    }

    #[test]
    fn patches_existing_unsigned_call_and_rejects_edited_history() {
        let base = request(json!([{"role":"user","parts":[{"text":"original"}]}]));
        let output = response(json!({
            "role":"model",
            "parts":[{"functionCall":{"name":"lookup","args":{"q":1}},"thoughtSignature":SIG_A}]
        }));
        let chain = capture_response(&base, &output, None).unwrap();
        let next = request(json!([
            {"role":"user","parts":[{"text":"original"}]},
            {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"q":1}}}]},
            {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"ok":true}}}]}
        ]));
        let patched = apply_replay(&next, &chain);
        assert!(patched.applied);
        assert_eq!(
            serde_json::from_slice::<Value>(&patched.body)
                .unwrap()
                .pointer("/request/contents/1/parts/0/thoughtSignature"),
            Some(&json!(SIG_A))
        );

        let edited = request(json!([
            {"role":"user","parts":[{"text":"edited"}]},
            {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"ok":true}}}]}
        ]));
        let rejected = apply_replay(&edited, &chain);
        assert!(!rejected.applied);
        assert!(rejected.context_mismatch);
    }

    #[test]
    fn duplicate_explicit_call_id_is_not_cached() {
        let base = request(json!([{"role":"user","parts":[{"text":"x"}]}]));
        let output = response(json!({
            "role":"model",
            "parts":[
                {"functionCall":{"id":"same","name":"one","args":{}},"thoughtSignature":SIG_A},
                {"functionCall":{"id":"same","name":"two","args":{}},"thoughtSignature":SIG_B}
            ]
        }));
        assert!(capture_response(&base, &output, None).is_none());
    }

    #[test]
    fn single_call_without_id_replays_but_ambiguous_calls_fail_closed() {
        let base = request(json!([{"role":"user","parts":[{"text":"x"}]}]));
        let output = response(json!({
            "role":"model",
            "parts":[{"functionCall":{"name":"lookup","args":{"q":1}},"thoughtSignature":SIG_A}]
        }));
        let chain = capture_response(&base, &output, None).unwrap();
        let continuation = request(json!([
            {"role":"user","parts":[{"text":"x"}]},
            {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"ok":true}}}]}
        ]));
        assert!(apply_replay(&continuation, &chain).applied);

        let ambiguous_output = response(json!({
            "role":"model",
            "parts":[
                {"functionCall":{"name":"lookup","args":{"q":1}},"thoughtSignature":SIG_A},
                {"functionCall":{"name":"lookup","args":{"q":2}},"thoughtSignature":SIG_B}
            ]
        }));
        let ambiguous = capture_response(&base, &ambiguous_output, None).unwrap();
        assert!(!apply_replay(&continuation, &ambiguous).applied);
    }

    #[test]
    fn multi_turn_chain_replays_each_signature_against_semantic_history() {
        let first_request = request(json!([{"role":"user","parts":[{"text":"start"}]}]));
        let first_response = response(json!({
            "role":"model",
            "parts":[{"functionCall":{"id":"first","name":"one","args":{}},"thoughtSignature":SIG_A}]
        }));
        let first_chain = capture_response(&first_request, &first_response, None).unwrap();
        let second_downstream = request(json!([
            {"role":"user","parts":[{"text":"start"}]},
            {"role":"model","parts":[{"functionCall":{"id":"first","name":"one","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"first","name":"one","response":{"ok":1}}}]},
            {"role":"user","parts":[{"text":"continue"}]}
        ]));
        let second_prepared = apply_replay(&second_downstream, &first_chain);
        assert!(second_prepared.applied);
        let second_response = response(json!({
            "role":"model",
            "parts":[{"functionCall":{"id":"second","name":"two","args":{}},"thoughtSignature":SIG_B}]
        }));
        let full_chain =
            capture_response(&second_prepared.body, &second_response, Some(&first_chain)).unwrap();
        let third = request(json!([
            {"role":"user","parts":[{"text":"start"}]},
            {"role":"model","parts":[{"functionCall":{"id":"first","name":"one","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"first","name":"one","response":{"ok":1}}}]},
            {"role":"user","parts":[{"text":"continue"}]},
            {"role":"model","parts":[{"functionCall":{"id":"second","name":"two","args":{}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"second","name":"two","response":{"ok":2}}}]}
        ]));
        let replayed = apply_replay(&third, &full_chain);
        assert!(replayed.applied);
        let document: Value = serde_json::from_slice(&replayed.body).unwrap();
        assert_eq!(
            document.pointer("/request/contents/1/parts/0/thoughtSignature"),
            Some(&json!(SIG_A))
        );
        assert_eq!(
            document.pointer("/request/contents/4/parts/0/thoughtSignature"),
            Some(&json!(SIG_B))
        );
    }

    #[tokio::test]
    async fn expired_snapshot_cannot_delete_newer_value() {
        let cache = AntigravityReplayCache::default();
        let scope = scope("expiry");
        let (_, old) = cache.get(&scope, 0).await;
        let (_, fresh) = cache.get(&scope, REPLAY_TTL_MS + 1).await;
        assert_ne!(old, fresh);
        assert!(
            !cache
                .delete_if_unchanged(&scope, old, REPLAY_TTL_MS + 2)
                .await
        );
        assert!(
            cache
                .delete_if_unchanged(&scope, fresh, REPLAY_TTL_MS + 2)
                .await
        );
    }

    #[test]
    fn fragmented_sse_accumulates_signed_output_only_after_terminal() {
        let base = request(json!([{"role":"user","parts":[{"text":"hi"}]}]));
        let mut accumulator = AntigravityReplayStreamAccumulator::default();
        let wire = format!(
            "data: {{\"response\":{{\"candidates\":[{{\"content\":{{\"role\":\"model\",\"parts\":[{{\"functionCall\":{{\"name\":\"lookup\",\"args\":{{}}}},\"thoughtSignature\":\"{SIG_A}\"}}]}}}}]}}}}\n\ndata: {{\"response\":{{\"candidates\":[{{\"finishReason\":\"STOP\"}}]}}}}\n\n"
        );
        for chunk in wire.as_bytes().chunks(7) {
            accumulator.push(chunk);
        }
        assert!(accumulator.is_complete());
        assert!(accumulator.finish(&base, None).is_some());
    }

    #[test]
    fn session_id_is_conversation_scoped_and_rollover_error_is_exact() {
        let first = derive_session_id("account", "conversation-a", 0).unwrap();
        assert_eq!(
            first,
            derive_session_id("account", "conversation-a", 0).unwrap()
        );
        assert_ne!(
            first,
            derive_session_id("account", "conversation-b", 0).unwrap()
        );
        assert_ne!(
            first,
            derive_session_id("account", "conversation-a", 1).unwrap()
        );
        assert!(first.parse::<i64>().is_ok());
        assert!(is_session_accumulation_error(
            400,
            b"The input token count exceeds the maximum number of tokens allowed 1048576"
        ));
        assert!(!is_session_accumulation_error(
            400,
            b"ordinary context length exceeded"
        ));
        assert!(!is_session_accumulation_error(
            429,
            b"The input token count exceeds the maximum number of tokens allowed 1048576"
        ));
        assert!(is_signature_rejection(
            400,
            b"invalid thoughtSignature in function call history"
        ));
        assert!(!is_signature_rejection(400, b"ordinary invalid schema"));
    }
}
