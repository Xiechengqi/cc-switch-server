use std::collections::{BTreeMap, HashMap};
use std::fmt;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::responses_transport::{ResponsesTransportDecoder, ResponsesTransportItem};

const MAX_ENTRIES: usize = 2_048;
const MAX_CALLS: usize = 64;
const MAX_ITEM_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_INPUT_ITEMS: usize = 256;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_STREAM_ITEMS: usize = 256;
pub(crate) const TTL_MS: i64 = 30 * 60 * 1_000;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct GrokReplayScope(String);

impl fmt::Debug for GrokReplayScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GrokReplayScope(<redacted>)")
    }
}

impl GrokReplayScope {
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
        turn_index: u64,
        model_family: &str,
        rail: &str,
        upstream_plane: &str,
    ) -> Option<Self> {
        let text = [
            app,
            provider_id,
            runtime_fingerprint,
            account_id,
            share_id,
            user_namespace,
            session_id,
            model_family,
            rail,
            upstream_plane,
        ];
        if text.iter().any(|value| value.trim().is_empty()) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-server:grok-reasoning-replay:v1\0");
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
            &turn_index.to_string(),
            model_family,
            rail,
            upstream_plane,
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        Some(Self(hex::encode(hasher.finalize())))
    }
}

pub(crate) fn user_namespace(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty()).then(|| hex::encode(&Sha256::digest(value.as_bytes())[..16]))
}

