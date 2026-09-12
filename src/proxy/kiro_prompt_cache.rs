//! Kiro prompt-cache metering fallback.
//!
//! This module models Anthropic prompt-cache accounting only when Kiro does not
//! return authoritative token-usage fields. It never changes routing, account
//! selection, or the bytes sent upstream, and therefore must not be interpreted
//! as proof that Kiro avoided inference work.

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

pub(super) const CACHE_CAPACITY: usize = 4096;
pub(super) const DEFAULT_TTL_SECS: i64 = 5 * 60;
pub(super) const MAX_TTL_SECS: i64 = 60 * 60;
const MAX_BREAKPOINTS: usize = 4;
const LOOKBACK_POSITIONS: usize = 20;
const WRITER_QUEUE_CAPACITY: usize = 2;
const WRITER_COALESCE: Duration = Duration::from_millis(20);
const WRITER_RETRY_INITIAL: Duration = Duration::from_millis(25);
const WRITER_RETRY_MAX: Duration = Duration::from_secs(2);
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(3);
const SQLITE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheDecision {
    Explicit,
    Auto,
    ExplicitAndAuto,
    NoCacheControl,
    EmptyPrompt,
    InvalidCacheControl,
    InvalidTtl,
    UncacheableBreakpoint,
    TooManyBreakpoints,
    MixedTtlOrder,
}

impl CacheDecision {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Auto => "auto",
            Self::ExplicitAndAuto => "explicit_and_auto",
            Self::NoCacheControl => "no_cache_control",
            Self::EmptyPrompt => "empty_prompt",
            Self::InvalidCacheControl => "invalid_cache_control",
            Self::InvalidTtl => "invalid_ttl",
            Self::UncacheableBreakpoint => "uncacheable_breakpoint",
            Self::TooManyBreakpoints => "too_many_breakpoints",
            Self::MixedTtlOrder => "mixed_ttl_order",
        }
    }

    const fn simulates_cache(self) -> bool {
        matches!(self, Self::Explicit | Self::Auto | Self::ExplicitAndAuto)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PromptCacheUsage {
    pub(super) cache_read_estimate: u32,
    pub(super) cache_covered_estimate: u32,
    pub(super) prompt_total_estimate: u32,
    pub(super) decision: CacheDecision,
}

impl Default for PromptCacheUsage {
    fn default() -> Self {
        Self {
            cache_read_estimate: 0,
            cache_covered_estimate: 0,
            prompt_total_estimate: 0,
            decision: CacheDecision::NoCacheControl,
        }
    }
}

impl PromptCacheUsage {
    /// Deterministically map local estimate ratios onto an authoritative total.
    /// The final subtraction assigns all rounding remainder to the uncached
    /// bucket, so the three non-negative fields always conserve `total`.
    pub(super) fn split_against_total(self, total: i32) -> (i32, i32, i32) {
        let total = total.max(0) as u64;
        let prompt_est = u64::from(self.prompt_total_estimate);
        let covered_est = u64::from(self.cache_covered_estimate).min(prompt_est);
        if total == 0 || prompt_est == 0 || covered_est == 0 {
            return (total.min(i32::MAX as u64) as i32, 0, 0);
        }

        let cache_total = rounded_ratio(total, covered_est, prompt_est).min(total);
        let read_est = u64::from(self.cache_read_estimate).min(covered_est);
        let cache_read = rounded_ratio(cache_total, read_est, covered_est).min(cache_total);
        let cache_creation = cache_total - cache_read;
        let input = total - cache_total;
        (
            input.min(i32::MAX as u64) as i32,
            cache_creation.min(i32::MAX as u64) as i32,
            cache_read.min(i32::MAX as u64) as i32,
        )
    }
}

fn rounded_ratio(value: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    let product = u128::from(value) * u128::from(numerator);
    let rounded = (product + u128::from(denominator / 2)) / u128::from(denominator);
    rounded.min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LookbackGroup {
    ToolUse,
    ToolResult,
}

#[derive(Debug, Clone)]
struct PromptBlock {
    signature: Vec<u8>,
    tokens: u32,
    cache_control: Option<Value>,
    cache_control_present: bool,
    cacheable: bool,
    lookback_group: Option<LookbackGroup>,
    current_user_input: bool,
}

#[derive(Debug, Clone, Copy)]
struct Breakpoint {
    block_index: usize,
    ttl_secs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheEntry {
    tokens: u32,
    #[serde(default = "default_ttl_secs")]
    ttl_secs: i64,
    expires_at: i64,
    last_hit_at: i64,
}

fn default_ttl_secs() -> i64 {
    DEFAULT_TTL_SECS
}

#[derive(Debug, Clone)]
struct PendingMutation {
    generation: u64,
    entry: Option<CacheEntry>,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<u64, CacheEntry>,
    pending: HashMap<u64, PendingMutation>,
    generation: u64,
}

enum WriterCommand {
    Flush,
    FlushAck {
        target: u64,
        acknowledge: mpsc::Sender<Result<(), String>>,
    },
    Shutdown {
        target: u64,
        acknowledge: mpsc::Sender<Result<(), String>>,
    },
}

struct PersistenceWriter {
    sender: mpsc::SyncSender<WriterCommand>,
    queued: Arc<AtomicBool>,
    persisted_generation: Arc<AtomicU64>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

pub(super) struct KiroPromptCache {
    state: Arc<Mutex<CacheState>>,
    writer: Option<PersistenceWriter>,
}

#[derive(Clone, Copy)]
pub(super) struct PromptCacheScope<'a> {
    pub(super) app: &'a str,
    pub(super) provider_id: &'a str,
    pub(super) provider_revision: u64,
    pub(super) runtime_fingerprint: &'a str,
    pub(super) account_id: &'a str,
    pub(super) auth_identity_generation: u64,
    pub(super) token_refresh_generation: u64,
    pub(super) share_id: &'a str,
    pub(super) signed_user: &'a str,
    pub(super) route: &'a str,
    pub(super) runtime_region: &'a str,
    pub(super) session: &'a str,
}

impl PromptCacheScope<'_> {
    pub(super) fn namespace(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "app": self.app,
            "providerId": self.provider_id,
            "providerRevision": self.provider_revision,
            "runtimeFingerprint": self.runtime_fingerprint,
            "accountId": self.account_id,
            "authIdentityGeneration": self.auth_identity_generation,
            "tokenRefreshGeneration": self.token_refresh_generation,
            "shareId": self.share_id,
            "signedUser": self.signed_user,
            "route": self.route,
            "runtimeRegion": self.runtime_region,
            "session": self.session,
        }))
        .expect("Kiro prompt-cache scope contains only serializable primitives")
    }
}

