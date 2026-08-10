use std::collections::VecDeque;
use std::sync::{OnceLock, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

const ENVELOPE_PREFIX: &str = "ccswitch-server-reasoning-v1:";
const KEY_INFO: &[u8] = b"cc-switch-server/proxy-reasoning-bridge/v1";
const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
#[cfg(not(test))]
const MAX_RETIRED_BRIDGE_KEYS: usize = 1;
#[cfg(test)]
const MAX_RETIRED_BRIDGE_KEYS: usize = 512;

static BRIDGE_KEY: OnceLock<BridgeKeySlot> = OnceLock::new();

#[derive(Debug)]
struct BridgeKeySlot {
    keys: RwLock<BridgeKeys>,
    max_retired: usize,
}

#[derive(Debug)]
struct BridgeKeys {
    current: [u8; 32],
    retired: VecDeque<[u8; 32]>,
}

impl BridgeKeySlot {
    fn new(key: [u8; 32]) -> Self {
        Self::with_retention(key, MAX_RETIRED_BRIDGE_KEYS)
    }

    fn with_retention(key: [u8; 32], max_retired: usize) -> Self {
        Self {
            keys: RwLock::new(BridgeKeys {
                current: key,
                retired: VecDeque::with_capacity(max_retired),
            }),
            max_retired,
        }
    }

    fn current(&self) -> [u8; 32] {
        self.keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .current
    }

    fn verification_keys(&self) -> Vec<[u8; 32]> {
        let keys = self
            .keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::iter::once(keys.current)
            .chain(keys.retired.iter().copied())
            .collect()
    }

    fn replace(&self, key: [u8; 32]) {
        let mut keys = self
            .keys
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if keys.current == key {
            return;
        }
        keys.retired.retain(|retired| *retired != key);
        let current = std::mem::replace(&mut keys.current, key);
        keys.retired.push_front(current);
        keys.retired.truncate(self.max_retired);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EnvelopeKind {
    OpenAiReasoning,
    AnthropicThinking,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReasoningEnvelope {
    version: u8,
    kind: EnvelopeKind,
    value: Value,
}

pub(crate) fn initialize(root_key: &[u8; 32]) -> anyhow::Result<()> {
    let key = derive_bridge_key(root_key)?;
    replace_bridge_key(&BRIDGE_KEY, key);
    Ok(())
}

pub(crate) fn rotate(root_key: &[u8; 32]) -> anyhow::Result<()> {
    let key = derive_bridge_key(root_key)?;
    replace_bridge_key(&BRIDGE_KEY, key);
    Ok(())
}

fn replace_bridge_key(slot: &OnceLock<BridgeKeySlot>, key: [u8; 32]) {
    slot.get_or_init(|| BridgeKeySlot::new(key)).replace(key);
}

fn derive_bridge_key(root_key: &[u8; 32]) -> anyhow::Result<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(
        Some(b"cc-switch-server/proxy-reasoning-bridge-salt/v1"),
        root_key,
    );
    let mut key = [0u8; 32];
    hkdf.expand(KEY_INFO, &mut key)
        .map_err(|_| anyhow::anyhow!("derive proxy reasoning bridge key"))?;
    Ok(key)
}

pub(crate) fn reasoning_summary_text(item: &Value) -> String {
    let mut text = Vec::new();
    for field in ["summary", "content"] {
        if let Some(parts) = item.get(field).and_then(Value::as_array) {
            text.extend(parts.iter().filter_map(|part| {
                matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("summary_text" | "reasoning_text" | "text")
                )
                .then(|| part.get("text").and_then(Value::as_str))
                .flatten()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            }));
        }
    }
    text.join("")
}

pub(crate) fn anthropic_block_from_openai_reasoning_item(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let text = reasoning_summary_text(item);
    let has_encrypted_content = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());

    if has_encrypted_content {
        if let Some(envelope) = encode_envelope(EnvelopeKind::OpenAiReasoning, item) {
            if text.is_empty() {
                return Some(json!({"type": "redacted_thinking", "data": envelope}));
            }
            return Some(json!({
                "type": "thinking",
                "thinking": text,
                "signature": envelope
            }));
        }
        return (!text.is_empty()).then(|| json!({"type": "thinking", "thinking": text}));
    }

    (!text.is_empty()).then(|| json!({"type": "thinking", "thinking": text}))
}

pub(crate) fn openai_reasoning_item_from_anthropic_block(block: &Value) -> Option<Value> {
    let encoded = match block.get("type").and_then(Value::as_str) {
        Some("thinking") => block.get("signature").and_then(Value::as_str),
        Some("redacted_thinking") => block.get("data").and_then(Value::as_str),
        _ => None,
    }?;
    let item = decode_envelope(encoded, EnvelopeKind::OpenAiReasoning)?;
    (item.get("type").and_then(Value::as_str) == Some("reasoning")).then_some(item)
}

pub(crate) fn responses_reasoning_item_from_anthropic_block(
    item_id: &str,
    block: &Value,
) -> Option<Value> {
    if !anthropic_block_is_signed(block) {
        return None;
    }
    let encrypted_content = encode_envelope(EnvelopeKind::AnthropicThinking, block)?;
    let summary = block
        .get("thinking")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(|text| vec![json!({"type": "summary_text", "text": text})])
        .unwrap_or_default();
    Some(json!({
        "id": item_id,
        "type": "reasoning",
        "summary": summary,
        "encrypted_content": encrypted_content
    }))
}

pub(crate) fn anthropic_block_from_responses_reasoning_item(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let encoded = item.get("encrypted_content").and_then(Value::as_str)?;
    let block = decode_envelope(encoded, EnvelopeKind::AnthropicThinking)?;
    anthropic_block_is_signed(&block).then_some(block)
}

pub(crate) fn unsigned_responses_reasoning_item(item_id: &str, text: &str) -> Option<Value> {
    let text = text.trim();
    (!text.is_empty()).then(|| {
        json!({
            "id": item_id,
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": text}]
        })
    })
}

fn anthropic_block_is_signed(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => block
            .get("signature")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        Some("redacted_thinking") => block
            .get("data")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        _ => false,
    }
}