pub(crate) fn model_family(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value.len() > 160 || !value.starts_with("grok") {
        return None;
    }
    Some(value.split('@').next().unwrap_or(&value).to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GrokReplayProof {
    calls: BTreeMap<String, GrokCallProof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrokCallProof {
    fingerprint: String,
    reasoning: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GrokReplaySnapshot(u64);

impl GrokReplaySnapshot {
    pub(crate) fn generation(self) -> u64 {
        self.0
    }
}

#[derive(Clone)]
struct Entry {
    proof: Option<GrokReplayProof>,
    generation: u64,
    touched_at_ms: i64,
    expires_at_ms: i64,
    bytes: usize,
}

#[derive(Default)]
struct Store {
    entries: HashMap<GrokReplayScope, Entry>,
    generation: u64,
    total_bytes: usize,
}

#[derive(Default)]
pub(crate) struct GrokReplayCache {
    store: Mutex<Store>,
}

impl fmt::Debug for GrokReplayCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GrokReplayCache(<opaque>)")
    }
}

impl GrokReplayCache {
    pub(crate) async fn get(
        &self,
        scope: &GrokReplayScope,
        now_ms: i64,
    ) -> (Option<GrokReplayProof>, GrokReplaySnapshot) {
        let mut store = self.store.lock().await;
        prune(&mut store, now_ms);
        if !store.entries.contains_key(scope) {
            reserve(&mut store, scope.clone(), now_ms);
        }
        let entry = store.entries.get_mut(scope).expect("scope is reserved");
        entry.touched_at_ms = now_ms;
        entry.expires_at_ms = now_ms.saturating_add(TTL_MS);
        (entry.proof.clone(), GrokReplaySnapshot(entry.generation))
    }

    pub(crate) async fn replace_if_unchanged(
        &self,
        scope: GrokReplayScope,
        snapshot: GrokReplaySnapshot,
        proof: GrokReplayProof,
        now_ms: i64,
    ) -> bool {
        let Some(bytes) = valid_proof_bytes(&proof) else {
            return false;
        };
        let mut store = self.store.lock().await;
        prune(&mut store, now_ms);
        if store.entries.get(&scope).map(|entry| entry.generation) != Some(snapshot.0) {
            return false;
        }
        remove(&mut store, &scope);
        let generation = next_generation(&mut store);
        store.total_bytes = store.total_bytes.saturating_add(bytes);
        store.entries.insert(
            scope,
            Entry {
                proof: Some(proof),
                generation,
                touched_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(TTL_MS),
                bytes,
            },
        );
        enforce_limits(&mut store);
        true
    }

    pub(crate) async fn delete_if_unchanged(
        &self,
        scope: &GrokReplayScope,
        snapshot: GrokReplaySnapshot,
        now_ms: i64,
    ) -> bool {
        let mut store = self.store.lock().await;
        if store.entries.get(scope).map(|entry| entry.generation) != Some(snapshot.0) {
            return false;
        }
        remove(&mut store, scope);
        reserve(&mut store, scope.clone(), now_ms);
        true
    }

    #[cfg(test)]
    pub(crate) async fn proof_count(&self) -> usize {
        self.store
            .lock()
            .await
            .entries
            .values()
            .filter(|entry| entry.proof.is_some())
            .count()
    }

    #[cfg(test)]
    pub(crate) async fn entry_count(&self) -> usize {
        self.store.lock().await.entries.len()
    }
}

fn next_generation(store: &mut Store) -> u64 {
    store.generation = store.generation.saturating_add(1).max(1);
    store.generation
}

fn reserve(store: &mut Store, scope: GrokReplayScope, now_ms: i64) {
    let generation = next_generation(store);
    store.entries.insert(
        scope,
        Entry {
            proof: None,
            generation,
            touched_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(TTL_MS),
            bytes: 0,
        },
    );
    enforce_limits(store);
}

fn remove(store: &mut Store, scope: &GrokReplayScope) {
    if let Some(entry) = store.entries.remove(scope) {
        store.total_bytes = store.total_bytes.saturating_sub(entry.bytes);
    }
}

fn prune(store: &mut Store, now_ms: i64) {
    let expired = store
        .entries
        .iter()
        .filter(|(_, entry)| entry.expires_at_ms <= now_ms)
        .map(|(scope, _)| scope.clone())
        .collect::<Vec<_>>();
    for scope in expired {
        remove(store, &scope);
    }
}

fn enforce_limits(store: &mut Store) {
    while store.entries.len() > MAX_ENTRIES || store.total_bytes > MAX_TOTAL_BYTES {
        let Some(scope) = store
            .entries
            .iter()
            .min_by_key(|(_, entry)| (entry.touched_at_ms, entry.generation))
            .map(|(scope, _)| scope.clone())
        else {
            break;
        };
        remove(store, &scope);
    }
}

fn valid_proof_bytes(proof: &GrokReplayProof) -> Option<usize> {
    if proof.calls.is_empty() || proof.calls.len() > MAX_CALLS {
        return None;
    }
    let bytes = serde_json::to_vec(proof).ok()?.len();
    (bytes <= MAX_ENTRY_BYTES).then_some(bytes)
}

#[derive(Debug)]
pub(crate) struct ApplyResult {
    pub(crate) body: Bytes,
    pub(crate) applied: bool,
    pub(crate) context_mismatch: bool,
}

pub(crate) fn apply(body: &[u8], proof: &GrokReplayProof) -> ApplyResult {
    let Ok(mut document) = serde_json::from_slice::<Value>(body) else {
        return unchanged(body, false);
    };
    let Some(input) = document.get_mut("input").and_then(Value::as_array_mut) else {
        return unchanged(body, false);
    };
    if input.len() > MAX_INPUT_ITEMS {
        return unchanged(body, true);
    }
    let mut seen = BTreeMap::new();
    for item in input.iter() {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let Some(call_id) = bounded_text(item.get("call_id"), 256) else {
            return unchanged(body, true);
        };
        let Some(fingerprint) = call_fingerprint(item) else {
            return unchanged(body, true);
        };
        if seen
            .insert(call_id.to_string(), fingerprint.clone())
            .is_some()
        {
            return unchanged(body, true);
        }
        if proof
            .calls
            .get(call_id)
            .is_some_and(|cached| cached.fingerprint != fingerprint)
        {
            return unchanged(body, true);
        }
    }
    let mut rebuilt = Vec::with_capacity(input.len().saturating_add(proof.calls.len()));
    let mut applied = false;
    let mut last_injected: Option<Value> = None;
    for item in input.drain(..) {
        let cached = (item.get("type").and_then(Value::as_str) == Some("function_call"))
            .then(|| item.get("call_id").and_then(Value::as_str))
            .flatten()
            .and_then(|call_id| proof.calls.get(call_id));
        if let Some(cached) = cached {
            let already_preceded = rebuilt
                .last()
                .and_then(|value: &Value| value.get("type"))
                .and_then(Value::as_str)
                == Some("reasoning");
            if !already_preceded && last_injected.as_ref() != Some(&cached.reasoning) {
                rebuilt.push(cached.reasoning.clone());
                last_injected = Some(cached.reasoning.clone());
                applied = true;
            }
        }
        rebuilt.push(item);
    }
    *input = rebuilt;
    if !applied {
        return unchanged(body, false);
    }
    serde_json::to_vec(&document)
        .map(Bytes::from)
        .map(|body| ApplyResult {
            body,
            applied: true,
            context_mismatch: false,
        })
        .unwrap_or_else(|_| unchanged(body, true))
}

fn unchanged(body: &[u8], context_mismatch: bool) -> ApplyResult {
    ApplyResult {
        body: Bytes::copy_from_slice(body),
        applied: false,
        context_mismatch,
    }
}

fn call_fingerprint(item: &Value) -> Option<String> {
    let name = bounded_text(item.get("name"), 256)?;
    let arguments = item.get("arguments").unwrap_or(&Value::Null);
    let canonical = canonical_value(arguments);
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-server:grok-call:v1\0");
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(&canonical).ok()?);
    Some(hex::encode(hasher.finalize()))
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(Map::from_iter(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect::<BTreeMap<_, _>>(),
        )),
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}

fn bounded_text(value: Option<&Value>, max: usize) -> Option<&str> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max)
}