impl KiroPromptCache {
    pub(super) fn new(persist_base_path: Option<PathBuf>) -> Self {
        let Some(base_path) = persist_base_path else {
            return Self {
                state: Arc::new(Mutex::new(CacheState::default())),
                writer: None,
            };
        };

        let sqlite_path = sqlite_path_for(&base_path);
        let (entries, generation) = match load_or_migrate(&base_path, &sqlite_path) {
            Ok(loaded) => loaded,
            Err(error) => {
                crate::metrics::record_kiro_prompt_cache_persistence("startup_load_failed");
                tracing::warn!(
                    path = %sqlite_path.display(),
                    %error,
                    "Kiro prompt-cache SQLite startup load failed; continuing in memory"
                );
                (HashMap::new(), 0)
            }
        };
        crate::metrics::set_kiro_prompt_cache_entries(entries.len());

        let state = Arc::new(Mutex::new(CacheState {
            entries,
            pending: HashMap::new(),
            generation,
        }));
        let queued = Arc::new(AtomicBool::new(false));
        let persisted_generation = Arc::new(AtomicU64::new(generation));
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let worker_state = Arc::clone(&state);
        let worker_queued = Arc::clone(&queued);
        let worker_persisted_generation = Arc::clone(&persisted_generation);
        let handle = std::thread::Builder::new()
            .name("cc-switch-kiro-cache-writer".to_string())
            .spawn(move || {
                run_writer(
                    sqlite_path,
                    worker_state,
                    receiver,
                    worker_queued,
                    worker_persisted_generation,
                );
            })
            .ok();
        let writer = handle.map(|handle| PersistenceWriter {
            sender,
            queued,
            persisted_generation,
            handle: Mutex::new(Some(handle)),
        });
        if writer.is_none() {
            crate::metrics::record_kiro_prompt_cache_persistence("writer_spawn_failed");
        }
        Self { state, writer }
    }

    pub(super) fn compute_usage(&self, body: &Value, namespace: &str) -> PromptCacheUsage {
        let blocks = extract_blocks(body);
        let prompt_total_estimate = blocks
            .iter()
            .fold(0_u32, |total, block| total.saturating_add(block.tokens));
        let (breakpoints, decision) = resolve_breakpoints(&blocks, body.get("cache_control"));
        crate::metrics::record_kiro_prompt_cache_decision(decision.as_str());
        if breakpoints.is_empty() || !decision.simulates_cache() {
            return PromptCacheUsage {
                prompt_total_estimate,
                decision,
                ..PromptCacheUsage::default()
            };
        }

        let (cumulative_tokens, cumulative_hashes) = cumulative_prompt(&blocks, namespace, body);
        let now = unix_timestamp_secs();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut touched = HashMap::<u64, Option<CacheEntry>>::new();

        let expired = state
            .entries
            .iter()
            .filter_map(|(hash, entry)| (entry.expires_at <= now).then_some(*hash))
            .collect::<Vec<_>>();
        for hash in expired {
            state.entries.remove(&hash);
            touched.insert(hash, None);
        }

        let mut deepest_read = 0_u32;
        for breakpoint in &breakpoints {
            let start = lookback_start_index(&blocks, breakpoint.block_index);
            for index in (start..=breakpoint.block_index).rev() {
                let hash = cumulative_hashes[index];
                let Some(entry) = state.entries.get_mut(&hash) else {
                    continue;
                };
                if entry.expires_at <= now {
                    continue;
                }
                let ttl = entry.ttl_secs.clamp(60, MAX_TTL_SECS);
                entry.ttl_secs = ttl;
                entry.last_hit_at = now;
                entry.expires_at = now.saturating_add(ttl);
                deepest_read = deepest_read.max(entry.tokens);
                touched.insert(hash, Some(entry.clone()));
                break;
            }
        }

        for breakpoint in &breakpoints {
            let hash = cumulative_hashes[breakpoint.block_index];
            let ttl = breakpoint.ttl_secs;
            let entry = CacheEntry {
                tokens: cumulative_tokens[breakpoint.block_index],
                ttl_secs: ttl,
                expires_at: now.saturating_add(ttl),
                last_hit_at: now,
            };
            state.entries.insert(hash, entry.clone());
            touched.insert(hash, Some(entry));
        }
        enforce_capacity(&mut state, &mut touched);
        let covered = breakpoints
            .last()
            .map(|breakpoint| cumulative_tokens[breakpoint.block_index])
            .unwrap_or(0);

        if !touched.is_empty() {
            state.generation = state.generation.saturating_add(1);
            let generation = state.generation;
            for (hash, entry) in touched {
                state
                    .pending
                    .insert(hash, PendingMutation { generation, entry });
            }
        }
        let entry_count = state.entries.len();
        drop(state);
        crate::metrics::set_kiro_prompt_cache_entries(entry_count);
        self.schedule_flush();

        PromptCacheUsage {
            cache_read_estimate: deepest_read.min(covered),
            cache_covered_estimate: covered,
            prompt_total_estimate,
            decision,
        }
    }

