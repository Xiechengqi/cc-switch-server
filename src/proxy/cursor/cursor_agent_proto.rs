//! Hand-rolled protobuf encoder/decoder for Cursor's
//! `agent.v1.AgentService/Run` Connect-RPC endpoint.
//!
//! This is a Rust port of OmniRoute's `cursorAgentProtobuf.ts`. The on-the-wire
//! schema is non-public and pinned to the protobuf descriptor shipped in
//! cursor-agent's bundle (≈ 2026.06.02-8c11d9f), cross-checked against
//! CLIProxyAPIPlus's reference impl. All field numbers below come from that
//! descriptor — adjust them if a future cursor-agent build drifts.
//!
//! Format: Connect-RPC framed protobuf. Each frame is `flags (u8) + length
//! (u32 BE) + body`. The body is one of:
//!   * `AgentClientMessage`  — outbound (RunRequest, ExecClient*, KvClient*)
//!   * `AgentServerMessage`  — inbound (InteractionUpdate, ExecServer*, KvServer*)

use bytes::{Buf, BufMut, Bytes, BytesMut};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

// ─── Wire types ────────────────────────────────────────────────────────────

pub(crate) const WT_VARINT: u8 = 0;
pub(crate) const WT_FIXED64: u8 = 1;
pub(crate) const WT_LEN: u8 = 2;
pub(crate) const WT_FIXED32: u8 = 5;

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
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

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

// ─── AgentClientMessage / AgentRunRequest ──────────────────────────────────

pub const ACM_RUN_REQUEST: u64 = 1;
pub const ACM_EXEC_CLIENT_MESSAGE: u64 = 2;
pub const ACM_KV_CLIENT_MESSAGE: u64 = 3;

pub const ARR_CONVERSATION_STATE: u64 = 1;
pub const ARR_ACTION: u64 = 2;
pub const ARR_MODEL_DETAILS: u64 = 3;
pub const ARR_MCP_TOOLS: u64 = 4;
pub const ARR_CONVERSATION_ID: u64 = 5;
pub const ARR_REQUESTED_MODEL: u64 = 9;
pub const ARR_UNKNOWN_12: u64 = 12;
pub const ARR_REQUEST_ID: u64 = 16;
pub const ARR_MCP_TOOLS_INNER: u64 = 1;

pub const CSS_ROOT_PROMPT: u64 = 1;
pub const CSS_TURNS: u64 = 8;

pub const CA_USER_MESSAGE_ACTION: u64 = 1;
pub const UMA_USER_MESSAGE: u64 = 1;

pub const UM_TEXT: u64 = 1;
pub const UM_MESSAGE_ID: u64 = 2;
pub const UM_SELECTED_CONTEXT: u64 = 3;
pub const UM_MODE: u64 = 4;

pub const SC_SELECTED_IMAGES: u64 = 1;
pub const SI_UUID: u64 = 2;
pub const SI_DIMENSION: u64 = 4;
pub const SI_MIME_TYPE: u64 = 7;
pub const SI_DATA: u64 = 8;
pub const DIM_WIDTH: u64 = 1;
pub const DIM_HEIGHT: u64 = 2;

pub const RM_MODEL_ID: u64 = 1;
pub const RM_PARAMETERS: u64 = 3;
pub const RMP_ID: u64 = 1;
pub const RMP_VALUE: u64 = 2;

pub const MD_MODEL_ID: u64 = 1;
pub const MD_DISPLAY_MODEL_ID: u64 = 3;
pub const MD_DISPLAY_NAME: u64 = 4;

// ─── ExecClientMessage / ExecServerMessage ─────────────────────────────────

pub const ECM_ID: u64 = 1;
pub const ECM_EXEC_ID: u64 = 15;
pub const ECM_SHELL_RESULT: u64 = 2;
pub const ECM_WRITE_RESULT: u64 = 3;
pub const ECM_DELETE_RESULT: u64 = 4;
pub const ECM_GREP_RESULT: u64 = 5;
pub const ECM_READ_RESULT: u64 = 7;
pub const ECM_LS_RESULT: u64 = 8;
pub const ECM_DIAGNOSTICS_RESULT: u64 = 9;
pub const ECM_REQUEST_CONTEXT_RESULT: u64 = 10;
pub const ECM_MCP_RESULT: u64 = 11;
pub const ECM_BACKGROUND_SHELL_SPAWN_RES: u64 = 16;
pub const ECM_FETCH_RESULT: u64 = 20;
pub const ECM_WRITE_SHELL_STDIN_RESULT: u64 = 23;

pub const ESM_ID: u64 = 1;
pub const ESM_EXEC_ID: u64 = 15;
pub const ESM_SHELL_ARGS: u64 = 2;
pub const ESM_WRITE_ARGS: u64 = 3;
pub const ESM_DELETE_ARGS: u64 = 4;
pub const ESM_GREP_ARGS: u64 = 5;
pub const ESM_READ_ARGS: u64 = 7;
pub const ESM_LS_ARGS: u64 = 8;
pub const ESM_DIAGNOSTICS_ARGS: u64 = 9;
pub const ESM_REQUEST_CONTEXT_ARGS: u64 = 10;
pub const ESM_MCP_ARGS: u64 = 11;
pub const ESM_SHELL_STREAM_ARGS: u64 = 14;
pub const ESM_BACKGROUND_SHELL_SPAWN: u64 = 16;
pub const ESM_FETCH_ARGS: u64 = 20;
pub const ESM_WRITE_SHELL_STDIN_ARGS: u64 = 23;

pub const ARG_PATH: u64 = 1;
pub const ARG_SHELL_COMMAND: u64 = 1;
pub const ARG_SHELL_WORKING_DIR: u64 = 2;
pub const ARG_FETCH_URL: u64 = 1;
pub const ARG_READ_TOOL_CALL_ID: u64 = 2;
pub const ARG_WRITE_FILE_TEXT: u64 = 2;
pub const ARG_WRITE_TOOL_CALL_ID: u64 = 3;
pub const ARG_READ_OFFSET: u64 = 4;
pub const ARG_READ_LIMIT: u64 = 5;
pub const ARG_WRITE_STREAM_CONTENT: u64 = 6;

pub const REJ_PATH: u64 = 1;
pub const REJ_REASON: u64 = 2;
pub const SREJ_COMMAND: u64 = 1;
pub const SREJ_WORKING_DIR: u64 = 2;
pub const SREJ_REASON: u64 = 3;
pub const ERR_MESSAGE: u64 = 1;
pub const FERR_URL: u64 = 1;
pub const FERR_ERROR: u64 = 2;
pub const RES_REJECTED: u64 = 2;

// ─── Request context / MCP tool definitions ────────────────────────────────

pub const RCR_SUCCESS: u64 = 1;
pub const RCS_REQUEST_CONTEXT: u64 = 1;
pub const RCS_TOOLS: u64 = 2;
pub const RCS_ENV: u64 = 4;

/// Nested environment block inside RequestContext (composer-api / Cursor SDK).
pub const RCE_HOSTNAME: u64 = 1;
pub const RCE_WORKING_DIR: u64 = 2;
pub const RCE_SHELL: u64 = 3;
pub const RCE_FLAG_5: u64 = 5;
pub const RCE_TIMEZONE: u64 = 10;
pub const RCE_CWD_ALT: u64 = 11;
pub const RCE_CWD_ALT2: u64 = 21;

/// Upper bound on a single Connect-RPC frame payload (OmniRoute: 16 MiB).
pub const CONNECT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub const MTD_NAME: u64 = 1;
pub const MTD_DESCRIPTION: u64 = 2;
pub const MTD_INPUT_SCHEMA: u64 = 3;
pub const MTD_PROVIDER_IDENTIFIER: u64 = 4;
pub const MTD_TOOL_NAME: u64 = 5;

pub const MCA_NAME: u64 = 1;
pub const MCA_ARGS: u64 = 2;
pub const MCA_TOOL_CALL_ID: u64 = 3;
pub const MCA_PROVIDER_IDENTIFIER: u64 = 4;
pub const MCA_TOOL_NAME: u64 = 5;

pub const MCR_SUCCESS: u64 = 1;
pub const MCR_ERROR: u64 = 2;
pub const MCS_CONTENT: u64 = 1;
pub const MCS_IS_ERROR: u64 = 2;
pub const MCC_TEXT: u64 = 1;
pub const MTC_TEXT: u64 = 1;

