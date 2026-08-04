use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use crate::domain::router::{
    InstallationLogBatchPayload, InstallationLogEvent, INSTALLATION_LOG_PROTOCOL_VERSION,
};
use crate::domain::settings::ui_settings::ParsedLogConfig;
use crate::state::ServerState;

const REMOTE_LOG_INTERNAL_TARGET: &str = "cc_switch_server::logging::remote";
const REMOTE_LOG_CHANNEL_CAPACITY: usize = 4_096;
const REMOTE_LOG_BATCH_MAX_EVENTS: usize = 200;
const REMOTE_LOG_BATCH_MAX_BYTES: usize = 220 * 1024;
const REMOTE_LOG_SPOOL_MAX_BYTES: u64 = 16 * 1024 * 1024;
const REMOTE_LOG_SPOOL_TARGET_BYTES: usize = 14 * 1024 * 1024;
const REMOTE_LOG_SPOOL_MAX_AGE_MS: i64 = 6 * 60 * 60 * 1_000;
const REMOTE_LOG_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpoolRecord {
    stream_id: String,
    event: InstallationLogEvent,
}

#[derive(Debug)]
struct StreamSequence {
    stream_id: String,
    next_sequence: u64,
}

#[derive(Debug)]
struct RemoteLogCollector {
    enabled: AtomicBool,
    sender: mpsc::Sender<SpoolRecord>,
    receiver: Mutex<Option<mpsc::Receiver<SpoolRecord>>>,
    stream: Mutex<StreamSequence>,
    dropped: AtomicU64,
}

impl RemoteLogCollector {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel(REMOTE_LOG_CHANNEL_CAPACITY);
        Self {
            enabled: AtomicBool::new(false),
            sender,
            receiver: Mutex::new(Some(receiver)),
            stream: Mutex::new(StreamSequence {
                stream_id: new_stream_id(),
                next_sequence: 1,
            }),
            dropped: AtomicU64::new(0),
        }
    }

    fn capture(&self, mut event: InstallationLogEvent) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let Ok(permit) = self.sender.try_reserve() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let mut stream = self.stream.lock().expect("remote log stream lock");
        event.sequence = stream.next_sequence;
        stream.next_sequence = stream.next_sequence.saturating_add(1);
        permit.send(SpoolRecord {
            stream_id: stream.stream_id.clone(),
            event,
        });
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<SpoolRecord>> {
        self.receiver
            .lock()
            .expect("remote log receiver lock")
            .take()
    }

    fn rotate_stream_if(&self, stream_id: &str) -> bool {
        let mut stream = self.stream.lock().expect("remote log stream lock");
        if stream.stream_id != stream_id {
            return false;
        }
        stream.stream_id = new_stream_id();
        stream.next_sequence = 1;
        true
    }
}

static REMOTE_LOG_COLLECTOR: OnceLock<Arc<RemoteLogCollector>> = OnceLock::new();

fn collector() -> &'static Arc<RemoteLogCollector> {
    REMOTE_LOG_COLLECTOR.get_or_init(|| Arc::new(RemoteLogCollector::new()))
}

#[derive(Clone)]
pub struct RemoteLogLayer {
    collector: Arc<RemoteLogCollector>,
}

pub fn remote_log_layer() -> RemoteLogLayer {
    RemoteLogLayer {
        collector: Arc::clone(collector()),
    }
}

pub fn apply_remote_log_config(config: &ParsedLogConfig) {
    let enabled =
        config.enabled && config.collection_enabled && config.level.eq_ignore_ascii_case("info");
    collector().enabled.store(enabled, Ordering::Release);
}

impl<S> Layer<S> for RemoteLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        if !self.collector.enabled.load(Ordering::Acquire)
            || metadata.target() == REMOTE_LOG_INTERNAL_TARGET
        {
            return;
        }
        let level = match *metadata.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            Level::INFO => "info",
            Level::DEBUG | Level::TRACE => return,
        };
        let mut visitor = RemoteEventVisitor::default();
        event.record(&mut visitor);
        self.collector.capture(InstallationLogEvent {
            sequence: 0,
            occurred_at_ms: now_ms(),
            level: level.to_string(),
            target: metadata.target().chars().take(512).collect(),
            message: crate::logging::redact_sensitive_text(
                visitor.message.as_deref().unwrap_or(""),
            )
            .chars()
            .take(16 * 1024)
            .collect(),
            fields: visitor.fields,
            file: metadata.file().map(str::to_string),
            line: metadata.line(),
        });
    }
}