    fn schedule_flush(&self) {
        let Some(writer) = self.writer.as_ref() else {
            return;
        };
        if writer
            .queued
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            crate::metrics::record_kiro_prompt_cache_persistence("write_coalesced");
            return;
        }
        match writer.sender.try_send(WriterCommand::Flush) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                crate::metrics::record_kiro_prompt_cache_persistence("queue_full_coalesced");
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                writer.queued.store(false, Ordering::Release);
                crate::metrics::record_kiro_prompt_cache_persistence("writer_unavailable");
            }
        }
    }

    #[cfg(test)]
    fn flush_for_test(&self, timeout: Duration) -> Result<(), String> {
        let Some(writer) = self.writer.as_ref() else {
            return Ok(());
        };
        let target = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation;
        if writer.persisted_generation.load(Ordering::Acquire) >= target {
            return Ok(());
        }
        let (acknowledge, acknowledged) = mpsc::channel();
        writer
            .sender
            .send(WriterCommand::FlushAck {
                target,
                acknowledge,
            })
            .map_err(|_| "Kiro prompt-cache writer is unavailable".to_string())?;
        acknowledged
            .recv_timeout(timeout)
            .map_err(|_| "Kiro prompt-cache flush timed out".to_string())?
    }

    fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        let Some(writer) = self.writer.as_ref() else {
            return Ok(());
        };
        // Taking the handle is the single-owner shutdown gate.  Besides making
        // repeated calls idempotent, holding this mutex prevents two callers
        // from racing two Shutdown commands into a writer that can only exit
        // once.
        let handle = {
            let mut handle = writer
                .handle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(handle) = handle.take() else {
                return Ok(());
            };
            handle
        };
        let target = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation;
        let (acknowledge, acknowledged) = mpsc::channel();
        writer
            .sender
            .send(WriterCommand::Shutdown {
                target,
                acknowledge,
            })
            .map_err(|_| "Kiro prompt-cache writer is unavailable".to_string())?;
        let result = acknowledged
            .recv_timeout(timeout)
            .map_err(|_| "Kiro prompt-cache shutdown flush timed out".to_string())?;
        handle
            .join()
            .map_err(|_| "Kiro prompt-cache writer panicked".to_string())?;
        result
    }

    #[cfg(test)]
    fn entry(&self, hash: u64) -> Option<CacheEntry> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .get(&hash)
            .cloned()
    }
}

impl Drop for KiroPromptCache {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown(SHUTDOWN_FLUSH_TIMEOUT) {
            tracing::warn!(%error, "Kiro prompt-cache shutdown flush did not complete");
        }
    }
}

pub(super) fn shutdown_global(cache: Option<&KiroPromptCache>) {
    let Some(cache) = cache else {
        return;
    };
    match cache.shutdown(SHUTDOWN_FLUSH_TIMEOUT) {
        Ok(()) => crate::metrics::record_kiro_prompt_cache_persistence("shutdown_flushed"),
        Err(error) => {
            crate::metrics::record_kiro_prompt_cache_persistence("shutdown_flush_failed");
            tracing::warn!(%error, "Kiro prompt-cache shutdown flush failed");
        }
    }
}

fn enforce_capacity(state: &mut CacheState, touched: &mut HashMap<u64, Option<CacheEntry>>) {
    if state.entries.len() <= CACHE_CAPACITY {
        return;
    }
    let drop_count = state.entries.len() - CACHE_CAPACITY;
    let mut victims = state
        .entries
        .iter()
        .map(|(hash, entry)| (*hash, entry.last_hit_at))
        .collect::<Vec<_>>();
    victims.sort_unstable_by_key(|(hash, last_hit_at)| (*last_hit_at, *hash));
    for (hash, _) in victims.into_iter().take(drop_count) {
        state.entries.remove(&hash);
        touched.insert(hash, None);
    }
}

fn extract_blocks(body: &Value) -> Vec<PromptBlock> {
    let mut blocks = Vec::new();
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for tool in tools {
            blocks.push(prompt_block("tool", None, tool, false));
        }
    }

    match body.get("system") {
        Some(Value::String(text)) => blocks.push(PromptBlock {
            signature: block_signature("system", None, &Value::String(text.clone())),
            tokens: estimate_text_tokens(text),
            cache_control: None,
            cache_control_present: false,
            cacheable: !text.trim().is_empty(),
            lookback_group: None,
            current_user_input: false,
        }),
        Some(Value::Array(items)) => {
            for item in items {
                blocks.push(prompt_block("system", None, item, false));
            }
        }
        Some(other) => blocks.push(prompt_block("system", None, other, false)),
        None => {}
    }

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for (message_index, message) in messages.iter().enumerate() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let is_last_user = message_index + 1 == messages.len()
            && role == "user"
            && !message_contains_tool_result(message);
        let first_index = blocks.len();
        match message.get("content") {
            Some(Value::String(text)) => blocks.push(PromptBlock {
                signature: block_signature("message", Some(role), &Value::String(text.clone())),
                tokens: estimate_text_tokens(text),
                cache_control: None,
                cache_control_present: false,
                cacheable: !text.trim().is_empty(),
                lookback_group: None,
                current_user_input: is_last_user,
            }),
            Some(Value::Array(items)) => {
                for item in items {
                    blocks.push(prompt_block("message", Some(role), item, is_last_user));
                }
            }
            Some(other) => blocks.push(prompt_block("message", Some(role), other, is_last_user)),
            None => {}
        }
        if let Some(control) = message.get("cache_control") {
            if blocks.len() > first_index {
                let last = blocks.last_mut().expect("message added at least one block");
                if last.cache_control_present && last.cache_control.as_ref() != Some(control) {
                    last.cache_control = None;
                    last.cache_control_present = true;
                } else {
                    last.cache_control = Some(control.clone());
                    last.cache_control_present = true;
                }
            }
        }
    }
    blocks
}

fn prompt_block(
    section: &str,
    role: Option<&str>,
    value: &Value,
    current_user_input: bool,
) -> PromptBlock {
    let cache_control_present = value.get("cache_control").is_some();
    let cache_control = value.get("cache_control").cloned();
    let canonical = canonical_without_cache_control(value);
    let block_type = canonical.get("type").and_then(Value::as_str);
    let lookback_group = match block_type {
        Some("tool_use") => Some(LookbackGroup::ToolUse),
        Some("tool_result") => Some(LookbackGroup::ToolResult),
        _ => None,
    };
    PromptBlock {
        signature: block_signature(section, role, &canonical),
        tokens: estimate_block_tokens(&canonical),
        cache_control,
        cache_control_present,
        cacheable: block_is_cacheable(&canonical),
        lookback_group,
        current_user_input,
    }
}

