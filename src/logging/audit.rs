use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::capture::{append_rotating_line, rotated_path};

const AUDIT_LOG_FILENAME: &str = "audit-events.jsonl";
const AUDIT_CURSOR_FILENAME: &str = "audit-upload-cursor.json";
const AUDIT_LOG_MAX_BYTES: u64 = 16 * 1024 * 1024;
const AUDIT_LOG_BACKUP_COUNT: usize = 7;
const AUDIT_WRITE_QUEUE_CAPACITY: usize = 16_384;
const AUDIT_EVENT_NAME_MAX_BYTES: usize = 128;
const AUDIT_STRING_FIELD_MAX_BYTES: usize = 256;
const AUDIT_MODEL_FIELD_MAX_BYTES: usize = 512;
const AUDIT_EVENT_MAX_BYTES: usize = 32 * 1024;
const AUDIT_WRITE_RETRY_INITIAL: std::time::Duration = std::time::Duration::from_millis(50);
const AUDIT_WRITE_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(1);
const AUDIT_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const AUDIT_UPLOAD_BATCH_LIMIT: usize = 256;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditEvent {
    pub schema_version: u8,
    pub sequence: u64,
    pub timestamp_ms: i64,
    pub boot_id: String,
    pub level: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saw_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saw_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saw_function_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saw_custom_tool_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_conversation_generation: Option<u32>,
}