fn sanitize_reasoning(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let encrypted = bounded_text(item.get("encrypted_content"), MAX_ITEM_BYTES)?;
    if encrypted.len() < 16 {
        return None;
    }
    let mut output = Map::new();
    output.insert("type".to_string(), Value::String("reasoning".to_string()));
    for key in ["id", "summary", "content", "encrypted_content"] {
        if let Some(value) = item.get(key).filter(|value| !value.is_null()) {
            output.insert(key.to_string(), value.clone());
        }
    }
    let value = Value::Object(output);
    (serde_json::to_vec(&value).ok()?.len() <= MAX_ITEM_BYTES).then_some(value)
}

pub(crate) fn capture_document(body: &[u8]) -> Option<GrokReplayProof> {
    let document = serde_json::from_slice::<Value>(body).ok()?;
    let response = document.get("response").unwrap_or(&document);
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return None;
    }
    capture_output(response.get("output")?.as_array()?)
}

fn capture_output(output: &[Value]) -> Option<GrokReplayProof> {
    let mut current = None;
    let mut calls = BTreeMap::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => current = sanitize_reasoning(item),
            Some("function_call") => {
                let reasoning = current.clone()?;
                let call_id = bounded_text(item.get("call_id"), 256)?.to_string();
                let fingerprint = call_fingerprint(item)?;
                if calls
                    .insert(
                        call_id,
                        GrokCallProof {
                            fingerprint,
                            reasoning,
                        },
                    )
                    .is_some()
                {
                    return None;
                }
            }
            _ => {}
        }
    }
    let proof = GrokReplayProof { calls };
    valid_proof_bytes(&proof).map(|_| proof)
}