fn message_contains_tool_result(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
        })
}

fn resolve_breakpoints(
    blocks: &[PromptBlock],
    top_control: Option<&Value>,
) -> (Vec<Breakpoint>, CacheDecision) {
    if blocks.is_empty() {
        return (Vec::new(), CacheDecision::EmptyPrompt);
    }
    let mut breakpoints = Vec::new();
    let mut explicit = false;
    for (index, block) in blocks.iter().enumerate() {
        if !block.cache_control_present {
            continue;
        }
        explicit = true;
        let Some(control) = block.cache_control.as_ref() else {
            return (Vec::new(), CacheDecision::InvalidCacheControl);
        };
        if !block.cacheable {
            return (Vec::new(), CacheDecision::UncacheableBreakpoint);
        }
        let ttl = match validated_ttl(control) {
            Ok(ttl) => ttl,
            Err(decision) => return (Vec::new(), decision),
        };
        breakpoints.push(Breakpoint {
            block_index: index,
            ttl_secs: ttl,
        });
    }

    let auto = top_control.is_some();
    if let Some(control) = top_control {
        let ttl = match validated_ttl(control) {
            Ok(ttl) => ttl,
            Err(decision) => return (Vec::new(), decision),
        };
        let target = blocks
            .iter()
            .enumerate()
            .rposition(|(_, block)| block.cacheable && !block.current_user_input)
            .or_else(|| blocks.iter().rposition(|block| block.cacheable));
        let Some(target) = target else {
            return (Vec::new(), CacheDecision::UncacheableBreakpoint);
        };
        if let Some(existing) = breakpoints
            .iter()
            .find(|breakpoint| breakpoint.block_index == target)
        {
            if existing.ttl_secs != ttl {
                return (Vec::new(), CacheDecision::InvalidCacheControl);
            }
        } else {
            breakpoints.push(Breakpoint {
                block_index: target,
                ttl_secs: ttl,
            });
        }
    }

    if !explicit && !auto {
        return (Vec::new(), CacheDecision::NoCacheControl);
    }
    breakpoints.sort_unstable_by_key(|breakpoint| breakpoint.block_index);
    if breakpoints.len() > MAX_BREAKPOINTS {
        return (Vec::new(), CacheDecision::TooManyBreakpoints);
    }
    let mut seen_short_ttl = false;
    for breakpoint in &breakpoints {
        if breakpoint.ttl_secs == DEFAULT_TTL_SECS {
            seen_short_ttl = true;
        } else if seen_short_ttl {
            return (Vec::new(), CacheDecision::MixedTtlOrder);
        }
    }
    let decision = match (explicit, auto) {
        (true, true) => CacheDecision::ExplicitAndAuto,
        (true, false) => CacheDecision::Explicit,
        (false, true) => CacheDecision::Auto,
        (false, false) => CacheDecision::NoCacheControl,
    };
    (breakpoints, decision)
}

fn validated_ttl(control: &Value) -> Result<i64, CacheDecision> {
    let Some(object) = control.as_object() else {
        return Err(CacheDecision::InvalidCacheControl);
    };
    if object.get("type").and_then(Value::as_str) != Some("ephemeral") {
        return Err(CacheDecision::InvalidCacheControl);
    }
    match object.get("ttl") {
        None => Ok(DEFAULT_TTL_SECS),
        Some(Value::String(ttl)) if ttl.eq_ignore_ascii_case("5m") => Ok(DEFAULT_TTL_SECS),
        Some(Value::String(ttl)) if ttl.eq_ignore_ascii_case("1h") => Ok(MAX_TTL_SECS),
        _ => Err(CacheDecision::InvalidTtl),
    }
}

fn cumulative_prompt(
    blocks: &[PromptBlock],
    namespace: &str,
    body: &Value,
) -> (Vec<u32>, Vec<u64>) {
    let mut hasher = Sha256::new();
    hash_frame(&mut hasher, b"cc-switch-kiro-cache-v3");
    hash_frame(&mut hasher, namespace.as_bytes());
    hash_frame(&mut hasher, &global_cache_context(body));
    let mut tokens = Vec::with_capacity(blocks.len());
    let mut hashes = Vec::with_capacity(blocks.len());
    let mut cumulative = 0_u32;
    for block in blocks {
        hash_frame(&mut hasher, &block.signature);
        cumulative = cumulative.saturating_add(block.tokens);
        tokens.push(cumulative);
        let digest = hasher.clone().finalize();
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        hashes.push(u64::from_be_bytes(bytes));
    }
    (tokens, hashes)
}

fn global_cache_context(body: &Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": body.get("model"),
        "tool_choice": body.get("tool_choice"),
        "thinking": body.get("thinking"),
        "output_config": body.get("output_config"),
    }))
    .unwrap_or_default()
}

fn lookback_start_index(blocks: &[PromptBlock], breakpoint_index: usize) -> usize {
    let mut index = breakpoint_index;
    let mut positions = 1_usize;
    while index > 0 {
        let previous = index - 1;
        let same_group = blocks[index].lookback_group.is_some()
            && blocks[index].lookback_group == blocks[previous].lookback_group;
        if !same_group {
            if positions == LOOKBACK_POSITIONS {
                break;
            }
            positions += 1;
        }
        index = previous;
    }
    index
}

fn block_signature(section: &str, role: Option<&str>, value: &Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "section": section,
        "role": role,
        "content": canonical_without_cache_control(value),
    }))
    .unwrap_or_default()
}

fn canonical_without_cache_control(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                if key != "cache_control" {
                    canonical.insert(key.clone(), canonical_without_cache_control(&object[key]));
                }
            }
            Value::Object(canonical)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(canonical_without_cache_control).collect())
        }
        _ => value.clone(),
    }
}

fn block_is_cacheable(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("thinking" | "redacted_thinking") => false,
            Some("text") => object
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
            Some(_) => true,
            None => !object.is_empty(),
        },
        _ => !value.is_null(),
    }
}