impl AuditEvent {
    pub fn new(event: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            level: "info".to_string(),
            event: event.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuditRequestDetails {
    pub provider_type: Option<String>,
    pub provider_ref: Option<String>,
    pub account_ref: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub upstream_status: Option<u16>,
    pub first_token_ms: Option<u64>,
    pub streaming: Option<bool>,
    pub stream_status: Option<String>,
    pub attempt: Option<u32>,
    pub retry_count: Option<u32>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl AuditRequestDetails {
    fn merge(&mut self, update: Self) {
        macro_rules! replace_some {
            ($($field:ident),+ $(,)?) => {
                $(if update.$field.is_some() { self.$field = update.$field; })+
            };
        }
        replace_some!(
            provider_type,
            provider_ref,
            account_ref,
            requested_model,
            actual_model,
            upstream_status,
            first_token_ms,
            streaming,
            stream_status,
            attempt,
            retry_count,
            input_tokens,
            output_tokens,
            total_tokens,
        );
    }

    pub fn apply_to(self, event: &mut AuditEvent) {
        event.provider_type = self.provider_type;
        event.provider_ref = self.provider_ref;
        event.account_ref = self.account_ref;
        event.requested_model = self.requested_model;
        event.actual_model = self.actual_model;
        event.upstream_status = self.upstream_status;
        event.first_token_ms = self.first_token_ms;
        event.streaming = self.streaming;
        event.stream_status = self.stream_status;
        event.attempt = self.attempt;
        event.retry_count = self.retry_count;
        event.input_tokens = self.input_tokens;
        event.output_tokens = self.output_tokens;
        event.total_tokens = self.total_tokens;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditCursor {
    pub boot_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditUploadCursor {
    pub router_api_base: String,
    pub installation_id: String,
    pub boot_id: String,
    pub sequence: u64,
}

impl AuditUploadCursor {
    pub fn event_cursor(&self) -> Option<AuditCursor> {
        (!self.boot_id.is_empty() && self.sequence > 0).then(|| AuditCursor {
            boot_id: self.boot_id.clone(),
            sequence: self.sequence,
        })
    }

    pub fn targets_destination(&self, router_api_base: &str, installation_id: &str) -> bool {
        normalize_router_api_base(&self.router_api_base)
            == normalize_router_api_base(router_api_base)
            && self.installation_id.trim() == installation_id.trim()
    }
}

fn normalize_router_api_base(router_api_base: &str) -> String {
    router_api_base.trim().trim_end_matches('/').to_string()
}

impl From<&AuditEvent> for AuditCursor {
    fn from(event: &AuditEvent) -> Self {
        Self {
            boot_id: event.boot_id.clone(),
            sequence: event.sequence,
        }
    }
}

#[derive(Debug)]
pub struct AuditBatch {
    pub events: Vec<AuditEvent>,
    pub cursor_found: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditWriteError {
    #[error("audit writer is unavailable")]
    Unavailable,
    #[error("audit writer queue is full")]
    QueueFull,
    #[error("invalid audit event: {0}")]
    InvalidEvent(&'static str),
    #[error("serialize audit event: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug)]
enum AuditWriteCommand {
    Append(String),
    Flush(SyncSender<()>),
}

#[derive(Debug, Clone, Copy)]
enum AuditDelivery {
    NonBlocking,
    Backpressure,
}

#[derive(Debug)]
pub struct AuditLog {
    enabled: AtomicBool,
    writer_failed: Arc<AtomicBool>,
    writer_shutdown: Arc<AtomicBool>,
    writer_recovery_pending: Arc<AtomicBool>,
    writer_recovery_attempts: Arc<AtomicU64>,
    sender: Option<SyncSender<AuditWriteCommand>>,
    writer: Option<JoinHandle<()>>,
    sequence: Mutex<u64>,
    dropped_events: AtomicU64,
    queue_overflowed: AtomicBool,
    queue_dropped_events: AtomicU64,
    boot_id: String,
    path: PathBuf,
    cursor_path: PathBuf,
    file_io: Arc<Mutex<()>>,
    read_cache: Arc<Mutex<Option<AuditReadCache>>>,
    active_requests: Mutex<HashMap<String, ActiveAuditRequest>>,
}

#[derive(Debug, Default)]
struct ActiveAuditRequest {
    details: AuditRequestDetails,
    route_selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuditSpoolPosition {
    generation: usize,
    offset: u64,
}

#[derive(Debug, Clone)]
struct AuditReadCache {
    cursor: Option<AuditCursor>,
    position: AuditSpoolPosition,
    cursor_found: bool,
}

pub type SharedAuditLog = Arc<AuditLog>;

impl AuditLog {
    pub fn new(config_dir: &Path, boot_id: String) -> io::Result<SharedAuditLog> {
        let directory = super::ensure_log_dir(config_dir)?;
        let path = directory.join(AUDIT_LOG_FILENAME);
        let cursor_path = directory.join(AUDIT_CURSOR_FILENAME);
        let (sender, receiver) =
            mpsc::sync_channel::<AuditWriteCommand>(AUDIT_WRITE_QUEUE_CAPACITY);
        let writer_failed = Arc::new(AtomicBool::new(false));
        let writer_state = writer_failed.clone();
        let writer_shutdown = Arc::new(AtomicBool::new(false));
        let writer_shutdown_state = writer_shutdown.clone();
        let writer_recovery_pending = Arc::new(AtomicBool::new(false));
        let writer_recovery_state = writer_recovery_pending.clone();
        let writer_recovery_attempts = Arc::new(AtomicU64::new(0));
        let writer_recovery_attempt_state = writer_recovery_attempts.clone();
        let writer_path = path.clone();
        let file_io = Arc::new(Mutex::new(()));
        let writer_file_io = file_io.clone();
        let read_cache = Arc::new(Mutex::new(None));
        let writer_read_cache = read_cache.clone();
        let writer = std::thread::Builder::new()
            .name("cc-switch-audit-writer".to_string())
            .spawn(move || {
                let mut pending_line = None;
                let mut retry_delay = AUDIT_WRITE_RETRY_INITIAL;
                let mut retry_attempts = 0_u64;
                loop {
                    let line = if let Some(line) = pending_line.take() {
                        line
                    } else {
                        match receiver.recv() {
                            Ok(AuditWriteCommand::Append(line)) => line,
                            Ok(AuditWriteCommand::Flush(acknowledge)) => {
                                let _ = acknowledge.send(());
                                continue;
                            }
                            Err(_) => break,
                        }
                    };
                    let rotation_expected = audit_rotation_expected(&writer_path, &line);
                    let result = writer_file_io
                        .lock()
                        .map_err(|_| io::Error::other("audit file lock poisoned"))
                        .and_then(|_guard| {
                            append_rotating_line(
                                &writer_path,
                                &line,
                                AUDIT_LOG_MAX_BYTES,
                                AUDIT_LOG_BACKUP_COUNT,
                            )
                        });
                    match result {
                        Ok(()) => {
                            if rotation_expected {
                                writer_read_cache
                                    .lock()
                                    .expect("audit read cache lock")
                                    .take();
                            }
                            if writer_state.swap(false, Ordering::AcqRel) {
                                writer_recovery_attempt_state
                                    .store(retry_attempts, Ordering::Release);
                                writer_recovery_state.store(true, Ordering::Release);
                                tracing::info!(
                                    target: "cc_switch_server::audit_spool",
                                    component = "audit_spool",
                                    failure_kind = "file_write",
                                    retry_attempts,
                                    "audit spool writer recovered"
                                );
                            }
                            retry_delay = AUDIT_WRITE_RETRY_INITIAL;
                            retry_attempts = 0;
                        }
                        Err(error) => {
                            retry_attempts = retry_attempts.saturating_add(1);
                            if !writer_state.swap(true, Ordering::AcqRel) {
                                tracing::error!(
                                    target: "cc_switch_server::audit_spool",
                                    component = "audit_spool",
                                    failure_kind = "file_write",
                                    error_kind = ?error.kind(),
                                    path = %writer_path.display(),
                                    "audit spool writer degraded; retrying the pending event"
                                );
                            }
                            pending_line = Some(line);
                            if writer_shutdown_state.load(Ordering::Acquire) {
                                break;
                            }
                            std::thread::sleep(retry_delay);
                            retry_delay = retry_delay.saturating_mul(2).min(AUDIT_WRITE_RETRY_MAX);
                        }
                    }
                }
                writer_state.store(true, Ordering::Release);
            })?;
        Ok(Arc::new(Self {
            enabled: AtomicBool::new(true),
            writer_failed,
            writer_shutdown,
            writer_recovery_pending,
            writer_recovery_attempts,
            sender: Some(sender),
            writer: Some(writer),
            sequence: Mutex::new(0),
            dropped_events: AtomicU64::new(0),
            queue_overflowed: AtomicBool::new(false),
            queue_dropped_events: AtomicU64::new(0),
            boot_id,
            path,
            cursor_path,
            file_io,
            read_cache,
            active_requests: Mutex::new(HashMap::new()),
        }))
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn is_healthy(&self) -> bool {
        !self.writer_failed.load(Ordering::Acquire)
    }

    pub fn emit(&self, mut event: AuditEvent) -> Result<Option<AuditCursor>, AuditWriteError> {
        self.emit_with_delivery(&mut event, AuditDelivery::NonBlocking, true)
    }

    fn emit_with_delivery(
        &self,
        event: &mut AuditEvent,
        delivery: AuditDelivery,
        require_enabled: bool,
    ) -> Result<Option<AuditCursor>, AuditWriteError> {
        if require_enabled && !self.is_enabled() {
            return Ok(None);
        }
        if require_enabled && !self.is_healthy() {
            return Err(AuditWriteError::Unavailable);
        }
        event.schema_version = 1;
        event.timestamp_ms = chrono::Utc::now().timestamp_millis();
        event.boot_id.clone_from(&self.boot_id);
        event.level = "info".to_string();
        sanitize_untrusted_fields(event);
        validate_event(event)?;
        let mut sequence = self
            .sequence
            .lock()
            .map_err(|_| AuditWriteError::Unavailable)?;
        event.sequence = (*sequence).saturating_add(1);
        let cursor = AuditCursor::from(&*event);
        let line = serde_json::to_string(event)?;
        if line.len() > AUDIT_EVENT_MAX_BYTES {
            return Err(AuditWriteError::InvalidEvent("encoded event is too large"));
        }
        let Some(sender) = self.sender.as_ref() else {
            self.writer_failed.store(true, Ordering::Release);
            return Err(AuditWriteError::Unavailable);
        };
        match delivery {
            AuditDelivery::NonBlocking => match sender.try_send(AuditWriteCommand::Append(line)) {
                Ok(()) => {
                    *sequence = event.sequence;
                    self.report_queue_recovered();
                    Ok(Some(cursor))
                }
                Err(TrySendError::Full(_)) => {
                    self.report_queue_overflow();
                    Err(AuditWriteError::QueueFull)
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.writer_failed.store(true, Ordering::Release);
                    Err(AuditWriteError::Unavailable)
                }
            },
            AuditDelivery::Backpressure => {
                if sender.send(AuditWriteCommand::Append(line)).is_err() {
                    self.writer_failed.store(true, Ordering::Release);
                    return Err(AuditWriteError::Unavailable);
                }
                *sequence = event.sequence;
                self.report_queue_recovered();
                Ok(Some(cursor))
            }
        }
    }

    pub fn emit_best_effort(&self, event: AuditEvent) {
        if let Err(error) = self.emit(event) {
            self.report_dropped_event(error);
        }
    }

    pub fn emit_backpressured_best_effort(&self, mut event: AuditEvent) {
        if let Err(error) = self.emit_with_delivery(&mut event, AuditDelivery::Backpressure, true) {
            self.report_dropped_event(error);
        }
    }

    pub fn emit_terminal_best_effort(&self, mut event: AuditEvent) {
        if let Err(error) = self.emit_with_delivery(&mut event, AuditDelivery::Backpressure, false)
        {
            self.report_dropped_event(error);
        }
    }

    fn report_dropped_event(&self, error: AuditWriteError) {
        if matches!(error, AuditWriteError::QueueFull) {
            return;
        }
        let dropped = self.dropped_events.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped == 1 || dropped.is_power_of_two() {
            tracing::warn!(
                target: "cc_switch_server::audit_spool",
                component = "audit_spool",
                failure_kind = "event_drop",
                dropped,
                error = %error,
                "audit event was not admitted to the local spool"
            );
        }
    }

    fn report_queue_overflow(&self) {
        let dropped = self
            .queue_dropped_events
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let first = !self.queue_overflowed.swap(true, Ordering::AcqRel);
        if first || dropped.is_power_of_two() {
            tracing::warn!(
                target: "cc_switch_server::audit_spool",
                component = "audit_spool",
                failure_kind = "queue_overflow",
                dropped,
                "audit spool queue is full; inference admission remains fail-closed"
            );
        }
    }

    fn report_queue_recovered(&self) {
        if self.queue_overflowed.swap(false, Ordering::AcqRel) {
            let dropped = self.queue_dropped_events.swap(0, Ordering::AcqRel);
            tracing::info!(
                target: "cc_switch_server::audit_spool",
                component = "audit_spool",
                failure_kind = "queue_overflow",
                dropped,
                "audit spool queue recovered"
            );
        }
    }

    pub fn register_request(&self, request_id: &str) {
        self.active_requests
            .lock()
            .expect("audit active request lock")
            .entry(request_id.to_string())
            .or_default();
    }

    pub fn enrich_request(&self, request_id: &str, details: AuditRequestDetails) {
        if let Some(request) = self
            .active_requests
            .lock()
            .expect("audit active request lock")
            .get_mut(request_id)
        {
            request.details.merge(details);
        }
    }

    pub fn mark_route_selected(&self, request_id: &str) -> bool {
        let mut requests = self
            .active_requests
            .lock()
            .expect("audit active request lock");
        let Some(request) = requests.get_mut(request_id) else {
            return false;
        };
        if request.route_selected {
            false
        } else {
            request.route_selected = true;
            true
        }
    }

    pub fn take_request_details(&self, request_id: &str) -> AuditRequestDetails {
        self.active_requests
            .lock()
            .expect("audit active request lock")
            .remove(request_id)
            .map(|request| request.details)
            .unwrap_or_default()
    }

    pub fn read_batch(&self, cursor: Option<&AuditCursor>, limit: usize) -> io::Result<AuditBatch> {
        self.enqueue_pending_writer_recovery();
        self.flush_writer()?;
        let _file_guard = self
            .file_io
            .lock()
            .map_err(|_| io::Error::other("audit file lock poisoned"))?;
        let mut read_cache = self
            .read_cache
            .lock()
            .map_err(|_| io::Error::other("audit read cache lock poisoned"))?;
        read_audit_batch(&self.path, cursor, limit, &mut read_cache)
    }

    pub fn latest_cursor(&self) -> io::Result<Option<AuditCursor>> {
        self.enqueue_pending_writer_recovery();
        self.flush_writer()?;
        let _file_guard = self
            .file_io
            .lock()
            .map_err(|_| io::Error::other("audit file lock poisoned"))?;
        Ok(read_all_events(&self.path)?.last().map(AuditCursor::from))
    }

    pub fn load_upload_cursor(&self) -> io::Result<Option<AuditUploadCursor>> {
        match std::fs::read(&self.cursor_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn store_upload_cursor(&self, cursor: &AuditUploadCursor) -> io::Result<()> {
        let bytes = serde_json::to_vec(cursor)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = self.cursor_path.with_extension("json.tmp");
        {
            let mut options = OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, &self.cursor_path)?;
        sync_parent_directory(&self.cursor_path)
    }

    pub fn fence_upload_destination(
        &self,
        router_api_base: &str,
        installation_id: &str,
    ) -> io::Result<Option<AuditCursor>> {
        let latest_cursor = self.latest_cursor()?;
        self.store_upload_cursor(&AuditUploadCursor {
            router_api_base: normalize_router_api_base(router_api_base),
            installation_id: installation_id.trim().to_string(),
            boot_id: latest_cursor
                .as_ref()
                .map(|cursor| cursor.boot_id.clone())
                .unwrap_or_default(),
            sequence: latest_cursor
                .as_ref()
                .map(|cursor| cursor.sequence)
                .unwrap_or_default(),
        })?;
        Ok(latest_cursor)
    }

    fn flush_writer(&self) -> io::Result<()> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "audit writer is closed"))?;
        let (acknowledge, acknowledged) = mpsc::sync_channel(0);
        sender
            .try_send(AuditWriteCommand::Flush(acknowledge))
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "audit writer queue is full")
                }
                TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "audit writer is unavailable")
                }
            })?;
        acknowledged
            .recv_timeout(AUDIT_FLUSH_TIMEOUT)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("audit writer flush timed out: {error}"),
                )
            })?;
        if self.is_healthy() {
            Ok(())
        } else {
            Err(io::Error::other("audit writer failed to persist events"))
        }
    }

    fn enqueue_pending_writer_recovery(&self) {
        if !self.writer_recovery_pending.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut event = AuditEvent::new("observability.component.recovered");
        event.component = Some("audit_spool".to_string());
        event.failure_kind = Some("file_write".to_string());
        event.error_fingerprint = Some(error_fingerprint(
            "audit_spool",
            "file_write",
            "transient audit spool file write failure",
        ));
        event.retry_decision = Some("resume".to_string());
        event.outcome = Some("recovered".to_string());
        event.retryable = Some(false);
        let retry_attempts = self.writer_recovery_attempts.swap(0, Ordering::AcqRel);
        event.retry_count = Some(retry_attempts.min(u64::from(u32::MAX)) as u32);
        if self
            .emit_with_delivery(&mut event, AuditDelivery::NonBlocking, false)
            .is_err()
        {
            self.writer_recovery_attempts
                .fetch_max(retry_attempts, Ordering::AcqRel);
            self.writer_recovery_pending.store(true, Ordering::Release);
        }
    }
}

impl Drop for AuditLog {
    fn drop(&mut self) {
        self.writer_shutdown.store(true, Ordering::Release);
        drop(self.sender.take());
        if self
            .writer
            .take()
            .is_some_and(|writer| writer.join().is_err())
        {
            eprintln!("cc-switch-server audit log writer panicked during shutdown");
        }
    }
}

pub fn opaque_ref(kind: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-server-audit-ref-v1\n");
    digest.update(kind.as_bytes());
    digest.update(b"\n");
    digest.update(value.trim().as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("{kind}_{}", &encoded[..16])
}

pub fn error_fingerprint(component: &str, failure_kind: &str, error: &str) -> String {
    opaque_ref(
        "error",
        &format!(
            "{}\n{}\n{}",
            component.trim(),
            failure_kind.trim(),
            super::redact_sensitive_text(error).to_ascii_lowercase()
        ),
    )
}

pub fn classify_network_error(error: &str) -> Option<&'static str> {
    let error = error.to_ascii_lowercase();
    if error.contains("dns")
        || error.contains("name or service not known")
        || error.contains("failed to lookup address")
    {
        Some("dns")
    } else if error.contains("tls") || error.contains("certificate") || error.contains("rustls") {
        Some("tls")
    } else if error.contains("timed out") || error.contains("timeout") {
        Some("timeout")
    } else if error.contains("connection refused") {
        Some("connection_refused")
    } else if error.contains("connection reset") || error.contains("broken pipe") {
        Some("connection_reset")
    } else if error.contains("connect") || error.contains("socket") {
        Some("connect")
    } else {
        None
    }
}

fn validate_event(event: &AuditEvent) -> Result<(), AuditWriteError> {
    if event.event.is_empty()
        || event.event.len() > AUDIT_EVENT_NAME_MAX_BYTES
        || !event.event.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_')
        })
    {
        return Err(AuditWriteError::InvalidEvent("event name is invalid"));
    }

    for value in [
        event.request_id.as_deref(),
        event.transport_request_id.as_deref(),
        event.parent_request_id.as_deref(),
        event.connection_id.as_deref(),
        event.turn_id.as_deref(),
        event.app.as_deref(),
        event.surface.as_deref(),
        event.operation.as_deref(),
        event.route.as_deref(),
        event.method.as_deref(),
        event.provider_type.as_deref(),
        event.provider_ref.as_deref(),
        event.previous_provider_ref.as_deref(),
        event.account_ref.as_deref(),
        event.outcome.as_deref(),
        event.stage.as_deref(),
        event.error_code.as_deref(),
        event.error_class.as_deref(),
        event.component.as_deref(),
        event.failure_kind.as_deref(),
        event.network_error_kind.as_deref(),
        event.retry_decision.as_deref(),
        event.stream_status.as_deref(),
    ] {
        if value.is_some_and(|value| {
            value.len() > AUDIT_STRING_FIELD_MAX_BYTES
                || value.contains('\r')
                || value.contains('\n')
        }) {
            return Err(AuditWriteError::InvalidEvent("string field is invalid"));
        }
    }
    for value in [
        event.requested_model.as_deref(),
        event.actual_model.as_deref(),
    ] {
        if value.is_some_and(|value| {
            value.len() > AUDIT_MODEL_FIELD_MAX_BYTES
                || value.contains('\r')
                || value.contains('\n')
        }) {
            return Err(AuditWriteError::InvalidEvent("model field is invalid"));
        }
    }
    if event
        .provider_ref
        .as_deref()
        .is_some_and(|value| !valid_opaque_ref(value, "provider"))
        || event
            .previous_provider_ref
            .as_deref()
            .is_some_and(|value| !valid_opaque_ref(value, "provider"))
        || event
            .account_ref
            .as_deref()
            .is_some_and(|value| !valid_opaque_ref(value, "account"))
        || event
            .error_fingerprint
            .as_deref()
            .is_some_and(|value| !valid_opaque_ref(value, "error"))
    {
        return Err(AuditWriteError::InvalidEvent(
            "identity reference is not opaque",
        ));
    }
    if event.body_sha256.as_deref().is_some_and(|value| {
        value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(AuditWriteError::InvalidEvent("body digest is invalid"));
    }
    if event
        .status_code
        .is_some_and(|status| !(100..=599).contains(&status))
        || event
            .upstream_status
            .is_some_and(|status| !(100..=599).contains(&status))
    {
        return Err(AuditWriteError::InvalidEvent("status code is invalid"));
    }
    Ok(())
}