// ─── KV channel ────────────────────────────────────────────────────────────

pub const KSM_ID: u64 = 1;
pub const KSM_GET_BLOB_ARGS: u64 = 2;
pub const KSM_SET_BLOB_ARGS: u64 = 3;
pub const KSM_REQUEST_METADATA: u64 = 4;

pub const KCM_ID: u64 = 1;
pub const KCM_GET_BLOB_RESULT: u64 = 2;
pub const KCM_SET_BLOB_RESULT: u64 = 3;
pub const KCM_REQUEST_METADATA: u64 = 4;

pub const GBA_BLOB_ID: u64 = 1;
pub const SBA_BLOB_ID: u64 = 1;
pub const SBA_BLOB_DATA: u64 = 2;
pub const GBR_BLOB_DATA: u64 = 1;

// ─── AgentServerMessage / InteractionUpdate ────────────────────────────────

pub const ASM_INTERACTION_UPDATE: u64 = 1;
pub const ASM_EXEC_SERVER_MESSAGE: u64 = 2;
pub const ASM_KV_SERVER_MESSAGE: u64 = 4;

pub const IU_TEXT_DELTA: u64 = 1;
pub const IU_TOOL_CALL_STARTED: u64 = 2;
pub const IU_TOOL_CALL_COMPLETED: u64 = 3;
pub const IU_THINKING_DELTA: u64 = 4;
pub const IU_THINKING_COMPLETED: u64 = 5;
pub const IU_TOKEN_DELTA: u64 = 8;
pub const IU_HEARTBEAT: u64 = 13;
pub const IU_TURN_ENDED: u64 = 14;

pub const TDU_TEXT: u64 = 1;

// ─── google.protobuf.Value / Struct / ListValue ────────────────────────────

pub const VAL_NULL: u64 = 1;
pub const VAL_NUMBER: u64 = 2;
pub const VAL_STRING: u64 = 3;
pub const VAL_BOOL: u64 = 4;
pub const VAL_STRUCT: u64 = 5;
pub const VAL_LIST: u64 = 6;
pub const STRUCT_FIELDS: u64 = 1;
pub const LIST_VALUES: u64 = 1;

// proto3 map entries
pub const MAP_KEY: u64 = 1;
pub const MAP_VALUE: u64 = 2;

// ─── Connect-RPC frame flags ───────────────────────────────────────────────

const FLAG_NONE: u8 = 0x00;
const FLAG_GZIP: u8 = 0x01;
const FLAG_END_STREAM: u8 = 0x02;

// ─── Errors ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("varint truncated")]
    VarintTruncated,
    #[error("length-delimited field overruns buffer (len={len}, remaining={remaining})")]
    LengthOverrun { len: u64, remaining: usize },
    #[error("unsupported wire type {0}")]
    UnsupportedWireType(u8),
    #[error("gzip decode failed: {0}")]
    Gzip(String),
    #[error("connect frame exceeds max size ({size} > {max})")]
    FrameTooLarge { size: usize, max: usize },
}

pub type ProtoResult<T> = Result<T, ProtoError>;

// ─── Varint / tag helpers ──────────────────────────────────────────────────

pub(crate) fn put_varint<B: BufMut>(out: &mut B, mut value: u64) {
    while value > 0x7F {
        out.put_u8((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.put_u8(value as u8);
}

pub(crate) fn put_tag<B: BufMut>(out: &mut B, field: u64, wire: u8) {
    put_varint(out, (field << 3) | wire as u64);
}

pub(crate) fn read_varint(src: &[u8], mut pos: usize) -> ProtoResult<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    while pos < src.len() {
        let b = src[pos];
        pos += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift >= 64 {
            return Err(ProtoError::VarintTruncated);
        }
    }
    Err(ProtoError::VarintTruncated)
}

fn checked_len(len: u64, pos: usize, src: &[u8]) -> ProtoResult<usize> {
    let remaining = src.len().saturating_sub(pos);
    if len > remaining as u64 {
        return Err(ProtoError::LengthOverrun { len, remaining });
    }
    Ok(len as usize)
}

// ─── Field encoders ────────────────────────────────────────────────────────

pub fn encode_string(field: u64, value: &str) -> Bytes {
    let bytes = value.as_bytes();
    let mut out = BytesMut::with_capacity(bytes.len() + 16);
    put_tag(&mut out, field, WT_LEN);
    put_varint(&mut out, bytes.len() as u64);
    out.extend_from_slice(bytes);
    out.freeze()
}

pub fn encode_bytes(field: u64, value: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(value.len() + 16);
    put_tag(&mut out, field, WT_LEN);
    put_varint(&mut out, value.len() as u64);
    out.extend_from_slice(value);
    out.freeze()
}

pub fn encode_message(field: u64, parts: &[Bytes]) -> Bytes {
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = BytesMut::with_capacity(total + 16);
    put_tag(&mut out, field, WT_LEN);
    put_varint(&mut out, total as u64);
    for p in parts {
        out.extend_from_slice(p);
    }
    out.freeze()
}

pub fn encode_uint32(field: u64, value: u64) -> Bytes {
    let mut out = BytesMut::with_capacity(16);
    put_tag(&mut out, field, WT_VARINT);
    put_varint(&mut out, value);
    out.freeze()
}

pub fn encode_bool(field: u64, value: bool) -> Bytes {
    encode_uint32(field, if value { 1 } else { 0 })
}

pub fn encode_double(field: u64, value: f64) -> Bytes {
    let mut out = BytesMut::with_capacity(16);
    put_tag(&mut out, field, WT_FIXED64);
    out.extend_from_slice(&value.to_le_bytes());
    out.freeze()
}

pub fn concat_bytes(parts: &[Bytes]) -> Bytes {
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = BytesMut::with_capacity(total);
    for p in parts {
        out.extend_from_slice(p);
    }
    out.freeze()
}

// ─── Field iterator (decoder) ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum FieldValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed64([u8; 8]),
    Fixed32([u8; 4]),
}

#[derive(Debug, Clone)]
pub struct Field<'a> {
    pub field: u64,
    pub value: FieldValue<'a>,
}

pub struct FieldIter<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> FieldIter<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Self { src, pos: 0 }
    }
}

impl<'a> Iterator for FieldIter<'a> {
    type Item = ProtoResult<Field<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.src.len() {
            return None;
        }
        let (tag, np) = match read_varint(self.src, self.pos) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        self.pos = np;
        let field = tag >> 3;
        let wire = (tag & 0x7) as u8;
        match wire {
            WT_VARINT => match read_varint(self.src, self.pos) {
                Ok((v, np)) => {
                    self.pos = np;
                    Some(Ok(Field {
                        field,
                        value: FieldValue::Varint(v),
                    }))
                }
                Err(e) => Some(Err(e)),
            },
            WT_LEN => {
                let (len, np) = match read_varint(self.src, self.pos) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                self.pos = np;
                let n = match checked_len(len, self.pos, self.src) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let slice = &self.src[self.pos..self.pos + n];
                self.pos += n;
                Some(Ok(Field {
                    field,
                    value: FieldValue::Bytes(slice),
                }))
            }
            WT_FIXED64 => {
                if self.pos + 8 > self.src.len() {
                    return Some(Err(ProtoError::LengthOverrun {
                        len: 8,
                        remaining: self.src.len() - self.pos,
                    }));
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&self.src[self.pos..self.pos + 8]);
                self.pos += 8;
                Some(Ok(Field {
                    field,
                    value: FieldValue::Fixed64(buf),
                }))
            }
            WT_FIXED32 => {
                if self.pos + 4 > self.src.len() {
                    return Some(Err(ProtoError::LengthOverrun {
                        len: 4,
                        remaining: self.src.len() - self.pos,
                    }));
                }
                let mut buf = [0u8; 4];
                buf.copy_from_slice(&self.src[self.pos..self.pos + 4]);
                self.pos += 4;
                Some(Ok(Field {
                    field,
                    value: FieldValue::Fixed32(buf),
                }))
            }
            other => Some(Err(ProtoError::UnsupportedWireType(other))),
        }
    }
}

fn find_bytes_field(src: &[u8], field: u64) -> Option<&[u8]> {
    for f in FieldIter::new(src).flatten() {
        if f.field == field {
            if let FieldValue::Bytes(b) = f.value {
                return Some(b);
            }
        }
    }
    None
}