fn estimate_block_tokens(value: &Value) -> u32 {
    if let Some(text) = value.as_str() {
        return estimate_text_tokens(text);
    }
    super::kiro::count_content_block_tokens(value)
        .unwrap_or_else(|_| {
            serde_json::to_string(value)
                .map(|text| u64::from(estimate_text_tokens(&text)))
                .unwrap_or(0)
        })
        .min(u64::from(u32::MAX)) as u32
}

fn estimate_text_tokens(text: &str) -> u32 {
    ((text.chars().count() as u64).saturating_add(3) / 4).min(u64::from(u32::MAX)) as u32
}

fn hash_frame(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn sqlite_path_for(base_path: &Path) -> PathBuf {
    let mut value = base_path.as_os_str().to_os_string();
    value.push(".sqlite");
    PathBuf::from(value)
}

fn load_or_migrate(
    legacy_path: &Path,
    sqlite_path: &Path,
) -> Result<(HashMap<u64, CacheEntry>, u64), String> {
    if let Some(parent) = sqlite_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut connection = open_connection(sqlite_path)?;
    initialize_schema(&mut connection)?;
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM prompt_cache_entries", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if count == 0 && legacy_path.exists() {
        if let Ok(bytes) = std::fs::read(legacy_path) {
            if let Ok(entries) = serde_json::from_slice::<HashMap<u64, CacheEntry>>(&bytes) {
                let now = unix_timestamp_secs();
                let mut valid = entries
                    .into_iter()
                    .filter(|(_, entry)| entry.expires_at > now)
                    .collect::<Vec<_>>();
                valid.sort_unstable_by_key(|(hash, entry)| {
                    (
                        std::cmp::Reverse(entry.last_hit_at),
                        std::cmp::Reverse(*hash),
                    )
                });
                let valid = valid
                    .into_iter()
                    .take(CACHE_CAPACITY)
                    .collect::<HashMap<_, _>>();
                import_legacy(&mut connection, &valid)?;
                crate::metrics::record_kiro_prompt_cache_persistence("legacy_json_imported");
            }
        }
    }
    load_entries(&mut connection)
}