fn sanitize_untrusted_fields(event: &mut AuditEvent) {
    for value in [
        &mut event.request_id,
        &mut event.transport_request_id,
        &mut event.parent_request_id,
        &mut event.connection_id,
        &mut event.turn_id,
        &mut event.app,
        &mut event.surface,
        &mut event.operation,
        &mut event.route,
        &mut event.method,
        &mut event.provider_type,
        &mut event.outcome,
        &mut event.stage,
        &mut event.error_code,
        &mut event.error_class,
        &mut event.component,
        &mut event.failure_kind,
        &mut event.network_error_kind,
        &mut event.retry_decision,
        &mut event.stream_status,
    ] {
        sanitize_bounded_string(value, AUDIT_STRING_FIELD_MAX_BYTES);
        if value.as_deref().is_some_and(contains_sensitive_audit_value) {
            *value = None;
        }
    }
    for value in [&mut event.requested_model, &mut event.actual_model] {
        sanitize_bounded_string(value, AUDIT_MODEL_FIELD_MAX_BYTES);
        if value.as_deref().is_some_and(contains_sensitive_audit_value) {
            *value = None;
        }
    }
}

fn contains_sensitive_audit_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    value.contains('@')
        || super::redact_sensitive_text(trimmed) != trimmed
        || [
            "bearer ",
            "sk-",
            "sk_",
            "xai-",
            "ghp_",
            "github_pat_",
            "ya29.",
            "aiza",
            "eyj",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn sync_parent_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn sanitize_bounded_string(value: &mut Option<String>, max_bytes: usize) {
    if value
        .as_deref()
        .is_some_and(|value| value.len() > max_bytes || value.contains(['\r', '\n']))
    {
        *value = None;
    }
}

fn valid_opaque_ref(value: &str, kind: &str) -> bool {
    value
        .strip_prefix(kind)
        .and_then(|value| value.strip_prefix('_'))
        .is_some_and(|digest| {
            digest.len() == 16
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn audit_rotation_expected(path: &Path, line: &str) -> bool {
    let current_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let additional_bytes = u64::try_from(line.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    current_bytes > 0 && current_bytes.saturating_add(additional_bytes) > AUDIT_LOG_MAX_BYTES
}

fn read_audit_batch(
    path: &Path,
    cursor: Option<&AuditCursor>,
    limit: usize,
    cache: &mut Option<AuditReadCache>,
) -> io::Result<AuditBatch> {
    let requested_cursor = cursor.cloned();
    let limit = limit.clamp(1, AUDIT_UPLOAD_BATCH_LIMIT);
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.cursor.as_ref() == cursor)
        .filter(|cached| audit_spool_position_is_valid(path, cached.position))
        .cloned()
    {
        let (events, position) = collect_audit_events(path, cached.position, limit)?;
        let next_cursor = events.last().map(AuditCursor::from).or(requested_cursor);
        *cache = Some(AuditReadCache {
            cursor: next_cursor,
            position,
            cursor_found: cached.cursor_found || !events.is_empty(),
        });
        return Ok(AuditBatch {
            events,
            cursor_found: cached.cursor_found,
        });
    }

    let (position, cursor_found) = match cursor {
        None => (
            AuditSpoolPosition {
                generation: AUDIT_LOG_BACKUP_COUNT,
                offset: 0,
            },
            true,
        ),
        Some(cursor) => match locate_audit_cursor(path, cursor)? {
            Some(position) => (position, true),
            None => (
                AuditSpoolPosition {
                    generation: AUDIT_LOG_BACKUP_COUNT,
                    offset: 0,
                },
                false,
            ),
        },
    };
    let (events, position) = collect_audit_events(path, position, limit)?;
    let next_cursor = events.last().map(AuditCursor::from).or(requested_cursor);
    *cache = Some(AuditReadCache {
        cursor: next_cursor,
        position,
        cursor_found: cursor_found || !events.is_empty(),
    });
    Ok(AuditBatch {
        events,
        cursor_found,
    })
}

fn locate_audit_cursor(
    path: &Path,
    cursor: &AuditCursor,
) -> io::Result<Option<AuditSpoolPosition>> {
    for generation in (0..=AUDIT_LOG_BACKUP_COUNT).rev() {
        let candidate = audit_spool_path(path, generation);
        let file = match File::open(candidate) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let position = reader.stream_position()?;
            let Ok(event) = serde_json::from_str::<AuditEvent>(line.trim_end()) else {
                continue;
            };
            if event.boot_id == cursor.boot_id && event.sequence == cursor.sequence {
                return Ok(Some(AuditSpoolPosition {
                    generation,
                    offset: position,
                }));
            }
        }
    }
    Ok(None)
}

fn collect_audit_events(
    path: &Path,
    start: AuditSpoolPosition,
    limit: usize,
) -> io::Result<(Vec<AuditEvent>, AuditSpoolPosition)> {
    let mut events = Vec::with_capacity(limit);
    let mut batch_boot_id = None::<String>;
    let mut position = start;
    for generation in (0..=start.generation).rev() {
        let candidate = audit_spool_path(path, generation);
        let file = match File::open(candidate) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                position = AuditSpoolPosition {
                    generation,
                    offset: 0,
                };
                continue;
            }
            Err(error) => return Err(error),
        };
        let offset = if generation == start.generation {
            start.offset
        } else {
            0
        };
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(offset))?;
        position = AuditSpoolPosition { generation, offset };
        let mut line = String::new();
        loop {
            line.clear();
            let line_start = reader.stream_position()?;
            if reader.read_line(&mut line)? == 0 {
                position.offset = reader.stream_position()?;
                break;
            }
            let line_end = reader.stream_position()?;
            position.offset = line_end;
            let Ok(event) = serde_json::from_str::<AuditEvent>(line.trim_end()) else {
                continue;
            };
            if batch_boot_id
                .as_deref()
                .is_some_and(|boot_id| boot_id != event.boot_id)
            {
                position.offset = line_start;
                return Ok((events, position));
            }
            batch_boot_id.get_or_insert_with(|| event.boot_id.clone());
            events.push(event);
            if events.len() >= limit {
                return Ok((events, position));
            }
        }
    }
    Ok((events, position))
}