fn encode_envelope(kind: EnvelopeKind, value: &Value) -> Option<String> {
    let payload = serde_json::to_vec(&ReasoningEnvelope {
        version: 1,
        kind,
        value: value.clone(),
    })
    .ok()?;
    if payload.len() > MAX_ENVELOPE_BYTES {
        crate::metrics::record_reasoning_bridge("encode", "too_large");
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(&bridge_keys().current()).ok()?;
    mac.update(ENVELOPE_PREFIX.as_bytes());
    mac.update(&payload);
    let tag = mac.finalize().into_bytes();
    crate::metrics::record_reasoning_bridge("encode", "success");
    Some(format!(
        "{ENVELOPE_PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(tag)
    ))
}

fn decode_envelope(encoded: &str, expected_kind: EnvelopeKind) -> Option<Value> {
    if encoded.len() > MAX_ENVELOPE_BYTES.saturating_mul(2) {
        crate::metrics::record_reasoning_bridge("decode", "too_large");
        return None;
    }
    let encoded = encoded.strip_prefix(ENVELOPE_PREFIX)?;
    let (payload, tag) = encoded.split_once('.')?;
    let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
    if payload.len() > MAX_ENVELOPE_BYTES {
        return None;
    }
    if !bridge_keys().verification_keys().iter().any(|key| {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
            return false;
        };
        mac.update(ENVELOPE_PREFIX.as_bytes());
        mac.update(&payload);
        mac.verify_slice(&tag).is_ok()
    }) {
        crate::metrics::record_reasoning_bridge("decode", "invalid_mac");
        return None;
    }
    let envelope: ReasoningEnvelope = serde_json::from_slice(&payload).ok()?;
    if envelope.version != 1 || envelope.kind != expected_kind {
        crate::metrics::record_reasoning_bridge("decode", "invalid_envelope");
        return None;
    }
    crate::metrics::record_reasoning_bridge("decode", "success");
    Some(envelope.value)
}