pub(crate) fn is_explicit_rejection(status: u16, body: &[u8]) -> bool {
    if status != 400 || body.len() > 64 * 1024 {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    is_explicit_rejection_value(&value)
}

pub(crate) fn is_explicit_rejection_value(value: &Value) -> bool {
    let error = value
        .pointer("/response/error")
        .or_else(|| value.pointer("/response/status_details/error"))
        .or_else(|| value.get("error"))
        .unwrap_or(value);
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let exact_code = matches!(
        code.as_str(),
        "invalid_reasoning"
            | "missing_reasoning"
            | "reasoning_rejected"
            | "invalid_encrypted_content"
    );
    let exact_message = message.contains("could not decode the compaction blob")
        || message.contains("could not decrypt the provided encrypted_content");
    (exact_code && (message.contains("reasoning") || message.contains("encrypted_content")))
        || exact_message
}

#[derive(Debug)]
pub(crate) struct GrokReplayStreamAccumulator {
    decoder: ResponsesTransportDecoder,
    output: Vec<Value>,
    terminal: Option<Value>,
    bytes: usize,
    invalid: bool,
}

impl Default for GrokReplayStreamAccumulator {
    fn default() -> Self {
        Self {
            decoder: ResponsesTransportDecoder::new(MAX_ITEM_BYTES, MAX_STREAM_BYTES),
            output: Vec::new(),
            terminal: None,
            bytes: 0,
            invalid: false,
        }
    }
}

impl GrokReplayStreamAccumulator {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.invalid {
            return;
        }
        let Some(next) = self.bytes.checked_add(chunk.len()) else {
            self.invalid = true;
            self.clear();
            return;
        };
        if next > MAX_STREAM_BYTES {
            self.invalid = true;
            self.clear();
            return;
        }
        self.bytes = next;
        match self.decoder.push(chunk) {
            Ok(items) => self.observe(items),
            Err(_) => {
                self.invalid = true;
                self.clear();
            }
        }
    }

    pub(crate) fn finish(mut self) -> Option<GrokReplayProof> {
        if self.invalid {
            return None;
        }
        let items = self.decoder.finish().ok()?;
        self.observe(items);
        if self.invalid {
            return None;
        }
        let terminal = self.terminal?;
        let response = terminal.get("response")?;
        if response.get("status").and_then(Value::as_str) != Some("completed") {
            return None;
        }
        if let Some(items) = response.get("output").and_then(Value::as_array) {
            if !items.is_empty() {
                self.output = items.clone();
            }
        }
        capture_output(&self.output)
    }

    fn observe(&mut self, items: Vec<ResponsesTransportItem>) {
        for item in items {
            let ResponsesTransportItem::Json { value, .. } = item else {
                continue;
            };
            match value.get("type").and_then(Value::as_str) {
                Some("response.output_item.done") => {
                    let Some(item) = value.get("item").filter(|item| item.is_object()) else {
                        self.invalid = true;
                        self.clear();
                        return;
                    };
                    if self.output.len() >= MAX_STREAM_ITEMS {
                        self.invalid = true;
                        self.clear();
                        return;
                    }
                    self.output.push(item.clone());
                }
                Some("response.completed") => {
                    if self.terminal.replace(value).is_some() {
                        self.invalid = true;
                        self.clear();
                        return;
                    }
                }
                Some("response.failed" | "response.incomplete") => {
                    self.invalid = true;
                    self.clear();
                    return;
                }
                _ => {}
            }
        }
    }

    fn clear(&mut self) {
        self.output.clear();
        self.terminal = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn proof() -> GrokReplayProof {
        capture_document(
            &serde_json::to_vec(&json!({
                "status":"completed",
                "output":[
                    {"type":"reasoning","id":"rs_1","encrypted_content":"opaque-reasoning-proof-1234567890","summary":[]},
                    {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"x\":1}"},
                    {"type":"function_call","call_id":"call_2","name":"lookup","arguments":"{\"x\":2}"}
                ]
            }))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn parallel_calls_restore_one_exact_reasoning_item() {
        let body = serde_json::to_vec(&json!({"input":[
            {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"x\":1}"},
            {"type":"function_call","call_id":"call_2","name":"lookup","arguments":"{\"x\":2}"},
            {"type":"function_call_output","call_id":"call_1","output":"a"},
            {"type":"function_call_output","call_id":"call_2","output":"b"}
        ]}))
        .unwrap();
        let applied = apply(&body, &proof());
        assert!(applied.applied);
        let value: Value = serde_json::from_slice(&applied.body).unwrap();
        assert_eq!(value["input"][0]["type"], "reasoning");
        assert_eq!(value["input"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn edited_or_duplicate_calls_fail_closed_without_partial_mutation() {
        for input in [
            json!([{"type":"function_call","call_id":"call_1","name":"other","arguments":"{\"x\":1}"}]),
            json!([
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"x\":1}"},
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"x\":1}"}
            ]),
        ] {
            let body = serde_json::to_vec(&json!({"input":input})).unwrap();
            let result = apply(&body, &proof());
            assert!(!result.applied);
            assert!(result.context_mismatch);
            assert_eq!(result.body.as_ref(), body);
        }
    }

    #[test]
    fn recovery_classifier_accepts_only_frozen_reasoning_rejections() {
        for body in [
            json!({"error":{"code":"missing_reasoning","message":"reasoning item is missing"}}),
            json!({"error":{"type":"invalid_request_error","message":"could not decrypt the provided encrypted_content"}}),
        ] {
            assert!(is_explicit_rejection(
                400,
                &serde_json::to_vec(&body).unwrap()
            ));
        }
        for body in [
            json!({"error":{"code":"invalid_schema","message":"reasoning property is malformed"}}),
            json!({"error":{"code":"invalid_reasoning","message":"token expired"}}),
            json!({"error":{"code":"rate_limit","message":"reasoning capacity exceeded"}}),
        ] {
            assert!(!is_explicit_rejection(
                400,
                &serde_json::to_vec(&body).unwrap()
            ));
        }
        let exact = serde_json::to_vec(&json!({
            "error":{"code":"missing_reasoning","message":"reasoning item is missing"}
        }))
        .unwrap();
        assert!(!is_explicit_rejection(401, &exact));
        assert!(!is_explicit_rejection(429, &exact));
        assert!(is_explicit_rejection_value(&json!({
            "type":"response.failed",
            "response":{"status":"failed","error":{
                "code":"missing_reasoning",
                "message":"reasoning item is missing"
            }}
        })));
    }

    #[test]
    fn stream_capture_handles_fragmented_crlf_and_raw_websocket_json() {
        let mut sse = GrokReplayStreamAccumulator::default();
        sse.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"opaque-reasoning-proof-1234567890\"}}\r\n\r");
        sse.push(b"\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"lookup\",\"arguments\":\"{}\"}}\r\n\r\n");
        sse.push(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\r\n\r\n");
        assert!(sse.finish().is_some());

        let mut websocket = GrokReplayStreamAccumulator::default();
        for value in [
            json!({"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":"opaque-reasoning-proof-1234567890"}}),
            json!({"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}}),
            json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        ] {
            websocket.push(&serde_json::to_vec(&value).unwrap());
        }
        assert!(websocket.finish().is_some());
    }

    #[tokio::test]
    async fn cache_cas_ttl_and_scope_are_bounded() {
        let cache = GrokReplayCache::default();
        let scope = GrokReplayScope("scope-a".into());
        let (_, snapshot) = cache.get(&scope, 1).await;
        assert!(
            cache
                .replace_if_unchanged(scope.clone(), snapshot, proof(), 2)
                .await
        );
        assert!(
            !cache
                .replace_if_unchanged(scope.clone(), snapshot, proof(), 3)
                .await
        );
        assert!(cache.get(&scope, 3).await.0.is_some());
        assert!(cache.get(&scope, 3 + TTL_MS).await.0.is_none());
        assert!(cache
            .get(&GrokReplayScope("scope-b".into()), 3)
            .await
            .0
            .is_none());
    }

    #[test]
    fn derived_scope_fences_every_binding_and_transport_dimension() {
        let base = [
            "codex",
            "provider-a",
            "runtime-a",
            "account-a",
            "share-a",
            "user-a",
            "session-a",
            "grok-4.6",
            "http",
            "api.x.ai",
        ];
        let derive =
            |values: [&str; 10], provider_revision, auth_generation, token_generation, turn| {
                GrokReplayScope::derive(
                    values[0],
                    values[1],
                    provider_revision,
                    values[2],
                    values[3],
                    auth_generation,
                    token_generation,
                    values[4],
                    values[5],
                    values[6],
                    turn,
                    values[7],
                    values[8],
                    values[9],
                )
                .unwrap()
            };
        let expected = derive(base, 7, 11, 13, 17);
        for index in 0..base.len() {
            let mut changed = base;
            changed[index] = match index {
                0 => "claude",
                1 => "provider-b",
                2 => "runtime-b",
                3 => "account-b",
                4 => "share-b",
                5 => "user-b",
                6 => "session-b",
                7 => "grok-4.7",
                8 => "websocket",
                _ => "cli-chat-proxy.grok.com",
            };
            assert_ne!(derive(changed, 7, 11, 13, 17), expected);
        }
        for changed in [
            derive(base, 8, 11, 13, 17),
            derive(base, 7, 12, 13, 17),
            derive(base, 7, 11, 14, 17),
            derive(base, 7, 11, 13, 18),
        ] {
            assert_ne!(changed, expected);
        }
    }
}