#[derive(Default)]
struct RemoteEventVisitor {
    message: Option<String>,
    fields: BTreeMap<String, Value>,
}

impl RemoteEventVisitor {
    fn record_value(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = value
                .as_str()
                .map(str::to_string)
                .or_else(|| Some(value.to_string()));
        } else if !is_sensitive_field(field.name()) {
            self.fields
                .insert(field.name().to_string(), sanitize_field_value(value));
        }
    }
}

impl Visit for RemoteEventVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(
            field,
            Value::String(crate::logging::redact_sensitive_text(value)),
        );
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_str(field, &value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        let rendered = rendered
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(&rendered);
        self.record_str(field, rendered);
    }
}

struct RemoteSpool {
    path: PathBuf,
    records: VecDeque<SpoolRecord>,
}

impl RemoteSpool {
    fn load(config_dir: &Path) -> std::io::Result<Self> {
        let directory = config_dir.join("remote-log-spool");
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let path = directory.join("events.jsonl");
        let mut records = VecDeque::new();
        if let Ok(file) = File::open(&path) {
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(record) = serde_json::from_str::<SpoolRecord>(&line) {
                    records.push_back(record);
                }
            }
        }
        let mut spool = Self { path, records };
        spool.discard_expired(now_ms());
        spool.rewrite()?;
        Ok(spool)
    }

    fn push(&mut self, record: SpoolRecord) -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        self.records.push_back(record);
        if file.metadata()?.len() > REMOTE_LOG_SPOOL_MAX_BYTES {
            self.trim_to_target();
            self.rewrite()?;
        }
        Ok(())
    }

    fn discard_expired(&mut self, now_ms: i64) -> bool {
        let cutoff = now_ms.saturating_sub(REMOTE_LOG_SPOOL_MAX_AGE_MS);
        let previous_len = self.records.len();
        self.records
            .retain(|record| record.event.occurred_at_ms >= cutoff);
        self.records.len() != previous_len
    }

    fn expire_old(&mut self, now_ms: i64) -> std::io::Result<bool> {
        let changed = self.discard_expired(now_ms);
        if changed {
            self.rewrite()?;
        }
        Ok(changed)
    }

    fn trim_to_target(&mut self) {
        let mut bytes = self
            .records
            .iter()
            .map(|record| {
                serde_json::to_vec(record)
                    .map(|value| value.len() + 1)
                    .unwrap_or(0)
            })
            .sum::<usize>();
        while bytes > REMOTE_LOG_SPOOL_TARGET_BYTES {
            let Some(record) = self.records.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(
                serde_json::to_vec(&record)
                    .map(|value| value.len() + 1)
                    .unwrap_or(0),
            );
        }
    }

    fn next_batch(&self) -> Option<(InstallationLogBatchPayload, usize)> {
        let first = self.records.front()?;
        let stream_id = first.stream_id.clone();
        let mut events = Vec::new();
        let mut bytes = 0usize;
        for record in &self.records {
            if record.stream_id != stream_id || events.len() >= REMOTE_LOG_BATCH_MAX_EVENTS {
                break;
            }
            let event_bytes = serde_json::to_vec(&record.event)
                .map(|value| value.len())
                .unwrap_or(REMOTE_LOG_BATCH_MAX_BYTES);
            if !events.is_empty() && bytes.saturating_add(event_bytes) > REMOTE_LOG_BATCH_MAX_BYTES
            {
                break;
            }
            bytes = bytes.saturating_add(event_bytes);
            events.push(record.event.clone());
        }
        let build = crate::build_info::build_info();
        Some((
            InstallationLogBatchPayload {
                protocol_version: INSTALLATION_LOG_PROTOCOL_VERSION,
                stream_id,
                server_version: build.version.to_string(),
                commit_id: build.commit_id.to_string(),
                events,
            },
            bytes,
        ))
    }

    fn acknowledge(&mut self, stream_id: &str, through_sequence: u64) -> std::io::Result<()> {
        self.records.retain(|record| {
            record.stream_id != stream_id || record.event.sequence > through_sequence
        });
        self.rewrite()
    }

    fn recover_sequence_gap(
        &mut self,
        stream_id: &str,
        expected_sequence: u64,
    ) -> std::io::Result<bool> {
        let first_sequence = self
            .records
            .iter()
            .find(|record| record.stream_id == stream_id)
            .map(|record| record.event.sequence);
        match first_sequence {
            Some(first_sequence) if expected_sequence > first_sequence => {
                self.records.retain(|record| {
                    record.stream_id != stream_id || record.event.sequence >= expected_sequence
                });
                self.rewrite()?;
                Ok(false)
            }
            Some(_) => {
                let replacement_stream_id = new_stream_id();
                let mut next_sequence = 1u64;
                for record in self
                    .records
                    .iter_mut()
                    .filter(|record| record.stream_id == stream_id)
                {
                    record.stream_id = replacement_stream_id.clone();
                    record.event.sequence = next_sequence;
                    next_sequence = next_sequence.saturating_add(1);
                }
                self.rewrite()?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn rewrite(&self) -> std::io::Result<()> {
        let temporary = self.path.with_extension("jsonl.new");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        {
            let mut file = options.open(&temporary)?;
            for record in &self.records {
                serde_json::to_writer(&mut file, record)?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
        }
        fs::rename(temporary, &self.path)
    }
}

pub fn spawn_remote_log_upload(state: ServerState) {
    let Some(receiver) = collector().take_receiver() else {
        return;
    };
    tokio::spawn(run_remote_log_upload(state, receiver));
}

async fn run_remote_log_upload(state: ServerState, mut receiver: mpsc::Receiver<SpoolRecord>) {
    let mut spool = match RemoteSpool::load(&state.config_dir) {
        Ok(spool) => spool,
        Err(error) => {
            tracing::error!(
                target: REMOTE_LOG_INTERNAL_TARGET,
                %error,
                "initialize remote log spool failed"
            );
            return;
        }
    };
    let mut interval = tokio::time::interval(REMOTE_LOG_FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut consecutive_failures = 0u32;
    let mut retry_at = tokio::time::Instant::now();
    let mut retired_streams = HashSet::new();

    loop {
        tokio::select! {
            record = receiver.recv() => {
                let Some(record) = record else {
                    break;
                };
                if retired_streams.contains(&record.stream_id) {
                    continue;
                }
                let stream_id = record.stream_id.clone();
                if let Err(error) = spool.push(record) {
                    retired_streams.insert(stream_id.clone());
                    collector().rotate_stream_if(&stream_id);
                    tracing::error!(
                        target: REMOTE_LOG_INTERNAL_TARGET,
                        %error,
                        "persist remote log spool event failed"
                    );
                }
            }
            _ = interval.tick() => {
                if let Err(error) = spool.expire_old(now_ms()) {
                    tracing::error!(
                        target: REMOTE_LOG_INTERNAL_TARGET,
                        %error,
                        "expire remote log spool events failed"
                    );
                }
                if !collector().enabled.load(Ordering::Acquire)
                    || tokio::time::Instant::now() < retry_at
                {
                    continue;
                }
                let Some((payload, _bytes)) = spool.next_batch() else {
                    consecutive_failures = 0;
                    continue;
                };
                let stream_id = payload.stream_id.clone();
                let through_sequence = payload.events.last().map(|event| event.sequence).unwrap_or(0);
                match state.upload_installation_log_batch(payload).await {
                    Ok(response) => {
                        if response.ok {
                            if let Err(error) = spool.acknowledge(&stream_id, through_sequence) {
                                tracing::error!(
                                    target: REMOTE_LOG_INTERNAL_TARGET,
                                    %error,
                                    "commit remote log spool acknowledgement failed"
                                );
                            }
                            consecutive_failures = 0;
                            retry_at = tokio::time::Instant::now();
                        }
                    }
                    Err(crate::clients::router::client::InstallationLogUploadError::SequenceGap { expected_sequence }) => {
                        match spool.recover_sequence_gap(&stream_id, expected_sequence) {
                            Ok(true) => {
                                retired_streams.insert(stream_id.clone());
                                collector().rotate_stream_if(&stream_id);
                            }
                            Ok(false) => {}
                            Err(error) => tracing::error!(
                                target: REMOTE_LOG_INTERNAL_TARGET,
                                %error,
                                "recover remote log sequence gap failed"
                            ),
                        }
                        consecutive_failures = 0;
                    }
                    Err(error) if error.is_transient() => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let backoff_seconds = 2u64
                            .saturating_pow(consecutive_failures.min(8))
                            .clamp(2, 300);
                        retry_at = tokio::time::Instant::now() + Duration::from_secs(backoff_seconds);
                        tracing::warn!(
                            target: REMOTE_LOG_INTERNAL_TARGET,
                            %error,
                            backoff_seconds,
                            "remote log upload deferred"
                        );
                    }
                    Err(error) => {
                        tracing::error!(
                            target: REMOTE_LOG_INTERNAL_TARGET,
                            %error,
                            "router permanently rejected remote log batch; dropping batch"
                        );
                        let _ = spool.acknowledge(&stream_id, through_sequence);
                        consecutive_failures = 0;
                    }
                }
            }
        }
    }
}

fn sanitize_field_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(
            crate::logging::redact_sensitive_text(&value)
                .chars()
                .take(4_096)
                .collect(),
        ),
        other => other,
    }
}

fn is_sensitive_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "private_key",
        "credential",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn new_stream_id() -> String {
    let mut random = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut random);
    format!(
        "{}-{}-{}",
        now_ms(),
        std::process::id(),
        hex::encode(random)
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(stream_id: &str, sequence: u64, occurred_at_ms: i64) -> SpoolRecord {
        SpoolRecord {
            stream_id: stream_id.into(),
            event: InstallationLogEvent {
                sequence,
                occurred_at_ms,
                level: "info".into(),
                target: "test".into(),
                message: "message".into(),
                fields: BTreeMap::new(),
                file: None,
                line: None,
            },
        }
    }

    #[test]
    fn spool_round_trips_and_acknowledges_sequences() {
        let directory =
            std::env::temp_dir().join(format!("remote-log-spool-test-{}", new_stream_id()));
        let mut spool = RemoteSpool::load(&directory).expect("load spool");
        spool.push(record("stream", 1, now_ms())).unwrap();
        spool.push(record("stream", 2, now_ms())).unwrap();
        drop(spool);

        let mut restored = RemoteSpool::load(&directory).expect("restore spool");
        assert_eq!(restored.records.len(), 2);
        restored.acknowledge("stream", 1).unwrap();
        assert_eq!(restored.records.front().unwrap().event.sequence, 2);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn spool_expires_events_older_than_six_hours() {
        let directory =
            std::env::temp_dir().join(format!("remote-log-expiry-test-{}", new_stream_id()));
        let mut spool = RemoteSpool::load(&directory).expect("load spool");
        spool
            .push(record("old", 1, now_ms() - REMOTE_LOG_SPOOL_MAX_AGE_MS - 1))
            .unwrap();
        spool.push(record("new", 1, now_ms())).unwrap();
        assert!(spool.expire_old(now_ms()).unwrap());
        assert_eq!(spool.records.len(), 1);
        assert_eq!(spool.records[0].stream_id, "new");

        drop(spool);
        let restored = RemoteSpool::load(&directory).expect("restore expired spool");
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.records[0].stream_id, "new");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn spool_rekeys_retained_events_when_router_cannot_fill_front_gap() {
        let directory =
            std::env::temp_dir().join(format!("remote-log-gap-test-{}", new_stream_id()));
        let mut spool = RemoteSpool::load(&directory).expect("load spool");
        spool.push(record("stream", 10, now_ms())).unwrap();
        spool.push(record("stream", 11, now_ms())).unwrap();

        assert!(spool.recover_sequence_gap("stream", 1).unwrap());
        assert_eq!(spool.records.len(), 2);
        assert_ne!(spool.records[0].stream_id, "stream");
        assert_eq!(spool.records[0].stream_id, spool.records[1].stream_id);
        assert_eq!(spool.records[0].event.sequence, 1);
        assert_eq!(spool.records[1].event.sequence, 2);

        drop(spool);
        let restored = RemoteSpool::load(&directory).expect("restore rekeyed spool");
        assert_eq!(restored.records[0].event.sequence, 1);
        assert_eq!(restored.records[1].event.sequence, 2);
        let _ = fs::remove_dir_all(directory);
    }
}