fn find_varint_field(src: &[u8], field: u64) -> Option<u64> {
    for f in FieldIter::new(src).flatten() {
        if f.field == field {
            if let FieldValue::Varint(v) = f.value {
                return Some(v);
            }
        }
    }
    None
}

fn decode_string_field(src: &[u8], field: u64) -> String {
    find_bytes_field(src, field)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

fn decode_varint_field(src: &[u8], field: u64) -> Option<u64> {
    for f in FieldIter::new(src).flatten() {
        if f.field == field {
            if let FieldValue::Varint(v) = f.value {
                return Some(v);
            }
        }
    }
    None
}

/// Decode repeated protobuf map-entry messages (`key` + `value` fields).
fn decode_repeated_map_entries(payload: &[u8], entry_field: u64) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    for f in FieldIter::new(payload).flatten() {
        if f.field != entry_field {
            continue;
        }
        let FieldValue::Bytes(entry) = f.value else {
            continue;
        };
        let key = decode_string_field(entry, MAP_KEY);
        if key.is_empty() {
            continue;
        }
        if let Some(vb) = find_bytes_field(entry, MAP_VALUE) {
            map.insert(key, decode_proto_value(vb));
        }
    }
    map
}

fn decode_mcp_args_map(body: &[u8]) -> serde_json::Map<String, Value> {
    let mut args = decode_repeated_map_entries(body, MCA_ARGS);
    for f in FieldIter::new(body).flatten() {
        if f.field == MCA_ARGS {
            if let FieldValue::Bytes(b) = f.value {
                for (k, v) in decode_repeated_map_entries(b, 2) {
                    args.insert(k, v);
                }
            }
        }
    }
    args
}

// ─── Connect-RPC framing ───────────────────────────────────────────────────

pub fn wrap_connect_frame(payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(5 + payload.len());
    out.put_u8(FLAG_NONE);
    out.put_u32(payload.len() as u32);
    out.extend_from_slice(payload);
    out.freeze()
}

#[derive(Debug, Clone)]
pub struct ConnectFrame {
    pub flags: u8,
    pub payload: Bytes,
}

/// Accumulating Connect-RPC frame parser. Feed inbound bytes incrementally;
/// returns any whole frames currently available. Half-frame bytes are kept in
/// the internal buffer until the next feed.
#[derive(Default)]
pub struct ConnectFrameParser {
    buf: BytesMut,
}

impl ConnectFrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> ProtoResult<Vec<ConnectFrame>> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 5 {
                break;
            }
            let flags = self.buf[0];
            let length =
                u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
            if length > CONNECT_MAX_FRAME_BYTES {
                return Err(ProtoError::FrameTooLarge {
                    size: length,
                    max: CONNECT_MAX_FRAME_BYTES,
                });
            }
            if self.buf.len() < 5 + length {
                break;
            }
            self.buf.advance(5);
            let raw = self.buf.split_to(length).freeze();
            let payload = if flags & FLAG_GZIP != 0 {
                match gunzip(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("[CursorProto] skip undecodable gzip frame: {e}");
                        continue;
                    }
                }
            } else {
                raw
            };
            out.push(ConnectFrame { flags, payload });
            // Connect-RPC trailers come on a frame with FLAG_END_STREAM; they're
            // text key:value lines, surfaced to caller as the payload with the
            // flag set so it can extract grpc-status / grpc-message.
        }
        Ok(out)
    }
}

fn gunzip(src: &[u8]) -> ProtoResult<Bytes> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(src);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ProtoError::Gzip(e.to_string()))?;
    Ok(Bytes::from(out))
}

pub fn is_end_stream(frame: &ConnectFrame) -> bool {
    frame.flags & FLAG_END_STREAM != 0
}

/// Parse Connect-RPC trailer payload (text body: `key: value\r\n` lines).
pub fn parse_trailers(payload: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(payload);
    text.split("\r\n")
        .filter_map(|line| {
            let mut parts = line.splitn(2, ':');
            let k = parts.next()?.trim().to_lowercase();
            let v = parts.next()?.trim().to_string();
            if k.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

// ─── Model id translation ──────────────────────────────────────────────────

/// Canonicalize known cursor composer spelling variants. Other ids pass through
/// untouched (case preserved).
pub fn normalize_cursor_model_id(model: &str) -> String {
    let id = model.trim();
    let alias = match id.to_ascii_lowercase().as_str() {
        "" => Some("composer-2.5"),
        "composer-2-5" => Some("composer-2.5"),
        "composer-2.5-sdk" => Some("composer-2.5"),
        "composer-latest" => Some("composer-2.5"),
        "composer-2-5-fast" => Some("composer-2.5-fast"),
        "composer-2.5-sdk-fast" => Some("composer-2.5-fast"),
        "composer-latest-fast" => Some("composer-2.5-fast"),
        _ => None,
    };
    alias.map(str::to_string).unwrap_or_else(|| id.to_string())
}

pub struct ResolvedModel {
    pub model_id: String,
    pub parameters: Vec<(String, String)>,
}

/// cursor-agent rewrites a few model ids before putting them on the wire:
///   "auto"            → RequestedModel { model_id: "default" }
///   "composer-*-fast" → RequestedModel { model_id: "composer-*",
///                                        parameters: [{id: "fast", value: "true"}] }
pub fn resolve_requested_model(model: &str) -> ResolvedModel {
    let normalized = normalize_cursor_model_id(model);
    if normalized == "auto" {
        return ResolvedModel {
            model_id: "default".to_string(),
            parameters: Vec::new(),
        };
    }
    if let Some(stripped) = normalized
        .strip_suffix("-fast")
        .filter(|_| normalized.starts_with("composer-"))
    {
        return ResolvedModel {
            model_id: stripped.to_string(),
            parameters: vec![("fast".to_string(), "true".to_string())],
        };
    }
    ResolvedModel {
        model_id: normalized,
        parameters: Vec::new(),
    }
}

// ─── google.protobuf.Value ─────────────────────────────────────────────────

pub fn json_to_value_bytes(value: &Value) -> Bytes {
    encode_proto_value(value)
}

fn encode_proto_value(v: &Value) -> Bytes {
    match v {
        Value::Null => {
            let mut out = BytesMut::with_capacity(2);
            put_tag(&mut out, VAL_NULL, WT_VARINT);
            put_varint(&mut out, 0);
            out.freeze()
        }
        Value::Bool(b) => {
            let mut out = BytesMut::with_capacity(2);
            put_tag(&mut out, VAL_BOOL, WT_VARINT);
            put_varint(&mut out, if *b { 1 } else { 0 });
            out.freeze()
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            encode_double(VAL_NUMBER, f)
        }
        Value::String(s) => encode_string(VAL_STRING, s),
        Value::Array(arr) => {
            let list_parts: Vec<Bytes> = arr
                .iter()
                .map(|v| encode_message(LIST_VALUES, &[encode_proto_value(v)]))
                .collect();
            encode_message(VAL_LIST, &list_parts)
        }
        Value::Object(obj) => {
            // Deterministic key order to keep wire output stable.
            let sorted: BTreeMap<&String, &Value> = obj.iter().collect();
            let struct_parts: Vec<Bytes> = sorted
                .iter()
                .map(|(k, v)| {
                    let key = encode_string(MAP_KEY, k);
                    let value = encode_message(MAP_VALUE, &[encode_proto_value(v)]);
                    let entry = concat_bytes(&[key, value]);
                    encode_message(STRUCT_FIELDS, &[entry])
                })
                .collect();
            encode_message(VAL_STRUCT, &struct_parts)
        }
    }
}

pub fn decode_proto_value(src: &[u8]) -> Value {
    let mut pos = 0;
    while pos < src.len() {
        let (tag, np) = match read_varint(src, pos) {
            Ok(v) => v,
            Err(_) => return Value::Null,
        };
        pos = np;
        let field = tag >> 3;
        let wire = (tag & 0x7) as u8;
        match field {
            f if f == VAL_NULL && wire == WT_VARINT => {
                let _ = read_varint(src, pos);
                return Value::Null;
            }
            f if f == VAL_NUMBER && wire == WT_FIXED64 => {
                if pos + 8 > src.len() {
                    return Value::Null;
                }
                let mut b = [0u8; 8];
                b.copy_from_slice(&src[pos..pos + 8]);
                return serde_json::Number::from_f64(f64::from_le_bytes(b))
                    .map(Value::Number)
                    .unwrap_or(Value::Null);
            }
            f if f == VAL_STRING && wire == WT_LEN => {
                let (len, np2) = match read_varint(src, pos) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                let n = match checked_len(len, np2, src) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                return Value::String(String::from_utf8_lossy(&src[np2..np2 + n]).into_owned());
            }
            f if f == VAL_BOOL && wire == WT_VARINT => {
                let (v, _) = read_varint(src, pos).unwrap_or((0, pos));
                return Value::Bool(v != 0);
            }
            f if f == VAL_STRUCT && wire == WT_LEN => {
                let (len, np2) = match read_varint(src, pos) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                let n = match checked_len(len, np2, src) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                return Value::Object(decode_proto_struct(&src[np2..np2 + n]));
            }
            f if f == VAL_LIST && wire == WT_LEN => {
                let (len, np2) = match read_varint(src, pos) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                let n = match checked_len(len, np2, src) {
                    Ok(v) => v,
                    Err(_) => return Value::Null,
                };
                return Value::Array(decode_proto_list(&src[np2..np2 + n]));
            }
            _ => {
                // skip unknown
                match wire {
                    WT_VARINT => {
                        let (_, np2) = read_varint(src, pos).unwrap_or((0, pos));
                        pos = np2;
                    }
                    WT_LEN => {
                        let (len, np2) = read_varint(src, pos).unwrap_or((0, pos));
                        pos = np2 + len as usize;
                    }
                    WT_FIXED64 => pos += 8,
                    WT_FIXED32 => pos += 4,
                    _ => return Value::Null,
                }
            }
        }
    }
    Value::Null
}

fn decode_proto_struct(src: &[u8]) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for f in FieldIter::new(src).flatten() {
        if f.field == STRUCT_FIELDS {
            if let FieldValue::Bytes(entry) = f.value {
                let mut key = String::new();
                let mut value_bytes: Option<&[u8]> = None;
                for ef in FieldIter::new(entry).flatten() {
                    match (ef.field, ef.value) {
                        (k, FieldValue::Bytes(b)) if k == MAP_KEY => {
                            key = String::from_utf8_lossy(b).into_owned();
                        }
                        (k, FieldValue::Bytes(b)) if k == MAP_VALUE => {
                            value_bytes = Some(b);
                        }
                        _ => {}
                    }
                }
                if let Some(vb) = value_bytes {
                    if !key.is_empty() {
                        out.insert(key, decode_proto_value(vb));
                    }
                }
            }
        }
    }
    out
}

fn decode_proto_list(src: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    for f in FieldIter::new(src).flatten() {
        if f.field == LIST_VALUES {
            if let FieldValue::Bytes(b) = f.value {
                out.push(decode_proto_value(b));
            }
        }
    }
    out
}

// ─── McpToolDefinition ─────────────────────────────────────────────────────

#[derive(Debug)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Bytes,
    pub provider_identifier: String,
    pub tool_name: String,
}

impl Clone for McpToolDef {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            provider_identifier: self.provider_identifier.clone(),
            tool_name: self.tool_name.clone(),
        }
    }
}