fn bridge_keys() -> &'static BridgeKeySlot {
    BRIDGE_KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        tracing::debug!("using process-local proxy reasoning bridge key");
        BridgeKeySlot::new(key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_key_slot_retains_only_the_configured_history() {
        let slot = BridgeKeySlot::with_retention([1; 32], 1);
        assert_eq!(slot.current(), [1; 32]);
        slot.replace([2; 32]);
        assert_eq!(slot.verification_keys(), vec![[2; 32], [1; 32]]);
        slot.replace([3; 32]);
        assert_eq!(slot.verification_keys(), vec![[3; 32], [2; 32]]);
    }

    #[test]
    fn persistent_initialization_replaces_a_process_local_fallback() {
        let slot = OnceLock::new();
        slot.get_or_init(|| BridgeKeySlot::new([1; 32]));

        replace_bridge_key(&slot, [2; 32]);

        assert_eq!(slot.get().unwrap().current(), [2; 32]);
        assert_eq!(
            slot.get().unwrap().verification_keys(),
            vec![[2; 32], [1; 32]]
        );
    }

    #[test]
    fn bridge_key_derivation_is_stable_and_purpose_separated() {
        let root = [7; 32];
        assert_eq!(
            derive_bridge_key(&root).unwrap(),
            derive_bridge_key(&root).unwrap()
        );
        assert_ne!(derive_bridge_key(&root).unwrap(), root);
        assert_ne!(
            derive_bridge_key(&root).unwrap(),
            derive_bridge_key(&[8; 32]).unwrap()
        );
    }

    #[test]
    fn envelope_created_before_key_rotation_remains_decodable() {
        replace_bridge_key(&BRIDGE_KEY, [41; 32]);
        let item = json!({
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "in-flight"
        });
        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();

        replace_bridge_key(&BRIDGE_KEY, [42; 32]);

        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&block),
            Some(item)
        );
    }

    #[test]
    fn openai_reasoning_round_trips_with_authenticated_envelope() {
        let item = json!({
            "id": "rs_1",
            "type": "reasoning",
            "summary": [
                {"type": "summary_text", "text": "first"},
                {"type": "reasoning_text", "text": " second"}
            ],
            "encrypted_content": "provider-opaque"
        });
        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();
        assert_eq!(block["thinking"], "first second");
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&block),
            Some(item)
        );
    }

    #[test]
    fn tampered_envelope_is_rejected() {
        let item = json!({
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "opaque"
        });
        let mut block = anthropic_block_from_openai_reasoning_item(&item).unwrap();
        let data = block["data"].as_str().unwrap().to_string();
        block["data"] = Value::String(format!("{data}x"));
        assert!(openai_reasoning_item_from_anthropic_block(&block).is_none());
    }

    #[test]
    fn oversized_opaque_reasoning_preserves_visible_summary_unsigned() {
        let item = json!({
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "visible"}],
            "encrypted_content": "x".repeat(MAX_ENVELOPE_BYTES)
        });

        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();

        assert_eq!(block, json!({"type": "thinking", "thinking": "visible"}));
    }

    #[test]
    fn unsigned_anthropic_thinking_never_becomes_encrypted_content() {
        let block = json!({"type": "thinking", "thinking": "visible only"});
        assert!(responses_reasoning_item_from_anthropic_block("rs_1", &block).is_none());
        let unsigned = unsigned_responses_reasoning_item("rs_1", "visible only").unwrap();
        assert!(unsigned.get("encrypted_content").is_none());
    }

    #[test]
    fn native_anthropic_thinking_round_trips_separately() {
        let block = json!({
            "type": "thinking",
            "thinking": "check",
            "signature": "anthropic-signature"
        });
        let item = responses_reasoning_item_from_anthropic_block("rs_2", &block).unwrap();
        assert_eq!(
            anthropic_block_from_responses_reasoning_item(&item),
            Some(block)
        );
    }

    #[test]
    fn proxy_bridge_contract_fixture_authenticates_reasoning_round_trips() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/reasoning.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "reasoning-authenticated-roundtrip");
        assert_eq!(fixture["category"], "reasoning");

        let openai_item = fixture["openAiItem"].clone();
        let mut bridged = anthropic_block_from_openai_reasoning_item(&openai_item).unwrap();
        assert_eq!(bridged["thinking"], fixture["expectedThinking"]);
        assert!(bridged["signature"]
            .as_str()
            .is_some_and(|value| value.starts_with(fixture["envelopePrefix"].as_str().unwrap())));
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&bridged),
            Some(openai_item)
        );

        let signature = bridged["signature"].as_str().unwrap().to_string();
        bridged["signature"] = Value::String(format!("{signature}x"));
        assert!(openai_reasoning_item_from_anthropic_block(&bridged).is_none());

        let anthropic_block = fixture["anthropicBlock"].clone();
        let responses_item =
            responses_reasoning_item_from_anthropic_block("rs_native_contract", &anthropic_block)
                .unwrap();
        assert!(responses_item["encrypted_content"]
            .as_str()
            .is_some_and(|value| value.starts_with(fixture["envelopePrefix"].as_str().unwrap())));
        assert_eq!(
            anthropic_block_from_responses_reasoning_item(&responses_item),
            Some(anthropic_block)
        );
    }
}
