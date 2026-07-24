use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::Context;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::domain::health::ProviderHealthStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::infra::time::now_ms;

const USAGE_FILE_NAME: &str = "usage-logs.json";
const USAGE_JSONL_FILE_NAME: &str = "usage-logs.jsonl";
const USAGE_ROLLUPS_FILE_NAME: &str = "usage-rollups.json";
const MAX_USAGE_LOGS: usize = 2_000;
const USAGE_ROLLUP_BUCKET_MS: u128 = 60 * 1000;
const USAGE_DAY_MS: u128 = 24 * 60 * 60 * 1000;
const USAGE_COMPACT_EVERY_EVENTS: u64 = 500;
const USAGE_SCHEMA_VERSION: u8 = 2;
const USAGE_JOURNAL_VERSION: u8 = 2;
const DEFAULT_USAGE_STATS_WINDOW_MS: u128 = 60 * 60 * 1000;
const DEFAULT_USAGE_STATS_LIMIT: usize = 50;

const fn legacy_usage_schema_version() -> u8 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStore {
    #[serde(default = "legacy_usage_schema_version")]
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
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub request_id: String,
    pub app: AppKind,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: ProviderType,
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
    pub status_code: u16,
    #[serde(default)]
    pub error_message: Option<String>,
    pub duration_ms: u128,
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
    pub share_id: Option<String>,
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
    pub created_at_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct UsageLogContext {
    pub request_id: Option<String>,
    pub share_id: Option<String>,
    pub share_name: Option<String>,
    pub user_email: Option<String>,
    pub session_id: Option<String>,
    pub data_source: Option<String>,
    pub user_country: Option<String>,
    pub user_country_iso3: Option<String>,
    pub is_health_check: bool,
    pub is_streaming: bool,
    pub stream_status: Option<String>,
}

impl UsageStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = usage_path(config_dir);
        let jsonl_path = usage_jsonl_path(config_dir);
        let provider_health = ProviderHealthStore::load_rebuildable(config_dir);
        let snapshot_exists = path.exists();
        let mut store = if snapshot_exists {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("read usage {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("parse usage {}", path.display()))?
        } else {
            Self::default()
        };
        store.provider_health = provider_health;
        let snapshot_needs_migration = store.schema_version < USAGE_SCHEMA_VERSION;
        let journal = load_usage_journal(&jsonl_path)?;
        let journal_needs_migration = journal.needs_migration();
        let mut loaded_rollups = load_usage_rollups(config_dir)?;
        let rollups_need_migration = loaded_rollups
            .as_ref()
            .is_some_and(|rollups| rollups.schema_version < USAGE_SCHEMA_VERSION);
        let normalized_rollups = loaded_rollups
            .as_mut()
            .is_some_and(UsageRollupStore::normalize_keys);

        if !snapshot_exists {
            store.schema_version = USAGE_SCHEMA_VERSION;
            for entry in &journal.entries {
                store.push_log_only(entry.log().clone());
            }
            store.trim_recent_window();
            let checkpoint = UsageJournalCheckpoint::new();
            store.journal_checkpoint = Some(checkpoint.clone());
            store.rollups = rebuild_usage_rollups(&store.logs, checkpoint);
            store.writes_since_compact = 0;
            store.save_rollups(config_dir)?;
            store.save_recent_snapshot(config_dir)?;
            if !journal.entries.is_empty() {
                truncate_usage_journal(config_dir)?;
            }
            return Ok(store);
        }

        let Some(snapshot_checkpoint) = store.journal_checkpoint.clone() else {
            let recovered = recover_unambiguous_legacy_journal_tail(&mut store, &journal);
            store.trim_recent_window();
            store.schema_version = USAGE_SCHEMA_VERSION;
            let checkpoint = UsageJournalCheckpoint::new();
            store.journal_checkpoint = Some(checkpoint.clone());
            store.rollups = loaded_rollups
                .unwrap_or_else(|| rebuild_usage_rollups(&store.logs, checkpoint.clone()));
            store.rollups.schema_version = USAGE_SCHEMA_VERSION;
            store.rollups.journal_checkpoint = Some(checkpoint);
            store.writes_since_compact = 0;
            store.save_rollups(config_dir)?;
            store.save_recent_snapshot(config_dir)?;
            truncate_usage_journal(config_dir)?;
            if !journal.entries.is_empty() {
                tracing::warn!(
                    recovered,
                    path = %jsonl_path.display(),
                    "migrated legacy usage journal conservatively; existing request ids remain snapshot-authoritative"
                );
            }
            return Ok(store);
        };

        store.trim_recent_window();
        store.rollups =
            compatible_usage_rollups(&store.logs, &snapshot_checkpoint, loaded_rollups, &journal);
        let replayed = replay_versioned_usage_journal(&mut store, &journal, &snapshot_checkpoint);
        store.writes_since_compact = replayed as u64;
        let normalized_active_rollups = store.rollups.normalize_keys();
        let needs_migration = snapshot_needs_migration
            || journal_needs_migration
            || rollups_need_migration
            || normalized_rollups
            || normalized_active_rollups;
        store.schema_version = USAGE_SCHEMA_VERSION;
        store.rollups.schema_version = USAGE_SCHEMA_VERSION;
        if needs_migration {
            store.save_rollups(config_dir)?;
            store.save_recent_snapshot(config_dir)?;
            truncate_usage_journal(config_dir)?;
            store.writes_since_compact = 0;
            tracing::info!("migrated usage storage to token-only schema");
        }
        Ok(store)
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        self.save_recent_snapshot(config_dir)?;
        self.save_rollups(config_dir)
    }

    pub fn save_recent_snapshot(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = usage_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write usage {}", path.display()))
    }

    pub fn save_rollups(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
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

    pub fn push_and_persist(&mut self, config_dir: &Path, log: UsageLog) -> anyhow::Result<()> {
        self.ensure_journal_checkpoint(config_dir)?;
        let sequence = self.append_usage_journal(config_dir, &log)?;
        self.push(log);
        self.advance_journal_checkpoint(sequence);
        self.save_rollups(config_dir)?;
        self.writes_since_compact = self.writes_since_compact.saturating_add(1);
        if self.compact_due(config_dir) {
            self.save_recent_snapshot(config_dir)?;
            truncate_usage_journal(config_dir)?;
            self.writes_since_compact = 0;
        }
        Ok(())
    }

    pub fn update_log_and_persist<F>(
        &mut self,
        config_dir: &Path,
        request_id: &str,
        update: F,
    ) -> anyhow::Result<Option<UsageLog>>
    where
        F: FnOnce(&mut UsageLog),
    {
        let Some(index) = self
            .logs
            .iter()
            .position(|log| log.request_id == request_id)
        else {
            return Ok(None);
        };
        self.ensure_journal_checkpoint(config_dir)?;
        let previous = self.logs[index].clone();
        let mut updated = previous.clone();
        update(&mut updated);
        let sequence = self.append_usage_journal(config_dir, &updated)?;
        self.rollups.replace_log(&previous, &updated);
        self.logs[index] = updated.clone();
        self.advance_journal_checkpoint(sequence);
        self.save_rollups(config_dir)?;
        self.writes_since_compact = self.writes_since_compact.saturating_add(1);
        if self.compact_due(config_dir) {
            self.save_recent_snapshot(config_dir)?;
            truncate_usage_journal(config_dir)?;
            self.writes_since_compact = 0;
        }
        Ok(Some(updated))
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

    fn ensure_journal_checkpoint(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        if self.journal_checkpoint.is_some() {
            return Ok(());
        }
        let checkpoint = UsageJournalCheckpoint::new();
        self.journal_checkpoint = Some(checkpoint.clone());
        self.rollups.journal_checkpoint = Some(checkpoint);
        self.save_rollups(config_dir)?;
        self.save_recent_snapshot(config_dir)
    }

    fn append_usage_journal(&self, config_dir: &Path, log: &UsageLog) -> anyhow::Result<u64> {
        let checkpoint = self
            .journal_checkpoint
            .as_ref()
            .context("usage journal checkpoint is unavailable")?;
        let sequence = checkpoint.through_sequence.saturating_add(1);
        append_usage_journal_record(
            config_dir,
            &UsageJournalRecord {
                version: USAGE_JOURNAL_VERSION,
                generation: checkpoint.generation.clone(),
                sequence,
                log: log.clone(),
            },
        )?;
        Ok(sequence)
    }

    fn advance_journal_checkpoint(&mut self, sequence: u64) {
        if let Some(checkpoint) = self.journal_checkpoint.as_mut() {
            checkpoint.through_sequence = sequence;
            self.rollups.journal_checkpoint = Some(checkpoint.clone());
        }
    }

    fn trim_recent_window(&mut self) {
        if self.logs.len() > MAX_USAGE_LOGS {
            let excess = self.logs.len() - MAX_USAGE_LOGS;
            self.logs.drain(0..excess);
        }
    }

    fn compact_due(&self, config_dir: &Path) -> bool {
        self.writes_since_compact >= USAGE_COMPACT_EVERY_EVENTS || !usage_path(config_dir).exists()
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

    pub fn rollup(&self) -> UsageRollup {
        if self.rollups.has_data() {
            return self.rollups.rollup_filtered(&UsageStatsFilter::default());
        }
        let mut rollup = UsageRollup::default();
        for log in self.logs.iter().filter(|log| !log.is_health_check) {
            rollup.requests += 1;
            if (200..400).contains(&log.status_code) {
                rollup.successes += 1;
            } else {
                rollup.failures += 1;
            }
            rollup.input_tokens += log.input_tokens.unwrap_or(0);
            rollup.output_tokens += log.output_tokens.unwrap_or(0);
            rollup.cache_read_tokens += log.cache_read_tokens.unwrap_or(0);
            rollup.cache_creation_tokens += log.cache_creation_tokens.unwrap_or(0);
            rollup.total_tokens += log.total_tokens.unwrap_or(0);
        }
        rollup
    }

    pub fn rollup_filtered(&self, query: &UsageStatsFilter) -> UsageRollup {
        if self.rollups.has_data() {
            return self.rollups.rollup_filtered(query);
        }
        let mut rollup = UsageRollup::default();
        for log in self
            .logs
            .iter()
            .filter(|log| matches_stats_filter(log, query))
        {
            add_log_to_rollup(&mut rollup, log);
        }
        rollup
    }

    pub fn summary_by_app(&self, query: &UsageStatsFilter) -> Vec<serde_json::Value> {
        if self.rollups.has_data() {
            return self.rollups.summary_by_app(query);
        }
        let mut by_app = BTreeMap::<String, UsageRollup>::new();
        for log in self
            .logs
            .iter()
            .filter(|log| matches_stats_filter(log, query))
        {
            add_log_to_rollup(by_app.entry(log.app.as_str().to_string()).or_default(), log);
        }
        usage_summary_by_app_items(by_app)
    }

    pub fn trends(&self, query: &UsageStatsFilter) -> Vec<UsageTrendPoint> {
        if self.rollups.has_data() {
            return self.rollups.trends(query);
        }
        let window_ms = query
            .window_ms
            .unwrap_or(DEFAULT_USAGE_STATS_WINDOW_MS)
            .max(1);
        let mut buckets = BTreeMap::<u128, UsageStatsAccumulator>::new();
        for log in self
            .logs
            .iter()
            .filter(|log| matches_stats_filter(log, query))
        {
            let start_ms = log.created_at_ms - (log.created_at_ms % window_ms);
            buckets.entry(start_ms).or_default().push(log);
        }
        let mut points = buckets
            .into_iter()
            .map(|(start_ms, accumulator)| {
                let avg_duration_ms = accumulator.avg_duration_ms();
                let avg_first_token_ms = accumulator.avg_first_token_ms();
                UsageTrendPoint {
                    start_ms,
                    end_ms: start_ms.saturating_add(window_ms),
                    rollup: accumulator.rollup,
                    avg_duration_ms,
                    avg_first_token_ms,
                    last_request_at_ms: accumulator.last_request_at_ms,
                }
            })
            .collect::<Vec<_>>();
        limit_latest_points(&mut points, query.limit);
        points
    }

    pub fn provider_stats(&self, query: &UsageStatsFilter) -> Vec<ProviderUsageStats> {
        if self.rollups.has_data() {
            return self.rollups.provider_stats(query);
        }
        let mut groups = BTreeMap::<String, ProviderUsageAccumulator>::new();
        for log in self
            .logs
            .iter()
            .filter(|log| matches_stats_filter(log, query))
        {
            let key = format!("{}:{}", log.app.as_str(), log.provider_id);
            groups
                .entry(key)
                .or_insert_with(|| ProviderUsageAccumulator::new(log))
                .push(log);
        }
        let mut stats = groups
            .into_values()
            .map(ProviderUsageAccumulator::finish)
            .collect::<Vec<_>>();
        sort_provider_stats(&mut stats);
        stats.truncate(query.limit.unwrap_or(DEFAULT_USAGE_STATS_LIMIT));
        stats
    }

    pub fn model_stats(&self, query: &UsageStatsFilter) -> Vec<ModelUsageStats> {
        if self.rollups.has_data() {
            return self.rollups.model_stats(query);
        }
        let mut groups = BTreeMap::<String, ModelUsageAccumulator>::new();
        for log in self
            .logs
            .iter()
            .filter(|log| matches_stats_filter(log, query))
        {
            let model = usage_model_key(log);
            let key = format!("{}:{model}", log.app.as_str());
            groups
                .entry(key)
                .or_insert_with(|| ModelUsageAccumulator::new(log, model))
                .push(log);
        }
        let mut stats = groups
            .into_values()
            .map(ModelUsageAccumulator::finish)
            .collect::<Vec<_>>();
        sort_model_stats(&mut stats);
        stats.truncate(query.limit.unwrap_or(DEFAULT_USAGE_STATS_LIMIT));
        stats
    }

    pub fn request_detail(&self, request_id: &str) -> Option<UsageLog> {
        self.logs
            .iter()
            .rev()
            .find(|log| log.request_id == request_id)
            .cloned()
    }
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

#[derive(Debug, Clone)]
pub struct UsageStatsFilter {
    pub limit: Option<usize>,
    pub from_ms: Option<u128>,
    pub to_ms: Option<u128>,
    pub window_ms: Option<u128>,
    pub app: Option<AppKind>,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub share_id: Option<String>,
    pub user_email: Option<String>,
    pub session_id: Option<String>,
    pub data_source: Option<String>,
    pub is_health_check: Option<bool>,
    pub stream_status: Option<String>,
}

impl Default for UsageStatsFilter {
    fn default() -> Self {
        Self {
            limit: None,
            from_ms: None,
            to_ms: None,
            window_ms: None,
            app: None,
            provider_id: None,
            provider_name: None,
            model: None,
            share_id: None,
            user_email: None,
            session_id: None,
            data_source: None,
            is_health_check: Some(false),
            stream_status: None,
        }
    }
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
        Self {
            request_id: generate_request_id(),
            app,
            provider_id,
            provider_name,
            provider_type,
            model: model.model,
            request_agent: None,
            session_id: None,
            requested_model: model.requested_model,
            actual_model: model.actual_model,
            actual_model_source: model.actual_model_source,
            status_code,
            error_message: None,
            duration_ms,
            first_token_ms: None,
            raw_input_tokens: usage.raw_input_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            total_tokens: usage.total_tokens,
            share_id: None,
            user_email: None,
            data_source: None,
            is_health_check: false,
            is_streaming: false,
            stream_status: None,
            share_name: None,
            user_country: None,
            user_country_iso3: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_sync_attempt_count: 0,
            created_at_ms: now_ms(),
        }
    }

    pub fn apply_context(&mut self, context: UsageLogContext) {
        if let Some(request_id) = context.request_id {
            self.request_id = request_id;
        }
        self.share_id = context.share_id;
        self.share_name = context.share_name;
        self.user_email = context.user_email;
        self.session_id = context.session_id;
        self.data_source = context.data_source;
        self.is_health_check = context.is_health_check;
        self.user_country = context.user_country;
        self.user_country_iso3 = context.user_country_iso3;
        self.is_streaming = context.is_streaming;
        self.stream_status = context.stream_status;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRollup {
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

impl UsageRollup {
    fn add_assign(&mut self, other: &UsageRollup) {
        self.requests = self.requests.saturating_add(other.requests);
        self.successes = self.successes.saturating_add(other.successes);
        self.failures = self.failures.saturating_add(other.failures);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRollupStore {
    #[serde(default = "legacy_usage_schema_version")]
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
    fn has_data(&self) -> bool {
        !self.buckets.is_empty()
    }

    fn normalize_keys(&mut self) -> bool {
        let previous = std::mem::take(&mut self.buckets);
        let mut changed = false;
        for (previous_key, bucket) in previous {
            let key = usage_rollup_bucket_key(&bucket);
            changed |= key != previous_key;
            if let Some(existing) = self.buckets.get_mut(&key) {
                existing.merge(bucket);
                changed = true;
            } else {
                self.buckets.insert(key, bucket);
            }
        }
        changed
    }

    fn add_log(&mut self, log: &UsageLog) {
        let key = usage_rollup_key(log);
        self.buckets
            .entry(key)
            .or_insert_with(|| UsageRollupBucket::new(log))
            .push(log);
    }

    fn remove_log(&mut self, log: &UsageLog) {
        let key = usage_rollup_key(log);
        let should_remove = if let Some(bucket) = self.buckets.get_mut(&key) {
            bucket.remove(log);
            bucket.stats.rollup.requests == 0
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

    fn rollup_filtered(&self, query: &UsageStatsFilter) -> UsageRollup {
        let mut accumulator = UsageStatsAccumulator::default();
        for bucket in self.buckets.values().filter(|bucket| bucket.matches(query)) {
            accumulator.merge(&bucket.stats);
        }
        accumulator.rollup
    }

    fn summary_by_app(&self, query: &UsageStatsFilter) -> Vec<serde_json::Value> {
        let mut by_app = BTreeMap::<String, UsageRollup>::new();
        for bucket in self.buckets.values().filter(|bucket| bucket.matches(query)) {
            by_app
                .entry(bucket.app.as_str().to_string())
                .or_default()
                .add_assign(&bucket.stats.rollup);
        }
        usage_summary_by_app_items(by_app)
    }

    fn trends(&self, query: &UsageStatsFilter) -> Vec<UsageTrendPoint> {
        let window_ms = query
            .window_ms
            .unwrap_or(DEFAULT_USAGE_STATS_WINDOW_MS)
            .max(1);
        let mut buckets = BTreeMap::<u128, UsageStatsAccumulator>::new();
        for bucket in self.buckets.values().filter(|bucket| bucket.matches(query)) {
            let start_ms = bucket.bucket_start_ms - (bucket.bucket_start_ms % window_ms);
            buckets.entry(start_ms).or_default().merge(&bucket.stats);
        }
        let mut points = buckets
            .into_iter()
            .map(|(start_ms, accumulator)| {
                let avg_duration_ms = accumulator.avg_duration_ms();
                let avg_first_token_ms = accumulator.avg_first_token_ms();
                UsageTrendPoint {
                    start_ms,
                    end_ms: start_ms.saturating_add(window_ms),
                    rollup: accumulator.rollup,
                    avg_duration_ms,
                    avg_first_token_ms,
                    last_request_at_ms: accumulator.last_request_at_ms,
                }
            })
            .collect::<Vec<_>>();
        limit_latest_points(&mut points, query.limit);
        points
    }

    fn provider_stats(&self, query: &UsageStatsFilter) -> Vec<ProviderUsageStats> {
        let mut groups = BTreeMap::<String, ProviderUsageAccumulator>::new();
        for bucket in self.buckets.values().filter(|bucket| bucket.matches(query)) {
            let key = format!("{}:{}", bucket.app.as_str(), bucket.provider_id);
            groups
                .entry(key)
                .or_insert_with(|| {
                    ProviderUsageAccumulator::new_parts(
                        bucket.app,
                        bucket.provider_id.clone(),
                        bucket.provider_name.clone(),
                        bucket.provider_type,
                    )
                })
                .merge(&bucket.stats);
        }
        let mut stats = groups
            .into_values()
            .map(ProviderUsageAccumulator::finish)
            .collect::<Vec<_>>();
        sort_provider_stats(&mut stats);
        stats.truncate(query.limit.unwrap_or(DEFAULT_USAGE_STATS_LIMIT));
        stats
    }

    fn model_stats(&self, query: &UsageStatsFilter) -> Vec<ModelUsageStats> {
        let mut groups = BTreeMap::<String, ModelUsageAccumulator>::new();
        for bucket in self.buckets.values().filter(|bucket| bucket.matches(query)) {
            let key = format!("{}:{}", bucket.app.as_str(), bucket.model);
            groups
                .entry(key)
                .or_insert_with(|| {
                    ModelUsageAccumulator::new_parts(
                        bucket.app,
                        bucket.model.clone(),
                        bucket.requested_model.clone(),
                        bucket.actual_model.clone(),
                        bucket.actual_model_source.clone(),
                    )
                })
                .merge(&bucket.stats);
        }
        let mut stats = groups
            .into_values()
            .map(ModelUsageAccumulator::finish)
            .collect::<Vec<_>>();
        sort_model_stats(&mut stats);
        stats.truncate(query.limit.unwrap_or(DEFAULT_USAGE_STATS_LIMIT));
        stats
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageRollupBucket {
    bucket_start_ms: u128,
    bucket_end_ms: u128,
    day_start_ms: u128,
    app: AppKind,
    provider_id: String,
    provider_name: String,
    provider_type: ProviderType,
    model: String,
    requested_model: Option<String>,
    actual_model: Option<String>,
    actual_model_source: Option<String>,
    share_id: Option<String>,
    user_email: Option<String>,
    session_id: Option<String>,
    data_source: Option<String>,
    #[serde(default)]
    is_health_check: bool,
    stream_status: Option<String>,
    stats: UsageStatsAccumulator,
}

impl UsageRollupBucket {
    fn new(log: &UsageLog) -> Self {
        let bucket_start_ms = log.created_at_ms - (log.created_at_ms % USAGE_ROLLUP_BUCKET_MS);
        let day_start_ms = log.created_at_ms - (log.created_at_ms % USAGE_DAY_MS);
        Self {
            bucket_start_ms,
            bucket_end_ms: bucket_start_ms.saturating_add(USAGE_ROLLUP_BUCKET_MS),
            day_start_ms,
            app: log.app,
            provider_id: log.provider_id.clone(),
            provider_name: log.provider_name.clone(),
            provider_type: log.provider_type,
            model: usage_model_key(log),
            requested_model: log.requested_model.clone(),
            actual_model: log.actual_model.clone(),
            actual_model_source: log.actual_model_source.clone(),
            share_id: log.share_id.clone(),
            user_email: log.user_email.clone(),
            session_id: log.session_id.clone(),
            data_source: log.data_source.clone(),
            is_health_check: log.is_health_check,
            stream_status: log.stream_status.clone(),
            stats: UsageStatsAccumulator::default(),
        }
    }

    fn push(&mut self, log: &UsageLog) {
        self.stats.push(log);
        if self.provider_name.is_empty() && !log.provider_name.is_empty() {
            self.provider_name = log.provider_name.clone();
        }
    }

    fn remove(&mut self, log: &UsageLog) {
        self.stats.remove(log);
    }

    fn merge(&mut self, other: Self) {
        self.bucket_end_ms = self.bucket_end_ms.max(other.bucket_end_ms);
        if self.provider_name.is_empty() {
            self.provider_name = other.provider_name;
        }
        if self.requested_model.is_none() {
            self.requested_model = other.requested_model;
        }
        if self.actual_model.is_none() {
            self.actual_model = other.actual_model;
        }
        if self.actual_model_source.is_none() {
            self.actual_model_source = other.actual_model_source;
        }
        self.stats.merge(&other.stats);
    }

    fn matches(&self, query: &UsageStatsFilter) -> bool {
        query.from_ms.is_none_or(|from| self.bucket_end_ms > from)
            && query.to_ms.is_none_or(|to| self.bucket_start_ms <= to)
            && query.app.is_none_or(|app| self.app == app)
            && query
                .provider_id
                .as_deref()
                .is_none_or(|provider_id| self.provider_id == provider_id)
            && query
                .provider_name
                .as_deref()
                .is_none_or(|provider_name| self.provider_name == provider_name)
            && query
                .model
                .as_deref()
                .is_none_or(|model| self.model == model)
            && query
                .share_id
                .as_deref()
                .is_none_or(|share_id| self.share_id.as_deref() == Some(share_id))
            && query
                .user_email
                .as_deref()
                .is_none_or(|user_email| self.user_email.as_deref() == Some(user_email))
            && query
                .session_id
                .as_deref()
                .is_none_or(|session_id| self.session_id.as_deref() == Some(session_id))
            && query
                .data_source
                .as_deref()
                .is_none_or(|source| self.data_source.as_deref() == Some(source))
            && query
                .is_health_check
                .is_none_or(|value| self.is_health_check == value)
            && query
                .stream_status
                .as_deref()
                .is_none_or(|status| self.stream_status.as_deref() == Some(status))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTrendPoint {
    pub start_ms: u128,
    pub end_ms: u128,
    pub rollup: UsageRollup,
    pub avg_duration_ms: Option<f64>,
    pub avg_first_token_ms: Option<f64>,
    pub last_request_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageStats {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: ProviderType,
    pub rollup: UsageRollup,
    pub avg_duration_ms: Option<f64>,
    pub avg_first_token_ms: Option<f64>,
    pub last_request_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageStats {
    pub app: AppKind,
    pub model: String,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub actual_model_source: Option<String>,
    pub rollup: UsageRollup,
    pub avg_duration_ms: Option<f64>,
    pub avg_first_token_ms: Option<f64>,
    pub last_request_at_ms: Option<u128>,
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
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/message/usage"))
        .or_else(|| value.pointer("/response/usage"))
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
            "candidatesTokenCount",
            "outputTokenCount",
        ],
    );
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

fn matches_stats_filter(log: &UsageLog, query: &UsageStatsFilter) -> bool {
    query.from_ms.is_none_or(|from| log.created_at_ms >= from)
        && query.to_ms.is_none_or(|to| log.created_at_ms <= to)
        && query.app.is_none_or(|app| log.app == app)
        && query
            .provider_id
            .as_deref()
            .is_none_or(|provider_id| log.provider_id == provider_id)
        && query
            .provider_name
            .as_deref()
            .is_none_or(|provider_name| log.provider_name == provider_name)
        && query
            .model
            .as_deref()
            .is_none_or(|model| usage_model_key(log) == model)
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

fn usage_summary_view(rollup: &UsageRollup) -> serde_json::Value {
    let success_rate = if rollup.requests > 0 {
        (rollup.successes as f32 / rollup.requests as f32) * 100.0
    } else {
        0.0
    };
    let cacheable_input =
        rollup.input_tokens + rollup.cache_creation_tokens + rollup.cache_read_tokens;
    let cache_hit_rate = if cacheable_input > 0 {
        rollup.cache_read_tokens as f64 / cacheable_input as f64
    } else {
        0.0
    };
    let real_total_tokens = rollup.input_tokens
        + rollup.output_tokens
        + rollup.cache_creation_tokens
        + rollup.cache_read_tokens;
    serde_json::json!({
        "totalRequests": rollup.requests,
        "totalInputTokens": rollup.input_tokens,
        "totalOutputTokens": rollup.output_tokens,
        "totalCacheCreationTokens": rollup.cache_creation_tokens,
        "totalCacheReadTokens": rollup.cache_read_tokens,
        "successRate": success_rate,
        "realTotalTokens": real_total_tokens,
        "cacheHitRate": cache_hit_rate,
    })
}

fn usage_summary_by_app_items(by_app: BTreeMap<String, UsageRollup>) -> Vec<serde_json::Value> {
    let mut items = by_app
        .into_iter()
        .map(|(app_type, rollup)| {
            let summary = usage_summary_view(&rollup);
            (app_type, summary)
        })
        .filter(|(_, summary)| {
            summary["totalRequests"].as_u64().unwrap_or(0) > 0
                || summary["realTotalTokens"].as_u64().unwrap_or(0) > 0
        })
        .map(|(app_type, summary)| {
            serde_json::json!({
                "appType": app_type,
                "summary": summary,
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        let left_tokens = left["summary"]["realTotalTokens"].as_u64().unwrap_or(0);
        let right_tokens = right["summary"]["realTotalTokens"].as_u64().unwrap_or(0);
        right_tokens.cmp(&left_tokens)
    });
    items
}

fn add_log_to_rollup(rollup: &mut UsageRollup, log: &UsageLog) {
    rollup.requests += 1;
    if (200..400).contains(&log.status_code) {
        rollup.successes += 1;
    } else {
        rollup.failures += 1;
    }
    rollup.input_tokens += log.input_tokens.unwrap_or(0);
    rollup.output_tokens += log.output_tokens.unwrap_or(0);
    rollup.cache_read_tokens += log.cache_read_tokens.unwrap_or(0);
    rollup.cache_creation_tokens += log.cache_creation_tokens.unwrap_or(0);
    rollup.total_tokens += log.total_tokens.unwrap_or(0);
}

fn subtract_log_from_rollup(rollup: &mut UsageRollup, log: &UsageLog) {
    rollup.requests = rollup.requests.saturating_sub(1);
    if (200..400).contains(&log.status_code) {
        rollup.successes = rollup.successes.saturating_sub(1);
    } else {
        rollup.failures = rollup.failures.saturating_sub(1);
    }
    rollup.input_tokens = rollup
        .input_tokens
        .saturating_sub(log.input_tokens.unwrap_or(0));
    rollup.output_tokens = rollup
        .output_tokens
        .saturating_sub(log.output_tokens.unwrap_or(0));
    rollup.cache_read_tokens = rollup
        .cache_read_tokens
        .saturating_sub(log.cache_read_tokens.unwrap_or(0));
    rollup.cache_creation_tokens = rollup
        .cache_creation_tokens
        .saturating_sub(log.cache_creation_tokens.unwrap_or(0));
    rollup.total_tokens = rollup
        .total_tokens
        .saturating_sub(log.total_tokens.unwrap_or(0));
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UsageStatsAccumulator {
    rollup: UsageRollup,
    duration_sum_ms: u128,
    duration_count: u64,
    first_token_sum_ms: u128,
    first_token_count: u64,
    last_request_at_ms: Option<u128>,
}

impl UsageStatsAccumulator {
    fn push(&mut self, log: &UsageLog) {
        add_log_to_rollup(&mut self.rollup, log);
        self.duration_sum_ms = self.duration_sum_ms.saturating_add(log.duration_ms);
        self.duration_count = self.duration_count.saturating_add(1);
        if let Some(first_token_ms) = log.first_token_ms {
            self.first_token_sum_ms = self.first_token_sum_ms.saturating_add(first_token_ms);
            self.first_token_count = self.first_token_count.saturating_add(1);
        }
        self.last_request_at_ms = Some(
            self.last_request_at_ms
                .map(|last| last.max(log.created_at_ms))
                .unwrap_or(log.created_at_ms),
        );
    }

    fn remove(&mut self, log: &UsageLog) {
        subtract_log_from_rollup(&mut self.rollup, log);
        self.duration_sum_ms = self.duration_sum_ms.saturating_sub(log.duration_ms);
        self.duration_count = self.duration_count.saturating_sub(1);
        if let Some(first_token_ms) = log.first_token_ms {
            self.first_token_sum_ms = self.first_token_sum_ms.saturating_sub(first_token_ms);
            self.first_token_count = self.first_token_count.saturating_sub(1);
        }
        if self.rollup.requests == 0 {
            self.last_request_at_ms = None;
        }
    }

    fn merge(&mut self, other: &UsageStatsAccumulator) {
        self.rollup.add_assign(&other.rollup);
        self.duration_sum_ms = self.duration_sum_ms.saturating_add(other.duration_sum_ms);
        self.duration_count = self.duration_count.saturating_add(other.duration_count);
        self.first_token_sum_ms = self
            .first_token_sum_ms
            .saturating_add(other.first_token_sum_ms);
        self.first_token_count = self
            .first_token_count
            .saturating_add(other.first_token_count);
        self.last_request_at_ms = match (self.last_request_at_ms, other.last_request_at_ms) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
    }

    fn avg_duration_ms(&self) -> Option<f64> {
        (self.duration_count > 0).then(|| self.duration_sum_ms as f64 / self.duration_count as f64)
    }

    fn avg_first_token_ms(&self) -> Option<f64> {
        (self.first_token_count > 0)
            .then(|| self.first_token_sum_ms as f64 / self.first_token_count as f64)
    }
}

struct ProviderUsageAccumulator {
    app: AppKind,
    provider_id: String,
    provider_name: String,
    provider_type: ProviderType,
    stats: UsageStatsAccumulator,
}

impl ProviderUsageAccumulator {
    fn new_parts(
        app: AppKind,
        provider_id: String,
        provider_name: String,
        provider_type: ProviderType,
    ) -> Self {
        Self {
            app,
            provider_id,
            provider_name,
            provider_type,
            stats: UsageStatsAccumulator::default(),
        }
    }

    fn new(log: &UsageLog) -> Self {
        Self::new_parts(
            log.app,
            log.provider_id.clone(),
            log.provider_name.clone(),
            log.provider_type,
        )
    }

    fn push(&mut self, log: &UsageLog) {
        self.stats.push(log);
        if self.provider_name.is_empty() && !log.provider_name.is_empty() {
            self.provider_name = log.provider_name.clone();
        }
    }

    fn merge(&mut self, stats: &UsageStatsAccumulator) {
        self.stats.merge(stats);
    }

    fn finish(self) -> ProviderUsageStats {
        let avg_duration_ms = self.stats.avg_duration_ms();
        let avg_first_token_ms = self.stats.avg_first_token_ms();
        ProviderUsageStats {
            app: self.app,
            provider_id: self.provider_id,
            provider_name: self.provider_name,
            provider_type: self.provider_type,
            rollup: self.stats.rollup,
            avg_duration_ms,
            avg_first_token_ms,
            last_request_at_ms: self.stats.last_request_at_ms,
        }
    }
}

struct ModelUsageAccumulator {
    app: AppKind,
    model: String,
    requested_model: Option<String>,
    actual_model: Option<String>,
    actual_model_source: Option<String>,
    stats: UsageStatsAccumulator,
}

impl ModelUsageAccumulator {
    fn new_parts(
        app: AppKind,
        model: String,
        requested_model: Option<String>,
        actual_model: Option<String>,
        actual_model_source: Option<String>,
    ) -> Self {
        Self {
            app,
            model,
            requested_model,
            actual_model,
            actual_model_source,
            stats: UsageStatsAccumulator::default(),
        }
    }

    fn new(log: &UsageLog, model: String) -> Self {
        Self::new_parts(
            log.app,
            model,
            log.requested_model.clone(),
            log.actual_model.clone(),
            log.actual_model_source.clone(),
        )
    }

    fn push(&mut self, log: &UsageLog) {
        self.stats.push(log);
        self.requested_model = self
            .requested_model
            .clone()
            .or_else(|| log.requested_model.clone());
        self.actual_model = self
            .actual_model
            .clone()
            .or_else(|| log.actual_model.clone());
        self.actual_model_source = self
            .actual_model_source
            .clone()
            .or_else(|| log.actual_model_source.clone());
    }

    fn merge(&mut self, stats: &UsageStatsAccumulator) {
        self.stats.merge(stats);
    }

    fn finish(self) -> ModelUsageStats {
        let avg_duration_ms = self.stats.avg_duration_ms();
        let avg_first_token_ms = self.stats.avg_first_token_ms();
        ModelUsageStats {
            app: self.app,
            model: self.model,
            requested_model: self.requested_model,
            actual_model: self.actual_model,
            actual_model_source: self.actual_model_source,
            rollup: self.stats.rollup,
            avg_duration_ms,
            avg_first_token_ms,
            last_request_at_ms: self.stats.last_request_at_ms,
        }
    }
}

fn usage_model_key(log: &UsageLog) -> String {
    log.actual_model
        .as_deref()
        .or(log.requested_model.as_deref())
        .or(log.model.as_deref())
        .unwrap_or("unknown")
        .to_string()
}

fn usage_rollup_key(log: &UsageLog) -> String {
    let bucket_start_ms = log.created_at_ms - (log.created_at_ms % USAGE_ROLLUP_BUCKET_MS);
    [
        bucket_start_ms.to_string(),
        log.app.as_str().to_string(),
        log.provider_id.clone(),
        log.provider_type.as_str().to_string(),
        usage_model_key(log),
        log.share_id.clone().unwrap_or_default(),
        log.user_email.clone().unwrap_or_default(),
        log.session_id.clone().unwrap_or_default(),
        log.data_source.clone().unwrap_or_default(),
        log.is_health_check.to_string(),
        log.stream_status.clone().unwrap_or_default(),
    ]
    .join("\u{1f}")
}

fn usage_rollup_bucket_key(bucket: &UsageRollupBucket) -> String {
    [
        bucket.bucket_start_ms.to_string(),
        bucket.app.as_str().to_string(),
        bucket.provider_id.clone(),
        bucket.provider_type.as_str().to_string(),
        bucket.model.clone(),
        bucket.share_id.clone().unwrap_or_default(),
        bucket.user_email.clone().unwrap_or_default(),
        bucket.session_id.clone().unwrap_or_default(),
        bucket.data_source.clone().unwrap_or_default(),
        bucket.is_health_check.to_string(),
        bucket.stream_status.clone().unwrap_or_default(),
    ]
    .join("\u{1f}")
}

fn sort_provider_stats(stats: &mut [ProviderUsageStats]) {
    stats.sort_by(|left, right| {
        right
            .rollup
            .total_tokens
            .cmp(&left.rollup.total_tokens)
            .then(right.rollup.requests.cmp(&left.rollup.requests))
            .then(left.app.as_str().cmp(right.app.as_str()))
            .then(left.provider_id.cmp(&right.provider_id))
    });
}

fn sort_model_stats(stats: &mut [ModelUsageStats]) {
    stats.sort_by(|left, right| {
        right
            .rollup
            .total_tokens
            .cmp(&left.rollup.total_tokens)
            .then(right.rollup.requests.cmp(&left.rollup.requests))
            .then(left.app.as_str().cmp(right.app.as_str()))
            .then(left.model.cmp(&right.model))
    });
}

fn limit_latest_points(points: &mut Vec<UsageTrendPoint>, limit: Option<usize>) {
    let limit = limit.unwrap_or(DEFAULT_USAGE_STATS_LIMIT);
    if points.len() <= limit {
        return;
    }
    let keep_from = points.len() - limit;
    points.drain(0..keep_from);
}

pub fn usage_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(USAGE_FILE_NAME)
}

pub fn usage_jsonl_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(USAGE_JSONL_FILE_NAME)
}

pub fn usage_rollups_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(USAGE_ROLLUPS_FILE_NAME)
}

fn load_usage_rollups(config_dir: &Path) -> anyhow::Result<Option<UsageRollupStore>> {
    let path = usage_rollups_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("read usage rollups {}", path.display()))?;
    let rollups = serde_json::from_str(&content)
        .with_context(|| format!("parse usage rollups {}", path.display()))?;
    Ok(Some(rollups))
}

#[derive(Debug, Default)]
struct LoadedUsageJournal {
    entries: Vec<LoadedUsageJournalEntry>,
}

impl LoadedUsageJournal {
    fn needs_migration(&self) -> bool {
        self.entries.iter().any(|entry| match entry {
            LoadedUsageJournalEntry::Legacy(_) => true,
            LoadedUsageJournalEntry::Versioned(record) => record.version < USAGE_JOURNAL_VERSION,
        })
    }
}

#[derive(Debug)]
enum LoadedUsageJournalEntry {
    Legacy(UsageLog),
    Versioned(UsageJournalRecord),
}

impl LoadedUsageJournalEntry {
    fn log(&self) -> &UsageLog {
        match self {
            Self::Legacy(log) => log,
            Self::Versioned(record) => &record.log,
        }
    }
}

fn load_usage_journal(path: &Path) -> anyhow::Result<LoadedUsageJournal> {
    if !path.exists() {
        return Ok(LoadedUsageJournal::default());
    }
    let file =
        fs::File::open(path).with_context(|| format!("open usage jsonl {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut journal = LoadedUsageJournal::default();
    for line in reader.lines() {
        let line = line.with_context(|| format!("read usage jsonl {}", path.display()))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<UsageJournalRecord>(line) {
            if (1..=USAGE_JOURNAL_VERSION).contains(&record.version) {
                journal
                    .entries
                    .push(LoadedUsageJournalEntry::Versioned(record));
            } else {
                tracing::warn!(
                    version = record.version,
                    path = %path.display(),
                    "skip unsupported usage journal record"
                );
            }
            continue;
        }
        match serde_json::from_str::<UsageLog>(line) {
            Ok(log) => journal.entries.push(LoadedUsageJournalEntry::Legacy(log)),
            Err(error) => tracing::warn!(
                error = %error,
                path = %path.display(),
                "skip malformed usage jsonl line"
            ),
        }
    }
    Ok(journal)
}

fn append_usage_journal_record(
    config_dir: &Path,
    record: &UsageJournalRecord,
) -> anyhow::Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("create config dir {}", config_dir.display()))?;
    let path = usage_jsonl_path(config_dir);
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
    Ok(())
}

fn truncate_usage_journal(config_dir: &Path) -> anyhow::Result<()> {
    let path = usage_jsonl_path(config_dir);
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("truncate usage jsonl {}", path.display()))?;
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
        .filter_map(|entry| match entry {
            LoadedUsageJournalEntry::Versioned(record)
                if record.generation == snapshot_checkpoint.generation =>
            {
                Some(record.sequence)
            }
            _ => None,
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
        .filter_map(|entry| match entry {
            LoadedUsageJournalEntry::Versioned(record)
                if record.generation == snapshot_checkpoint.generation =>
            {
                Some(record)
            }
            _ => None,
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

fn recover_unambiguous_legacy_journal_tail(
    store: &mut UsageStore,
    journal: &LoadedUsageJournal,
) -> usize {
    let snapshot_ids = store
        .logs
        .iter()
        .map(|log| log.request_id.as_str())
        .collect::<HashSet<_>>();
    let snapshot_latest = store.logs.iter().map(|log| log.created_at_ms).max();
    let mut candidates = BTreeMap::<String, UsageLog>::new();
    for entry in &journal.entries {
        let log = entry.log();
        if snapshot_ids.contains(log.request_id.as_str())
            || snapshot_latest.is_some_and(|latest| log.created_at_ms <= latest)
        {
            continue;
        }
        candidates.insert(log.request_id.clone(), log.clone());
    }
    let mut recovered = candidates.into_values().collect::<Vec<_>>();
    recovered.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then(left.request_id.cmp(&right.request_id))
    });
    let count = recovered.len();
    for log in recovered {
        store.push_log_only(log);
    }
    count
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
                "cachedContentTokenCount": 4,
                "totalTokenCount": 20
            }
        }));

        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.cache_read_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(20));
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
    fn business_usage_stats_exclude_health_checks_by_default() {
        let mut business = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                ..Default::default()
            },
        );
        business.request_id = "business".to_string();

        let mut health_check = business.clone();
        health_check.request_id = "health".to_string();
        health_check.status_code = 599;
        health_check.is_health_check = true;
        health_check.input_tokens = Some(100);
        health_check.output_tokens = Some(50);
        health_check.total_tokens = Some(150);

        let mut store = UsageStore::default();
        store.push(business);
        store.push(health_check);

        let business_rollup = store.rollup();
        assert_eq!(business_rollup.requests, 1);
        assert_eq!(business_rollup.successes, 1);
        assert_eq!(business_rollup.failures, 0);
        assert_eq!(business_rollup.total_tokens, 15);
        assert_eq!(
            store.rollup_filtered(&UsageStatsFilter::default()).requests,
            1
        );

        let health_rollup = store.rollup_filtered(&UsageStatsFilter {
            is_health_check: Some(true),
            ..UsageStatsFilter::default()
        });
        assert_eq!(health_rollup.requests, 1);
        assert_eq!(health_rollup.failures, 1);
        assert_eq!(health_rollup.total_tokens, 150);
    }

    #[test]
    fn usage_stats_share_one_filter_fixture() {
        let mut codex = UsageLog::new(
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
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                ..Default::default()
            },
        );
        codex.request_id = "req_codex".to_string();
        codex.created_at_ms = 10_000;
        codex.first_token_ms = Some(25);
        codex.data_source = Some("direct".to_string());

        let mut claude = UsageLog::new(
            AppKind::Claude,
            "p2".to_string(),
            "provider 2".to_string(),
            ProviderType::Claude,
            500,
            200,
            UsageModelMetadata {
                model: Some("claude-sonnet".to_string()),
                requested_model: Some("claude-sonnet".to_string()),
                actual_model: None,
                actual_model_source: None,
            },
            TokenUsage {
                input_tokens: Some(20),
                output_tokens: Some(10),
                total_tokens: Some(30),
                ..Default::default()
            },
        );
        claude.request_id = "req_claude".to_string();
        claude.created_at_ms = 20_000;
        claude.data_source = Some("market".to_string());

        let store = UsageStore {
            logs: vec![codex, claude],
            ..Default::default()
        };
        let filter = UsageStatsFilter {
            from_ms: Some(0),
            to_ms: Some(15_000),
            window_ms: Some(10_000),
            data_source: Some("direct".to_string()),
            ..Default::default()
        };

        let summary = store.rollup_filtered(&filter);
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.successes, 1);
        assert_eq!(summary.total_tokens, 15);

        let trends = store.trends(&filter);
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].start_ms, 10_000);
        assert_eq!(trends[0].rollup.total_tokens, 15);

        let providers = store.provider_stats(&filter);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider_id, "p1");
        assert_eq!(providers[0].avg_first_token_ms, Some(25.0));

        let models = store.model_stats(&filter);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "glm-5.2");
        assert_eq!(models[0].rollup.total_tokens, 15);

        let detail = store.request_detail("req_codex").unwrap();
        assert_eq!(detail.actual_model.as_deref(), Some("glm-5.2"));
    }

    #[test]
    fn rollups_keep_token_totals_after_recent_window_trims() {
        let mut store = UsageStore::default();
        for index in 0..3_000 {
            let mut log = UsageLog::new(
                AppKind::Codex,
                "p1".to_string(),
                "provider 1".to_string(),
                ProviderType::Codex,
                200,
                10,
                UsageModelMetadata {
                    model: Some("gpt-5.5".to_string()),
                    requested_model: Some("gpt-5.5".to_string()),
                    actual_model: Some("gpt-5.5".to_string()),
                    actual_model_source: Some("test".to_string()),
                },
                TokenUsage {
                    input_tokens: Some(1),
                    output_tokens: Some(1),
                    total_tokens: Some(2),
                    ..Default::default()
                },
            );
            log.request_id = format!("req_{index}");
            log.created_at_ms = 1_000_000 + index;
            store.push(log);
        }

        assert_eq!(store.logs.len(), MAX_USAGE_LOGS);
        let filter = UsageStatsFilter {
            from_ms: Some(0),
            app: Some(AppKind::Codex),
            provider_id: Some("p1".to_string()),
            ..Default::default()
        };
        let summary = store.rollup_filtered(&filter);
        assert_eq!(summary.requests, 3_000);
        assert_eq!(summary.total_tokens, 6_000);
        let by_app = store.summary_by_app(&filter);
        assert_eq!(by_app[0]["summary"]["totalRequests"], 3_000);
        assert_eq!(by_app[0]["summary"]["realTotalTokens"], 6_000);
    }

    #[test]
    fn summary_by_app_groups_logs_and_sorts_by_tokens() {
        let mut store = UsageStore::default();
        for (app, request_id, input_tokens) in [
            (AppKind::Claude, "req_claude", 100),
            (AppKind::Codex, "req_codex", 300),
            (AppKind::Gemini, "req_gemini", 200),
        ] {
            let mut log = UsageLog::new(
                app,
                "p1".to_string(),
                "provider 1".to_string(),
                ProviderType::Claude,
                200,
                10,
                UsageModelMetadata::default(),
                TokenUsage {
                    input_tokens: Some(input_tokens),
                    output_tokens: Some(1),
                    total_tokens: Some(input_tokens + 1),
                    ..Default::default()
                },
            );
            log.request_id = request_id.to_string();
            store.push(log);
        }

        let items = store.summary_by_app(&UsageStatsFilter::default());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["appType"].as_str(), Some("codex"));
        assert_eq!(items[0]["summary"]["realTotalTokens"].as_u64(), Some(301));
        assert_eq!(items[2]["appType"].as_str(), Some("claude"));
    }

    #[test]
    fn single_persisted_push_appends_jsonl_without_rewriting_snapshot() {
        let dir = std::env::temp_dir().join(format!("cc-switch-server-usage-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let mut store = UsageStore::default();
        store.save_recent_snapshot(&dir).unwrap();
        let before = fs::read_to_string(usage_path(&dir)).unwrap();

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
        log.request_id = "req_append_only".to_string();
        store.push_and_persist(&dir, log).unwrap();

        let after = fs::read_to_string(usage_path(&dir)).unwrap();
        let jsonl = fs::read_to_string(usage_jsonl_path(&dir)).unwrap();
        assert_eq!(before, after);
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

        let mut first = UsageLog::new(
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
        first.request_id = "req_replay_first".to_string();
        first.created_at_ms = 1_000;
        store.push_and_persist(&dir, first).unwrap();

        let mut second = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            5,
            UsageModelMetadata::default(),
            TokenUsage::default(),
        );
        second.request_id = "req_replay_second".to_string();
        second.created_at_ms = 2_000;
        store.push_and_persist(&dir, second).unwrap();
        store
            .update_log_and_persist(&dir, "req_replay_second", |log| {
                log.duration_ms = 55;
                log.input_tokens = Some(7);
                log.output_tokens = Some(3);
                log.total_tokens = Some(10);
                log.stream_status = Some("completed".to_string());
            })
            .unwrap();

        let disk_snapshot = fs::read_to_string(usage_path(&dir)).unwrap();
        assert!(!disk_snapshot.contains("req_replay_first"));
        let journal = fs::read_to_string(usage_jsonl_path(&dir)).unwrap();
        assert!(journal.contains("\"version\":2"));
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
        let rollup = loaded.rollup();
        assert_eq!(rollup.requests, 2);
        assert_eq!(rollup.input_tokens, 8);
        assert_eq!(rollup.output_tokens, 4);

        loaded.save_recent_snapshot(&dir).unwrap();
        let reloaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(reloaded.logs.len(), 2);
        assert_eq!(reloaded.rollup().requests, 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_journal_recovery_keeps_snapshot_updates_authoritative() {
        let dir =
            std::env::temp_dir().join(format!("cc-switch-server-usage-legacy-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();

        let mut snapshot_log = UsageLog::new(
            AppKind::Codex,
            "p1".to_string(),
            "provider 1".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata::default(),
            TokenUsage {
                input_tokens: Some(5),
                output_tokens: Some(1),
                total_tokens: Some(6),
                ..Default::default()
            },
        );
        snapshot_log.request_id = "req_snapshot".to_string();
        snapshot_log.created_at_ms = 1_000;
        fs::write(
            usage_path(&dir),
            serde_json::to_vec_pretty(&json!({"logs": [snapshot_log.clone()]})).unwrap(),
        )
        .unwrap();

        let mut stale_same_id = snapshot_log.clone();
        stale_same_id.input_tokens = Some(1);
        let mut newer_request = snapshot_log.clone();
        newer_request.request_id = "req_legacy_new".to_string();
        newer_request.created_at_ms = 2_000;
        let mut trimmed_old_request = snapshot_log.clone();
        trimmed_old_request.request_id = "req_legacy_trimmed".to_string();
        trimmed_old_request.created_at_ms = 500;
        let legacy_journal = [stale_same_id, newer_request, trimmed_old_request]
            .into_iter()
            .map(|log| serde_json::to_string(&log).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(usage_jsonl_path(&dir), format!("{legacy_journal}\n")).unwrap();

        let mut loaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(loaded.logs.len(), 2);
        assert_eq!(
            loaded
                .logs
                .iter()
                .find(|log| log.request_id == "req_snapshot")
                .and_then(|log| log.input_tokens),
            Some(5)
        );
        assert!(loaded
            .logs
            .iter()
            .any(|log| log.request_id == "req_legacy_new"));
        assert!(loaded
            .logs
            .iter()
            .all(|log| log.request_id != "req_legacy_trimmed"));

        let mut post_migration = snapshot_log;
        post_migration.request_id = "req_post_migration".to_string();
        post_migration.created_at_ms = 3_000;
        loaded.push_and_persist(&dir, post_migration).unwrap();
        let reloaded = UsageStore::load_or_default(&dir).unwrap();
        assert!(reloaded
            .logs
            .iter()
            .any(|log| log.request_id == "req_post_migration"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn token_only_migration_strips_cost_fields_and_merges_pricing_buckets() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-usage-token-only-migration-test-{}",
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();

        let legacy_log = json!({
            "requestId": "req_legacy_cost",
            "app": "codex",
            "providerId": "p1",
            "providerName": "provider 1",
            "providerType": "codex",
            "model": "gpt-5.5",
            "requestedModel": "gpt-5.5",
            "actualModel": "gpt-5.5",
            "actualModelSource": "response",
            "pricingModel": "priced-a",
            "costMultiplier": 1.5,
            "statusCode": 200,
            "durationMs": 10,
            "rawInputTokens": 7,
            "billedInputTokens": 7,
            "inputTokens": 7,
            "outputTokens": 3,
            "cacheReadTokens": 0,
            "cacheCreationTokens": 0,
            "totalTokens": 10,
            "inputCostUsd": 0.01,
            "outputCostUsd": 0.02,
            "totalCostUsd": 0.03,
            "isHealthCheck": false,
            "isStreaming": false,
            "routerSyncAttemptCount": 0,
            "createdAtMs": 1_000
        });
        fs::write(
            usage_path(&dir),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": 1,
                "logs": [legacy_log]
            }))
            .unwrap(),
        )
        .unwrap();

        let bucket = |pricing_model: &str, requests: u64, total_tokens: u64| {
            json!({
                "bucketStartMs": 0,
                "bucketEndMs": 60_000,
                "dayStartMs": 0,
                "app": "codex",
                "providerId": "p1",
                "providerName": "provider 1",
                "providerType": "codex",
                "model": "gpt-5.5",
                "requestedModel": "gpt-5.5",
                "actualModel": "gpt-5.5",
                "actualModelSource": "response",
                "pricingModel": pricing_model,
                "stats": {
                    "rollup": {
                        "requests": requests,
                        "successes": requests,
                        "failures": 0,
                        "inputTokens": total_tokens,
                        "outputTokens": 0,
                        "cacheReadTokens": 0,
                        "cacheCreationTokens": 0,
                        "totalTokens": total_tokens,
                        "totalCostUsd": 99.0
                    },
                    "duration_sum_ms": requests * 10,
                    "duration_count": requests,
                    "first_token_sum_ms": 0,
                    "first_token_count": 0,
                    "last_request_at_ms": 1_000
                }
            })
        };
        fs::write(
            usage_rollups_path(&dir),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": 1,
                "buckets": {
                    "legacy-priced-a": bucket("priced-a", 2, 20),
                    "legacy-priced-b": bucket("priced-b", 3, 30)
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let loaded = UsageStore::load_or_default(&dir).unwrap();
        assert_eq!(loaded.schema_version, USAGE_SCHEMA_VERSION);
        assert_eq!(loaded.rollups.schema_version, USAGE_SCHEMA_VERSION);
        assert_eq!(loaded.rollups.buckets.len(), 1);
        let rollup = loaded.rollup();
        assert_eq!(rollup.requests, 5);
        assert_eq!(rollup.total_tokens, 50);

        let snapshot = fs::read_to_string(usage_path(&dir)).unwrap();
        let rollups = fs::read_to_string(usage_rollups_path(&dir)).unwrap();
        for obsolete in [
            "pricingModel",
            "costMultiplier",
            "billedInputTokens",
            "inputCostUsd",
            "outputCostUsd",
            "totalCostUsd",
        ] {
            assert!(!snapshot.contains(obsolete), "snapshot kept {obsolete}");
            assert!(!rollups.contains(obsolete), "rollups kept {obsolete}");
        }

        fs::remove_dir_all(&dir).unwrap();
    }
}
