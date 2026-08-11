use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::Context;
use chrono::{TimeZone, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::domain::health::ProviderHealthStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::infra::time::now_ms;

const USAGE_DIRECTORY_NAME: &str = "usage";
const USAGE_MANIFEST_FILE_NAME: &str = "manifest.json";
const USAGE_SNAPSHOT_FILE_NAME: &str = "requests.json";
const USAGE_EVENTS_DIRECTORY_NAME: &str = "events";
const USAGE_ROLLUPS_FILE_NAME: &str = "rollups.json";
const USAGE_DETAIL_RETENTION_DAYS: u128 = 32;
const USAGE_DETAIL_RETENTION_MS: u128 = USAGE_DETAIL_RETENTION_DAYS * USAGE_DAY_MS;
const USAGE_ROLLUP_BUCKET_MS: u128 = 60 * 1000;
const USAGE_DAY_MS: u128 = 24 * 60 * 60 * 1000;
const USAGE_COMPACT_EVERY_EVENTS: u64 = 500;
const USAGE_SCHEMA_VERSION: u8 = 1;
const USAGE_JOURNAL_VERSION: u8 = 1;

pub const CODEX_OVERFLOW_COMPACT_SUMMARY_DATA_SOURCE: &str = "codex_overflow_compact_summary";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRecordKind {
    #[default]
    UserInference,
    InternalSupplemental,
    HealthProbe,
}

impl UsageRecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserInference => "user_inference",
            Self::InternalSupplemental => "internal_supplemental",
            Self::HealthProbe => "health_probe",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageOutcome {
    Pending,
    #[default]
    Success,
    ClientError,
    RateLimited,
    UpstreamError,
    Timeout,
    Interrupted,
    InternalError,
}

impl UsageOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::ClientError => "client_error",
            Self::RateLimited => "rate_limited",
            Self::UpstreamError => "upstream_error",
            Self::Timeout => "timeout",
            Self::Interrupted => "interrupted",
            Self::InternalError => "internal_error",
        }
    }

    pub fn from_status(status_code: u16) -> Self {
        match status_code {
            200..=299 => Self::Success,
            408 | 504 => Self::Timeout,
            429 => Self::RateLimited,
            400..=499 => Self::ClientError,
            500..=599 => Self::UpstreamError,
            _ => Self::InternalError,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UsageManifest {
    schema_version: u8,
    detail_retention_days: u16,
}

impl Default for UsageManifest {
    fn default() -> Self {
        Self {
            schema_version: USAGE_SCHEMA_VERSION,
            detail_retention_days: USAGE_DETAIL_RETENTION_DAYS as u16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageStore {
    pub schema_version: u8,
    #[serde(default)]
    pub logs: Vec<UsageLog>,
    #[serde(default, skip)]
    pub rollups: UsageRollupStore,
    #[serde(default, skip)]
    pub writes_since_compact: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) journal_checkpoint: Option<UsageJournalCheckpoint>,
    #[serde(skip)]
    pub provider_health: ProviderHealthStore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageJournalCheckpoint {
    generation: String,
    through_sequence: u64,
}

impl UsageJournalCheckpoint {
    fn new() -> Self {
        Self {
            generation: generate_journal_generation(),
            through_sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageJournalRecord {
    version: u8,
    generation: String,
    sequence: u64,
    log: UsageLog,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedUsageAppend {
    record: UsageJournalRecord,
}

impl PreparedUsageAppend {
    pub(crate) fn log(&self) -> &UsageLog {
        &self.record.log
    }

    pub(crate) fn persist(&self, config_dir: &Path) -> anyhow::Result<()> {
        append_usage_journal_record(config_dir, &self.record)
    }
}

impl Default for UsageStore {
    fn default() -> Self {
        let checkpoint = UsageJournalCheckpoint::new();
        Self {
            schema_version: USAGE_SCHEMA_VERSION,
            logs: Vec::new(),
            rollups: UsageRollupStore {
                schema_version: USAGE_SCHEMA_VERSION,
                buckets: BTreeMap::new(),
                journal_checkpoint: Some(checkpoint.clone()),
            },
            writes_since_compact: 0,
            journal_checkpoint: Some(checkpoint),
            provider_health: ProviderHealthStore::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageLog {
    pub request_id: String,
    pub record_kind: UsageRecordKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    pub app: AppKind,
    pub bundle_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_id: Option<String>,
    pub supported_apps: Vec<AppKind>,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_display: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_identity_generation: Option<u64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub request_agent: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub actual_model: Option<String>,
    #[serde(default)]
    pub actual_model_source: Option<String>,
    #[serde(default)]
    pub requested_reasoning_effort: Option<String>,
    #[serde(default)]
    pub effective_reasoning_effort: Option<String>,
    #[serde(default)]
    pub client_service_tier: Option<String>,
    #[serde(default)]
    pub effective_service_tier: Option<String>,
    #[serde(default)]
    pub service_tier_decision: Option<String>,
    pub status_code: u16,
    pub outcome: UsageOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    pub duration_ms: u128,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub end_to_end_duration_ms: u128,
    pub upstream_duration_ms: u128,
    pub attempt_count: u32,
    #[serde(default)]
    pub first_token_ms: Option<u128>,
    #[serde(default)]
    pub raw_input_tokens: Option<u64>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub image_count: Option<u32>,
    #[serde(default)]
    pub image_bytes: Option<u64>,
    #[serde(default)]
    pub image_format: Option<String>,
    #[serde(default)]
    pub image_width: Option<u32>,
    #[serde(default)]
    pub image_height: Option<u32>,
    #[serde(default)]
    pub image_size: Option<String>,
    #[serde(default)]
    pub share_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_slug: Option<String>,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub data_source: Option<String>,
    #[serde(default)]
    pub is_health_check: bool,
    #[serde(default)]
    pub is_streaming: bool,
    #[serde(default)]
    pub stream_status: Option<String>,
    #[serde(default)]
    pub usage_state: UsageState,
    #[serde(default)]
    pub usage_revision: u64,
    #[serde(default)]
    pub usage_estimated: bool,
    #[serde(default)]
    pub share_name: Option<String>,
    #[serde(default)]
    pub user_country: Option<String>,
    #[serde(default)]
    pub user_country_iso3: Option<String>,
    #[serde(default)]
    pub router_last_synced_at_ms: Option<u128>,
    #[serde(default)]
    pub router_last_sync_error: Option<String>,
    #[serde(default)]
    pub router_sync_attempt_count: u32,
    #[serde(default)]
    pub router_export_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_synced_usage_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_last_sync_attempt_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_next_sync_attempt_at_ms: Option<u128>,
    pub created_at_ms: u128,
}

impl UsageLog {
    pub fn is_user_inference(&self) -> bool {
        self.record_kind == UsageRecordKind::UserInference
    }

    pub fn processed_tokens(&self) -> u64 {
        self.input_tokens
            .unwrap_or(0)
            .saturating_add(self.output_tokens.unwrap_or(0))
            .saturating_add(self.cache_read_tokens.unwrap_or(0))
            .saturating_add(self.cache_creation_tokens.unwrap_or(0))
    }

    pub fn quota_tokens(&self) -> u64 {
        let processed = self.processed_tokens();
        if processed > 0 {
            processed
        } else {
            self.total_tokens.unwrap_or(0)
        }
    }

    pub fn reset_router_sync_state(&mut self) {
        self.router_last_synced_at_ms = None;
        self.router_last_sync_error = None;
        self.router_sync_attempt_count = 0;
        self.router_synced_usage_revision = None;
        self.router_last_sync_attempt_at_ms = None;
        self.router_next_sync_attempt_at_ms = None;
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageLogContext {
    pub request_id: Option<String>,
    pub record_kind: UsageRecordKind,
    pub parent_request_id: Option<String>,
    pub share_id: Option<String>,
    pub share_name: Option<String>,
    pub share_slug: Option<String>,
    pub user_email: Option<String>,
    pub session_id: Option<String>,
    pub data_source: Option<String>,
    pub user_country: Option<String>,
    pub user_country_iso3: Option<String>,
    pub is_health_check: bool,
    pub is_streaming: bool,
    pub stream_status: Option<String>,
    pub usage_state: Option<UsageState>,
    pub usage_estimated: bool,
    pub error_message: Option<String>,
    pub image: Option<ImageUsageMetadata>,
    pub requested_reasoning_effort: Option<String>,
    pub effective_reasoning_effort: Option<String>,
    pub client_service_tier: Option<String>,
    pub effective_service_tier: Option<String>,
    pub service_tier_decision: Option<String>,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    pub end_to_end_duration_ms: Option<u128>,
    pub upstream_duration_ms: Option<u128>,
    pub attempt_count: Option<u32>,
    pub outcome: Option<UsageOutcome>,
    pub failure_kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageUsageMetadata {
    pub count: u32,
    pub bytes: u64,
    pub format: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    Pending,
    Observed,
    #[default]
    Missing,
    ParseError,
    Interrupted,
}

impl UsageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Observed => "observed",
            Self::Missing => "missing",
            Self::ParseError => "parse_error",
            Self::Interrupted => "interrupted",
        }
    }
}

impl UsageStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = usage_path(config_dir);
        let provider_health = ProviderHealthStore::load_rebuildable(config_dir);
        let usage_dir = usage_directory(config_dir);
        if !usage_dir.exists() {
            let mut store = Self::default();
            store.provider_health = provider_health;
            store.save(config_dir)?;
            return Ok(store);
        }

        let manifest = load_usage_manifest(config_dir)?;
        anyhow::ensure!(
            manifest.schema_version == USAGE_SCHEMA_VERSION,
            "unsupported Usage schema version {}; expected {}",
            manifest.schema_version,
            USAGE_SCHEMA_VERSION
        );
        anyhow::ensure!(
            manifest.detail_retention_days == USAGE_DETAIL_RETENTION_DAYS as u16,
            "Usage detail retention does not match the Server contract"
        );

        let snapshot_exists = path.exists();
        let mut store: UsageStore = if snapshot_exists {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("read usage {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("parse usage {}", path.display()))?
        } else {
            Self::default()
        };
        anyhow::ensure!(
            store.schema_version == USAGE_SCHEMA_VERSION,
            "unsupported Usage snapshot schema version {}; expected {}",
            store.schema_version,
            USAGE_SCHEMA_VERSION
        );
        store.provider_health = provider_health;
        let journal = load_usage_journal(config_dir)?;
        let loaded_rollups = load_usage_rollups(config_dir)?;

        if !snapshot_exists {
            anyhow::ensure!(
                journal.entries.is_empty(),
                "Usage events exist without their authoritative snapshot"
            );
            store.save(config_dir)?;
            return Ok(store);
        }

        let snapshot_checkpoint = store
            .journal_checkpoint
            .clone()
            .context("Usage snapshot is missing its journal checkpoint")?;

        store.rollups =
            compatible_usage_rollups(&store.logs, &snapshot_checkpoint, loaded_rollups, &journal);
        let replayed = replay_versioned_usage_journal(&mut store, &journal, &snapshot_checkpoint);
        store.writes_since_compact = replayed as u64;
        store.trim_recent_window();
        if store.recover_pending_after_restart() {
            store.save(config_dir)?;
        }
        Ok(store)
    }

    fn recover_pending_after_restart(&mut self) -> bool {
        let now = now_ms();
        let mut export_sequence = self
            .journal_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.through_sequence)
            .unwrap_or_default();
        let mut replacements = Vec::new();
        for log in &mut self.logs {
            if log.record_kind != UsageRecordKind::UserInference
                || (log.usage_state != UsageState::Pending && log.outcome != UsageOutcome::Pending)
            {
                continue;
            }
            let previous = log.clone();
            log.usage_state = UsageState::Interrupted;
            log.outcome = UsageOutcome::Interrupted;
            log.failure_kind = Some("server_restarted".to_string());
            log.completed_at_ms = now;
            log.end_to_end_duration_ms = now.saturating_sub(log.started_at_ms);
            log.usage_revision = log.usage_revision.saturating_add(1);
            log.reset_router_sync_state();
            export_sequence = export_sequence.saturating_add(1);
            log.router_export_sequence = export_sequence;
            replacements.push((previous, log.clone()));
        }
        for (previous, updated) in &replacements {
            self.rollups.replace_log(previous, updated);
        }
        if !replacements.is_empty() {
            self.advance_journal_checkpoint(export_sequence);
        }
        !replacements.is_empty()
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        self.save_recent_snapshot(config_dir)?;
        self.save_rollups(config_dir)?;
        save_usage_manifest(config_dir)
    }

    pub fn save_recent_snapshot(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(usage_directory(config_dir)).with_context(|| {
            format!(
                "create Usage directory {}",
                usage_directory(config_dir).display()
            )
        })?;
        let path = usage_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write usage {}", path.display()))
    }

    pub fn save_rollups(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(usage_directory(config_dir)).with_context(|| {
            format!(
                "create Usage directory {}",
                usage_directory(config_dir).display()
            )
        })?;
        let path = usage_rollups_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, &self.rollups)
            .with_context(|| format!("write usage rollups {}", path.display()))
    }

    pub fn push(&mut self, log: UsageLog) {
        if let Some(existing) = self
            .logs
            .iter_mut()
            .find(|existing| existing.request_id == log.request_id)
        {
            let previous = existing.clone();
            self.rollups.replace_log(&previous, &log);
            *existing = log;
            return;
        }
        self.rollups.add_log(&log);
        self.logs.push(log);
        self.trim_recent_window();
    }

    pub fn share_user_quota_usage(
        &self,
        share_id: &str,
        user_email: &str,
        starts_at_ms: i64,
        ends_at_ms: i64,
    ) -> (u64, u64) {
        let (Ok(starts_at_ms), Ok(ends_at_ms)) =
            (u128::try_from(starts_at_ms), u128::try_from(ends_at_ms))
        else {
            return (0, 0);
        };
        let normalized_email = user_email.trim();
        if starts_at_ms >= ends_at_ms {
            return (0, 0);
        }
        let first_full_bucket_ms = ceil_to_usage_bucket(starts_at_ms);
        let after_last_full_bucket_ms = ends_at_ms - (ends_at_ms % USAGE_ROLLUP_BUCKET_MS);
        if first_full_bucket_ms >= after_last_full_bucket_ms {
            return quota_usage_from_logs(
                &self.logs,
                share_id,
                normalized_email,
                starts_at_ms,
                ends_at_ms,
            );
        }

        let rolled_up = self
            .rollups
            .buckets
            .values()
            .filter(|bucket| {
                bucket.bucket_start_ms >= first_full_bucket_ms
                    && bucket.bucket_start_ms < after_last_full_bucket_ms
                    && bucket.share_id == share_id
                    && bucket.user_email.eq_ignore_ascii_case(normalized_email)
            })
            .fold((0u64, 0u64), |(tokens, requests), bucket| {
                (
                    tokens.saturating_add(bucket.stats.quota_tokens),
                    requests.saturating_add(bucket.stats.request_count),
                )
            });
        let edge = self
            .logs
            .iter()
            .filter(|log| {
                let created_at_ms = log.created_at_ms;
                (created_at_ms >= starts_at_ms && created_at_ms < first_full_bucket_ms)
                    || (created_at_ms >= after_last_full_bucket_ms && created_at_ms < ends_at_ms)
            })
            .filter(|log| quota_log_matches(log, share_id, normalized_email))
            .fold((0u64, 0u64), |(tokens, requests), log| {
                (
                    tokens.saturating_add(log.quota_tokens()),
                    requests.saturating_add(u64::from(
                        log.record_kind == UsageRecordKind::UserInference,
                    )),
                )
            });
        (
            rolled_up.0.saturating_add(edge.0),
            rolled_up.1.saturating_add(edge.1),
        )
    }

    fn push_log_only(&mut self, log: UsageLog) {
        if let Some(existing) = self
            .logs
            .iter_mut()
            .find(|existing| existing.request_id == log.request_id)
        {
            *existing = log;
            return;
        }
        self.logs.push(log);
        self.trim_recent_window();
    }

    pub(crate) fn prepare_append(&self, mut log: UsageLog) -> anyhow::Result<PreparedUsageAppend> {
        let checkpoint = self
            .journal_checkpoint
            .as_ref()
            .context("usage journal checkpoint is unavailable")?;
        let sequence = checkpoint.through_sequence.saturating_add(1);
        let existing = self
            .logs
            .iter()
            .find(|existing| existing.request_id == log.request_id);
        if existing.is_none_or(|existing| {
            existing.usage_revision != log.usage_revision || existing.router_export_sequence == 0
        }) {
            log.router_export_sequence = sequence;
        } else if let Some(existing) = existing {
            log.router_export_sequence = existing.router_export_sequence;
        }
        Ok(PreparedUsageAppend {
            record: UsageJournalRecord {
                version: USAGE_JOURNAL_VERSION,
                generation: checkpoint.generation.clone(),
                sequence,
                log,
            },
        })
    }

    pub(crate) fn apply_append(&mut self, append: PreparedUsageAppend) {
        let sequence = append.record.sequence;
        self.push(append.record.log);
        self.advance_journal_checkpoint(sequence);
        self.writes_since_compact = self.writes_since_compact.saturating_add(1);
    }

    fn advance_journal_checkpoint(&mut self, sequence: u64) {
        if let Some(checkpoint) = self.journal_checkpoint.as_mut() {
            checkpoint.through_sequence = sequence;
            self.rollups.journal_checkpoint = Some(checkpoint.clone());
        }
    }

    fn trim_recent_window(&mut self) {
        let cutoff = now_ms().saturating_sub(USAGE_DETAIL_RETENTION_MS);
        self.logs.retain(|log| log.created_at_ms >= cutoff);
    }

    fn compact_due(&self, config_dir: &Path) -> bool {
        self.writes_since_compact >= USAGE_COMPACT_EVERY_EVENTS || !usage_path(config_dir).exists()
    }

    pub(crate) fn needs_compaction(&self, config_dir: &Path) -> bool {
        self.compact_due(config_dir)
    }

    pub(crate) fn compact_if_due(&mut self, config_dir: &Path) -> anyhow::Result<bool> {
        if !self.compact_due(config_dir) {
            return Ok(false);
        }
        self.save_rollups(config_dir)?;
        self.save_recent_snapshot(config_dir)?;
        truncate_usage_journal(config_dir)?;
        self.writes_since_compact = 0;
        Ok(true)
    }

    pub fn latest_filtered(&self, query: UsageLogFilter) -> Vec<UsageLog> {
        self.logs
            .iter()
            .rev()
            .filter(|log| matches_log_filter(log, &query))
            .take(query.limit.unwrap_or(100))
            .cloned()
            .collect()
    }

    pub fn request_detail(&self, request_id: &str) -> Option<UsageLog> {
        self.logs
            .iter()
            .rev()
            .find(|log| log.is_user_inference() && log.request_id == request_id)
            .cloned()
    }
}

fn ceil_to_usage_bucket(value: u128) -> u128 {
    let remainder = value % USAGE_ROLLUP_BUCKET_MS;
    if remainder == 0 {
        value
    } else {
        value.saturating_add(USAGE_ROLLUP_BUCKET_MS - remainder)
    }
}

fn quota_log_matches(log: &UsageLog, share_id: &str, user_email: &str) -> bool {
    is_share_user_quota_record(log)
        && log.share_id.as_deref() == Some(share_id)
        && log
            .user_email
            .as_deref()
            .is_some_and(|email| email.eq_ignore_ascii_case(user_email))
}

fn quota_usage_from_logs(
    logs: &[UsageLog],
    share_id: &str,
    user_email: &str,
    starts_at_ms: u128,
    ends_at_ms: u128,
) -> (u64, u64) {
    logs.iter()
        .filter(|log| log.created_at_ms >= starts_at_ms && log.created_at_ms < ends_at_ms)
        .filter(|log| quota_log_matches(log, share_id, user_email))
        .fold((0u64, 0u64), |(tokens, requests), log| {
            (
                tokens.saturating_add(log.quota_tokens()),
                requests
                    .saturating_add(u64::from(log.record_kind == UsageRecordKind::UserInference)),
            )
        })
}

#[derive(Debug, Clone, Default)]
pub struct UsageLogFilter {
    pub limit: Option<usize>,
    pub from_ms: Option<u128>,
    pub to_ms: Option<u128>,
    pub app: Option<AppKind>,
    pub provider_id: Option<String>,
    pub share_id: Option<String>,
    pub user_email: Option<String>,
    pub session_id: Option<String>,
    pub data_source: Option<String>,
    pub is_health_check: Option<bool>,
    pub stream_status: Option<String>,
}

impl UsageLog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: AppKind,
        provider_id: String,
        provider_name: String,
        provider_type: ProviderType,
        status_code: u16,
        duration_ms: u128,
        model: UsageModelMetadata,
        usage: TokenUsage,
    ) -> Self {
        let completed_at_ms = now_ms();
        let started_at_ms = completed_at_ms.saturating_sub(duration_ms);
        let outcome = UsageOutcome::from_status(status_code);
        Self {
            request_id: generate_request_id(),
            record_kind: UsageRecordKind::UserInference,
            parent_request_id: None,
            app,
            bundle_id: provider_id.clone(),
            family_id: None,
            supported_apps: vec![app],
            provider_id,
            provider_name,
            provider_type,
            profile_id: None,
            account_ref: None,
            account_display: None,
            auth_identity_generation: None,
            model: model.model,
            request_agent: None,
            session_id: None,
            requested_model: model.requested_model,
            actual_model: model.actual_model,
            actual_model_source: model.actual_model_source,
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            client_service_tier: None,
            effective_service_tier: None,
            service_tier_decision: None,
            status_code,
            outcome,
            failure_kind: None,
            error_message: None,
            duration_ms,
            started_at_ms,
            completed_at_ms,
            end_to_end_duration_ms: duration_ms,
            upstream_duration_ms: duration_ms,
            attempt_count: 1,
            first_token_ms: None,
            raw_input_tokens: usage.raw_input_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            total_tokens: usage.total_tokens,
            image_count: None,
            image_bytes: None,
            image_format: None,
            image_width: None,
            image_height: None,
            image_size: None,
            share_id: None,
            share_slug: None,
            user_email: None,
            data_source: None,
            is_health_check: false,
            is_streaming: false,
            stream_status: None,
            usage_state: if usage.has_observation() {
                UsageState::Observed
            } else {
                UsageState::Missing
            },
            usage_revision: 1,
            usage_estimated: false,
            share_name: None,
            user_country: None,
            user_country_iso3: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_sync_attempt_count: 0,
            router_export_sequence: 0,
            router_synced_usage_revision: None,
            router_last_sync_attempt_at_ms: None,
            router_next_sync_attempt_at_ms: None,
            created_at_ms: started_at_ms,
        }
    }

    pub fn apply_context(&mut self, context: UsageLogContext) {
        if let Some(request_id) = context.request_id {
            self.request_id = request_id;
        }
        self.record_kind = if context.is_health_check {
            UsageRecordKind::HealthProbe
        } else if context.data_source.as_deref() == Some(CODEX_OVERFLOW_COMPACT_SUMMARY_DATA_SOURCE)
        {
            UsageRecordKind::InternalSupplemental
        } else {
            context.record_kind
        };
        self.parent_request_id = context.parent_request_id;
        self.share_id = context.share_id;
        self.share_name = context.share_name;
        self.share_slug = context.share_slug;
        self.user_email = context
            .user_email
            .map(|email| email.trim().to_ascii_lowercase())
            .filter(|email| !email.is_empty());
        self.session_id = context.session_id;
        self.data_source = context.data_source;
        self.is_health_check = context.is_health_check;
        self.user_country = context.user_country;
        self.user_country_iso3 = context.user_country_iso3;
        self.is_streaming = context.is_streaming;
        self.stream_status = context.stream_status;
        self.usage_state = context.usage_state.unwrap_or_else(|| {
            if self.stream_status.as_deref() == Some("pending") {
                UsageState::Pending
            } else {
                self.usage_state
            }
        });
        self.usage_estimated = context.usage_estimated;
        self.error_message = context.error_message;
        self.requested_reasoning_effort = context.requested_reasoning_effort;
        self.effective_reasoning_effort = context.effective_reasoning_effort;
        self.client_service_tier = context.client_service_tier;
        self.effective_service_tier = context.effective_service_tier;
        self.service_tier_decision = context.service_tier_decision;
        if let Some(started_at_ms) = context.started_at_ms {
            self.started_at_ms = started_at_ms;
            self.created_at_ms = started_at_ms;
        }
        if self.usage_state == UsageState::Pending {
            self.completed_at_ms = 0;
            self.end_to_end_duration_ms = 0;
        } else {
            self.completed_at_ms = context.completed_at_ms.unwrap_or_else(now_ms);
            self.end_to_end_duration_ms = context
                .end_to_end_duration_ms
                .unwrap_or_else(|| self.completed_at_ms.saturating_sub(self.started_at_ms));
        }
        self.upstream_duration_ms = context.upstream_duration_ms.unwrap_or(self.duration_ms);
        self.attempt_count = context.attempt_count.unwrap_or(1).max(1);
        self.failure_kind = context.failure_kind;
        self.outcome = context.outcome.unwrap_or_else(|| {
            if self.usage_state == UsageState::Pending {
                UsageOutcome::Pending
            } else if self.usage_state == UsageState::Interrupted {
                UsageOutcome::Interrupted
            } else {
                UsageOutcome::from_status(self.status_code)
            }
        });
        if let Some(image) = context.image {
            self.image_count = Some(image.count);
            self.image_bytes = Some(image.bytes);
            self.image_format = image.format;
            self.image_width = image.width;
            self.image_height = image.height;
            self.image_size = image.size;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRollupStore {
    schema_version: u8,
    #[serde(default)]
    buckets: BTreeMap<String, UsageRollupBucket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal_checkpoint: Option<UsageJournalCheckpoint>,
}

impl Default for UsageRollupStore {
    fn default() -> Self {
        Self {
            schema_version: USAGE_SCHEMA_VERSION,
            buckets: BTreeMap::new(),
            journal_checkpoint: None,
        }
    }
}

impl UsageRollupStore {
    fn add_log(&mut self, log: &UsageLog) {
        if !is_share_user_quota_record(log) {
            return;
        }
        let key = usage_rollup_key(log);
        self.buckets
            .entry(key)
            .or_insert_with(|| UsageRollupBucket::new(log))
            .push(log);
    }

    fn remove_log(&mut self, log: &UsageLog) {
        if !is_share_user_quota_record(log) {
            return;
        }
        let key = usage_rollup_key(log);
        let should_remove = if let Some(bucket) = self.buckets.get_mut(&key) {
            bucket.remove(log);
            bucket.stats.is_empty()
        } else {
            false
        };
        if should_remove {
            self.buckets.remove(&key);
        }
    }

    fn replace_log(&mut self, previous: &UsageLog, updated: &UsageLog) {
        self.remove_log(previous);
        self.add_log(updated);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageRollupBucket {
    bucket_start_ms: u128,
    share_id: String,
    user_email: String,
    record_kind: UsageRecordKind,
    stats: UsageQuotaAccumulator,
}

impl UsageRollupBucket {
    fn new(log: &UsageLog) -> Self {
        let bucket_start_ms = log.created_at_ms - (log.created_at_ms % USAGE_ROLLUP_BUCKET_MS);
        Self {
            bucket_start_ms,
            share_id: log.share_id.clone().expect("quota record has a Share ID"),
            user_email: log
                .user_email
                .as_deref()
                .map(|email| email.trim().to_ascii_lowercase())
                .expect("quota record has a user email"),
            record_kind: log.record_kind,
            stats: UsageQuotaAccumulator::default(),
        }
    }

    fn push(&mut self, log: &UsageLog) {
        self.stats.push(log);
    }

    fn remove(&mut self, log: &UsageLog) {
        self.stats.remove(log);
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageModelMetadata {
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub actual_model_source: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub raw_input_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn has_observation(self) -> bool {
        self.raw_input_tokens.is_some()
            || self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_creation_tokens.is_some()
            || self.total_tokens.is_some()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputTokenSemantics {
    /// The upstream input count already includes cache reads and cache writes,
    /// as in OpenAI Responses/Chat and Gemini usage payloads.
    Inclusive,
    /// The upstream input count is fresh input only, as in Anthropic usage.
    Exclusive,
    /// Infer from protocol-specific field shapes. Callers on a known hot path
    /// should prefer an explicit variant.
    #[default]
    Auto,
}

pub fn usage_from_json(value: &serde_json::Value) -> TokenUsage {
    usage_from_json_with_semantics(value, InputTokenSemantics::Auto)
}

pub fn usage_from_json_with_semantics(
    value: &serde_json::Value,
    semantics: InputTokenSemantics,
) -> TokenUsage {
    let usage = preferred_image_tool_token_usage(value)
        .or_else(|| value.get("usage"))
        .or_else(|| value.pointer("/message/usage"))
        .or_else(|| value.pointer("/response/usage"))
        .or_else(|| value.pointer("/response/usageMetadata"))
        .or_else(|| value.pointer("/delta/usage"))
        .or_else(|| value.get("usageMetadata"))
        .unwrap_or(value);
    let input_tokens = first_u64(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
            "promptTokenCount",
            "inputTokenCount",
        ],
    );
    let output_tokens = first_u64(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "completionTokens",
            "outputTokenCount",
        ],
    )
    .or_else(|| {
        let candidates = first_u64(usage, &["candidatesTokenCount"]);
        let thoughts = first_u64(usage, &["thoughtsTokenCount", "thoughts_token_count"]);
        match (candidates, thoughts) {
            (Some(candidates), Some(thoughts)) => Some(candidates.saturating_add(thoughts)),
            (Some(candidates), None) => Some(candidates),
            (None, Some(thoughts)) => Some(thoughts),
            (None, None) => None,
        }
    });
    let cache_read_tokens = first_u64(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read_tokens",
            "cacheReadTokens",
            "cached_tokens",
            "cachedTokens",
            "cachedContentTokenCount",
            "cached_content_token_count",
        ],
    )
    .or_else(|| {
        usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(serde_json::Value::as_u64)
    })
    .or_else(|| {
        usage
            .pointer("/prompt_tokens_details/cached_tokens")
            .and_then(serde_json::Value::as_u64)
    });
    let cache_creation_tokens = first_u64(
        usage,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_creation_tokens",
            "cacheCreationTokens",
            "cacheWriteInputTokens",
            "cache_write_input_tokens",
            "cache_write_tokens",
            "cacheWriteTokens",
        ],
    )
    .or_else(|| {
        usage
            .pointer("/input_tokens_details/cache_creation_tokens")
            .and_then(serde_json::Value::as_u64)
    })
    .or_else(|| {
        usage
            .pointer("/input_tokens_details/cache_write_tokens")
            .and_then(serde_json::Value::as_u64)
    })
    .or_else(|| {
        usage
            .pointer("/prompt_tokens_details/cache_creation_tokens")
            .and_then(serde_json::Value::as_u64)
    })
    .or_else(|| {
        usage
            .pointer("/prompt_tokens_details/cached_creation_tokens")
            .and_then(serde_json::Value::as_u64)
    })
    .or_else(|| {
        usage
            .pointer("/prompt_tokens_details/cache_write_tokens")
            .and_then(serde_json::Value::as_u64)
    });
    let semantics = match semantics {
        InputTokenSemantics::Auto => infer_input_token_semantics(usage),
        explicit => explicit,
    };
    let cache_total = cache_read_tokens
        .unwrap_or(0)
        .saturating_add(cache_creation_tokens.unwrap_or(0));
    let (input_tokens, raw_input_tokens) = match semantics {
        InputTokenSemantics::Inclusive => {
            let fresh = input_tokens.map(|input| input.saturating_sub(cache_total));
            (fresh, input_tokens)
        }
        InputTokenSemantics::Exclusive | InputTokenSemantics::Auto => {
            let raw = input_tokens.map(|input| input.saturating_add(cache_total));
            (input_tokens, raw)
        }
    };
    let total_tokens = first_u64(usage, &["total_tokens", "totalTokens", "totalTokenCount"])
        .or_else(|| {
            if raw_input_tokens.is_some() || output_tokens.is_some() {
                Some(raw_input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0))
            } else if cache_read_tokens.is_some() || cache_creation_tokens.is_some() {
                Some(cache_read_tokens.unwrap_or(0) + cache_creation_tokens.unwrap_or(0))
            } else {
                None
            }
        });

    TokenUsage {
        input_tokens,
        raw_input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        total_tokens,
    }
}

fn preferred_image_tool_token_usage(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .pointer("/response/tool_usage/image_gen")
        .or_else(|| value.pointer("/tool_usage/image_gen"))
        .filter(|usage| {
            [
                "input_tokens",
                "inputTokens",
                "output_tokens",
                "outputTokens",
                "total_tokens",
                "totalTokens",
            ]
            .iter()
            .filter_map(|key| usage.get(*key).and_then(serde_json::Value::as_u64))
            .any(|tokens| tokens > 0)
        })
}

fn infer_input_token_semantics(usage: &serde_json::Value) -> InputTokenSemantics {
    let has_inclusive_shape = usage.get("prompt_tokens").is_some()
        || usage.get("promptTokens").is_some()
        || usage.get("promptTokenCount").is_some()
        || usage.get("inputTokenCount").is_some()
        || usage.get("input_tokens_details").is_some()
        || usage.get("prompt_tokens_details").is_some()
        || usage.get("usageMetadata").is_some();
    if has_inclusive_shape {
        InputTokenSemantics::Inclusive
    } else {
        InputTokenSemantics::Exclusive
    }
}

fn first_u64(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_u64))
}

fn matches_log_filter(log: &UsageLog, query: &UsageLogFilter) -> bool {
    query.from_ms.is_none_or(|from| log.created_at_ms >= from)
        && query.to_ms.is_none_or(|to| log.created_at_ms <= to)
        && query.app.is_none_or(|app| log.app == app)
        && query
            .provider_id
            .as_deref()
            .is_none_or(|provider_id| log.provider_id == provider_id)
        && query
            .share_id
            .as_deref()
            .is_none_or(|share_id| log.share_id.as_deref() == Some(share_id))
        && query
            .user_email
            .as_deref()
            .is_none_or(|user_email| log.user_email.as_deref() == Some(user_email))
        && query
            .session_id
            .as_deref()
            .is_none_or(|session_id| log.session_id.as_deref() == Some(session_id))
        && query
            .data_source
            .as_deref()
            .is_none_or(|source| log.data_source.as_deref() == Some(source))
        && query
            .is_health_check
            .is_none_or(|value| log.is_health_check == value)
        && query
            .stream_status
            .as_deref()
            .is_none_or(|status| log.stream_status.as_deref() == Some(status))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UsageQuotaAccumulator {
    quota_tokens: u64,
    request_count: u64,
    record_count: u64,
}

impl UsageQuotaAccumulator {
    fn push(&mut self, log: &UsageLog) {
        self.quota_tokens = self.quota_tokens.saturating_add(log.quota_tokens());
        if log.record_kind == UsageRecordKind::UserInference {
            self.request_count = self.request_count.saturating_add(1);
        }
        self.record_count = self.record_count.saturating_add(1);
    }

    fn remove(&mut self, log: &UsageLog) {
        self.quota_tokens = self.quota_tokens.saturating_sub(log.quota_tokens());
        if log.record_kind == UsageRecordKind::UserInference {
            self.request_count = self.request_count.saturating_sub(1);
        }
        self.record_count = self.record_count.saturating_sub(1);
    }

    fn is_empty(&self) -> bool {
        self.record_count == 0
    }
}

fn is_share_user_quota_record(log: &UsageLog) -> bool {
    !log.is_health_check
        && log.record_kind != UsageRecordKind::HealthProbe
        && log
            .share_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && log
            .user_email
            .as_deref()
            .is_some_and(|value| !value.is_empty())
}

fn usage_rollup_key(log: &UsageLog) -> String {
    let bucket_start_ms = log.created_at_ms - (log.created_at_ms % USAGE_ROLLUP_BUCKET_MS);
    [
        bucket_start_ms.to_string(),
        log.share_id.clone().unwrap_or_default(),
        log.user_email
            .as_deref()
            .map(|email| email.trim().to_ascii_lowercase())
            .unwrap_or_default(),
        log.record_kind.as_str().to_string(),
    ]
    .join("\u{1f}")
}

pub fn usage_path(config_dir: &Path) -> std::path::PathBuf {
    usage_directory(config_dir).join(USAGE_SNAPSHOT_FILE_NAME)
}

pub fn usage_jsonl_path(config_dir: &Path) -> std::path::PathBuf {
    usage_event_path(config_dir, now_ms())
}

pub fn usage_rollups_path(config_dir: &Path) -> std::path::PathBuf {
    usage_directory(config_dir).join(USAGE_ROLLUPS_FILE_NAME)
}

pub fn usage_directory(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(USAGE_DIRECTORY_NAME)
}

fn usage_manifest_path(config_dir: &Path) -> std::path::PathBuf {
    usage_directory(config_dir).join(USAGE_MANIFEST_FILE_NAME)
}

fn usage_events_directory(config_dir: &Path) -> std::path::PathBuf {
    usage_directory(config_dir).join(USAGE_EVENTS_DIRECTORY_NAME)
}

fn usage_event_path(config_dir: &Path, timestamp_ms: u128) -> std::path::PathBuf {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    let day = Utc
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("Unix epoch is valid")
        })
        .format("%Y-%m-%d");
    usage_events_directory(config_dir).join(format!("{day}.jsonl"))
}

fn load_usage_manifest(config_dir: &Path) -> anyhow::Result<UsageManifest> {
    let path = usage_manifest_path(config_dir);
    let content = fs::read_to_string(&path)
        .with_context(|| format!("read Usage manifest {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parse Usage manifest {}", path.display()))
}

fn save_usage_manifest(config_dir: &Path) -> anyhow::Result<()> {
    let directory = usage_directory(config_dir);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create Usage directory {}", directory.display()))?;
    let path = usage_manifest_path(config_dir);
    crate::infra::storage::write_json_pretty(&path, &UsageManifest::default())
        .with_context(|| format!("write Usage manifest {}", path.display()))
}

fn load_usage_rollups(config_dir: &Path) -> anyhow::Result<Option<UsageRollupStore>> {
    let path = usage_rollups_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("read usage rollups {}", path.display()))?;
    let rollups: UsageRollupStore = serde_json::from_str(&content)
        .with_context(|| format!("parse usage rollups {}", path.display()))?;
    anyhow::ensure!(
        rollups.schema_version == USAGE_SCHEMA_VERSION,
        "unsupported Usage rollup schema version {}; expected {}",
        rollups.schema_version,
        USAGE_SCHEMA_VERSION
    );
    Ok(Some(rollups))
}

#[derive(Debug, Default)]
struct LoadedUsageJournal {
    entries: Vec<LoadedUsageJournalEntry>,
}

#[derive(Debug)]
struct LoadedUsageJournalEntry {
    record: UsageJournalRecord,
}

fn load_usage_journal(config_dir: &Path) -> anyhow::Result<LoadedUsageJournal> {
    let events_directory = usage_events_directory(config_dir);
    if !events_directory.exists() {
        return Ok(LoadedUsageJournal::default());
    }
    let mut paths = fs::read_dir(&events_directory)
        .with_context(|| format!("read Usage events {}", events_directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut journal = LoadedUsageJournal::default();
    for path in paths {
        let file = fs::File::open(&path)
            .with_context(|| format!("open Usage events {}", path.display()))?;
        let reader = BufReader::new(file);
        for (line_number, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read Usage events {}", path.display()))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let record: UsageJournalRecord = serde_json::from_str(line).with_context(|| {
                format!(
                    "parse Usage event {}:{}",
                    path.display(),
                    line_number.saturating_add(1)
                )
            })?;
            anyhow::ensure!(
                record.version == USAGE_JOURNAL_VERSION,
                "unsupported Usage event version {} in {}:{}; expected {}",
                record.version,
                path.display(),
                line_number.saturating_add(1),
                USAGE_JOURNAL_VERSION
            );
            journal.entries.push(LoadedUsageJournalEntry { record });
        }
    }
    Ok(journal)
}

fn append_usage_journal_record(
    config_dir: &Path,
    record: &UsageJournalRecord,
) -> anyhow::Result<()> {
    let directory = usage_events_directory(config_dir);
    fs::create_dir_all(&directory)
        .with_context(|| format!("create Usage events directory {}", directory.display()))?;
    let path = usage_event_path(config_dir, record.log.created_at_ms);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open usage jsonl {}", path.display()))?;
    serde_json::to_writer(&mut file, record)
        .with_context(|| format!("serialize usage jsonl {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("append usage jsonl {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flush usage jsonl {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("sync usage jsonl {}", path.display()))?;
    Ok(())
}

fn truncate_usage_journal(config_dir: &Path) -> anyhow::Result<()> {
    let directory = usage_events_directory(config_dir);
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("read Usage events {}", directory.display()))?
    {
        let path = entry
            .with_context(|| format!("read Usage event entry {}", directory.display()))?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            fs::remove_file(&path)
                .with_context(|| format!("remove compacted Usage events {}", path.display()))?;
        }
    }
    Ok(())
}

fn rebuild_usage_rollups(
    logs: &[UsageLog],
    checkpoint: UsageJournalCheckpoint,
) -> UsageRollupStore {
    let mut rollups = UsageRollupStore {
        schema_version: USAGE_SCHEMA_VERSION,
        buckets: BTreeMap::new(),
        journal_checkpoint: Some(checkpoint),
    };
    for log in logs {
        rollups.add_log(log);
    }
    rollups
}

fn compatible_usage_rollups(
    logs: &[UsageLog],
    snapshot_checkpoint: &UsageJournalCheckpoint,
    loaded: Option<UsageRollupStore>,
    journal: &LoadedUsageJournal,
) -> UsageRollupStore {
    let max_journal_sequence = journal
        .entries
        .iter()
        .filter_map(|entry| {
            (entry.record.generation == snapshot_checkpoint.generation)
                .then_some(entry.record.sequence)
        })
        .max()
        .unwrap_or(snapshot_checkpoint.through_sequence);
    if let Some(rollups) = loaded {
        if rollups
            .journal_checkpoint
            .as_ref()
            .is_some_and(|checkpoint| {
                checkpoint.generation == snapshot_checkpoint.generation
                    && checkpoint.through_sequence >= snapshot_checkpoint.through_sequence
                    && checkpoint.through_sequence <= max_journal_sequence
            })
        {
            return rollups;
        }
    }
    rebuild_usage_rollups(logs, snapshot_checkpoint.clone())
}

fn replay_versioned_usage_journal(
    store: &mut UsageStore,
    journal: &LoadedUsageJournal,
    snapshot_checkpoint: &UsageJournalCheckpoint,
) -> usize {
    let mut records = journal
        .entries
        .iter()
        .filter_map(|entry| {
            (entry.record.generation == snapshot_checkpoint.generation).then_some(&entry.record)
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.sequence);

    let mut log_through = snapshot_checkpoint.through_sequence;
    let mut rollup_through = store
        .rollups
        .journal_checkpoint
        .as_ref()
        .filter(|checkpoint| checkpoint.generation == snapshot_checkpoint.generation)
        .map(|checkpoint| checkpoint.through_sequence)
        .unwrap_or(snapshot_checkpoint.through_sequence);
    let mut replayed = 0;
    for record in records {
        if record.sequence <= log_through {
            continue;
        }
        if record.sequence > rollup_through {
            store.push(record.log.clone());
            rollup_through = record.sequence;
        } else {
            store.push_log_only(record.log.clone());
        }
        log_through = record.sequence;
        replayed += 1;
    }

    store.journal_checkpoint = Some(UsageJournalCheckpoint {
        generation: snapshot_checkpoint.generation.clone(),
        through_sequence: log_through,
    });
    store.rollups.journal_checkpoint = Some(UsageJournalCheckpoint {
        generation: snapshot_checkpoint.generation.clone(),
        through_sequence: rollup_through,
    });
    replayed
}

fn generate_journal_generation() -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn generate_request_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("req_{suffix}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn persisted_test_log(request_id: impl Into<String>, created_at_ms: u128) -> UsageLog {
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: Some(2),
                ..Default::default()
            },
        );
        log.request_id = request_id.into();
        log.created_at_ms = created_at_ms;
        log
    }

    fn persist_test_append(store: &mut UsageStore, dir: &Path, log: UsageLog) {
        let append = store.prepare_append(log).unwrap();
        append.persist(dir).unwrap();
        store.apply_append(append);
    }

    #[test]
    fn parses_openai_and_anthropic_usage_shapes() {
        let openai = usage_from_json(&json!({
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }));
        assert_eq!(openai.input_tokens, Some(10));
        assert_eq!(openai.output_tokens, Some(5));
        assert_eq!(openai.total_tokens, Some(15));

        let anthropic = usage_from_json(&json!({
            "usage": {
                "input_tokens": 7,
                "output_tokens": 3
            }
        }));
        assert_eq!(anthropic.input_tokens, Some(7));
        assert_eq!(anthropic.raw_input_tokens, Some(7));
        assert_eq!(anthropic.output_tokens, Some(3));
        assert_eq!(anthropic.total_tokens, Some(10));
    }

    #[test]
    fn image_tool_usage_only_overrides_response_usage_when_it_has_billed_tokens() {
        let text = usage_from_json(&json!({
            "response": {
                "tool_usage": {
                    "image_gen": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "total_tokens": 0
                    }
                },
                "usage": {"input_tokens": 82, "output_tokens": 48, "total_tokens": 130}
            }
        }));
        assert_eq!(text.input_tokens, Some(82));
        assert_eq!(text.output_tokens, Some(48));
        assert_eq!(text.total_tokens, Some(130));

        let image = usage_from_json(&json!({
            "response": {
                "tool_usage": {
                    "image_gen": {
                        "images": 1,
                        "input_tokens": 34,
                        "output_tokens": 1756,
                        "total_tokens": 1790
                    }
                },
                "usage": {"input_tokens": 5, "output_tokens": 9, "total_tokens": 14}
            }
        }));
        assert_eq!(image.input_tokens, Some(34));
        assert_eq!(image.output_tokens, Some(1756));
        assert_eq!(image.total_tokens, Some(1790));

        let count_only = usage_from_json(&json!({
            "response": {
                "tool_usage": {"image_gen": {"images": 1}},
                "usage": {"input_tokens": 13, "output_tokens": 21, "total_tokens": 34}
            }
        }));
        assert_eq!(count_only.input_tokens, Some(13));
        assert_eq!(count_only.output_tokens, Some(21));
        assert_eq!(count_only.total_tokens, Some(34));
    }

    #[test]
    fn parses_cache_usage_shapes() {
        let usage = usage_from_json(&json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_read_input_tokens": 50,
                "cache_creation_input_tokens": 5
            }
        }));

        assert_eq!(usage.cache_read_tokens, Some(50));
        assert_eq!(usage.cache_creation_tokens, Some(5));
        assert_eq!(usage.raw_input_tokens, Some(155));
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.total_tokens, Some(175));
    }

    #[test]
    fn parses_nested_cache_write_and_preserves_explicit_zero() {
        let written = usage_from_json_with_semantics(
            &json!({
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 4,
                    "input_tokens_details": {
                        "cached_tokens": 60,
                        "cache_write_tokens": 15
                    }
                }
            }),
            InputTokenSemantics::Inclusive,
        );
        assert_eq!(written.input_tokens, Some(25));
        assert_eq!(written.cache_creation_tokens, Some(15));

        let zero = usage_from_json_with_semantics(
            &json!({
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": {"cache_write_tokens": 0}
                }
            }),
            InputTokenSemantics::Inclusive,
        );
        assert_eq!(zero.cache_creation_tokens, Some(0));
    }

    #[test]
    fn explicit_input_semantics_normalize_to_same_four_buckets() {
        let inclusive = usage_from_json_with_semantics(
            &json!({
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 8,
                    "cache_read_input_tokens": 60,
                    "cache_creation_input_tokens": 20
                }
            }),
            InputTokenSemantics::Inclusive,
        );
        let exclusive = usage_from_json_with_semantics(
            &json!({
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 8,
                    "cache_read_input_tokens": 60,
                    "cache_creation_input_tokens": 20
                }
            }),
            InputTokenSemantics::Exclusive,
        );

        for usage in [inclusive, exclusive] {
            assert_eq!(usage.input_tokens, Some(20));
            assert_eq!(usage.raw_input_tokens, Some(100));
            assert_eq!(usage.cache_read_tokens, Some(60));
            assert_eq!(usage.cache_creation_tokens, Some(20));
            assert_eq!(usage.total_tokens, Some(108));
        }
    }

    #[test]
    fn parses_nested_claude_and_codex_response_usage_shapes() {
        let claude = usage_from_json(&json!({
            "message": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "cache_read_input_tokens": 60
                }
            }
        }));
        assert_eq!(claude.input_tokens, Some(100));
        assert_eq!(claude.raw_input_tokens, Some(160));
        assert_eq!(claude.output_tokens, Some(20));

        let codex = usage_from_json(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 80,
                    "output_tokens": 10,
                    "input_tokens_details": {
                        "cached_tokens": 30
                    }
                }
            }
        }));
        assert_eq!(codex.input_tokens, Some(50));
        assert_eq!(codex.cache_read_tokens, Some(30));
    }

    #[test]
    fn parses_gemini_usage_metadata() {
        let usage = usage_from_json(&json!({
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 8,
                "thoughtsTokenCount": 7,
                "cachedContentTokenCount": 4,
                "totalTokenCount": 27
            }
        }));

        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(15));
        assert_eq!(usage.cache_read_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(27));
    }

    #[test]
    fn gemini_thought_tokens_are_counted_without_explicit_total() {
        let usage = usage_from_json(&json!({
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 8,
                "thoughtsTokenCount": 7
            }
        }));

        assert_eq!(usage.raw_input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(15));
        assert_eq!(usage.total_tokens, Some(27));
    }

    #[test]
    fn parses_claude_message_delta_usage_and_cache_aliases() {
        let usage = usage_from_json(&json!({
            "type": "message_delta",
            "usage": {
                "inputTokens": 120,
                "outputTokens": 9,
                "cacheReadInputTokens": 70,
                "cacheWriteInputTokens": 3
            }
        }));

        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(9));
        assert_eq!(usage.cache_read_tokens, Some(70));
        assert_eq!(usage.cache_creation_tokens, Some(3));
        assert_eq!(usage.raw_input_tokens, Some(193));
    }

    #[test]
    fn parses_openai_include_usage_terminal_block() {
        let usage = usage_from_json(&json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 40,
                "completion_tokens": 6,
                "prompt_tokens_details": {
                    "cached_tokens": 25
                }
            }
        }));

        assert_eq!(usage.input_tokens, Some(15));
        assert_eq!(usage.output_tokens, Some(6));
        assert_eq!(usage.cache_read_tokens, Some(25));
        assert_eq!(usage.total_tokens, Some(46));
    }

    #[test]
    fn parses_delta_nested_usage_shape() {
        let usage = usage_from_json(&json!({
            "type": "message_delta",
            "delta": {
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 7,
                    "cache_creation_tokens": 2
                }
            }
        }));

        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.cache_creation_tokens, Some(2));
    }

    #[test]
    fn keeps_cache_only_usage_non_zero() {
        let usage = usage_from_json(&json!({
            "usage": {
                "cache_read_input_tokens": 50,
                "cache_creation_input_tokens": 7
            }
        }));

        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cache_read_tokens, Some(50));
        assert_eq!(usage.cache_creation_tokens, Some(7));
        assert_eq!(usage.total_tokens, Some(57));
    }

    #[test]
    fn filters_latest_usage_by_share_user_source_and_provider() {
        let mut first = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        first.share_id = Some("share-1".to_string());
        first.user_email = Some("user@example.com".to_string());
        first.data_source = Some("market".to_string());

        let mut second = UsageLog::new(
            AppKind::Codex,
            "p2".to_string(),
            "provider 2".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        second.share_id = Some("share-2".to_string());
        second.user_email = Some("other@example.com".to_string());
        second.data_source = Some("direct".to_string());

        let store = UsageStore {
            logs: vec![first, second],
            ..Default::default()
        };
        let logs = store.latest_filtered(UsageLogFilter {
            limit: Some(10),
            from_ms: None,
            to_ms: None,
            app: Some(AppKind::Codex),
            provider_id: Some("p1".to_string()),
            share_id: Some("share-1".to_string()),
            user_email: Some("user@example.com".to_string()),
            session_id: None,
            data_source: Some("market".to_string()),
            is_health_check: Some(false),
            stream_status: None,
        });

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].provider_id, "p1");
        assert_eq!(logs[0].share_id.as_deref(), Some("share-1"));
    }

    #[test]
    fn filters_latest_usage_by_time_range() {
        let mut early = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        early.request_id = "req_early".to_string();
        early.created_at_ms = 1_000;

        let mut in_range = early.clone();
        in_range.request_id = "req_in_range".to_string();
        in_range.created_at_ms = 2_000;

        let mut late = early.clone();
        late.request_id = "req_late".to_string();
        late.created_at_ms = 3_000;

        let store = UsageStore {
            logs: vec![early, in_range, late],
            ..Default::default()
        };
        let logs = store.latest_filtered(UsageLogFilter {
            limit: Some(10),
            from_ms: Some(1_500),
            to_ms: Some(2_500),
            ..Default::default()
        });

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "req_in_range");
    }

    #[test]
    fn push_deduplicates_by_request_id() {
        let mut first = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            500,
            10,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        first.request_id = "req_same".to_string();

        let mut second = first.clone();
        second.status_code = 200;
        second.duration_ms = 20;

        let mut store = UsageStore::default();
        store.push(first);
        store.push(second);

        assert_eq!(store.logs.len(), 1);
        assert_eq!(store.logs[0].status_code, 200);
        assert_eq!(store.logs[0].duration_ms, 20);
    }

    #[test]
    fn share_user_quota_uses_persistent_rollups_beyond_detail_retention() {
        let current_ms = now_ms();
        let expired_ms = current_ms
            .saturating_sub(USAGE_DETAIL_RETENTION_MS + USAGE_ROLLUP_BUCKET_MS)
            / USAGE_ROLLUP_BUCKET_MS
            * USAGE_ROLLUP_BUCKET_MS;
        let mut store = UsageStore::default();
        for (request_id, created_at_ms) in [
            ("req_quota_expired", expired_ms),
            ("req_quota_current", current_ms),
        ] {
            let mut log = UsageLog::new(
                AppKind::Codex,
                "p1".to_string(),
                "provider 1".to_string(),
                ProviderType::Codex,
                200,
                10,
                UsageModelMetadata::default(),
                TokenUsage {
                    total_tokens: Some(7),
                    ..TokenUsage::default()
                },
            );
            log.request_id = request_id.to_string();
            log.share_id = Some("share-1".to_string());
            log.user_email = Some("User@Example.com".to_string());
            log.created_at_ms = created_at_ms;
            store.push(log);
        }

        assert_eq!(store.logs.len(), 1);
        assert_eq!(store.logs[0].request_id, "req_quota_current");
        assert_eq!(
            store.share_user_quota_usage(
                "share-1",
                "user@example.com",
                expired_ms as i64,
                current_ms.saturating_add(60_000) as i64,
            ),
            (14, 2)
        );
    }

    #[test]
    fn share_user_quota_counts_summary_tokens_without_a_second_request() {
        let starts_at_ms = 1_800_000_000_000u128;
        let mut invocation = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage {
                total_tokens: Some(7),
                ..TokenUsage::default()
            },
        );
        invocation.request_id = "req_invocation".to_string();
        invocation.share_id = Some("share-1".to_string());
        invocation.user_email = Some("user@example.com".to_string());
        invocation.created_at_ms = starts_at_ms;

        let mut summary = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage {
                total_tokens: Some(10),
                ..TokenUsage::default()
            },
        );
        summary.request_id = "req_summary".to_string();
        summary.share_id = Some("share-1".to_string());
        summary.user_email = Some("user@example.com".to_string());
        summary.record_kind = UsageRecordKind::InternalSupplemental;
        summary.data_source = Some(CODEX_OVERFLOW_COMPACT_SUMMARY_DATA_SOURCE.to_string());
        summary.created_at_ms = starts_at_ms + 1;

        let mut store = UsageStore::default();
        store.push(invocation);
        store.push(summary);

        assert_eq!(
            store.share_user_quota_usage(
                "share-1",
                "user@example.com",
                starts_at_ms as i64,
                (starts_at_ms + 60_000) as i64,
            ),
            (17, 1)
        );
    }

    #[test]
    fn share_user_quota_uses_exact_half_open_edges_for_unaligned_anchors() {
        let bucket_start_ms = now_ms().saturating_sub(USAGE_ROLLUP_BUCKET_MS)
            / USAGE_ROLLUP_BUCKET_MS
            * USAGE_ROLLUP_BUCKET_MS;
        let mut store = UsageStore::default();
        for (request_id, offset_ms) in [
            ("before", 9_999),
            ("start", 10_000),
            ("middle", 30_000),
            ("end", 50_000),
        ] {
            let mut log = UsageLog::new(
                AppKind::Codex,
                "p1".to_string(),
                "provider 1".to_string(),
                ProviderType::Codex,
                200,
                10,
                UsageModelMetadata::default(),
                TokenUsage {
                    total_tokens: Some(7),
                    ..TokenUsage::default()
                },
            );
            log.request_id = request_id.to_string();
            log.share_id = Some("share-1".to_string());
            log.user_email = Some("user@example.com".to_string());
            log.created_at_ms = bucket_start_ms + offset_ms;
            store.push(log);
        }

        assert_eq!(
            store.share_user_quota_usage(
                "share-1",
                "user@example.com",
                (bucket_start_ms + 10_000) as i64,
                (bucket_start_ms + 50_000) as i64,
            ),
            (14, 2)
        );
    }

    #[test]
    fn request_detail_exposes_only_user_inference_records() {
        let mut inference = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        inference.request_id = "inference".to_string();
        let mut supplemental = inference.clone();
        supplemental.request_id = "supplemental".to_string();
        supplemental.record_kind = UsageRecordKind::InternalSupplemental;
        let store = UsageStore {
            logs: vec![inference, supplemental],
            ..UsageStore::default()
        };

        assert!(store.request_detail("inference").is_some());
        assert!(store.request_detail("supplemental").is_none());
    }

    #[test]
    fn push_deduplicates_router_direct_and_market_request_id() {
        let mut direct = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            36_000,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: Some("glm-5.2".to_string()),
                actual_model_source: Some("model_mapping".to_string()),
            },
            TokenUsage {
                raw_input_tokens: Some(175_000),
                input_tokens: Some(175_000),
                output_tokens: Some(18),
                cache_read_tokens: Some(0),
                cache_creation_tokens: Some(0),
                total_tokens: Some(175_018),
            },
        );
        direct.apply_context(UsageLogContext {
            request_id: Some("router-request-1".to_string()),
            share_id: Some("share-codex".to_string()),
            share_name: Some("route-10wcy".to_string()),
            data_source: Some("direct".to_string()),
            user_country: Some("Japan".to_string()),
            user_country_iso3: Some("JPN".to_string()),
            ..Default::default()
        });

        let mut market = direct.clone();
        market.data_source = Some("market".to_string());
        market.user_email = Some("buyer@example.com".to_string());
        market.duration_ms = 5_157;

        let mut store = UsageStore::default();
        store.push(direct);
        store.push(market);

        assert_eq!(store.logs.len(), 1);
        assert_eq!(store.logs[0].request_id, "router-request-1");
        assert_eq!(store.logs[0].data_source.as_deref(), Some("market"));
        assert_eq!(
            store.logs[0].user_email.as_deref(),
            Some("buyer@example.com")
        );
        assert_eq!(store.logs[0].user_country_iso3.as_deref(), Some("JPN"));
        assert_eq!(store.logs[0].total_tokens, Some(175_018));
    }

    #[test]
    fn token_total_uses_raw_input_plus_output() {
        let usage = usage_from_json(&json!({
            "usage": {
                "input_tokens": 156_605,
                "output_tokens": 18,
                "input_tokens_details": {
                    "cached_tokens": 150_000
                }
            }
        }));

        assert_eq!(usage.raw_input_tokens, Some(156_605));
        assert_eq!(usage.input_tokens, Some(6_605));
        assert_eq!(usage.cache_read_tokens, Some(150_000));
        assert_eq!(usage.output_tokens, Some(18));
        assert_eq!(usage.total_tokens, Some(156_623));
    }

    #[test]
    fn usage_snapshots_cover_stream_statuses_and_health_checks() {
        let mut completed = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            100,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: Some("glm-5.2".to_string()),
                actual_model_source: Some("model_mapping".to_string()),
            },
            TokenUsage {
                raw_input_tokens: Some(100),
                input_tokens: Some(100),
                output_tokens: Some(10),
                cache_read_tokens: Some(60),
                cache_creation_tokens: None,
                total_tokens: Some(110),
            },
        );
        completed.apply_context(UsageLogContext {
            request_id: Some("req_stream".to_string()),
            is_streaming: true,
            stream_status: Some("completed".to_string()),
            is_health_check: true,
            ..Default::default()
        });

        let mut interrupted = completed.clone();
        interrupted.request_id = "req_interrupted".to_string();
        interrupted.status_code = 499;
        interrupted.stream_status = Some("interrupted".to_string());

        let store = UsageStore {
            logs: vec![completed, interrupted],
            ..Default::default()
        };
        let health_checks = store.latest_filtered(UsageLogFilter {
            limit: Some(10),
            is_health_check: Some(true),
            stream_status: Some("completed".to_string()),
            ..Default::default()
        });

        assert_eq!(health_checks.len(), 1);
        assert_eq!(health_checks[0].actual_model.as_deref(), Some("glm-5.2"));
        assert_eq!(health_checks[0].cache_read_tokens, Some(60));
    }

    #[test]
    fn single_persisted_push_appends_jsonl_without_rewriting_snapshot() {
        let dir = std::env::temp_dir().join(format!("cc-switch-server-usage-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save(&dir).unwrap();
        let before = fs::read_to_string(usage_path(&dir)).unwrap();
        let rollups_before = fs::read_to_string(usage_rollups_path(&dir)).unwrap();

        let log = persisted_test_log("req_append_only", now_ms());
        let event_path = usage_event_path(&dir, log.created_at_ms);
        persist_test_append(&mut store, &dir, log);

        let after = fs::read_to_string(usage_path(&dir)).unwrap();
        let rollups_after = fs::read_to_string(usage_rollups_path(&dir)).unwrap();
        let jsonl = fs::read_to_string(event_path).unwrap();
        assert_eq!(before, after);
        assert_eq!(rollups_before, rollups_after);
        assert!(jsonl.contains("req_append_only"));
        assert!(usage_rollups_path(&dir).exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn versioned_journal_replays_new_and_updated_logs_after_restart() {
        let dir =
            std::env::temp_dir().join(format!("cc-switch-server-usage-replay-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save(&dir).unwrap();
        let created_at_ms = now_ms();

        persist_test_append(
            &mut store,
            &dir,
            persisted_test_log("req_replay_first", created_at_ms),
        );

        let second = persisted_test_log("req_replay_second", created_at_ms.saturating_add(1));
        persist_test_append(&mut store, &dir, second.clone());
        let mut updated_second = second;
        updated_second.duration_ms = 55;
        updated_second.input_tokens = Some(7);
        updated_second.output_tokens = Some(3);
        updated_second.total_tokens = Some(10);
        updated_second.stream_status = Some("completed".to_string());
        persist_test_append(&mut store, &dir, updated_second);

        let disk_snapshot = fs::read_to_string(usage_path(&dir)).unwrap();
        assert!(!disk_snapshot.contains("req_replay_first"));
        let journal = fs::read_to_string(usage_event_path(&dir, created_at_ms)).unwrap();
        assert!(journal.contains("\"version\":1"));
        assert!(journal.contains("\"sequence\":3"));

        let loaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(loaded.logs.len(), 2);
        let final_second = loaded
            .logs
            .iter()
            .find(|log| log.request_id == "req_replay_second")
            .unwrap();
        assert_eq!(final_second.duration_ms, 55);
        assert_eq!(final_second.input_tokens, Some(7));
        assert_eq!(final_second.output_tokens, Some(3));
        assert_eq!(final_second.stream_status.as_deref(), Some("completed"));
        let overview = loaded.query_overview(&crate::domain::usage::query::UsageQuery::default());
        assert_eq!(overview.metrics.request_count, 2);
        assert_eq!(overview.metrics.fresh_input_tokens, 8);
        assert_eq!(overview.metrics.output_tokens, 4);

        loaded.save_recent_snapshot(&dir).unwrap();
        let reloaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(reloaded.logs.len(), 2);
        assert_eq!(
            reloaded
                .query_overview(&crate::domain::usage::query::UsageQuery::default())
                .metrics
                .request_count,
            2
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn five_hundredth_journal_write_compacts_snapshot_and_rollups() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-usage-compaction-test-{}",
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save(&dir).unwrap();
        let created_at_ms = now_ms();

        for index in 0..USAGE_COMPACT_EVERY_EVENTS {
            persist_test_append(
                &mut store,
                &dir,
                persisted_test_log(format!("req_compact_{index}"), created_at_ms),
            );
        }

        assert!(store.needs_compaction(&dir));
        assert!(store.compact_if_due(&dir).unwrap());
        assert_eq!(store.writes_since_compact, 0);
        assert!(load_usage_journal(&dir).unwrap().entries.is_empty());

        let snapshot: UsageStore =
            serde_json::from_str(&fs::read_to_string(usage_path(&dir)).unwrap()).unwrap();
        let rollups = load_usage_rollups(&dir).unwrap().unwrap();
        assert_eq!(snapshot.logs.len(), USAGE_COMPACT_EVERY_EVENTS as usize);
        assert_eq!(snapshot.journal_checkpoint, store.journal_checkpoint);
        assert_eq!(rollups.journal_checkpoint, store.journal_checkpoint);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn compacted_store_recovers_after_journal_truncation_and_accepts_new_writes() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-usage-compacted-restart-test-{}",
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save(&dir).unwrap();
        let created_at_ms = now_ms();

        for index in 0..USAGE_COMPACT_EVERY_EVENTS {
            persist_test_append(
                &mut store,
                &dir,
                persisted_test_log(format!("req_restart_{index}"), created_at_ms),
            );
        }
        store.compact_if_due(&dir).unwrap();
        drop(store);

        let mut restarted = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(restarted.logs.len(), USAGE_COMPACT_EVERY_EVENTS as usize);
        assert_eq!(restarted.writes_since_compact, 0);
        persist_test_append(
            &mut restarted,
            &dir,
            persisted_test_log("req_after_restart", created_at_ms.saturating_add(1)),
        );
        drop(restarted);

        let reloaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(reloaded.logs.len(), USAGE_COMPACT_EVERY_EVENTS as usize + 1);
        assert!(reloaded
            .logs
            .iter()
            .any(|log| log.request_id == "req_after_restart"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_compaction_keeps_live_state_and_journal_authoritative() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-usage-compaction-failure-test-{}",
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save(&dir).unwrap();
        let created_at_ms = now_ms();

        for index in 0..USAGE_COMPACT_EVERY_EVENTS {
            persist_test_append(
                &mut store,
                &dir,
                persisted_test_log(format!("req_failed_compact_{index}"), created_at_ms),
            );
        }

        let rollups_path = usage_rollups_path(&dir);
        fs::remove_file(&rollups_path).unwrap();
        fs::create_dir(&rollups_path).unwrap();
        let mut candidate = store.clone();
        assert!(candidate.compact_if_due(&dir).is_err());
        assert_eq!(store.logs.len(), USAGE_COMPACT_EVERY_EVENTS as usize);
        assert_eq!(
            load_usage_journal(&dir).unwrap().entries.len(),
            USAGE_COMPACT_EVERY_EVENTS as usize
        );

        fs::remove_dir(&rollups_path).unwrap();
        store.save_rollups(&dir).unwrap();
        let recovered = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(
            serde_json::to_value(&recovered.logs).unwrap(),
            serde_json::to_value(&store.logs).unwrap()
        );
        assert_eq!(
            recovered
                .query_overview(&crate::domain::usage::query::UsageQuery::default())
                .metrics
                .request_count,
            USAGE_COMPACT_EVERY_EVENTS
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
