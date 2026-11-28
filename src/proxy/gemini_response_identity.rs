//! Local bridge correlation IDs, never upstream resource or resume grants.
use rand::RngCore;
use serde_json::Value;

pub(super) fn explicit_message_id(input: &Value) -> Option<&str> {
    ["responseId", "response_id"]
        .into_iter()
        .filter_map(|name| input.get(name).and_then(Value::as_str))
        .find(|id| !id.trim().is_empty())
}

pub(super) fn message_id(input: &Value) -> String {
    explicit_message_id(input)
        .map(str::to_string)
        .unwrap_or_else(|| correlation_id("msg"))
}

pub(super) fn search_tool_id() -> String {
    correlation_id("srvtoolu")
}

fn correlation_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}