pub fn encode_mcp_tool_def_body(def: &McpToolDef) -> Bytes {
    let mut parts = vec![
        encode_string(MTD_NAME, &def.name),
        encode_string(MTD_DESCRIPTION, &def.description),
        encode_bytes(MTD_INPUT_SCHEMA, &def.input_schema),
    ];
    if !def.provider_identifier.is_empty() {
        parts.push(encode_string(
            MTD_PROVIDER_IDENTIFIER,
            &def.provider_identifier,
        ));
    }
    if !def.tool_name.is_empty() {
        parts.push(encode_string(MTD_TOOL_NAME, &def.tool_name));
    }
    concat_bytes(&parts)
}

/// Convert OpenAI-style tool entries to cursor `McpToolDef`. Skips entries
/// that don't have a function-shape body.
pub fn openai_tools_to_mcp_defs(tools: &Value) -> Vec<McpToolDef> {
    let arr = match tools.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for t in arr {
        // Support both OpenAI Chat ({type:"function", function:{...}}) and
        // OpenAI Responses ({type:"function", name, parameters, description}).
        let (name, description, parameters) = if let Some(func) = t.get("function") {
            (
                func.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                func.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                func.get("parameters").cloned(),
            )
        } else {
            (
                t.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                t.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                t.get("parameters")
                    .or_else(|| t.get("input_schema"))
                    .cloned(),
            )
        };
        if name.is_empty() {
            continue;
        }
        let schema =
            parameters.unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
        out.push(McpToolDef {
            name: name.clone(),
            description,
            input_schema: json_to_value_bytes(&schema),
            provider_identifier: "cc-switch".to_string(),
            tool_name: name,
        });
    }
    out
}

/// Anthropic-style tool list (`{name, description, input_schema}`).
pub fn anthropic_tools_to_mcp_defs(tools: &Value) -> Vec<McpToolDef> {
    let arr = match tools.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for t in arr {
        let name = t
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let description = t
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let schema = t
            .get("input_schema")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
        out.push(McpToolDef {
            name: name.clone(),
            description,
            input_schema: json_to_value_bytes(&schema),
            provider_identifier: "cc-switch".to_string(),
            tool_name: name,
        });
    }
    out
}

// ─── RunRequest builder ────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct EncodedImage {
    pub data: Bytes,
    pub mime_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub uuid: String,
}

pub struct AgentRunInput<'a> {
    pub model_id: &'a str,
    pub user_text: &'a str,
    pub conversation_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub tools: Vec<McpToolDef>,
    pub system_prompt: Option<&'a str>,
    pub blob_store: Option<&'a mut std::collections::HashMap<String, Bytes>>,
    pub images: Vec<EncodedImage>,
}

fn encode_selected_image_body(img: &EncodedImage) -> Bytes {
    let mut parts = vec![encode_string(SI_UUID, &img.uuid)];
    if let (Some(w), Some(h)) = (img.width, img.height) {
        if w > 0 && h > 0 {
            parts.push(encode_message(
                SI_DIMENSION,
                &[
                    encode_uint32(DIM_WIDTH, w as u64),
                    encode_uint32(DIM_HEIGHT, h as u64),
                ],
            ));
        }
    }
    if let Some(mime) = &img.mime_type {
        parts.push(encode_string(SI_MIME_TYPE, mime));
    }
    parts.push(encode_bytes(SI_DATA, &img.data));
    concat_bytes(&parts)
}

/// Encode a full `AgentClientMessage { run_request: ... }`. Caller must wrap
/// the returned payload in a Connect-RPC frame before writing to the h2 stream.
pub fn encode_agent_run_request(input: &mut AgentRunInput<'_>) -> Bytes {
    let conversation_id = input
        .conversation_id
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(random_uuid_like);
    let message_id = input
        .message_id
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(random_uuid_like);

    let resolved = resolve_requested_model(input.model_id);

    // UserMessage { text, message_id, selected_context, mode=1 }
    let mut selected_context_parts: Vec<Bytes> = Vec::new();
    for img in &input.images {
        selected_context_parts.push(encode_message(
            SC_SELECTED_IMAGES,
            &[encode_selected_image_body(img)],
        ));
    }
    let user_message = encode_message(
        UMA_USER_MESSAGE,
        &[
            encode_string(UM_TEXT, input.user_text),
            encode_string(UM_MESSAGE_ID, &message_id),
            encode_message(UM_SELECTED_CONTEXT, &selected_context_parts),
            encode_uint32(UM_MODE, 1),
        ],
    );
    let user_message_action = encode_message(CA_USER_MESSAGE_ACTION, &[user_message]);
    let action = encode_message(ARR_ACTION, &[user_message_action]);

    // ConversationStateStructure — optional system blob.
    let mut css_parts: Vec<Bytes> = Vec::new();
    if let (Some(sys), Some(store)) = (input.system_prompt, input.blob_store.as_deref_mut()) {
        let system_json = serde_json::json!({ "role": "system", "content": sys }).to_string();
        let bytes = Bytes::from(system_json.into_bytes());
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let blob_id_raw = hasher.finalize();
        let hex_key = hex_lower(&blob_id_raw);
        store.insert(hex_key, bytes);
        css_parts.push(encode_bytes(CSS_ROOT_PROMPT, &blob_id_raw));
    }
    let conversation_state = encode_message(ARR_CONVERSATION_STATE, &css_parts);

    // RequestedModel { model_id, [parameters...] }
    let mut rm_parts = vec![encode_string(RM_MODEL_ID, &resolved.model_id)];
    for (id, value) in &resolved.parameters {
        rm_parts.push(encode_message(
            RM_PARAMETERS,
            &[encode_string(RMP_ID, id), encode_string(RMP_VALUE, value)],
        ));
    }
    let requested_model = encode_message(ARR_REQUESTED_MODEL, &rm_parts);

    // ModelDetails (msg 88) — same model id repeated three ways (cursor-agent
    // requires this envelope to resolve pinned Claude/GPT thinking variants).
    let model_details = encode_message(
        ARR_MODEL_DETAILS,
        &[
            encode_string(MD_MODEL_ID, &resolved.model_id),
            encode_string(MD_DISPLAY_MODEL_ID, &resolved.model_id),
            encode_string(MD_DISPLAY_NAME, &resolved.model_id),
        ],
    );

    // mcp_tools envelope — observably required even when empty.
    let tool_parts: Vec<Bytes> = input
        .tools
        .iter()
        .map(|def| encode_message(ARR_MCP_TOOLS_INNER, &[encode_mcp_tool_def_body(def)]))
        .collect();
    let mcp_tools_block = encode_message(ARR_MCP_TOOLS, &tool_parts);

    // AgentRunRequest body in field-number order.
    encode_message(
        ACM_RUN_REQUEST,
        &[
            conversation_state,
            action,
            model_details,
            mcp_tools_block,
            encode_string(ARR_CONVERSATION_ID, &conversation_id),
            requested_model,
            encode_uint32(ARR_UNKNOWN_12, 0),
            encode_string(ARR_REQUEST_ID, &conversation_id),
        ],
    )
}

