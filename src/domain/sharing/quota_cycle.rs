//! Account-owned quota periods. This module contains no I/O: callers must
//! validate the captured credential fence and durably commit the candidate
//! ledger before publishing a new period.
use std::collections::BTreeMap;

use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const QUOTA_CYCLE_SCHEMA: u16 = 1;

pub fn opaque_id() -> String {
    let mut bytes = [0; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaSourceFence {
    pub account_incarnation: u64,
    pub identity_generation: u64,
    pub credential_generation: u64,
    pub quota_credential_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaTimeBasis {
    UpstreamWindow,
    DerivedFromWindow,
    KnownNaturalBoundary,
    PublicAnnouncement,
    RedeemConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaWindowObservation {
    pub bucket_id: String,
    pub starts_at_ms: Option<i64>,
    pub ends_at_ms: i64,
    pub duration_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub basis: QuotaTimeBasis,
}

impl QuotaWindowObservation {
    pub fn validate_end(&self) -> Result<(), &'static str> {
        if self.bucket_id.is_empty()
            || self.observed_at_ms < 0
            || self.ends_at_ms <= self.observed_at_ms
            || self
                .ends_at_ms
                .checked_sub(self.observed_at_ms)
                .is_none_or(|delay| delay > 400 * 24 * 60 * 60 * 1000)
        {
            return Err("quota_boundary_invalid");
        }
        Ok(())
    }
    pub fn start(&self) -> Result<i64, &'static str> {
        self.validate_end()?;
        let start = self
            .starts_at_ms
            .or_else(|| {
                self.duration_ms
                    .filter(|duration| *duration > 0)
                    .and_then(|duration| self.ends_at_ms.checked_sub(duration))
            })
            .ok_or("quota_boundary_unavailable")?;
        if self.bucket_id.is_empty()
            || start < 0
            || start > self.observed_at_ms
            || self.ends_at_ms <= start
            || self.ends_at_ms <= self.observed_at_ms
        {
            return Err("quota_boundary_invalid");
        }
        Ok(start)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountQuotaCycle {
    pub stream_incarnation: String,
    pub source_ref: String,
    pub source_epoch: u64,
    pub bucket_id: String,
    pub cycle_id: String,
    pub cycle_sequence: u64,
    pub cycle_revision: u64,
    pub starts_at_ms: i64,
    pub ends_at_ms: Option<i64>,
    pub basis: QuotaTimeBasis,
    pub observed_at_ms: i64,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountQuotaBaseline {
    pub source_ref: String,
    pub cycle_id: String,
    pub target_tokens: u64,
    pub usage_watermark: u64,
    pub applied_at_ms: i64,
    pub applied_by: Option<String>,
}

impl AccountQuotaCycle {
    pub fn contains(&self, started_at_ms: i64) -> bool {
        started_at_ms >= self.starts_at_ms && self.ends_at_ms.is_none_or(|end| started_at_ms < end)
    }

    pub fn same_cycle(&self, other: &Self) -> bool {
        self.stream_incarnation == other.stream_incarnation
            && self.source_ref == other.source_ref
            && self.source_epoch == other.source_epoch
            && self.bucket_id == other.bucket_id
            && self.cycle_id == other.cycle_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaCycleSource {
    pub source_ref: String,
    pub source_epoch: u64,
    pub provider_type: String,
    pub bucket_id: String,
    pub fence: QuotaSourceFence,
    pub cycle: Option<AccountQuotaCycle>,
    #[serde(default)]
    pub next_boundary_ms: Option<i64>,
    #[serde(default)]
    pub last_observed_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaEventStatus {
    Committed,
    Historical,
    NeedsReconcile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaResetEvent {
    pub event_id: String,
    pub source_ref: String,
    pub bucket_id: String,
    pub effective_at_ms: i64,
    pub discovered_at_ms: i64,
    pub basis: QuotaTimeBasis,
    pub status: QuotaEventStatus,
    pub cycle_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaResetCorrection {
    pub event_key: String,
    pub previous_effective_at_ms: i64,
    pub effective_at_ms: i64,
    pub discovered_at_ms: i64,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetCreditStatus {
    IntentDurable,
    Dispatched,
    Unknown,
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetCreditOperation {
    pub operation_id: String,
    pub request_id: String,
    pub source_ref: String,
    pub fence: QuotaSourceFence,
    pub status: ResetCreditStatus,
    pub created_at_ms: i64,
    pub confirmed_at_ms: Option<i64>,
    pub result: Option<String>,
    #[serde(default)]
    pub credit_fingerprint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicResetCursor {
    pub etag: Option<String>,
    pub next_cursor: Option<String>,
    pub from_at_ms: Option<i64>,
    pub checked_at_ms: Option<i64>,
    pub retry_at_ms: Option<i64>,
    #[serde(default)]
    pub cache_query: Option<String>,
    #[serde(default)]
    pub cache_evidence: Option<String>,
    #[serde(default)]
    pub high_watermark_ms: Option<i64>,
    #[serde(default)]
    pub failures: u16,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaCycleLedger {
    pub schema_version: u16,
    pub stream_incarnation: String,
    /// Internal, hashed canonical identities. Never project these map keys.
    pub sources: BTreeMap<String, QuotaCycleSource>,
    pub events: BTreeMap<String, QuotaResetEvent>,
    pub operations: BTreeMap<String, ResetCreditOperation>,
    pub public_cursor: PublicResetCursor,
    /// Minimal validated announcement evidence; no untrusted text or URLs.
    #[serde(default)]
    pub public_events: BTreeMap<String, i64>,
    #[serde(default)]
    pub public_status_cursor: PublicResetCursor,
    #[serde(default)]
    pub public_status_latest: Option<String>,
    #[serde(default)]
    pub corrections: Vec<QuotaResetCorrection>,
    #[serde(default)]
    pub redeem_recovery_required: bool,
    #[serde(default)]
    pub revision: u64,
}

impl Default for QuotaCycleLedger {
    fn default() -> Self {
        Self {
            schema_version: QUOTA_CYCLE_SCHEMA,
            stream_incarnation: opaque_id(),
            sources: BTreeMap::new(),
            events: BTreeMap::new(),
            operations: BTreeMap::new(),
            public_cursor: PublicResetCursor::default(),
            public_events: BTreeMap::new(),
            revision: 0,
            public_status_cursor: PublicResetCursor::default(),
            public_status_latest: None,
            corrections: Vec::new(),
            redeem_recovery_required: false,
        }
    }
}

impl QuotaCycleLedger {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != QUOTA_CYCLE_SCHEMA {
            return Err("unsupported_quota_cycle_schema");
        }
        if self.stream_incarnation.is_empty() {
            return Err("quota_stream_invalid");
        }
        let mut refs = std::collections::BTreeSet::new();
        for (identity, source) in &self.sources {
            if identity.is_empty()
                || source.source_ref.is_empty()
                || source.provider_type.is_empty()
                || source.bucket_id.is_empty()
                || source.source_epoch == 0
                || source.last_observed_at_ms < 0
                || source.next_boundary_ms.is_some_and(|time| time < 0)
                || !refs.insert(&source.source_ref)
            {
                return Err("quota_source_invalid");
            }
            if let Some(cycle) = &source.cycle {
                if cycle.stream_incarnation != self.stream_incarnation
                    || cycle.source_ref != source.source_ref
                    || cycle.source_epoch != source.source_epoch
                    || cycle.bucket_id != source.bucket_id
                    || cycle.cycle_id.is_empty()
                    || cycle.cycle_sequence == 0
                    || cycle.cycle_revision == 0
                    || cycle.starts_at_ms < 0
                    || cycle.observed_at_ms < cycle.starts_at_ms
                    || cycle
                        .ends_at_ms
                        .is_some_and(|end| end <= cycle.starts_at_ms)
                {
                    return Err("quota_cycle_invalid");
                }
                let event = self
                    .events
                    .get(&format!("{}:{}", source.source_ref, cycle.event_id))
                    .ok_or("quota_cycle_event_missing")?;
                if event.cycle_id != cycle.cycle_id
                    || event.effective_at_ms != cycle.starts_at_ms
                    || event.status == QuotaEventStatus::Historical
                {
                    return Err("quota_cycle_event_mismatch");
                }
            }
        }
        for (key, event) in &self.events {
            let source = self
                .source(&event.source_ref)
                .ok_or("quota_event_source_missing")?;
            if event.event_id.is_empty()
                || event.event_id.len() > 256
                || *key != format!("{}:{}", event.source_ref, event.event_id)
                || event.bucket_id != source.bucket_id
                || event.effective_at_ms < 0
                || event.effective_at_ms > event.discovered_at_ms
                || (event.status == QuotaEventStatus::Committed && event.cycle_id.is_empty())
                || (event.status == QuotaEventStatus::Historical && !event.cycle_id.is_empty())
            {
                return Err("quota_event_invalid");
            }
        }
        let mut corrections = std::collections::BTreeSet::new();
        let mut applied = BTreeMap::new();
        for correction in &self.corrections {
            if !self.events.contains_key(&correction.event_key)
                || correction.previous_effective_at_ms < 0
                || correction.effective_at_ms < 0
                || correction.effective_at_ms > correction.discovered_at_ms
                || correction.previous_effective_at_ms == correction.effective_at_ms
                || !corrections.insert((
                    &correction.event_key,
                    correction.previous_effective_at_ms,
                    correction.effective_at_ms,
                    correction.applied,
                ))
            {
                return Err("quota_correction_invalid");
            }
            if correction.applied
                && applied
                    .insert(&correction.event_key, correction.effective_at_ms)
                    .is_some_and(|previous| previous != correction.previous_effective_at_ms)
            {
                return Err("quota_correction_chain_invalid");
            }
        }
        if applied
            .iter()
            .any(|(key, time)| self.events[*key].effective_at_ms != *time)
        {
            return Err("quota_correction_chain_invalid");
        }
        let mut request_ids = std::collections::BTreeSet::new();
        for (key, operation) in &self.operations {
            if operation.operation_id.is_empty()
                || *key != operation.operation_id
                || operation.request_id.is_empty()
                || !request_ids.insert(&operation.request_id)
                || self.source(&operation.source_ref).is_none()
                || operation.created_at_ms < 0
                || operation
                    .confirmed_at_ms
                    .is_some_and(|time| time < operation.created_at_ms)
                || (operation.status == ResetCreditStatus::Confirmed)
                    != operation.confirmed_at_ms.is_some()
                || (matches!(
                    operation.status,
                    ResetCreditStatus::Confirmed | ResetCreditStatus::Rejected
                ) && operation.result.is_none())
            {
                return Err("quota_reset_operation_invalid");
            }
            if operation.status == ResetCreditStatus::Confirmed {
                let event = self
                    .events
                    .get(&format!(
                        "{}:redeem:{}",
                        operation.source_ref, operation.request_id
                    ))
                    .ok_or("quota_reset_receipt_event_missing")?;
                if event.basis != QuotaTimeBasis::RedeemConfirmation
                    || event.status == QuotaEventStatus::Historical
                {
                    return Err("quota_reset_receipt_event_invalid");
                }
            }
        }
        for cursor in [&self.public_cursor, &self.public_status_cursor] {
            if [
                cursor.from_at_ms,
                cursor.checked_at_ms,
                cursor.retry_at_ms,
                cursor.high_watermark_ms,
            ]
            .into_iter()
            .flatten()
            .any(|time| time < 0)
                || cursor
                    .etag
                    .as_ref()
                    .is_some_and(|etag| etag.len() > 1024 || etag.contains(['\r', '\n']))
                || cursor.next_cursor.as_ref().is_some_and(|value| {
                    value.is_empty()
                        || value.len() > 1024
                        || !value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                })
                || (cursor.next_cursor.is_some() && cursor.from_at_ms.is_none())
            {
                return Err("quota_public_cursor_invalid");
            }
        }
        if self.public_events.iter().any(|(id, time)| {
            id.is_empty()
                || id.len() > 64
                || *time < 0
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        }) {
            return Err("quota_public_event_invalid");
        }
        Ok(())
    }

    /// Only a freshly verified identity may register/update a fence. Token
    /// refresh generation is deliberately absent from QuotaSourceFence.
    pub fn register_source(
        &mut self,
        identity: String,
        provider_type: String,
        bucket_id: String,
        fence: QuotaSourceFence,
    ) -> Result<String, &'static str> {
        self.validate()?;
        if identity.is_empty() || bucket_id.is_empty() {
            return Err("identity_unverified");
        }
        let existed = self.sources.contains_key(&identity);
        let source = self
            .sources
            .entry(identity)
            .or_insert_with(|| QuotaCycleSource {
                source_ref: opaque_id(),
                source_epoch: 1,
                provider_type: provider_type.clone(),
                bucket_id: bucket_id.clone(),
                fence: fence.clone(),
                cycle: None,
                next_boundary_ms: None,
                last_observed_at_ms: 0,
            });
        if source.provider_type != provider_type || source.bucket_id != bucket_id {
            return Err("quota_source_scope_mismatch");
        }
        if !existed || source.fence != fence {
            source.fence = fence;
            self.revision = self
                .revision
                .checked_add(1)
                .ok_or("quota_revision_overflow")?;
        }
        Ok(source.source_ref.clone())
    }

    pub fn source(&self, source_ref: &str) -> Option<&QuotaCycleSource> {
        self.sources
            .values()
            .find(|source| source.source_ref == source_ref)
    }

    pub fn observe(
        &mut self,
        source_ref: &str,
        fence: &QuotaSourceFence,
        observation: &QuotaWindowObservation,
    ) -> Result<bool, &'static str> {
        observation.validate_end()?;
        let inferred_start = match observation.start() {
            Ok(start) => Some(start),
            Err("quota_boundary_unavailable") => None,
            Err(error) => return Err(error),
        };
        let source = self
            .sources
            .values_mut()
            .find(|s| s.source_ref == source_ref)
            .ok_or("quota_source_missing")?;
        if &source.fence != fence {
            return Err("quota_source_fence_changed");
        }
        if source.bucket_id != observation.bucket_id {
            return Err("quota_bucket_mismatch");
        }
        if observation.observed_at_ms < source.last_observed_at_ms {
            return Ok(false);
        }
        let boundary_changed = source.next_boundary_ms != Some(observation.ends_at_ms);
        let freshness_changed = source.last_observed_at_ms != observation.observed_at_ms;
        source.next_boundary_ms = Some(observation.ends_at_ms);
        source.last_observed_at_ms = observation.observed_at_ms;
        if boundary_changed || freshness_changed {
            self.revision = self.revision.saturating_add(1);
        }
        if source.cycle.is_none() {
            let Some(inferred_start) = inferred_start else {
                return Ok(boundary_changed || freshness_changed);
            };
            source.cycle = Some(AccountQuotaCycle {
                stream_incarnation: self.stream_incarnation.clone(),
                source_ref: source.source_ref.clone(),
                source_epoch: source.source_epoch,
                bucket_id: source.bucket_id.clone(),
                cycle_id: opaque_id(),
                cycle_sequence: 1,
                cycle_revision: 1,
                starts_at_ms: inferred_start,
                ends_at_ms: Some(observation.ends_at_ms),
                basis: observation.basis,
                observed_at_ms: observation.observed_at_ms,
                event_id: format!("initial:{}", observation.ends_at_ms),
            });
            let cycle = source.cycle.as_ref().expect("initialized above");
            self.events.insert(
                format!("{source_ref}:{}", cycle.event_id),
                QuotaResetEvent {
                    event_id: cycle.event_id.clone(),
                    source_ref: source_ref.into(),
                    bucket_id: source.bucket_id.clone(),
                    effective_at_ms: cycle.starts_at_ms,
                    discovered_at_ms: observation.observed_at_ms,
                    basis: observation.basis,
                    status: QuotaEventStatus::Committed,
                    cycle_id: cycle.cycle_id.clone(),
                },
            );
            self.revision = self.revision.saturating_add(1);
            return Ok(true);
        }
        let cycle = source.cycle.as_mut().expect("initialized above");
        if observation.observed_at_ms < cycle.observed_at_ms {
            return Ok(false);
        }
        // Once a known boundary has elapsed, advance there (not at discovery
        // time). With no recurring proof the following end remains unknown.
        let boundary = cycle
            .ends_at_ms
            .filter(|end| *end <= observation.observed_at_ms);
        if let Some(boundary) = boundary {
            self.apply_reset(
                source_ref,
                fence,
                &format!("natural:{boundary}"),
                boundary,
                observation.observed_at_ms,
                QuotaTimeBasis::KnownNaturalBoundary,
            )?;
            return self.observe(source_ref, fence, observation).map(|_| true);
        }
        if observation
            .starts_at_ms
            .is_some_and(|start| start > cycle.starts_at_ms)
        {
            let start = observation.starts_at_ms.expect("checked above");
            self.apply_reset(
                source_ref,
                fence,
                &format!("upstream:{start}"),
                start,
                observation.observed_at_ms,
                observation.basis,
            )?;
            return self.observe(source_ref, fence, observation).map(|_| true);
        }
        // An explicit upstream start may corroborate a known cycle, but an
        // end-duration inference must never overwrite a confirmed early reset.
        let changed = cycle.ends_at_ms != Some(observation.ends_at_ms);
        if changed {
            cycle.ends_at_ms = Some(observation.ends_at_ms);
            cycle.cycle_revision = cycle.cycle_revision.saturating_add(1);
            self.revision = self.revision.saturating_add(1);
        }
        cycle.observed_at_ms = observation.observed_at_ms;
        Ok(changed || freshness_changed)
    }

    pub fn apply_reset(
        &mut self,
        source_ref: &str,
        fence: &QuotaSourceFence,
        event_id: &str,
        effective_at_ms: i64,
        discovered_at_ms: i64,
        basis: QuotaTimeBasis,
    ) -> Result<bool, &'static str> {
        if event_id.is_empty()
            || event_id.len() > 256
            || effective_at_ms < 0
            || effective_at_ms > discovered_at_ms
        {
            return Err("quota_event_invalid");
        }
        let source = self
            .sources
            .values_mut()
            .find(|s| s.source_ref == source_ref)
            .ok_or("quota_source_missing")?;
        if &source.fence != fence {
            return Err("quota_source_fence_changed");
        }
        let key = format!("{source_ref}:{event_id}");
        if source.cycle.is_none() {
            let cycle_id = opaque_id();
            source.cycle = Some(AccountQuotaCycle {
                stream_incarnation: self.stream_incarnation.clone(),
                source_ref: source_ref.into(),
                source_epoch: source.source_epoch,
                bucket_id: source.bucket_id.clone(),
                cycle_id: cycle_id.clone(),
                cycle_sequence: 1,
                cycle_revision: 1,
                starts_at_ms: effective_at_ms,
                ends_at_ms: source.next_boundary_ms.filter(|end| *end > effective_at_ms),
                basis,
                observed_at_ms: discovered_at_ms,
                event_id: event_id.into(),
            });
            self.events.insert(
                key,
                QuotaResetEvent {
                    event_id: event_id.into(),
                    source_ref: source_ref.into(),
                    bucket_id: source.bucket_id.clone(),
                    effective_at_ms,
                    discovered_at_ms,
                    basis,
                    status: QuotaEventStatus::Committed,
                    cycle_id,
                },
            );
            self.revision = self.revision.saturating_add(1);
            return Ok(true);
        }
        let cycle = source.cycle.as_mut().expect("initialized above");
        let earlier_boundary = self
            .events
            .values()
            .filter(|event| {
                event.source_ref == source_ref
                    && event.status == QuotaEventStatus::Committed
                    && event.cycle_id != cycle.cycle_id
                    && event.effective_at_ms < cycle.starts_at_ms
            })
            .map(|event| event.effective_at_ms)
            .max();
        if let Some(previous) = self.events.get_mut(&key) {
            if previous.effective_at_ms == effective_at_ms {
                return Ok(false);
            }
            let applied = !(previous.cycle_id != cycle.cycle_id
                || cycle.event_id != event_id
                || effective_at_ms >= cycle.ends_at_ms.unwrap_or(i64::MAX)
                || earlier_boundary.is_some_and(|boundary| effective_at_ms <= boundary));
            let correction = QuotaResetCorrection {
                event_key: key,
                previous_effective_at_ms: previous.effective_at_ms,
                effective_at_ms,
                discovered_at_ms,
                applied,
            };
            if self.corrections.iter().any(|prior| {
                prior.event_key == correction.event_key
                    && prior.previous_effective_at_ms == correction.previous_effective_at_ms
                    && prior.effective_at_ms == correction.effective_at_ms
                    && prior.applied == correction.applied
            }) {
                return Ok(false);
            }
            self.corrections.push(correction);
            if !applied {
                previous.status = QuotaEventStatus::NeedsReconcile;
                self.revision = self.revision.saturating_add(1);
                return Ok(false);
            }
            previous.effective_at_ms = effective_at_ms;
            previous.discovered_at_ms = discovered_at_ms;
            cycle.starts_at_ms = effective_at_ms;
            cycle.basis = basis;
            cycle.observed_at_ms = cycle.observed_at_ms.max(discovered_at_ms);
            cycle.cycle_revision = cycle.cycle_revision.saturating_add(1);
            self.revision = self.revision.saturating_add(1);
            return Ok(true);
        }
        let historical = effective_at_ms < cycle.starts_at_ms;
        let advanced = effective_at_ms > cycle.starts_at_ms;
        if advanced {
            cycle.cycle_id = opaque_id();
            cycle.cycle_sequence = cycle
                .cycle_sequence
                .checked_add(1)
                .ok_or("quota_sequence_overflow")?;
            cycle.cycle_revision = 1;
            cycle.starts_at_ms = effective_at_ms;
            cycle.ends_at_ms = cycle.ends_at_ms.filter(|end| *end > effective_at_ms);
            cycle.basis = basis;
            cycle.observed_at_ms = discovered_at_ms;
            cycle.event_id = event_id.to_string();
        }
        self.events.insert(
            key,
            QuotaResetEvent {
                event_id: event_id.to_string(),
                source_ref: source_ref.to_string(),
                bucket_id: source.bucket_id.clone(),
                effective_at_ms,
                discovered_at_ms,
                basis,
                status: if historical {
                    QuotaEventStatus::Historical
                } else {
                    QuotaEventStatus::Committed
                },
                cycle_id: if historical {
                    String::new()
                } else {
                    cycle.cycle_id.clone()
                },
            },
        );
        self.revision = self.revision.saturating_add(1);
        Ok(advanced)
    }

    pub fn advance_due(&mut self, now_ms: i64) -> Result<bool, &'static str> {
        let due: Vec<_> = self
            .sources
            .values()
            .filter_map(|source| {
                let end = source
                    .cycle
                    .as_ref()?
                    .ends_at_ms
                    .filter(|end| *end <= now_ms)?;
                Some((source.source_ref.clone(), source.fence.clone(), end))
            })
            .collect();
        let mut changed = false;
        for (source, fence, end) in due {
            changed |= self.apply_reset(
                &source,
                &fence,
                &format!("natural:{end}"),
                end,
                now_ms,
                QuotaTimeBasis::KnownNaturalBoundary,
            )?;
        }
        Ok(changed)
    }

    pub fn advance_source_due(
        &mut self,
        source_ref: &str,
        now_ms: i64,
    ) -> Result<bool, &'static str> {
        let source = self.source(source_ref).ok_or("quota_source_missing")?;
        let Some(end) = source
            .cycle
            .as_ref()
            .and_then(|cycle| cycle.ends_at_ms)
            .or(source.next_boundary_ms)
            .filter(|end| *end <= now_ms)
        else {
            return Ok(false);
        };
        let fence = source.fence.clone();
        if source.cycle.is_none() {
            let source = self
                .sources
                .values_mut()
                .find(|source| source.source_ref == source_ref)
                .expect("resolved above");
            let event_id = format!("natural:{end}");
            let cycle_id = opaque_id();
            source.cycle = Some(AccountQuotaCycle {
                stream_incarnation: self.stream_incarnation.clone(),
                source_ref: source_ref.into(),
                source_epoch: source.source_epoch,
                bucket_id: source.bucket_id.clone(),
                cycle_id: cycle_id.clone(),
                cycle_sequence: 1,
                cycle_revision: 1,
                starts_at_ms: end,
                ends_at_ms: None,
                basis: QuotaTimeBasis::KnownNaturalBoundary,
                observed_at_ms: now_ms,
                event_id: event_id.clone(),
            });
            self.events.insert(
                format!("{source_ref}:{event_id}"),
                QuotaResetEvent {
                    event_id,
                    source_ref: source_ref.into(),
                    bucket_id: source.bucket_id.clone(),
                    effective_at_ms: end,
                    discovered_at_ms: now_ms,
                    basis: QuotaTimeBasis::KnownNaturalBoundary,
                    status: QuotaEventStatus::Committed,
                    cycle_id,
                },
            );
            source.next_boundary_ms = None;
            self.revision = self.revision.saturating_add(1);
            return Ok(true);
        }
        let changed = self.apply_reset(
            source_ref,
            &fence,
            &format!("natural:{end}"),
            end,
            now_ms,
            QuotaTimeBasis::KnownNaturalBoundary,
        )?;
        if changed {
            self.sources
                .values_mut()
                .find(|source| source.source_ref == source_ref)
                .expect("resolved above")
                .next_boundary_ms = None;
        }
        Ok(changed)
    }

    pub fn begin_redeem(
        &mut self,
        operation: ResetCreditOperation,
    ) -> Result<&ResetCreditOperation, &'static str> {
        let source = self
            .source(&operation.source_ref)
            .ok_or("quota_source_missing")?;
        if source.fence != operation.fence {
            return Err("quota_source_fence_changed");
        }
        if let Some(previous) = self.operations.get(&operation.operation_id) {
            if previous.source_ref != operation.source_ref
                || previous.request_id != operation.request_id
            {
                return Err("reset_operation_conflict");
            }
        }
        if !self.operations.contains_key(&operation.operation_id) {
            self.revision = self.revision.saturating_add(1);
        }
        Ok(self
            .operations
            .entry(operation.operation_id.clone())
            .or_insert(operation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialized() -> (QuotaCycleLedger, String, QuotaSourceFence) {
        let mut ledger = QuotaCycleLedger::default();
        let fence = QuotaSourceFence {
            account_incarnation: 1,
            identity_generation: 1,
            ..Default::default()
        };
        let source = ledger
            .register_source(
                "verified-subject-workspace".into(),
                "codex_oauth".into(),
                "shared_weekly".into(),
                fence.clone(),
            )
            .unwrap();
        ledger
            .observe(
                &source,
                &fence,
                &QuotaWindowObservation {
                    bucket_id: "shared_weekly".into(),
                    starts_at_ms: Some(1_000),
                    ends_at_ms: 10_000,
                    duration_ms: None,
                    observed_at_ms: 2_000,
                    basis: QuotaTimeBasis::UpstreamWindow,
                },
            )
            .unwrap();
        (ledger, source, fence)
    }

    #[test]
    fn delayed_event_is_idempotent_and_old_event_does_not_roll_back() {
        let (mut ledger, source, fence) = initialized();
        assert!(ledger
            .apply_reset(
                &source,
                &fence,
                "public:1",
                4_123,
                8_000,
                QuotaTimeBasis::PublicAnnouncement
            )
            .unwrap());
        let cycle = ledger.source(&source).unwrap().cycle.clone().unwrap();
        assert!(!ledger
            .apply_reset(
                &source,
                &fence,
                "public:1",
                4_123,
                9_000,
                QuotaTimeBasis::PublicAnnouncement
            )
            .unwrap());
        assert!(!ledger
            .apply_reset(
                &source,
                &fence,
                "old",
                3_000,
                9_000,
                QuotaTimeBasis::PublicAnnouncement
            )
            .unwrap());
        assert_eq!(
            ledger.source(&source).unwrap().cycle.as_ref().unwrap(),
            &cycle
        );
        assert!(!cycle.contains(4_122));
        assert!(cycle.contains(4_123));
    }

    #[test]
    fn boundary_correction_keeps_cycle_and_increments_revision() {
        let (mut ledger, source, fence) = initialized();
        ledger
            .apply_reset(
                &source,
                &fence,
                "reset:1",
                4_000,
                8_000,
                QuotaTimeBasis::RedeemConfirmation,
            )
            .unwrap();
        let before = ledger.source(&source).unwrap().cycle.clone().unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "reset:1",
                4_123,
                9_000,
                QuotaTimeBasis::RedeemConfirmation,
            )
            .unwrap();
        let after = ledger.source(&source).unwrap().cycle.as_ref().unwrap();
        assert!(before.same_cycle(after));
        assert_eq!(before.cycle_sequence, after.cycle_sequence);
        assert_eq!(after.cycle_revision, before.cycle_revision + 1);
        assert_eq!(after.observed_at_ms, 9_000);
        ledger.validate().unwrap();
    }

    #[test]
    fn unknown_end_is_open_and_does_not_repeat_resets_offline() {
        let (mut ledger, source, _) = initialized();
        assert!(ledger.advance_due(12_000).unwrap());
        let cycle = ledger.source(&source).unwrap().cycle.clone().unwrap();
        assert_eq!(cycle.starts_at_ms, 10_000);
        assert_eq!(cycle.ends_at_ms, None);
        assert!(!ledger.advance_due(1_000_000).unwrap());
        assert!(cycle.contains(100_000));
    }

    #[test]
    fn identity_fence_rejects_deleted_and_recreated_account_results() {
        let (mut ledger, source, fence) = initialized();
        let mut new_fence = fence.clone();
        new_fence.account_incarnation = 2;
        assert_eq!(
            ledger.apply_reset(
                &source,
                &new_fence,
                "reset",
                5_000,
                6_000,
                QuotaTimeBasis::PublicAnnouncement
            ),
            Err("quota_source_fence_changed")
        );
        let mut restored: QuotaCycleLedger =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        restored.advance_due(10_000).unwrap();
        assert_eq!(
            restored
                .source(&source)
                .unwrap()
                .cycle
                .as_ref()
                .unwrap()
                .cycle_sequence,
            2
        );
    }

    #[test]
    fn quota_freshness_changes_dynamic_revision_not_cycle_identity() {
        let (mut ledger, source, fence) = initialized();
        let before = ledger.source(&source).unwrap().cycle.clone().unwrap();
        let revision = ledger.revision;
        let observation = QuotaWindowObservation {
            bucket_id: "shared_weekly".into(),
            starts_at_ms: None,
            ends_at_ms: 10_000,
            duration_ms: Some(9_000),
            observed_at_ms: 3_000,
            basis: QuotaTimeBasis::DerivedFromWindow,
        };
        assert!(ledger.observe(&source, &fence, &observation).unwrap());
        assert!(ledger.revision > revision);
        let after = ledger.source(&source).unwrap().cycle.as_ref().unwrap();
        assert!(before.same_cycle(after));
        assert_eq!(before.cycle_revision, after.cycle_revision);
        assert_eq!(after.observed_at_ms, 3_000);
        assert!(!ledger.observe(&source, &fence, &observation).unwrap());
    }

    #[test]
    fn quota_repeated_quarantined_correction_does_not_grow_history() {
        let (mut ledger, source, fence) = initialized();
        ledger
            .apply_reset(
                &source,
                &fence,
                "first",
                4_000,
                5_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "second",
                6_000,
                7_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "first",
                6_500,
                8_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        let revision = ledger.revision;
        ledger
            .apply_reset(
                &source,
                &fence,
                "first",
                6_500,
                9_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        assert_eq!(ledger.corrections.len(), 1);
        assert_eq!(ledger.revision, revision);
        assert_eq!(
            ledger
                .source(&source)
                .unwrap()
                .cycle
                .as_ref()
                .unwrap()
                .starts_at_ms,
            6_000
        );
    }

    #[test]
    fn quota_ledger_load_rejects_missing_events_corrupt_corrections_and_cursors() {
        let (mut ledger, source, fence) = initialized();
        ledger
            .apply_reset(
                &source,
                &fence,
                "public:1",
                4_000,
                5_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "public:1",
                4_123,
                6_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "old",
                3_000,
                7_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        ledger.public_cursor.from_at_ms = Some(1_000);
        ledger.public_cursor.next_cursor = Some("page_2".into());
        let restored: QuotaCycleLedger =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        restored.validate().unwrap();

        let mut broken = restored.clone();
        broken.events.remove(&format!("{source}:public:1"));
        assert_eq!(broken.validate(), Err("quota_cycle_event_missing"));
        let mut broken = restored.clone();
        broken
            .events
            .get_mut(&format!("{source}:public:1"))
            .unwrap()
            .effective_at_ms = 4_124;
        assert_eq!(broken.validate(), Err("quota_cycle_event_mismatch"));
        let mut broken = restored.clone();
        broken.corrections[0].effective_at_ms = 4_122;
        assert_eq!(broken.validate(), Err("quota_correction_chain_invalid"));
        let mut broken = restored.clone();
        broken.public_cursor.from_at_ms = None;
        assert_eq!(broken.validate(), Err("quota_public_cursor_invalid"));
        let mut broken = restored;
        broken.public_events.insert("bad/id".into(), 1_000);
        assert_eq!(broken.validate(), Err("quota_public_event_invalid"));
    }

    #[test]
    fn quota_ledger_load_checks_receipts_but_allows_old_identity_fences() {
        let (mut ledger, source, fence) = initialized();
        ledger
            .begin_redeem(ResetCreditOperation {
                operation_id: "operation".into(),
                request_id: "request".into(),
                source_ref: source.clone(),
                fence: fence.clone(),
                status: ResetCreditStatus::Dispatched,
                created_at_ms: 3_000,
                confirmed_at_ms: None,
                result: None,
                credit_fingerprint: "credit".into(),
            })
            .unwrap();
        ledger.validate().unwrap();
        ledger
            .apply_reset(
                &source,
                &fence,
                "redeem:request",
                4_000,
                4_000,
                QuotaTimeBasis::RedeemConfirmation,
            )
            .unwrap();
        let operation = ledger.operations.get_mut("operation").unwrap();
        operation.status = ResetCreditStatus::Confirmed;
        operation.confirmed_at_ms = Some(4_000);
        operation.result = Some("{}".into());
        ledger.validate().unwrap();
        // An identity refresh must not invalidate historical durable receipts.
        ledger
            .sources
            .values_mut()
            .next()
            .unwrap()
            .fence
            .account_incarnation += 1;
        ledger.validate().unwrap();
        let mut broken = ledger.clone();
        broken.operations.get_mut("operation").unwrap().result = None;
        assert_eq!(broken.validate(), Err("quota_reset_operation_invalid"));
        let mut broken = ledger;
        let mut duplicate = broken.operations["operation"].clone();
        duplicate.operation_id = "duplicate".into();
        broken.operations.insert("duplicate".into(), duplicate);
        assert_eq!(broken.validate(), Err("quota_reset_operation_invalid"));
    }
}
