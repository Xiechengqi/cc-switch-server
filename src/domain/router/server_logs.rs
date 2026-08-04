use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const INSTALLATION_LOG_PROTOCOL_VERSION: u8 = 1;
pub const INSTALLATION_LOG_BATCH_ACTION: &str = "installation_log_batch_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogEvent {
    pub sequence: u64,
    pub occurred_at_ms: i64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogBatchPayload {
    pub protocol_version: u8,
    pub stream_id: String,
    pub server_version: String,
    pub commit_id: String,
    pub events: Vec<InstallationLogEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogBatchResponse {
    pub ok: bool,
    pub accepted: usize,
    pub next_sequence: u64,
}