// ─── ExecClient encoders ───────────────────────────────────────────────────

fn wrap_exec_client_message(
    exec_msg_id: u64,
    exec_id: &str,
    result_field: u64,
    result_body: Bytes,
) -> Bytes {
    let ecm = encode_message(
        ACM_EXEC_CLIENT_MESSAGE,
        &[
            encode_uint32(ECM_ID, exec_msg_id),
            encode_string(ECM_EXEC_ID, exec_id),
            encode_message(result_field, &[result_body]),
        ],
    );
    wrap_connect_frame(&ecm)
}

/// Encode a `RequestContextResult` ack for Cursor's AgentService handshake.
///
/// **Production must pass an empty `tools` slice.** Tools are already declared in
/// the initial `AgentRunRequest.mcp_tools` envelope; re-sending them here causes
/// Cursor's upstream to stall silently (OmniRoute `cursor.ts` uses an empty ack
/// at runtime). The `tools` parameter is retained for protobuf round-trip tests.
pub fn encode_request_context_response(
    exec_msg_id: u64,
    exec_id: &str,
    tools: &[McpToolDef],
) -> Bytes {
    let mut rc_parts: Vec<Bytes> = Vec::new();
    for t in tools {
        rc_parts.push(encode_message(RCS_TOOLS, &[encode_mcp_tool_def_body(t)]));
    }
    let request_context = encode_message(RCS_REQUEST_CONTEXT, &rc_parts);
    let success = encode_message(RCR_SUCCESS, &[request_context]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_REQUEST_CONTEXT_RESULT, success)
}

fn normalize_working_directory(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("undefined")
        || trimmed.eq_ignore_ascii_case("null")
    {
        "."
    } else {
        trimmed
    }
}

/// Rich RequestContext ack with environment + capability flags (composer-api SDK).
/// Tools are **not** re-sent — only env metadata.
pub fn encode_rich_request_context_response(
    exec_msg_id: u64,
    exec_id: &str,
    working_directory: &str,
) -> Bytes {
    let wd = normalize_working_directory(working_directory);
    let env_inner = concat_bytes(&[
        encode_string(RCE_HOSTNAME, "cc-switch"),
        encode_string(RCE_WORKING_DIR, wd),
        encode_string(RCE_SHELL, "sh"),
        encode_bool(RCE_FLAG_5, false),
        encode_string(RCE_TIMEZONE, "UTC"),
        encode_string(RCE_CWD_ALT, wd),
        encode_string(RCE_CWD_ALT2, wd),
    ]);
    let request_context = encode_message(
        RCS_REQUEST_CONTEXT,
        &[
            encode_message(RCS_ENV, &[env_inner]),
            encode_bool(17, false),
            encode_bool(24, false),
            encode_bool(32, true),
            encode_bool(33, true),
            encode_bool(35, false),
            encode_bool(36, true),
            encode_bool(39, true),
            encode_bool(40, true),
            encode_bool(41, true),
            encode_bool(42, true),
            encode_bool(43, true),
            encode_bool(44, true),
            encode_bool(45, true),
        ],
    );
    let success = encode_message(RCR_SUCCESS, &[request_context]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_REQUEST_CONTEXT_RESULT, success)
}

fn encode_path_rejection(path: &str, reason: &str) -> Bytes {
    concat_bytes(&[
        encode_string(REJ_PATH, path),
        encode_string(REJ_REASON, reason),
    ])
}

fn encode_shell_rejection(command: &str, working_dir: &str, reason: &str) -> Bytes {
    concat_bytes(&[
        encode_string(SREJ_COMMAND, command),
        encode_string(SREJ_WORKING_DIR, working_dir),
        encode_string(SREJ_REASON, reason),
    ])
}

pub fn encode_exec_read_rejected(
    exec_msg_id: u64,
    exec_id: &str,
    path: &str,
    reason: &str,
) -> Bytes {
    let r = encode_message(RES_REJECTED, &[encode_path_rejection(path, reason)]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_READ_RESULT, r)
}

pub fn encode_exec_write_rejected(
    exec_msg_id: u64,
    exec_id: &str,
    path: &str,
    reason: &str,
) -> Bytes {
    let r = encode_message(RES_REJECTED, &[encode_path_rejection(path, reason)]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_WRITE_RESULT, r)
}

pub fn encode_exec_delete_rejected(
    exec_msg_id: u64,
    exec_id: &str,
    path: &str,
    reason: &str,
) -> Bytes {
    let r = encode_message(RES_REJECTED, &[encode_path_rejection(path, reason)]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_DELETE_RESULT, r)
}

pub fn encode_exec_ls_rejected(exec_msg_id: u64, exec_id: &str, path: &str, reason: &str) -> Bytes {
    let r = encode_message(RES_REJECTED, &[encode_path_rejection(path, reason)]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_LS_RESULT, r)
}

pub fn encode_exec_shell_rejected(
    exec_msg_id: u64,
    exec_id: &str,
    command: &str,
    working_dir: &str,
    reason: &str,
) -> Bytes {
    let r = encode_message(
        RES_REJECTED,
        &[encode_shell_rejection(command, working_dir, reason)],
    );
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_SHELL_RESULT, r)
}

pub fn encode_exec_background_shell_rejected(
    exec_msg_id: u64,
    exec_id: &str,
    command: &str,
    working_dir: &str,
    reason: &str,
) -> Bytes {
    let r = encode_message(
        RES_REJECTED,
        &[encode_shell_rejection(command, working_dir, reason)],
    );
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_BACKGROUND_SHELL_SPAWN_RES, r)
}

pub fn encode_exec_grep_error(exec_msg_id: u64, exec_id: &str, msg: &str) -> Bytes {
    let err = encode_string(ERR_MESSAGE, msg);
    let v = encode_message(RES_REJECTED, &[err]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_GREP_RESULT, v)
}

pub fn encode_exec_fetch_error(exec_msg_id: u64, exec_id: &str, url: &str, msg: &str) -> Bytes {
    let body = concat_bytes(&[encode_string(FERR_URL, url), encode_string(FERR_ERROR, msg)]);
    let v = encode_message(RES_REJECTED, &[body]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_FETCH_RESULT, v)
}

pub fn encode_exec_write_shell_stdin_error(exec_msg_id: u64, exec_id: &str, msg: &str) -> Bytes {
    let err = encode_string(ERR_MESSAGE, msg);
    let v = encode_message(RES_REJECTED, &[err]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_WRITE_SHELL_STDIN_RESULT, v)
}

pub fn encode_exec_diagnostics_result(exec_msg_id: u64, exec_id: &str) -> Bytes {
    // DiagnosticsResult is intentionally empty.
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_DIAGNOSTICS_RESULT, Bytes::new())
}

