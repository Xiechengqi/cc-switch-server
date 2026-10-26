//! Minimal exact quota history. Unlike report details, these entries are not
//! evicted after 32 days. All revisions needed by an active manual baseline
//! remain available; no guessed identity or minute-rollup backfill is allowed.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::store::{UsageLog, UsageRecordKind};
use crate::domain::sharing::quota_cycle::AccountQuotaCycle;

/// v1 retains all exact revisions, including closed cycles. Report-detail
/// retention is unrelated. Existing in-flight settlements must never be lost.
pub const QUOTA_USAGE_RETENTION_POLICY: &str = "retainAllExactRevisions";
pub const QUOTA_USAGE_WARN_REVISIONS: u64 = 250_000;
pub const QUOTA_USAGE_ADMISSION_LIMIT: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaAttribution {
    pub source_ref: String,
    pub source_epoch: u64,
    pub bucket_id: String,
    pub started_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaUsageEntry {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub share_id: String,
    pub user_email: String,
    pub attribution: QuotaAttribution,
    pub usage_revision: u64,
    pub watermark: u64,
    pub tokens: u64,
    pub count_request: bool,
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaUsageLedger {
    pub schema_version: u16,
    pub watermark: u64,
    /// Collection is complete only after this instant for each source. An old
    /// installation cannot claim an empty current window from missing logs.
    pub coverage_from_ms: BTreeMap<String, i64>,
    /// Indexed by source/epoch/bucket/Share/user, then stable metering record.
    /// A revision stores the cumulative value, not a second charge.
    pub entries: BTreeMap<String, BTreeMap<String, Vec<QuotaUsageEntry>>>,
    /// Global metering identities cannot move to a different scope on retry.
    #[serde(default)]
    pub record_scopes: BTreeMap<String, String>,
    #[serde(default)]
    pub recovery_required: BTreeMap<String, QuotaAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity_error: Option<String>,
}

impl Default for QuotaUsageLedger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            watermark: 0,
            coverage_from_ms: BTreeMap::new(),
            entries: BTreeMap::new(),
            record_scopes: BTreeMap::new(),
            recovery_required: BTreeMap::new(),
            integrity_error: None,
        }
    }
}

fn scope_key(source: &str, epoch: u64, bucket: &str, share: &str, user: &str) -> String {
    // JSON tuple encoding avoids delimiter collisions in imported identifiers.
    serde_json::to_string(&(source, epoch, bucket, share, user.to_ascii_lowercase()))
        .expect("string tuple is serializable")
}