fn audit_spool_position_is_valid(path: &Path, position: AuditSpoolPosition) -> bool {
    match std::fs::metadata(audit_spool_path(path, position.generation)) {
        Ok(metadata) => metadata.len() >= position.offset,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            position.generation == 0 && position.offset == 0
        }
        Err(_) => false,
    }
}

fn audit_spool_path(path: &Path, generation: usize) -> PathBuf {
    if generation == 0 {
        path.to_path_buf()
    } else {
        rotated_path(path, generation)
    }
}

fn read_all_events(path: &Path) -> io::Result<Vec<AuditEvent>> {
    let mut events = Vec::new();
    for index in (1..=AUDIT_LOG_BACKUP_COUNT).rev() {
        read_event_file(&rotated_path(path, index), &mut events)?;
    }
    read_event_file(path, &mut events)?;
    Ok(events)
}

fn read_event_file(path: &Path, events: &mut Vec<AuditEvent>) -> io::Result<()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Ok(event) = serde_json::from_str::<AuditEvent>(&line) {
            events.push(event);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cc-switch-server-audit-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn audit_events_are_structured_and_cursor_addressable() {
        let directory = test_dir("cursor");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let first = log
            .emit(AuditEvent::new("inference.request.accepted"))
            .unwrap()
            .unwrap();
        log.emit(AuditEvent::new("inference.request.completed"))
            .unwrap();
        let batch = log.read_batch(Some(&first), 10).unwrap();
        assert!(batch.cursor_found);
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].event, "inference.request.completed");
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn audit_writer_recovers_after_a_transient_file_failure() {
        let directory = test_dir("writer-recovery");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        std::fs::create_dir(&log.path).unwrap();

        log.emit(AuditEvent::new("inference.request.accepted"))
            .unwrap();
        let degraded_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while log.is_healthy() && std::time::Instant::now() < degraded_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!log.is_healthy());

        std::fs::remove_dir(&log.path).unwrap();
        let recovered_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !log.is_healthy() && std::time::Instant::now() < recovered_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(log.is_healthy());
        let events = log.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "inference.request.accepted");
        assert_eq!(events[1].event, "observability.component.recovered");
        assert_eq!(events[1].component.as_deref(), Some("audit_spool"));
        assert_eq!(events[1].failure_kind.as_deref(), Some("file_write"));
        assert!(events[1].retry_count.is_some_and(|count| count > 0));

        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn admitted_terminal_waits_for_a_degraded_writer_to_recover() {
        let directory = test_dir("terminal-writer-recovery");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut accepted = AuditEvent::new("inference.request.accepted");
        accepted.request_id = Some("request-a".to_string());
        log.emit(accepted).unwrap().unwrap();
        log.read_batch(None, 10).unwrap();

        std::fs::rename(&log.path, rotated_path(&log.path, 1)).unwrap();
        std::fs::create_dir(&log.path).unwrap();
        log.emit(AuditEvent::new("inference.route.selected"))
            .unwrap()
            .unwrap();
        let degraded_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while log.is_healthy() && std::time::Instant::now() < degraded_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!log.is_healthy());

        let mut terminal = AuditEvent::new("inference.request.interrupted");
        terminal.request_id = Some("request-a".to_string());
        log.emit_terminal_best_effort(terminal);
        std::fs::remove_dir(&log.path).unwrap();

        let recovered_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !log.is_healthy() && std::time::Instant::now() < recovered_deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(log.is_healthy());
        let events = log.read_batch(None, 10).unwrap().events;
        assert!(events.iter().any(|event| {
            event.event == "inference.request.interrupted"
                && event.request_id.as_deref() == Some("request-a")
        }));

        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn batch_reader_preserves_boot_boundaries_and_incremental_position() {
        let directory = test_dir("incremental-reader");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(AUDIT_LOG_FILENAME);
        let events = [
            persisted_event("boot-a", 1, "inference.request.accepted"),
            persisted_event("boot-a", 2, "inference.request.completed"),
            persisted_event("boot-b", 1, "inference.request.accepted"),
        ];
        let contents = events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
            + "\n";
        std::fs::write(&path, contents).unwrap();
        let mut cache = None;

        let first = read_audit_batch(&path, None, 10, &mut cache).unwrap();
        assert_eq!(first.events.len(), 2);
        assert!(first.events.iter().all(|event| event.boot_id == "boot-a"));
        let cursor = AuditCursor::from(first.events.last().unwrap());

        let second = read_audit_batch(&path, Some(&cursor), 10, &mut cache).unwrap();
        assert!(second.cursor_found);
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].boot_id, "boot-b");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_upload_cursor_marks_a_destination_without_skipping_future_events() {
        let cursor = AuditUploadCursor {
            router_api_base: "https://router.example".into(),
            installation_id: "installation-a".into(),
            boot_id: String::new(),
            sequence: 0,
        };
        assert_eq!(cursor.event_cursor(), None);
    }

    #[test]
    fn upload_cursor_targets_only_the_same_normalized_destination() {
        let cursor = AuditUploadCursor {
            router_api_base: "https://router.example/".into(),
            installation_id: "installation-a".into(),
            boot_id: "boot-a".into(),
            sequence: 7,
        };

        assert!(cursor.targets_destination(" https://router.example ", "installation-a"));
        assert!(cursor.targets_destination("https://router.example/", " installation-a "));
        assert!(!cursor.targets_destination("https://other-router.example", "installation-a"));
        assert!(!cursor.targets_destination("https://router.example", "installation-b"));
    }

    #[test]
    fn upload_cursor_replacement_is_atomic_and_private() {
        let directory = test_dir("upload-cursor-atomic");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let cursor = AuditUploadCursor {
            router_api_base: "https://router.example".into(),
            installation_id: "installation-a".into(),
            boot_id: "boot-a".into(),
            sequence: 7,
        };

        log.store_upload_cursor(&cursor).unwrap();

        assert_eq!(log.load_upload_cursor().unwrap(), Some(cursor));
        assert!(!log.cursor_path.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&log.cursor_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fencing_upload_destination_skips_events_from_the_previous_installation() {
        let directory = test_dir("destination-fence");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        log.emit(AuditEvent::new("inference.request.accepted"))
            .unwrap();
        log.emit(AuditEvent::new("inference.request.completed"))
            .unwrap();

        let latest = log
            .fence_upload_destination(" https://router.example/ ", " installation-b ")
            .unwrap()
            .unwrap();
        let stored = log.load_upload_cursor().unwrap().unwrap();

        assert_eq!(stored.router_api_base, "https://router.example");
        assert_eq!(stored.installation_id, "installation-b");
        assert_eq!(stored.event_cursor(), Some(latest));
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn persisted_event(boot_id: &str, sequence: u64, event: &str) -> AuditEvent {
        AuditEvent {
            schema_version: 1,
            sequence,
            timestamp_ms: 1,
            boot_id: boot_id.to_string(),
            level: "info".to_string(),
            event: event.to_string(),
            ..AuditEvent::default()
        }
    }

    #[test]
    fn audit_log_drop_drains_pending_events() {
        let directory = test_dir("drop-drain");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let path = log.path.clone();
        for index in 0..1_000 {
            log.emit(AuditEvent::new(format!("event_{index}"))).unwrap();
        }

        drop(log);

        let events = read_all_events(&path).unwrap();
        assert_eq!(events.len(), 1_000);
        assert_eq!(events.first().unwrap().event, "event_0");
        assert_eq!(events.last().unwrap().event, "event_999");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn admitted_request_terminal_is_spooled_after_collection_is_disabled() {
        let directory = test_dir("terminal-after-disable");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        log.emit(AuditEvent::new("inference.request.accepted"))
            .unwrap()
            .unwrap();
        log.set_enabled(false);

        log.emit_terminal_best_effort(AuditEvent::new("inference.request.interrupted"));

        let events = log.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event, "inference.request.interrupted");
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn audit_schema_has_no_free_form_message_or_identity_fields() {
        let mut event = AuditEvent::new("inference.request.completed");
        event.provider_ref = Some(opaque_ref("provider", "provider-secret-name"));
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("provider-secret-name"));
        assert!(!serialized.contains("email"));
        assert!(!serialized.contains("message"));
        assert!(!serialized.contains("headers"));
        assert!(!serialized.contains("body\""));
    }

    #[test]
    fn invalid_audit_events_are_rejected_without_consuming_sequence() {
        let directory = test_dir("invalid-event");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();

        let mut raw_identity = AuditEvent::new("inference.route.selected");
        raw_identity.provider_ref = Some("provider-secret-name".to_string());
        assert!(matches!(
            log.emit(raw_identity),
            Err(AuditWriteError::InvalidEvent(
                "identity reference is not opaque"
            ))
        ));

        let invalid_name = AuditEvent::new("inference-route-selected");
        assert!(matches!(
            log.emit(invalid_name),
            Err(AuditWriteError::InvalidEvent("event name is invalid"))
        ));

        let cursor = log
            .emit(AuditEvent::new("inference.request.completed"))
            .unwrap()
            .unwrap();
        assert_eq!(cursor.sequence, 1);
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diagnostic_fingerprints_are_opaque_and_network_errors_are_classified() {
        let fingerprint = error_fingerprint(
            "router_log_upload",
            "request",
            "TLS certificate rejected authorization: Bearer secret-token",
        );
        assert!(valid_opaque_ref(&fingerprint, "error"));
        assert!(!fingerprint.contains("secret-token"));
        assert_eq!(
            classify_network_error("TLS certificate rejected"),
            Some("tls")
        );
        assert_eq!(
            classify_network_error("operation timed out"),
            Some("timeout")
        );
        assert_eq!(classify_network_error("HTTP 500"), None);
    }

    #[test]
    fn untrusted_model_fields_are_omitted_without_dropping_the_event() {
        let directory = test_dir("model-sanitization");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut event = AuditEvent::new("inference.route.selected");
        event.requested_model = Some("model\nsecret".to_string());
        event.actual_model = Some("x".repeat(AUDIT_MODEL_FIELD_MAX_BYTES + 1));

        log.emit(event).unwrap();
        let events = log.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].requested_model, None);
        assert_eq!(events[0].actual_model, None);
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sensitive_model_fields_are_omitted_without_dropping_the_event() {
        let directory = test_dir("sensitive-model-sanitization");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut event = AuditEvent::new("inference.route.selected");
        event.requested_model = Some("owner@example.com".to_string());
        event.actual_model = Some("sk-secret-looking-value".to_string());
        event.operation = Some("owner@example.com".to_string());
        event.error_code = Some("sk-secret-looking-value".to_string());

        log.emit(event).unwrap();
        let events = log.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].requested_model, None);
        assert_eq!(events[0].actual_model, None);
        assert_eq!(events[0].operation, None);
        assert_eq!(events[0].error_code, None);
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn untrusted_terminal_metadata_is_omitted_without_losing_the_event() {
        let directory = test_dir("terminal-metadata-sanitization");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        let mut event = AuditEvent::new("inference.request.failed");
        event.error_code = Some("upstream\nheader".to_string());

        log.emit_terminal_best_effort(event);

        let events = log.read_batch(None, 10).unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].error_code, None);
        assert_eq!(
            log.latest_cursor().unwrap(),
            Some(AuditCursor::from(&events[0]))
        );
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn late_request_enrichment_does_not_recreate_finished_state() {
        let directory = test_dir("late-enrichment");
        std::fs::create_dir_all(&directory).unwrap();
        let log = AuditLog::new(&directory, "boot-a".to_string()).unwrap();
        log.register_request("request-a");
        log.take_request_details("request-a");

        log.enrich_request(
            "request-a",
            AuditRequestDetails {
                requested_model: Some("late-model".to_string()),
                ..AuditRequestDetails::default()
            },
        );

        assert!(!log.mark_route_selected("request-a"));
        assert!(log
            .active_requests
            .lock()
            .expect("audit active request lock")
            .is_empty());
        drop(log);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