pub fn encode_exec_mcp_result(
    exec_msg_id: u64,
    exec_id: &str,
    content: &str,
    is_error: bool,
) -> Bytes {
    let text_content = encode_message(MCC_TEXT, &[encode_string(MTC_TEXT, content)]);
    let mut success_fields = vec![encode_message(MCS_CONTENT, &[text_content])];
    if is_error {
        success_fields.push(encode_bool(MCS_IS_ERROR, true));
    }
    let success = encode_message(MCR_SUCCESS, &success_fields);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_MCP_RESULT, success)
}

pub fn encode_exec_mcp_error(exec_msg_id: u64, exec_id: &str, msg: &str) -> Bytes {
    let err = encode_string(ERR_MESSAGE, msg);
    let v = encode_message(MCR_ERROR, &[err]);
    wrap_exec_client_message(exec_msg_id, exec_id, ECM_MCP_RESULT, v)
}

// ─── KV channel encoders ───────────────────────────────────────────────────

pub fn encode_kv_get_blob_result(
    kv_id: u64,
    blob_data: &[u8],
    request_metadata: Option<&[u8]>,
) -> Bytes {
    let result = encode_bytes(GBR_BLOB_DATA, blob_data);
    let mut parts: Vec<Bytes> = Vec::new();
    if kv_id != 0 {
        parts.push(encode_uint32(KCM_ID, kv_id));
    }
    parts.push(encode_message(KCM_GET_BLOB_RESULT, &[result]));
    if let Some(md) = request_metadata {
        if !md.is_empty() {
            parts.push(encode_bytes(KCM_REQUEST_METADATA, md));
        }
    }
    let kcm = encode_message(ACM_KV_CLIENT_MESSAGE, &parts);
    wrap_connect_frame(&kcm)
}

pub fn encode_kv_set_blob_result(kv_id: u64, request_metadata: Option<&[u8]>) -> Bytes {
    let mut parts: Vec<Bytes> = Vec::new();
    if kv_id != 0 {
        parts.push(encode_uint32(KCM_ID, kv_id));
    }
    parts.push(encode_message(KCM_SET_BLOB_RESULT, &[]));
    if let Some(md) = request_metadata {
        if !md.is_empty() {
            parts.push(encode_bytes(KCM_REQUEST_METADATA, md));
        }
    }
    let kcm = encode_message(ACM_KV_CLIENT_MESSAGE, &parts);
    wrap_connect_frame(&kcm)
}

// ─── AgentServerMessage decoder ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum InteractionDelta {
    Text(String),
    Thinking(String),
    ThinkingComplete,
    TokenDelta(u64),
    Heartbeat,
    TurnEnded,
    ToolCallStarted,
    ToolCallCompleted,
    KvServerMessage,
    Unknown(u64),
}