impl QuotaUsageLedger {
    pub fn capacity_warning(&self) -> bool {
        self.watermark >= QUOTA_USAGE_WARN_REVISIONS
    }
    pub fn capacity_limited(&self) -> bool {
        self.watermark >= QUOTA_USAGE_ADMISSION_LIMIT
    }
    pub fn validate_metadata(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported_quota_usage_schema");
        }
        if self
            .coverage_from_ms
            .iter()
            .any(|(source, start)| source.is_empty() || *start < 0)
            || self.recovery_required.values().any(|attribution| {
                attribution.source_ref.is_empty()
                    || attribution.source_epoch == 0
                    || attribution.bucket_id.is_empty()
                    || attribution.started_at_ms < 0
            })
        {
            return Err("quota_usage_integrity_error");
        }
        Ok(())
    }
    pub fn recover_pending(&mut self) -> bool {
        let mut changed = false;
        for records in self.entries.values() {
            for entry in records
                .values()
                .filter_map(|versions| versions.last())
                .filter(|entry| entry.pending)
            {
                if self
                    .recovery_required
                    .insert(entry.request_id.clone(), entry.attribution.clone())
                    .is_none()
                {
                    changed = true;
                }
            }
        }
        changed
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_metadata()?;
        let mut identities = BTreeMap::new();
        let mut watermarks = std::collections::BTreeSet::new();
        for (scope, records) in &self.entries {
            for (id, versions) in records {
                if versions.is_empty() {
                    return Err("quota_usage_integrity_error");
                }
                if identities.insert(id, scope).is_some() {
                    return Err("quota_attribution_changed");
                }
                let mut previous: Option<&QuotaUsageEntry> = None;
                for entry in versions {
                    if entry.request_id != *id
                        || entry.watermark == 0
                        || entry.watermark > self.watermark
                        || entry.attribution.source_epoch == 0
                        || entry.attribution.started_at_ms < 0
                        || entry.attribution.source_ref.is_empty()
                        || entry.attribution.bucket_id.is_empty()
                        || !watermarks.insert(entry.watermark)
                        || scope_key(
                            &entry.attribution.source_ref,
                            entry.attribution.source_epoch,
                            &entry.attribution.bucket_id,
                            &entry.share_id,
                            &entry.user_email,
                        ) != *scope
                    {
                        return Err("quota_usage_integrity_error");
                    }
                    if let Some(prior) = previous {
                        if prior.usage_revision >= entry.usage_revision
                            || prior.watermark >= entry.watermark
                            || prior.attribution != entry.attribution
                            || prior.parent_request_id != entry.parent_request_id
                            || prior.count_request != entry.count_request
                        {
                            return Err("quota_revision_conflict");
                        }
                    }
                    previous = Some(entry);
                }
            }
        }
        // A maximum alone cannot detect a deleted middle revision. All commits
        // are pinned in v1, so unique positive watermarks must cover [1, w].
        if u64::try_from(watermarks.len()).ok() != Some(self.watermark)
            || identities.len() != self.record_scopes.len()
            || identities
                .iter()
                .any(|(id, scope)| self.record_scopes.get(*id) != Some(*scope))
        {
            return Err("quota_usage_missing_revisions");
        }
        Ok(())
    }

    pub fn insert_persisted(&mut self, entry: QuotaUsageEntry) -> Result<(), &'static str> {
        let key = scope_key(
            &entry.attribution.source_ref,
            entry.attribution.source_epoch,
            &entry.attribution.bucket_id,
            &entry.share_id,
            &entry.user_email,
        );
        if self
            .record_scopes
            .get(&entry.request_id)
            .is_some_and(|previous| previous != &key)
        {
            return Err("quota_attribution_changed");
        }
        self.record_scopes
            .insert(entry.request_id.clone(), key.clone());
        self.entries
            .entry(key)
            .or_default()
            .entry(entry.request_id.clone())
            .or_default()
            .push(entry);
        Ok(())
    }

    pub fn metadata(&self) -> Self {
        Self {
            schema_version: self.schema_version,
            watermark: self.watermark,
            coverage_from_ms: self.coverage_from_ms.clone(),
            entries: BTreeMap::new(),
            record_scopes: BTreeMap::new(),
            recovery_required: self.recovery_required.clone(),
            integrity_error: self.integrity_error.clone(),
        }
    }

    pub fn begin_collection(&mut self, source: &str, at_ms: i64) -> bool {
        if self.coverage_from_ms.contains_key(source) {
            return false;
        }
        self.coverage_from_ms.insert(source.to_string(), at_ms);
        true
    }

    pub fn complete_for(&self, cycle: &AccountQuotaCycle) -> bool {
        self.schema_version == 1
            && self.integrity_error.is_none()
            && self
                .coverage_from_ms
                .get(&cycle.source_ref)
                .is_some_and(|start| *start <= cycle.starts_at_ms)
            && !self.recovery_required.values().any(|attribution| {
                attribution.source_ref == cycle.source_ref
                    && attribution.source_epoch == cycle.source_epoch
                    && attribution.bucket_id == cycle.bucket_id
                    && cycle.contains(attribution.started_at_ms)
            })
    }

    pub fn validate_log(&self, log: &UsageLog) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported_quota_usage_schema");
        }
        let previous_scope = self.record_scopes.get(&log.request_id).or_else(|| {
            self.entries
                .iter()
                .find(|(_, entries)| entries.contains_key(&log.request_id))
                .map(|(key, _)| key)
        });
        let Some(attribution) = log.quota_attribution.as_ref() else {
            return if previous_scope.is_some() {
                Err("quota_attribution_changed")
            } else {
                Ok(())
            };
        };
        if attribution.started_at_ms < 0
            || attribution.source_epoch == 0
            || attribution.source_ref.is_empty()
            || attribution.bucket_id.is_empty()
            || log.share_id.is_none()
            || log.user_email.is_none()
            || i64::try_from(log.started_at_ms).ok() != Some(attribution.started_at_ms)
        {
            return Err("quota_attribution_invalid");
        }
        let key = scope_key(
            &attribution.source_ref,
            attribution.source_epoch,
            &attribution.bucket_id,
            log.share_id.as_deref().unwrap_or_default(),
            log.user_email.as_deref().unwrap_or_default(),
        );
        if previous_scope.is_some_and(|previous| previous != &key) {
            return Err("quota_attribution_changed");
        }
        if let Some(previous) = self
            .entries
            .get(&key)
            .and_then(|entries| entries.get(&log.request_id))
            .and_then(|versions| versions.last())
        {
            if previous.attribution != *attribution
                || previous.parent_request_id != log.parent_request_id
            {
                return Err("quota_attribution_changed");
            }
            if previous.count_request != (log.record_kind == UsageRecordKind::UserInference) {
                return Err("quota_attribution_changed");
            }
            if previous.usage_revision == log.usage_revision
                && previous.tokens != log.quota_tokens()
            {
                return Err("quota_revision_conflict");
            }
        }
        Ok(())
    }

    pub fn record(&mut self, log: &UsageLog) -> Result<bool, &'static str> {
        self.validate_log(log)?;
        let Some(attribution) = log.quota_attribution.as_ref() else {
            return Ok(false);
        };
        if log.record_kind == UsageRecordKind::HealthProbe {
            return Ok(false);
        }
        let (Some(share), Some(user)) = (log.share_id.as_ref(), log.user_email.as_ref()) else {
            return Ok(false);
        };
        let key = scope_key(
            &attribution.source_ref,
            attribution.source_epoch,
            &attribution.bucket_id,
            share,
            user,
        );
        self.record_scopes
            .insert(log.request_id.clone(), key.clone());
        let versions = self
            .entries
            .entry(key)
            .or_default()
            .entry(log.request_id.clone())
            .or_default();
        if versions
            .last()
            .is_some_and(|previous| previous.usage_revision >= log.usage_revision)
        {
            return Ok(false);
        }
        if log.failure_kind.as_deref() == Some("server_restarted") {
            self.recovery_required
                .insert(log.request_id.clone(), attribution.clone());
        } else if log.usage_state == super::store::UsageState::Observed {
            self.recovery_required.remove(&log.request_id);
        }
        self.watermark = self
            .watermark
            .checked_add(1)
            .ok_or("quota_usage_watermark_overflow")?;
        versions.push(QuotaUsageEntry {
            request_id: log.request_id.clone(),
            parent_request_id: log.parent_request_id.clone(),
            share_id: share.clone(),
            user_email: user.to_ascii_lowercase(),
            attribution: attribution.clone(),
            usage_revision: log.usage_revision,
            watermark: self.watermark,
            tokens: log.quota_tokens(),
            count_request: log.record_kind == UsageRecordKind::UserInference,
            pending: log.usage_state == super::store::UsageState::Pending,
        });
        Ok(true)
    }

    pub fn query(
        &self,
        cycle: &AccountQuotaCycle,
        share: &str,
        user: &str,
        watermark: u64,
    ) -> Result<(u64, u64), &'static str> {
        if !self.complete_for(cycle) {
            return Err("history_incomplete");
        }
        let key = scope_key(
            &cycle.source_ref,
            cycle.source_epoch,
            &cycle.bucket_id,
            share,
            user,
        );
        let mut tokens = 0u64;
        let mut requests = 0u64;
        if let Some(records) = self.entries.get(&key) {
            for versions in records.values() {
                if let Some(entry) = versions
                    .iter()
                    .rev()
                    .find(|entry| entry.watermark <= watermark)
                {
                    if cycle.contains(entry.attribution.started_at_ms) {
                        tokens = tokens
                            .checked_add(entry.tokens)
                            .ok_or("quota_token_overflow")?;
                        requests = requests
                            .checked_add(u64::from(entry.count_request))
                            .ok_or("quota_request_overflow")?;
                    }
                }
            }
        }
        Ok((tokens, requests))
    }

    pub fn effective_with_baseline(observed: u64, target: u64, observed_at_edit: u64) -> u64 {
        // Saturate after subtraction, not before: late downward corrections
        // must be reflected without minting a permanent positive offset.
        (i128::from(target) + i128::from(observed) - i128::from(observed_at_edit))
            .clamp(0, i128::from(u64::MAX)) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        providers::model::{AppKind, ProviderType},
        sharing::quota_cycle::QuotaTimeBasis,
        usage::store::{TokenUsage, UsageModelMetadata},
    };

    fn cycle(start: i64) -> AccountQuotaCycle {
        AccountQuotaCycle {
            stream_incarnation: "stream".into(),
            source_ref: "opaque".into(),
            source_epoch: 1,
            bucket_id: "weekly".into(),
            cycle_id: "c".into(),
            cycle_sequence: 1,
            cycle_revision: 1,
            starts_at_ms: start,
            ends_at_ms: None,
            basis: QuotaTimeBasis::UpstreamWindow,
            observed_at_ms: start,
            event_id: "e".into(),
        }
    }
    fn log(id: &str, start: i64, tokens: u64) -> UsageLog {
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p".into(),
            "P".into(),
            ProviderType::CodexOAuth,
            200,
            1,
            UsageModelMetadata::default(),
            TokenUsage {
                total_tokens: Some(tokens),
                ..Default::default()
            },
        );
        log.request_id = id.into();
        log.share_id = Some("share".into());
        log.user_email = Some("u@example.com".into());
        log.started_at_ms = start as u128;
        log.created_at_ms = start as u128;
        log.quota_attribution = Some(QuotaAttribution {
            source_ref: "opaque".into(),
            source_epoch: 1,
            bucket_id: "weekly".into(),
            started_at_ms: start,
        });
        log
    }
    #[test]
    fn millisecond_boundary_and_late_completion_are_exact() {
        let mut ledger = QuotaUsageLedger::default();
        ledger.begin_collection("opaque", 0);
        ledger.record(&log("before", 4_122, 10)).unwrap();
        ledger.record(&log("after", 4_123, 20)).unwrap();
        ledger.record(&log("old-finished-late", 4_000, 30)).unwrap();
        assert_eq!(
            ledger
                .query(&cycle(4_123), "share", "U@EXAMPLE.COM", u64::MAX)
                .unwrap(),
            (20, 1)
        );
        let restored: QuotaUsageLedger =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        assert_eq!(
            restored
                .query(&cycle(4_123), "share", "u@example.com", u64::MAX)
                .unwrap(),
            (20, 1)
        );
    }

    #[test]
    fn quota_capacity_thresholds_warn_then_restrict_but_never_evict_settlements() {
        let mut ledger = QuotaUsageLedger {
            watermark: QUOTA_USAGE_WARN_REVISIONS - 1,
            ..Default::default()
        };
        assert!(!ledger.capacity_warning());
        ledger.watermark += 1;
        assert!(ledger.capacity_warning());
        assert!(!ledger.capacity_limited());
        ledger.watermark = QUOTA_USAGE_ADMISSION_LIMIT;
        assert!(ledger.capacity_limited());
        ledger
            .record(&log("still-settle-inflight", 4_123, 10))
            .unwrap();
        assert_eq!(ledger.watermark, QUOTA_USAGE_ADMISSION_LIMIT + 1);
        assert_eq!(ledger.entries.values().next().unwrap().len(), 1);
        assert_eq!(QUOTA_USAGE_RETENTION_POLICY, "retainAllExactRevisions");
    }
    #[test]
    fn revisions_deduplicate_and_baseline_watermark_remains_rebuildable() {
        let mut ledger = QuotaUsageLedger::default();
        ledger.begin_collection("opaque", 0);
        let mut record = log("r", 5_000, 100);
        ledger.record(&record).unwrap();
        let at_edit = ledger.watermark;
        assert!(!ledger.record(&record).unwrap());
        record.total_tokens = Some(150);
        record.usage_revision += 1;
        ledger.record(&record).unwrap();
        assert_eq!(
            ledger
                .query(&cycle(4_123), "share", "u@example.com", at_edit)
                .unwrap()
                .0,
            100
        );
        assert_eq!(
            ledger
                .query(&cycle(4_123), "share", "u@example.com", ledger.watermark)
                .unwrap()
                .0,
            150
        );
        assert_eq!(QuotaUsageLedger::effective_with_baseline(150, 10, 100), 60);
        assert_eq!(QuotaUsageLedger::effective_with_baseline(50, 10, 100), 0);
    }
    #[test]
    fn missing_history_or_different_identity_never_becomes_free_quota() {
        let mut ledger = QuotaUsageLedger::default();
        ledger.begin_collection("opaque", 5_000);
        assert_eq!(
            ledger.query(&cycle(4_123), "share", "u@example.com", u64::MAX),
            Err("history_incomplete")
        );
        let mut other = cycle(6_000);
        other.source_ref = "other".into();
        assert!(!ledger.complete_for(&other));
    }

    #[test]
    fn quota_missing_middle_revision_and_pending_crash_cannot_be_complete() {
        let mut ledger = QuotaUsageLedger::default();
        ledger.begin_collection("opaque", 0);
        for id in ["a", "b", "c"] {
            ledger.record(&log(id, 5_000, 10)).unwrap();
        }
        ledger.validate().unwrap();
        let scope = ledger.record_scopes["b"].clone();
        ledger.entries.get_mut(&scope).unwrap().remove("b");
        ledger.record_scopes.remove("b");
        assert_eq!(ledger.validate(), Err("quota_usage_missing_revisions"));
        let mut ledger = QuotaUsageLedger::default();
        ledger.begin_collection("opaque", 0);
        let mut pending = log("pending", 5_000, 0);
        pending.usage_state = super::super::store::UsageState::Pending;
        ledger.record(&pending).unwrap();
        assert!(ledger.recover_pending());
        assert!(!ledger.complete_for(&cycle(4_123)));
        pending.usage_state = super::super::store::UsageState::Observed;
        pending.usage_revision += 1;
        pending.total_tokens = Some(30);
        ledger.record(&pending).unwrap();
        assert!(ledger.complete_for(&cycle(4_123)));
        assert_eq!(
            ledger
                .query(&cycle(4_123), "share", "u@example.com", ledger.watermark)
                .unwrap()
                .0,
            30
        );
    }

    #[test]
    fn quota_revision_permutations_and_supplemental_identity_are_exact() {
        for order in [[1, 2, 3], [3, 1, 2], [2, 3, 1]] {
            let mut ledger = QuotaUsageLedger::default();
            ledger.begin_collection("opaque", 0);
            for revision in order {
                let mut record = log("r", 5_000, revision * 10);
                record.usage_revision = revision;
                ledger.record(&record).unwrap();
            }
            assert_eq!(
                ledger
                    .query(&cycle(4_123), "share", "u@example.com", ledger.watermark)
                    .unwrap(),
                (30, 1)
            );
            let mut supplemental = log("supplemental", 5_000, 7);
            supplemental.record_kind = UsageRecordKind::InternalSupplemental;
            supplemental.parent_request_id = Some("r".into());
            ledger.record(&supplemental).unwrap();
            assert_eq!(
                ledger
                    .query(&cycle(4_123), "share", "u@example.com", ledger.watermark)
                    .unwrap(),
                (37, 1)
            );
            ledger.validate().unwrap();
        }
    }
}