fn open_connection(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn initialize_schema(connection: &mut Connection) -> Result<(), String> {
    let current: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if current > SQLITE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported Kiro prompt-cache schema version {current}"
        ));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS prompt_cache_entries (
                hash BLOB PRIMARY KEY NOT NULL CHECK(length(hash) = 8),
                tokens INTEGER NOT NULL CHECK(tokens >= 0),
                ttl_secs INTEGER NOT NULL CHECK(ttl_secs BETWEEN 60 AND 3600),
                expires_at INTEGER NOT NULL,
                last_hit_at INTEGER NOT NULL,
                mutation_generation INTEGER NOT NULL CHECK(mutation_generation >= 0)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS prompt_cache_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value INTEGER NOT NULL CHECK(value >= 0)
             ) WITHOUT ROWID;
             INSERT OR IGNORE INTO prompt_cache_meta(key, value) VALUES ('generation', 0);",
        )
        .map_err(|error| error.to_string())?;
    transaction
        .pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn load_entries(connection: &mut Connection) -> Result<(HashMap<u64, CacheEntry>, u64), String> {
    let now = unix_timestamp_secs();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM prompt_cache_entries WHERE expires_at <= ?1",
            [now],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM prompt_cache_entries WHERE hash IN (
                SELECT hash FROM prompt_cache_entries
                ORDER BY last_hit_at DESC, hash DESC LIMIT -1 OFFSET ?1
             )",
            [CACHE_CAPACITY as i64],
        )
        .map_err(|error| error.to_string())?;
    let generation: i64 = transaction
        .query_row(
            "SELECT value FROM prompt_cache_meta WHERE key = 'generation'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let mut entries = HashMap::new();
    {
        let mut statement = transaction
            .prepare(
                "SELECT hash, tokens, ttl_secs, expires_at, last_hit_at
                 FROM prompt_cache_entries ORDER BY last_hit_at DESC, hash DESC LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([CACHE_CAPACITY as i64], |row| {
                let bytes: Vec<u8> = row.get(0)?;
                let hash_bytes: [u8; 8] = bytes.try_into().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Blob,
                        "invalid Kiro prompt-cache hash length".into(),
                    )
                })?;
                let tokens: i64 = row.get(1)?;
                Ok((
                    u64::from_be_bytes(hash_bytes),
                    CacheEntry {
                        tokens: u32::try_from(tokens).unwrap_or(u32::MAX),
                        ttl_secs: row.get(2)?,
                        expires_at: row.get(3)?,
                        last_hit_at: row.get(4)?,
                    },
                ))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            let (hash, entry) = row.map_err(|error| error.to_string())?;
            entries.insert(hash, entry);
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok((entries, generation.max(0) as u64))
}

fn import_legacy(
    connection: &mut Connection,
    entries: &HashMap<u64, CacheEntry>,
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    for (hash, entry) in entries {
        transaction
            .execute(
                "INSERT OR REPLACE INTO prompt_cache_entries
                 (hash, tokens, ttl_secs, expires_at, last_hit_at, mutation_generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                params![
                    hash.to_be_bytes().as_slice(),
                    i64::from(entry.tokens),
                    entry.ttl_secs.clamp(60, MAX_TTL_SECS),
                    entry.expires_at,
                    entry.last_hit_at,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn persist_mutations(
    connection: &mut Connection,
    expected_generation: u64,
    target_generation: u64,
    mutations: &HashMap<u64, PendingMutation>,
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let current: i64 = transaction
        .query_row(
            "SELECT value FROM prompt_cache_meta WHERE key = 'generation'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if current.max(0) as u64 != expected_generation {
        return Err(format!(
            "Kiro prompt-cache generation conflict: expected {expected_generation}, found {current}"
        ));
    }
    for (hash, mutation) in mutations {
        match mutation.entry.as_ref() {
            Some(entry) => {
                transaction
                    .execute(
                        "INSERT INTO prompt_cache_entries
                         (hash, tokens, ttl_secs, expires_at, last_hit_at, mutation_generation)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(hash) DO UPDATE SET
                           tokens = excluded.tokens,
                           ttl_secs = excluded.ttl_secs,
                           expires_at = excluded.expires_at,
                           last_hit_at = excluded.last_hit_at,
                           mutation_generation = excluded.mutation_generation
                         WHERE prompt_cache_entries.mutation_generation <= excluded.mutation_generation",
                        params![
                            hash.to_be_bytes().as_slice(),
                            i64::from(entry.tokens),
                            entry.ttl_secs,
                            entry.expires_at,
                            entry.last_hit_at,
                            mutation.generation.min(i64::MAX as u64) as i64,
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
            None => {
                transaction
                    .execute(
                        "DELETE FROM prompt_cache_entries
                         WHERE hash = ?1 AND mutation_generation <= ?2",
                        params![
                            hash.to_be_bytes().as_slice(),
                            mutation.generation.min(i64::MAX as u64) as i64,
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    transaction
        .execute(
            "DELETE FROM prompt_cache_entries WHERE expires_at <= ?1",
            [unix_timestamp_secs()],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM prompt_cache_entries WHERE hash IN (
                SELECT hash FROM prompt_cache_entries
                ORDER BY last_hit_at ASC, hash ASC
                LIMIT MAX(0, (SELECT COUNT(*) FROM prompt_cache_entries) - ?1)
             )",
            [CACHE_CAPACITY as i64],
        )
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE prompt_cache_meta SET value = ?1
             WHERE key = 'generation' AND value = ?2",
            params![
                target_generation.min(i64::MAX as u64) as i64,
                expected_generation.min(i64::MAX as u64) as i64,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Kiro prompt-cache generation CAS failed".to_string());
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn run_writer(
    path: PathBuf,
    state: Arc<Mutex<CacheState>>,
    receiver: mpsc::Receiver<WriterCommand>,
    queued: Arc<AtomicBool>,
    persisted_generation: Arc<AtomicU64>,
) {
    let mut connection = None;
    let mut retry_delay = WRITER_RETRY_INITIAL;
    let mut pending_acknowledgements = Vec::new();
    let mut shutdown_acknowledgement = None;

    while let Ok(command) = receiver.recv() {
        collect_command(
            command,
            &mut pending_acknowledgements,
            &mut shutdown_acknowledgement,
        );
        if shutdown_acknowledgement.is_none() {
            std::thread::sleep(WRITER_COALESCE);
        }
        while let Ok(command) = receiver.try_recv() {
            collect_command(
                command,
                &mut pending_acknowledgements,
                &mut shutdown_acknowledgement,
            );
        }

        loop {
            let (mutations, target_generation) = take_pending(&state);
            let expected_generation = persisted_generation.load(Ordering::Acquire);
            if mutations.is_empty() || target_generation <= expected_generation {
                acknowledge_ready(&mut pending_acknowledgements, expected_generation, Ok(()));
                queued.store(false, Ordering::Release);
                if let Some(acknowledge) = shutdown_acknowledgement.take() {
                    let _ = acknowledge.send(Ok(()));
                    return;
                }
                let current = state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .generation;
                if current > expected_generation
                    && queued
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    continue;
                }
                break;
            }

            let result = (|| {
                if connection.is_none() {
                    let mut opened = open_connection(&path)?;
                    initialize_schema(&mut opened)?;
                    connection = Some(opened);
                }
                persist_mutations(
                    connection.as_mut().expect("connection initialized"),
                    expected_generation,
                    target_generation,
                    &mutations,
                )
            })();
            match result {
                Ok(()) => {
                    persisted_generation.store(target_generation, Ordering::Release);
                    retry_delay = WRITER_RETRY_INITIAL;
                    crate::metrics::record_kiro_prompt_cache_persistence("write_success");
                    crate::metrics::set_kiro_prompt_cache_persistence_degraded(false);
                    acknowledge_ready(&mut pending_acknowledgements, target_generation, Ok(()));
                }
                Err(error) => {
                    merge_pending(&state, mutations);
                    connection = None;
                    crate::metrics::record_kiro_prompt_cache_persistence("write_failed");
                    crate::metrics::set_kiro_prompt_cache_persistence_degraded(true);
                    tracing::warn!(path = %path.display(), %error, "Kiro prompt-cache SQLite write failed; retrying asynchronously");
                    if let Some(acknowledge) = shutdown_acknowledgement.take() {
                        acknowledge_ready(
                            &mut pending_acknowledgements,
                            persisted_generation.load(Ordering::Acquire),
                            Err(error.clone()),
                        );
                        let _ = acknowledge.send(Err(error));
                        return;
                    }
                    std::thread::sleep(retry_delay);
                    retry_delay = retry_delay.saturating_mul(2).min(WRITER_RETRY_MAX);
                    while let Ok(command) = receiver.try_recv() {
                        collect_command(
                            command,
                            &mut pending_acknowledgements,
                            &mut shutdown_acknowledgement,
                        );
                    }
                }
            }
        }
    }
}

fn collect_command(
    command: WriterCommand,
    pending: &mut Vec<(u64, mpsc::Sender<Result<(), String>>)>,
    shutdown: &mut Option<mpsc::Sender<Result<(), String>>>,
) {
    match command {
        WriterCommand::Flush => {}
        WriterCommand::FlushAck {
            target,
            acknowledge,
        } => pending.push((target, acknowledge)),
        WriterCommand::Shutdown {
            target: _,
            acknowledge,
        } => {
            *shutdown = Some(acknowledge);
        }
    }
}

fn take_pending(state: &Mutex<CacheState>) -> (HashMap<u64, PendingMutation>, u64) {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    let target = state.generation;
    (std::mem::take(&mut state.pending), target)
}

fn merge_pending(state: &Mutex<CacheState>, mutations: HashMap<u64, PendingMutation>) {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    for (hash, mutation) in mutations {
        if state
            .pending
            .get(&hash)
            .is_none_or(|current| current.generation <= mutation.generation)
        {
            state.pending.insert(hash, mutation);
        }
    }
}

fn acknowledge_ready(
    acknowledgements: &mut Vec<(u64, mpsc::Sender<Result<(), String>>)>,
    persisted_generation: u64,
    result: Result<(), String>,
) {
    let mut waiting = Vec::new();
    for (target, acknowledge) in acknowledgements.drain(..) {
        if result.is_err() || target <= persisted_generation {
            let _ = acknowledge.send(result.clone());
        } else {
            waiting.push((target, acknowledge));
        }
    }
    *acknowledgements = waiting;
}

fn unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(name: &str) -> PathBuf {
        let unique = format!(
            "cc-switch-kiro-cache-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    fn explicit_body() -> Value {
        serde_json::json!({
            "model": "claude-sonnet-4",
            "system": [{
                "type": "text",
                "text": "stable system prompt ".repeat(100),
                "cache_control": {"type": "ephemeral", "ttl": "5m"}
            }],
            "messages": [{"role": "user", "content": "hello"}]
        })
    }

    #[test]
    fn namespace_isolated_by_every_binding_and_runtime_component() {
        let base = PromptCacheScope {
            app: "claude",
            provider_id: "provider-a",
            provider_revision: 1,
            runtime_fingerprint: "runtime-a",
            account_id: "account-a",
            auth_identity_generation: 2,
            token_refresh_generation: 3,
            share_id: "share-a",
            signed_user: "user-a",
            route: "Messages",
            runtime_region: "us-east-1",
            session: "session-a",
        };
        let expected = base.namespace();
        let variants = [
            PromptCacheScope {
                app: "codex",
                ..base
            },
            PromptCacheScope {
                provider_id: "provider-b",
                ..base
            },
            PromptCacheScope {
                provider_revision: 2,
                ..base
            },
            PromptCacheScope {
                runtime_fingerprint: "runtime-b",
                ..base
            },
            PromptCacheScope {
                account_id: "account-b",
                ..base
            },
            PromptCacheScope {
                auth_identity_generation: 3,
                ..base
            },
            PromptCacheScope {
                token_refresh_generation: 4,
                ..base
            },
            PromptCacheScope {
                share_id: "share-b",
                ..base
            },
            PromptCacheScope {
                signed_user: "user-b",
                ..base
            },
            PromptCacheScope {
                route: "Responses",
                ..base
            },
            PromptCacheScope {
                runtime_region: "eu-central-1",
                ..base
            },
            PromptCacheScope {
                session: "session-b",
                ..base
            },
        ];
        for variant in variants {
            assert_ne!(variant.namespace(), expected);
        }
    }

    #[test]
    fn auto_cache_keeps_latest_user_tail_uncached() {
        let cache = KiroPromptCache::new(None);
        let body = serde_json::json!({
            "model": "m",
            "cache_control": {"type": "ephemeral"},
            "messages": [
                {"role": "user", "content": "old question ".repeat(20)},
                {"role": "assistant", "content": "old answer ".repeat(20)},
                {"role": "user", "content": "latest question"}
            ]
        });
        let first = cache.compute_usage(&body, "scope");
        assert_eq!(first.decision, CacheDecision::Auto);
        assert!(first.cache_covered_estimate > 0);
        assert!(first.prompt_total_estimate > first.cache_covered_estimate);
        let second = cache.compute_usage(&body, "scope");
        assert_eq!(second.cache_read_estimate, first.cache_covered_estimate);
        let (input, creation, read) = second.split_against_total(1_001);
        assert_eq!(input + creation + read, 1_001);
        assert!(input > 0);
    }

    #[test]
    fn invalid_and_excess_breakpoints_fail_closed_with_reason() {
        let cache = KiroPromptCache::new(None);
        let invalid = serde_json::json!({
            "messages": [{"role":"user","content":[
                {"type":"text","text":"a","cache_control":{"type":"persistent"}}
            ]}]
        });
        assert_eq!(
            cache.compute_usage(&invalid, "scope").decision,
            CacheDecision::InvalidCacheControl
        );

        let too_many = serde_json::json!({
            "messages": [{"role":"user","content": (0..5).map(|n| serde_json::json!({
                "type":"text", "text": format!("block-{n}"),
                "cache_control":{"type":"ephemeral"}
            })).collect::<Vec<_>>()}]
        });
        assert_eq!(
            cache.compute_usage(&too_many, "scope").decision,
            CacheDecision::TooManyBreakpoints
        );
    }

    #[test]
    fn mixed_ttl_order_is_enforced() {
        let cache = KiroPromptCache::new(None);
        let valid = serde_json::json!({
            "messages": [{"role":"user","content":[
                {"type":"text","text":"long","cache_control":{"type":"ephemeral","ttl":"1h"}},
                {"type":"text","text":"short","cache_control":{"type":"ephemeral","ttl":"5m"}}
            ]}]
        });
        assert_eq!(
            cache.compute_usage(&valid, "scope").decision,
            CacheDecision::Explicit
        );
        let invalid = serde_json::json!({
            "messages": [{"role":"user","content":[
                {"type":"text","text":"short","cache_control":{"type":"ephemeral","ttl":"5m"}},
                {"type":"text","text":"long","cache_control":{"type":"ephemeral","ttl":"1h"}}
            ]}]
        });
        assert_eq!(
            cache.compute_usage(&invalid, "scope").decision,
            CacheDecision::MixedTtlOrder
        );
    }

    #[test]
    fn twenty_position_lookback_groups_consecutive_tool_blocks() {
        let mut items = vec![serde_json::json!({
            "type":"text", "text":"prefix", "cache_control":{"type":"ephemeral"}
        })];
        items.extend((0..25).map(|n| {
            serde_json::json!({
                "type":"tool_result", "tool_use_id":format!("t{n}"), "content":"ok"
            })
        }));
        let warm = serde_json::json!({"model":"m","messages":[{"role":"user","content":items[0..1].to_vec()}]});
        let cache = KiroPromptCache::new(None);
        let first = cache.compute_usage(&warm, "scope");
        assert!(first.cache_covered_estimate > 0);
        let extended = serde_json::json!({
            "model":"m", "messages":[{"role":"user","content":items}],
            "cache_control":{"type":"ephemeral"}
        });
        let hit = cache.compute_usage(&extended, "scope");
        assert!(
            hit.cache_read_estimate > 0,
            "tool_result run counts as one lookback position"
        );
    }

    #[test]
    fn hit_renews_with_entry_own_ttl() {
        let cache = KiroPromptCache::new(None);
        let body = explicit_body();
        let blocks = extract_blocks(&body);
        let (_, hashes) = cumulative_prompt(&blocks, "scope", &body);
        let hash = hashes[0];
        let _ = cache.compute_usage(&body, "scope");
        {
            let mut state = cache.state.lock().unwrap();
            let entry = state.entries.get_mut(&hash).unwrap();
            entry.ttl_secs = 60;
            entry.expires_at = unix_timestamp_secs() + 1;
        }
        let extended = serde_json::json!({
            "model": "claude-sonnet-4",
            "cache_control": {"type": "ephemeral", "ttl": "5m"},
            "system": [{
                "type": "text",
                "text": "stable system prompt ".repeat(100)
            }, {
                "type": "text",
                "text": "new system suffix"
            }],
            "messages": [{"role": "user", "content": "hello"}]
        });
        let hit = cache.compute_usage(&extended, "scope");
        assert!(hit.cache_read_estimate > 0);
        let renewed = cache.entry(hash).unwrap();
        assert_eq!(renewed.ttl_secs, 60);
        assert!(renewed.expires_at >= unix_timestamp_secs() + 59);
    }

    #[test]
    fn proportional_split_is_integer_deterministic_and_conserving() {
        let usage = PromptCacheUsage {
            cache_read_estimate: 30,
            cache_covered_estimate: 80,
            prompt_total_estimate: 100,
            decision: CacheDecision::Explicit,
        };
        assert_eq!(usage.split_against_total(1_001), (200, 501, 300));
        assert_eq!(usage.split_against_total(-1), (0, 0, 0));
    }

    #[test]
    fn sqlite_restart_preserves_unexpired_entries_and_legacy_source() {
        let base = temp_base("restart");
        let body = explicit_body();
        {
            let cache = KiroPromptCache::new(Some(base.clone()));
            let miss = cache.compute_usage(&body, "scope");
            assert_eq!(miss.cache_read_estimate, 0);
            cache.flush_for_test(Duration::from_secs(2)).unwrap();
        }
        let cache = KiroPromptCache::new(Some(base.clone()));
        let hit = cache.compute_usage(&body, "scope");
        assert!(hit.cache_read_estimate > 0);
        assert!(
            !base.exists(),
            "new SQLite persistence does not create a JSON snapshot"
        );
        assert!(sqlite_path_for(&base).exists());
        let _ = cache.shutdown(Duration::from_secs(2));
        let _ = std::fs::remove_file(sqlite_path_for(&base));
        let _ = std::fs::remove_file(sqlite_path_for(&base).with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(sqlite_path_for(&base).with_extension("sqlite-shm"));
    }

    #[test]
    fn shutdown_is_idempotent_after_writer_exit() {
        let base = temp_base("shutdown-idempotent");
        let cache = KiroPromptCache::new(Some(base.clone()));
        let _ = cache.compute_usage(&explicit_body(), "scope");
        cache.shutdown(Duration::from_secs(2)).unwrap();
        cache.shutdown(Duration::from_millis(10)).unwrap();
        let _ = std::fs::remove_file(sqlite_path_for(&base));
        let _ = std::fs::remove_file(sqlite_path_for(&base).with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(sqlite_path_for(&base).with_extension("sqlite-shm"));
    }

    #[test]
    fn legacy_json_is_imported_once_and_preserved_for_rollback() {
        let base = temp_base("legacy");
        let entry = CacheEntry {
            tokens: 42,
            ttl_secs: 300,
            expires_at: unix_timestamp_secs() + 300,
            last_hit_at: unix_timestamp_secs(),
        };
        std::fs::write(
            &base,
            serde_json::to_vec(&HashMap::from([(7_u64, entry.clone())])).unwrap(),
        )
        .unwrap();
        let cache = KiroPromptCache::new(Some(base.clone()));
        assert_eq!(cache.entry(7), Some(entry));
        assert!(
            base.exists(),
            "legacy source must remain available for rollback"
        );
        assert!(sqlite_path_for(&base).exists());
        let _ = cache.shutdown(Duration::from_secs(2));
        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_file(sqlite_path_for(&base));
    }

    #[test]
    fn unavailable_disk_never_blocks_or_changes_memory_semantics() {
        let parent_file = temp_base("not-directory");
        std::fs::write(&parent_file, b"not a directory").unwrap();
        let base = parent_file.join("cache");
        let cache = KiroPromptCache::new(Some(base));
        let body = explicit_body();
        let started = Instant::now();
        assert_eq!(cache.compute_usage(&body, "scope").cache_read_estimate, 0);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(cache.compute_usage(&body, "scope").cache_read_estimate > 0);
        assert!(cache.flush_for_test(Duration::from_millis(100)).is_err());
        let _ = cache.shutdown(Duration::from_secs(1));
        let _ = std::fs::remove_file(parent_file);
    }

    #[test]
    fn sqlite_generation_cas_rolls_back_conflicting_batch() {
        let base = temp_base("cas");
        let path = sqlite_path_for(&base);
        let mut connection = open_connection(&path).unwrap();
        initialize_schema(&mut connection).unwrap();
        connection
            .execute(
                "UPDATE prompt_cache_meta SET value = 2 WHERE key = 'generation'",
                [],
            )
            .unwrap();
        let mutations = HashMap::from([(
            1,
            PendingMutation {
                generation: 1,
                entry: Some(CacheEntry {
                    tokens: 10,
                    ttl_secs: 300,
                    expires_at: unix_timestamp_secs() + 300,
                    last_hit_at: unix_timestamp_secs(),
                }),
            },
        )]);
        assert!(persist_mutations(&mut connection, 0, 1, &mutations).is_err());
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM prompt_cache_entries", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            count, 0,
            "conflicting transaction must not partially commit"
        );
        drop(connection);
        let _ = std::fs::remove_file(path);
    }
}