pub fn decode_agent_server_message(payload: &[u8]) -> Vec<InteractionDelta> {
    let mut out = Vec::new();
    for top in FieldIter::new(payload).flatten() {
        match (top.field, &top.value) {
            (f, FieldValue::Bytes(_)) if f == ASM_KV_SERVER_MESSAGE => {
                out.push(InteractionDelta::KvServerMessage);
            }
            (f, FieldValue::Bytes(bytes)) if f == ASM_INTERACTION_UPDATE => {
                for u in FieldIter::new(bytes).flatten() {
                    match u.field {
                        x if x == IU_TEXT_DELTA => {
                            if let FieldValue::Bytes(b) = u.value {
                                out.push(InteractionDelta::Text(decode_string_field(b, TDU_TEXT)));
                            }
                        }
                        x if x == IU_THINKING_DELTA => {
                            if let FieldValue::Bytes(b) = u.value {
                                out.push(InteractionDelta::Thinking(decode_string_field(
                                    b, TDU_TEXT,
                                )));
                            }
                        }
                        x if x == IU_THINKING_COMPLETED => {
                            out.push(InteractionDelta::ThinkingComplete)
                        }
                        x if x == IU_TOOL_CALL_STARTED => {
                            out.push(InteractionDelta::ToolCallStarted)
                        }
                        x if x == IU_TOOL_CALL_COMPLETED => {
                            out.push(InteractionDelta::ToolCallCompleted)
                        }
                        x if x == IU_TOKEN_DELTA => {
                            if let FieldValue::Bytes(b) = u.value {
                                let tokens = find_varint_field(b, 1).unwrap_or(0);
                                out.push(InteractionDelta::TokenDelta(tokens));
                            }
                        }
                        x if x == IU_HEARTBEAT => out.push(InteractionDelta::Heartbeat),
                        x if x == IU_TURN_ENDED => out.push(InteractionDelta::TurnEnded),
                        other => out.push(InteractionDelta::Unknown(other)),
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ─── KvServer / ExecServer decoder ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum KvServerEvent {
    GetBlob {
        kv_id: u64,
        blob_id: Bytes,
        request_metadata: Option<Bytes>,
    },
    SetBlob {
        kv_id: u64,
        blob_id: Bytes,
        blob_data: Bytes,
        request_metadata: Option<Bytes>,
    },
}

pub fn decode_kv_server_event(payload: &[u8]) -> Option<KvServerEvent> {
    for top in FieldIter::new(payload).flatten() {
        if top.field != ASM_KV_SERVER_MESSAGE {
            continue;
        }
        let inner = match top.value {
            FieldValue::Bytes(b) => b,
            _ => continue,
        };
        let mut kv_id = 0u64;
        let mut get_args: Option<&[u8]> = None;
        let mut set_args: Option<&[u8]> = None;
        let mut req_meta: Option<Bytes> = None;
        for f in FieldIter::new(inner).flatten() {
            match (f.field, f.value) {
                (k, FieldValue::Varint(v)) if k == KSM_ID => kv_id = v,
                (k, FieldValue::Bytes(b)) if k == KSM_GET_BLOB_ARGS => get_args = Some(b),
                (k, FieldValue::Bytes(b)) if k == KSM_SET_BLOB_ARGS => set_args = Some(b),
                (k, FieldValue::Bytes(b)) if k == KSM_REQUEST_METADATA => {
                    req_meta = Some(Bytes::copy_from_slice(b))
                }
                _ => {}
            }
        }
        if let Some(b) = get_args {
            let blob_id = find_bytes_field(b, GBA_BLOB_ID)
                .map(Bytes::copy_from_slice)
                .unwrap_or_default();
            return Some(KvServerEvent::GetBlob {
                kv_id,
                blob_id,
                request_metadata: req_meta,
            });
        }
        if let Some(b) = set_args {
            let blob_id = find_bytes_field(b, SBA_BLOB_ID)
                .map(Bytes::copy_from_slice)
                .unwrap_or_default();
            let blob_data = find_bytes_field(b, SBA_BLOB_DATA)
                .map(Bytes::copy_from_slice)
                .unwrap_or_default();
            return Some(KvServerEvent::SetBlob {
                kv_id,
                blob_id,
                blob_data,
                request_metadata: req_meta,
            });
        }
    }
    None
}

#[derive(Debug, Clone)]
pub enum ExecServerEvent {
    RequestContext {
        exec_msg_id: u64,
        exec_id: String,
    },
    Read {
        exec_msg_id: u64,
        exec_id: String,
        path: String,
        tool_call_id: String,
        offset: Option<u64>,
        limit: Option<u64>,
    },
    Write {
        exec_msg_id: u64,
        exec_id: String,
        path: String,
        file_text: String,
        stream_content: String,
        tool_call_id: String,
    },
    Delete {
        exec_msg_id: u64,
        exec_id: String,
        path: String,
    },
    Ls {
        exec_msg_id: u64,
        exec_id: String,
        path: String,
    },
    Grep {
        exec_msg_id: u64,
        exec_id: String,
        pattern: String,
        path: String,
        glob: String,
        output_mode: String,
        case_insensitive: bool,
        head_limit: Option<u64>,
    },
    Diagnostics {
        exec_msg_id: u64,
        exec_id: String,
    },
    Shell {
        exec_msg_id: u64,
        exec_id: String,
        command: String,
        working_dir: String,
    },
    ShellStream {
        exec_msg_id: u64,
        exec_id: String,
        command: String,
        working_dir: String,
    },
    BackgroundShell {
        exec_msg_id: u64,
        exec_id: String,
        command: String,
        working_dir: String,
    },
    Fetch {
        exec_msg_id: u64,
        exec_id: String,
        url: String,
    },
    WriteShellStdin {
        exec_msg_id: u64,
        exec_id: String,
    },
    Mcp {
        exec_msg_id: u64,
        exec_id: String,
        tool_name: String,
        tool_call_id: String,
        args: Value,
    },
}

impl ExecServerEvent {
    /// Dedup key for exec server events. `request_context` and `mcp` may share an
    /// empty `exec_id` in the current Cursor schema, so kind + ids are required.
    pub fn dedup_key(&self) -> String {
        let (kind, exec_msg_id, exec_id) = match self {
            ExecServerEvent::RequestContext {
                exec_msg_id,
                exec_id,
            } => ("request_context", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Read {
                exec_msg_id,
                exec_id,
                ..
            } => ("read", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Write {
                exec_msg_id,
                exec_id,
                ..
            } => ("write", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Delete {
                exec_msg_id,
                exec_id,
                ..
            } => ("delete", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Ls {
                exec_msg_id,
                exec_id,
                ..
            } => ("ls", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Grep {
                exec_msg_id,
                exec_id,
                ..
            } => ("grep", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Diagnostics {
                exec_msg_id,
                exec_id,
                ..
            } => ("diagnostics", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Shell {
                exec_msg_id,
                exec_id,
                ..
            }
            | ExecServerEvent::ShellStream {
                exec_msg_id,
                exec_id,
                ..
            } => ("shell", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::BackgroundShell {
                exec_msg_id,
                exec_id,
                ..
            } => ("background_shell", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Fetch {
                exec_msg_id,
                exec_id,
                ..
            } => ("fetch", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::WriteShellStdin {
                exec_msg_id,
                exec_id,
            } => ("write_shell_stdin", *exec_msg_id, exec_id.as_str()),
            ExecServerEvent::Mcp {
                exec_msg_id,
                exec_id,
                tool_name,
                tool_call_id,
                ..
            } => {
                return format!("mcp:{exec_id}:{exec_msg_id}:{tool_name}:{tool_call_id}");
            }
        };
        format!("{kind}:{exec_id}:{exec_msg_id}")
    }
}

pub fn decode_exec_server_event(payload: &[u8]) -> Option<ExecServerEvent> {
    for top in FieldIter::new(payload).flatten() {
        if top.field != ASM_EXEC_SERVER_MESSAGE {
            continue;
        }
        let inner = match top.value {
            FieldValue::Bytes(b) => b,
            _ => continue,
        };
        let mut exec_msg_id = 0u64;
        let mut exec_id = String::new();
        let mut variant_field: u64 = 0;
        let mut variant_bytes: Option<&[u8]> = None;
        for f in FieldIter::new(inner).flatten() {
            match (f.field, f.value) {
                (k, FieldValue::Varint(v)) if k == ESM_ID => exec_msg_id = v,
                (k, FieldValue::Bytes(b)) if k == ESM_EXEC_ID => {
                    exec_id = String::from_utf8_lossy(b).into_owned();
                }
                (_, FieldValue::Bytes(b)) if variant_field == 0 => {
                    variant_field = f.field;
                    variant_bytes = Some(b);
                }
                _ => {}
            }
        }
        let body = variant_bytes?;
        return Some(match variant_field {
            x if x == ESM_REQUEST_CONTEXT_ARGS => ExecServerEvent::RequestContext {
                exec_msg_id,
                exec_id,
            },
            x if x == ESM_READ_ARGS => ExecServerEvent::Read {
                exec_msg_id,
                exec_id,
                path: decode_string_field(body, ARG_PATH),
                tool_call_id: decode_string_field(body, ARG_READ_TOOL_CALL_ID),
                offset: decode_varint_field(body, ARG_READ_OFFSET),
                limit: decode_varint_field(body, ARG_READ_LIMIT),
            },
            x if x == ESM_WRITE_ARGS => ExecServerEvent::Write {
                exec_msg_id,
                exec_id,
                path: decode_string_field(body, ARG_PATH),
                file_text: decode_string_field(body, ARG_WRITE_FILE_TEXT),
                stream_content: decode_string_field(body, ARG_WRITE_STREAM_CONTENT),
                tool_call_id: decode_string_field(body, ARG_WRITE_TOOL_CALL_ID),
            },
            x if x == ESM_DELETE_ARGS => ExecServerEvent::Delete {
                exec_msg_id,
                exec_id,
                path: decode_string_field(body, ARG_PATH),
            },
            x if x == ESM_LS_ARGS => ExecServerEvent::Ls {
                exec_msg_id,
                exec_id,
                path: decode_string_field(body, ARG_PATH),
            },
            x if x == ESM_GREP_ARGS => {
                let case_insensitive = decode_varint_field(body, 8).unwrap_or(0) != 0;
                let head_limit = decode_varint_field(body, 10);
                ExecServerEvent::Grep {
                    exec_msg_id,
                    exec_id,
                    pattern: decode_string_field(body, 1),
                    path: decode_string_field(body, 2),
                    glob: decode_string_field(body, 3),
                    output_mode: decode_string_field(body, 4),
                    case_insensitive,
                    head_limit,
                }
            }
            x if x == ESM_DIAGNOSTICS_ARGS => ExecServerEvent::Diagnostics {
                exec_msg_id,
                exec_id,
            },
            x if x == ESM_SHELL_ARGS => ExecServerEvent::Shell {
                exec_msg_id,
                exec_id,
                command: decode_string_field(body, ARG_SHELL_COMMAND),
                working_dir: decode_string_field(body, ARG_SHELL_WORKING_DIR),
            },
            x if x == ESM_SHELL_STREAM_ARGS => ExecServerEvent::ShellStream {
                exec_msg_id,
                exec_id,
                command: decode_string_field(body, ARG_SHELL_COMMAND),
                working_dir: decode_string_field(body, ARG_SHELL_WORKING_DIR),
            },
            x if x == ESM_BACKGROUND_SHELL_SPAWN => ExecServerEvent::BackgroundShell {
                exec_msg_id,
                exec_id,
                command: decode_string_field(body, ARG_SHELL_COMMAND),
                working_dir: decode_string_field(body, ARG_SHELL_WORKING_DIR),
            },
            x if x == ESM_FETCH_ARGS => ExecServerEvent::Fetch {
                exec_msg_id,
                exec_id,
                url: decode_string_field(body, ARG_FETCH_URL),
            },
            x if x == ESM_WRITE_SHELL_STDIN_ARGS => ExecServerEvent::WriteShellStdin {
                exec_msg_id,
                exec_id,
            },
            x if x == ESM_MCP_ARGS => {
                let mut tool_name = String::new();
                let mut tool_call_id = String::new();
                for f in FieldIter::new(body).flatten() {
                    match (f.field, f.value) {
                        (k, FieldValue::Bytes(b)) if k == MCA_TOOL_NAME => {
                            tool_name = String::from_utf8_lossy(b).into_owned();
                        }
                        (k, FieldValue::Bytes(b)) if k == MCA_NAME && tool_name.is_empty() => {
                            tool_name = String::from_utf8_lossy(b).into_owned();
                        }
                        (k, FieldValue::Bytes(b)) if k == MCA_TOOL_CALL_ID => {
                            tool_call_id = String::from_utf8_lossy(b).into_owned();
                        }
                        _ => {}
                    }
                }
                let args = decode_mcp_args_map(body);
                ExecServerEvent::Mcp {
                    exec_msg_id,
                    exec_id,
                    tool_name,
                    tool_call_id,
                    args: Value::Object(args),
                }
            }
            _ => continue,
        });
    }
    None
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 0x7F, 0x80, 0x3FFF, 0x4000, u64::MAX / 2] {
            let mut buf = BytesMut::new();
            put_varint(&mut buf, v);
            let (decoded, _) = read_varint(&buf, 0).unwrap();
            assert_eq!(decoded, v);
        }
    }

    #[test]
    fn connect_frame_roundtrip() {
        let payload = b"hello-cursor".to_vec();
        let frame = wrap_connect_frame(&payload);
        let mut parser = ConnectFrameParser::new();
        let frames = parser.feed(&frame).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0);
        assert_eq!(&frames[0].payload[..], &payload[..]);
    }

    #[test]
    fn fragmented_frame_completes_after_feed() {
        let payload = b"streamed".to_vec();
        let frame = wrap_connect_frame(&payload);
        let mut parser = ConnectFrameParser::new();
        let first = parser.feed(&frame[..3]).unwrap();
        assert!(first.is_empty());
        let second = parser.feed(&frame[3..]).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(&second[0].payload[..], b"streamed");
    }

    #[test]
    fn resolve_known_aliases() {
        assert_eq!(normalize_cursor_model_id("auto"), "auto");
        assert_eq!(normalize_cursor_model_id("composer-2-5"), "composer-2.5");
        let r = resolve_requested_model("auto");
        assert_eq!(r.model_id, "default");
        let r = resolve_requested_model("composer-2.5-fast");
        assert_eq!(r.model_id, "composer-2.5");
        assert_eq!(r.parameters, vec![("fast".to_string(), "true".to_string())]);
    }

    #[test]
    fn proto_value_roundtrip() {
        let v = serde_json::json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" },
                "n": { "type": "number" }
            },
            "required": ["x"],
            "extra": true,
            "missing": null,
            "nums": [1, 2, 3]
        });
        let bytes = json_to_value_bytes(&v);
        let back = decode_proto_value(&bytes);
        // Compare as JSON (numbers reconstructed via f64).
        let original = serde_json::to_string(&v).unwrap();
        let recovered = serde_json::to_string(&back).unwrap();
        // Order of keys may differ; do field-by-field set comparison.
        let original_v: Value = serde_json::from_str(&original).unwrap();
        let recovered_v: Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(
            original_v.get("required"),
            recovered_v.get("required"),
            "required mismatch: {} vs {}",
            original,
            recovered
        );
        assert_eq!(original_v.get("extra"), recovered_v.get("extra"));
        assert_eq!(original_v.get("missing"), recovered_v.get("missing"));
    }

    #[test]
    fn agent_run_request_builds() {
        let mut input = AgentRunInput {
            model_id: "claude-4.6-sonnet-medium",
            user_text: "hi there",
            conversation_id: Some("conv-123"),
            message_id: Some("msg-456"),
            tools: Vec::new(),
            system_prompt: None,
            blob_store: None,
            images: Vec::new(),
        };
        let body = encode_agent_run_request(&mut input);
        assert!(body.len() > 30, "encoded body too short");
        // Top-level must contain ACM_RUN_REQUEST (field 1, wire LEN).
        let first = FieldIter::new(&body)
            .next()
            .expect("at least one field")
            .unwrap();
        assert_eq!(first.field, ACM_RUN_REQUEST);
    }

    #[test]
    fn round_trip_exec_request_context() {
        let frame = encode_request_context_response(7, "exec-x", &[]);
        let mut parser = ConnectFrameParser::new();
        let frames = parser.feed(&frame).unwrap();
        assert_eq!(frames.len(), 1);
        // The body is an AgentClientMessage with exec_client_message (2) set.
        let first = FieldIter::new(&frames[0].payload).next().unwrap().unwrap();
        assert_eq!(first.field, ACM_EXEC_CLIENT_MESSAGE);
    }

    #[test]
    fn request_context_empty_ack_smaller_than_tools_ack() {
        let tool = McpToolDef {
            name: "Bash".to_string(),
            description: "run bash".to_string(),
            input_schema: Bytes::from_static(br#"{"type":"object"}"#),
            provider_identifier: "cc-switch".to_string(),
            tool_name: "Bash".to_string(),
        };
        let empty = encode_request_context_response(1, "exec-a", &[]);
        let with_tools = encode_request_context_response(1, "exec-a", &[tool]);
        assert!(
            empty.len() < with_tools.len(),
            "production empty ack must be smaller than tools-bearing ack"
        );
    }

    #[test]
    fn decode_read_exec_includes_offset_limit() {
        let args = concat_bytes(&[
            encode_string(ARG_PATH, "/src/main.rs"),
            encode_uint32(ARG_READ_OFFSET, 10),
            encode_uint32(ARG_READ_LIMIT, 200),
        ]);
        let esm = concat_bytes(&[
            encode_uint32(ESM_ID, 3),
            encode_string(ESM_EXEC_ID, "exec-r"),
            encode_message(ESM_READ_ARGS, &[args]),
        ]);
        let payload = encode_message(ASM_EXEC_SERVER_MESSAGE, &[esm]);
        let event = decode_exec_server_event(&payload).expect("read");
        match event {
            ExecServerEvent::Read {
                path,
                offset,
                limit,
                ..
            } => {
                assert_eq!(path, "/src/main.rs");
                assert_eq!(offset, Some(10));
                assert_eq!(limit, Some(200));
            }
            other => panic!("expected read, got {other:?}"),
        }
    }

    #[test]
    fn decode_mcp_args_collects_multiple_keys() {
        let entry_a = concat_bytes(&[
            encode_string(MAP_KEY, "path"),
            encode_message(
                MAP_VALUE,
                &[encode_proto_value(&Value::String("/tmp".into()))],
            ),
        ]);
        let entry_b = concat_bytes(&[
            encode_string(MAP_KEY, "pattern"),
            encode_message(
                MAP_VALUE,
                &[encode_proto_value(&Value::String("main".into()))],
            ),
        ]);
        let body = concat_bytes(&[
            encode_string(MCA_TOOL_NAME, "Grep"),
            encode_message(MCA_ARGS, &[entry_a]),
            encode_message(MCA_ARGS, &[entry_b]),
        ]);
        let esm = concat_bytes(&[
            encode_uint32(ESM_ID, 4),
            encode_string(ESM_EXEC_ID, "exec-m"),
            encode_message(ESM_MCP_ARGS, &[body]),
        ]);
        let payload = encode_message(ASM_EXEC_SERVER_MESSAGE, &[esm]);
        let event = decode_exec_server_event(&payload).expect("mcp");
        match event {
            ExecServerEvent::Mcp {
                tool_name, args, ..
            } => {
                assert_eq!(tool_name, "Grep");
                assert_eq!(args.get("path").and_then(Value::as_str), Some("/tmp"));
                assert_eq!(args.get("pattern").and_then(Value::as_str), Some("main"));
            }
            other => panic!("expected mcp, got {other:?}"),
        }
    }

    #[test]
    fn decode_grep_exec_event_fields() {
        let grep_args = concat_bytes(&[
            encode_string(1, "fn main"),
            encode_string(2, "/proj"),
            encode_string(3, "*.rs"),
            encode_uint32(8, 1),
            encode_uint32(10, 50),
        ]);
        let esm = concat_bytes(&[
            encode_uint32(ESM_ID, 9),
            encode_string(ESM_EXEC_ID, "exec-g"),
            encode_message(ESM_GREP_ARGS, &[grep_args]),
        ]);
        let payload = encode_message(ASM_EXEC_SERVER_MESSAGE, &[esm]);
        let event = decode_exec_server_event(&payload).expect("grep event");
        match event {
            ExecServerEvent::Grep {
                pattern,
                path,
                glob,
                case_insensitive,
                head_limit,
                ..
            } => {
                assert_eq!(pattern, "fn main");
                assert_eq!(path, "/proj");
                assert_eq!(glob, "*.rs");
                assert!(case_insensitive);
                assert_eq!(head_limit, Some(50));
            }
            other => panic!("expected grep, got {other:?}"),
        }
    }

    #[test]
    fn exec_dedup_key_distinguishes_request_context_and_mcp() {
        let rc = ExecServerEvent::RequestContext {
            exec_msg_id: 1,
            exec_id: String::new(),
        };
        let mcp = ExecServerEvent::Mcp {
            exec_msg_id: 1,
            exec_id: String::new(),
            tool_name: "Bash".to_string(),
            tool_call_id: "call_1".to_string(),
            args: serde_json::json!({}),
        };
        assert_ne!(rc.dedup_key(), mcp.dedup_key());
    }

    #[test]
    fn exec_dedup_key_is_stable_for_same_event() {
        let a = ExecServerEvent::RequestContext {
            exec_msg_id: 9,
            exec_id: "exec-z".to_string(),
        };
        let b = ExecServerEvent::RequestContext {
            exec_msg_id: 9,
            exec_id: "exec-z".to_string(),
        };
        assert_eq!(a.dedup_key(), b.dedup_key());
    }

    #[test]
    fn anthropic_tool_def() {
        let tools = serde_json::json!([
            { "name": "weather", "description": "wx", "input_schema": { "type": "object" } }
        ]);
        let defs = anthropic_tools_to_mcp_defs(&tools);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "weather");
        assert_eq!(defs[0].description, "wx");
    }

    #[test]
    fn openai_tool_def_supports_responses_shape() {
        let tools = serde_json::json!([
            { "type": "function", "name": "weather", "parameters": { "type": "object" } }
        ]);
        let defs = openai_tools_to_mcp_defs(&tools);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "weather");
    }
}
