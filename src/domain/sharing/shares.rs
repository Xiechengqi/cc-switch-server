use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Context;
use chrono::{Datelike, TimeZone, Utc, Weekday};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::ProviderStore;
use crate::domain::router::{ClientSubdomain, ShareSlug};
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareAnchoredUsageBucket, ShareGrantManager,
    ShareManagedGrantAction, ShareManagedGrantOperation, ShareSettingsPatch, ShareTokenPeriod,
    ShareTotalUsageEdit, ShareUserGrant, ShareUserPolicy, ShareUserQuotaView, ShareUserUsage,
    ShareUserUsageBucket, ShareUserUsageEdit, ShareUserUsageEditAction,
};
use crate::domain::sharing::token_period::{token_period_window, validate_user_policy};
use crate::domain::usage::store::UsageStore;
use crate::infra::time::now_ms;

const SHARES_FILE_NAME: &str = "shares.json";
pub const DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 60;
pub const MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 10;
pub const MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 7 * 24 * 60;
const MAX_MANAGED_GRANT_DURATION_SECONDS: u64 = 365 * 24 * 60 * 60;

fn default_banked_reset_expiry_lead_minutes() -> u32 {
    DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES
}

impl ShareUserUsage {
    pub fn tokens_for(&self, period: ShareTokenPeriod, now_ms: i64) -> u64 {
        match period {
            ShareTokenPeriod::Lifetime => self.lifetime.tokens_used,
            ShareTokenPeriod::Day => current_bucket_tokens(&self.day, utc_day_start_ms(now_ms)),
            ShareTokenPeriod::Week => current_bucket_tokens(&self.week, utc_week_start_ms(now_ms)),
            ShareTokenPeriod::CalendarMonth => {
                current_bucket_tokens(&self.calendar_month, utc_calendar_month_start_ms(now_ms))
            }
            ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays => 0,
        }
    }

    pub fn tokens_for_policy(&self, policy: &ShareUserPolicy, now_ms: i64) -> u64 {
        if !policy.token_period.requires_anchor() {
            return self.tokens_for(policy.token_period, now_ms);
        }
        let Ok(window) = token_period_window(policy, now_ms) else {
            return 0;
        };
        let Some(anchor_at_ms) = policy.token_period_anchor_at_ms else {
            return 0;
        };
        let Some(started_at_ms) = window.starts_at_ms else {
            return 0;
        };
        self.anchored
            .as_ref()
            .filter(|bucket| {
                bucket.period == policy.token_period
                    && bucket.anchor_at_ms == anchor_at_ms
                    && bucket.started_at_ms == started_at_ms
            })
            .map(|bucket| bucket.tokens_used)
            .unwrap_or(0)
    }

    fn rebuild_current_policy_bucket(
        &mut self,
        policy: &ShareUserPolicy,
        now_ms: i64,
        tokens_used: u64,
        requests_count: u64,
    ) -> Result<(), String> {
        match policy.token_period {
            ShareTokenPeriod::Lifetime => {
                self.lifetime = ShareUserUsageBucket {
                    started_at_ms: 0,
                    tokens_used,
                    requests_count,
                };
                self.anchored = None;
            }
            ShareTokenPeriod::Day => {
                self.day = ShareUserUsageBucket {
                    started_at_ms: utc_day_start_ms(now_ms),
                    tokens_used,
                    requests_count,
                };
                self.anchored = None;
            }
            ShareTokenPeriod::Week => {
                self.week = ShareUserUsageBucket {
                    started_at_ms: utc_week_start_ms(now_ms),
                    tokens_used,
                    requests_count,
                };
                self.anchored = None;
            }
            ShareTokenPeriod::CalendarMonth => {
                self.calendar_month = ShareUserUsageBucket {
                    started_at_ms: utc_calendar_month_start_ms(now_ms),
                    tokens_used,
                    requests_count,
                };
                self.anchored = None;
            }
            period @ (ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays) => {
                let window = token_period_window(policy, now_ms)?;
                self.anchored = Some(ShareAnchoredUsageBucket {
                    period,
                    anchor_at_ms: policy
                        .token_period_anchor_at_ms
                        .ok_or_else(|| "fixed token period has no anchor".to_string())?,
                    started_at_ms: window
                        .starts_at_ms
                        .ok_or_else(|| "fixed token period has no start".to_string())?,
                    tokens_used,
                    requests_count,
                });
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn record(&mut self, tokens: u64, now_ms: i64) {
        self.record_inner(tokens, now_ms, true);
    }

    fn record_inner(&mut self, tokens: u64, now_ms: i64, count_request: bool) {
        record_bucket(&mut self.lifetime, 0, tokens, count_request);
        record_bucket(
            &mut self.day,
            utc_day_start_ms(now_ms),
            tokens,
            count_request,
        );
        record_bucket(
            &mut self.week,
            utc_week_start_ms(now_ms),
            tokens,
            count_request,
        );
        record_bucket(
            &mut self.calendar_month,
            utc_calendar_month_start_ms(now_ms),
            tokens,
            count_request,
        );
    }

    fn record_for_policy(&mut self, policy: &ShareUserPolicy, tokens: u64, now_ms: i64) {
        self.record_for_policy_inner(policy, tokens, now_ms, true);
    }

    fn record_supplemental_for_policy(
        &mut self,
        policy: &ShareUserPolicy,
        tokens: u64,
        now_ms: i64,
    ) {
        self.record_for_policy_inner(policy, tokens, now_ms, false);
    }

    fn record_for_policy_inner(
        &mut self,
        policy: &ShareUserPolicy,
        tokens: u64,
        now_ms: i64,
        count_request: bool,
    ) {
        self.record_inner(tokens, now_ms, count_request);
        if !policy.token_period.requires_anchor() {
            return;
        }
        let Ok(window) = token_period_window(policy, now_ms) else {
            return;
        };
        let (Some(anchor_at_ms), Some(started_at_ms)) =
            (policy.token_period_anchor_at_ms, window.starts_at_ms)
        else {
            return;
        };
        let replace = self.anchored.as_ref().is_none_or(|bucket| {
            bucket.period != policy.token_period
                || bucket.anchor_at_ms != anchor_at_ms
                || bucket.started_at_ms != started_at_ms
        });
        if replace {
            self.anchored = Some(ShareAnchoredUsageBucket {
                period: policy.token_period,
                anchor_at_ms,
                started_at_ms,
                tokens_used: 0,
                requests_count: 0,
            });
        }
        if let Some(bucket) = self.anchored.as_mut() {
            bucket.tokens_used = bucket.tokens_used.saturating_add(tokens);
            if count_request {
                bucket.requests_count = bucket.requests_count.saturating_add(1);
            }
        }
    }

    pub fn rebuild_anchored(
        &mut self,
        policy: &ShareUserPolicy,
        now_ms: i64,
        tokens_used: u64,
        requests_count: u64,
    ) -> Result<(), String> {
        if !policy.token_period.requires_anchor() {
            self.anchored = None;
            return Ok(());
        }
        let window = token_period_window(policy, now_ms)?;
        self.anchored = Some(ShareAnchoredUsageBucket {
            period: policy.token_period,
            anchor_at_ms: policy
                .token_period_anchor_at_ms
                .ok_or_else(|| "tokenPeriodAnchorAtMs is required".to_string())?,
            started_at_ms: window
                .starts_at_ms
                .ok_or_else(|| "fixed token period has no start".to_string())?,
            tokens_used,
            requests_count,
        });
        Ok(())
    }
}

fn current_bucket_tokens(bucket: &ShareUserUsageBucket, expected_start_ms: i64) -> u64 {
    if bucket.started_at_ms == expected_start_ms {
        bucket.tokens_used
    } else {
        0
    }
}

fn record_bucket(
    bucket: &mut ShareUserUsageBucket,
    start_ms: i64,
    tokens: u64,
    count_request: bool,
) {
    if bucket.started_at_ms != start_ms {
        *bucket = ShareUserUsageBucket {
            started_at_ms: start_ms,
            ..ShareUserUsageBucket::default()
        };
    }
    bucket.tokens_used = bucket.tokens_used.saturating_add(tokens);
    if count_request {
        bucket.requests_count = bucket.requests_count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShareUserQuotaWindow {
    period: ShareTokenPeriod,
    anchor_at_ms: Option<i64>,
    starts_at_ms: Option<i64>,
    ends_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShareUserQuotaSnapshot {
    window: ShareUserQuotaWindow,
    observed_tokens: u64,
    observed_requests: u64,
}

fn quota_window_for_policy(
    policy: &ShareUserPolicy,
    now_ms: i64,
) -> Result<ShareUserQuotaWindow, String> {
    if policy.token_period == ShareTokenPeriod::Lifetime {
        return Ok(ShareUserQuotaWindow {
            period: policy.token_period,
            anchor_at_ms: None,
            starts_at_ms: None,
            ends_at_ms: None,
        });
    }
    let window = token_period_window(policy, now_ms)?;
    Ok(ShareUserQuotaWindow {
        period: policy.token_period,
        anchor_at_ms: policy.token_period_anchor_at_ms,
        starts_at_ms: window.starts_at_ms,
        ends_at_ms: window.ends_at_ms,
    })
}

fn observe_user_quota(
    usage: &UsageStore,
    share_id: &str,
    email: &str,
    grant: &ShareUserGrant,
    now_ms: i64,
) -> Result<ShareUserQuotaSnapshot, String> {
    let window = quota_window_for_policy(&grant.policy, now_ms)?;
    let (starts_at_ms, ends_at_ms) = match (window.starts_at_ms, window.ends_at_ms) {
        (Some(start), Some(end)) => (start, end),
        (None, None) => (0, now_ms.saturating_add(1)),
        _ => return Err("quota window has only one boundary".to_string()),
    };
    let (observed_tokens, observed_requests) =
        usage.share_user_quota_usage(share_id, email, starts_at_ms, ends_at_ms);
    Ok(ShareUserQuotaSnapshot {
        window,
        observed_tokens,
        observed_requests,
    })
}

fn rebase_matches_window(
    rebase: &crate::domain::sharing::router_contract::ShareUserUsageRebase,
    window: &ShareUserQuotaWindow,
) -> bool {
    rebase.period == window.period
        && rebase.anchor_at_ms == window.anchor_at_ms
        && rebase.window_starts_at_ms == window.starts_at_ms
        && rebase.window_ends_at_ms == window.ends_at_ms
}

fn effective_user_tokens(grant: &ShareUserGrant, snapshot: &ShareUserQuotaSnapshot) -> u64 {
    let Some(rebase) = grant
        .usage_rebase
        .as_ref()
        .filter(|rebase| rebase_matches_window(rebase, &snapshot.window))
    else {
        return snapshot.observed_tokens;
    };
    rebase.target_tokens.saturating_add(
        snapshot
            .observed_tokens
            .saturating_sub(rebase.observed_tokens_at_rebase),
    )
}

fn effective_grant_tokens_for_policy(grant: &ShareUserGrant, now_ms: i64) -> u64 {
    let bucket = grant.usage.tokens_for_policy(&grant.policy, now_ms);
    if let Some(rebase) = grant.usage_rebase.as_ref() {
        if rebase.period == grant.policy.token_period
            && rebase.anchor_at_ms == grant.policy.token_period_anchor_at_ms
        {
            return rebase.target_tokens.max(bucket).max(
                grant
                    .usage_quota
                    .map(|quota| quota.effective_tokens_used)
                    .unwrap_or(0),
            );
        }
    }
    if let Some(quota) = grant.usage_quota {
        if quota.rebase_applies {
            return quota.effective_tokens_used.max(bucket);
        }
    }
    bucket
}

/// Builds the derived quota view a client can render without re-deriving the
/// rebase arithmetic itself.
fn quota_view(
    grant: &ShareUserGrant,
    snapshot: &ShareUserQuotaSnapshot,
    effective_tokens: u64,
    rebase_applies: bool,
) -> ShareUserQuotaView {
    ShareUserQuotaView {
        period: snapshot.window.period,
        anchor_at_ms: snapshot.window.anchor_at_ms,
        window_starts_at_ms: snapshot.window.starts_at_ms,
        window_ends_at_ms: snapshot.window.ends_at_ms,
        effective_tokens_used: effective_tokens,
        observed_tokens_used: snapshot.observed_tokens,
        // i64 keeps a baseline set below the observed history representable;
        // clamping it to zero would hide exactly the correction an operator
        // made and needs to see.
        manual_offset_tokens: i64::try_from(effective_tokens)
            .unwrap_or(i64::MAX)
            .saturating_sub(i64::try_from(snapshot.observed_tokens).unwrap_or(i64::MAX)),
        observed_requests_count: snapshot.observed_requests,
        rebase_applies: rebase_applies && grant.usage_rebase.is_some(),
    }
}

fn utc_datetime(now_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_else(Utc::now)
}

fn utc_day_start_ms(now_ms: i64) -> i64 {
    utc_datetime(now_ms)
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("UTC midnight is valid")
        .and_utc()
        .timestamp_millis()
}

fn utc_week_start_ms(now_ms: i64) -> i64 {
    let now = utc_datetime(now_ms);
    let days = match now.weekday() {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    (now.date_naive() - chrono::Duration::days(days))
        .and_hms_opt(0, 0, 0)
        .expect("UTC week boundary is valid")
        .and_utc()
        .timestamp_millis()
}

fn utc_calendar_month_start_ms(now_ms: i64) -> i64 {
    let now = utc_datetime(now_ms);
    chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
        .expect("UTC month boundary is valid")
        .and_hms_opt(0, 0, 0)
        .expect("UTC month midnight is valid")
        .and_utc()
        .timestamp_millis()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStore {
    #[serde(default)]
    pub shares: Vec<Share>,
    #[serde(default)]
    pub pending_router_deletes: Vec<ShareDeleteTombstone>,
    #[serde(default)]
    pub router_share_prune_marker: Option<RouterSharePruneMarker>,
    #[serde(default)]
    pub router_registered: bool,
    #[serde(default)]
    pub last_router_error: Option<String>,
    #[serde(default)]
    pub last_router_heartbeat_ms: Option<u128>,
    #[serde(default)]
    pub router_descriptor_sync_mode: RouterDescriptorSyncMode,
    #[serde(default)]
    pub router_descriptor_sync_diagnostic: Option<String>,
    #[serde(default)]
    pub applied_router_control_operations: BTreeMap<String, AppliedRouterControlOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedRouterControlOperation {
    applied_at_ms: u128,
    fingerprint: String,
    #[serde(default)]
    share_id: String,
    #[serde(default)]
    share_sequence: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_applied_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_effective_policy: Option<ShareUserPolicy>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterDescriptorSyncMode {
    #[default]
    Unknown,
    Legacy,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDeleteTombstone {
    pub share_id: String,
    pub operation_id: String,
    pub created_at_ms: u128,
    #[serde(default)]
    pub router_api_base: Option<String>,
    #[serde(default)]
    pub installation_id: Option<String>,
    #[serde(default)]
    pub last_attempt_at_ms: Option<u128>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl ShareDeleteTombstone {
    pub fn has_legacy_router_target(&self) -> bool {
        self.router_api_base.is_none() && self.installation_id.is_none()
    }

    pub fn router_target_matches(&self, router_api_base: &str, installation_id: &str) -> bool {
        self.router_api_base.as_deref().is_some_and(|target| {
            normalize_router_api_base(target) == normalize_router_api_base(router_api_base)
        }) && self
            .installation_id
            .as_deref()
            .is_some_and(|target| target.trim() == installation_id.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterSharePruneMarker {
    pub router_api_base: String,
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePolicy {
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub free_access: bool,
    #[serde(default)]
    pub allow_personal_credits: bool,
    #[serde(default)]
    pub auto_consume_banked_reset: bool,
    #[serde(default = "default_banked_reset_expiry_lead_minutes")]
    pub banked_reset_expiry_lead_minutes: u32,
    #[serde(default)]
    pub previous_response_cache_enabled: bool,
}

impl Default for SharePolicy {
    fn default() -> Self {
        Self {
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            free_access: false,
            allow_personal_credits: false,
            auto_consume_banked_reset: false,
            banked_reset_expiry_lead_minutes: DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES,
            previous_response_cache_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub id: String,
    #[serde(default)]
    pub capacity_pool_id: String,
    #[serde(default)]
    pub owner_email: Option<String>,
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_share_status")]
    pub status: String,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(flatten)]
    pub policy: SharePolicy,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_apps: Option<BTreeSet<AppKind>>,
    #[serde(default)]
    pub bindings: Vec<ShareBinding>,
    #[serde(default)]
    pub binding_history: Vec<ShareBindingHistory>,
    #[serde(default)]
    pub runtime_snapshot: Option<Value>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity_error: Option<ShareIntegrityError>,
    #[serde(default)]
    pub router_last_synced_at_ms: Option<u128>,
    #[serde(default)]
    pub router_last_sync_error: Option<String>,
    #[serde(default)]
    pub router_url: Option<String>,
    #[serde(default)]
    pub config_revision: u64,
    #[serde(default)]
    pub router_synced_revision: u64,
    #[serde(default)]
    pub descriptor_generation: u64,
    #[serde(default)]
    pub descriptor_fingerprint: Option<String>,
    #[serde(default)]
    pub router_synced_descriptor_generation: u64,
    #[serde(default)]
    pub router_synced_descriptor_fingerprint: Option<String>,
    #[serde(default)]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareIntegrityError {
    pub code: String,
    pub message: String,
    pub checked_at_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareIntegrityStatus {
    Healthy,
    Repaired,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareIntegrityOutcome {
    pub share_id: String,
    pub status: ShareIntegrityStatus,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ShareIntegrityError>,
}

impl ShareIntegrityOutcome {
    pub fn changed(&self) -> bool {
        self.changed
    }
}

impl std::ops::Deref for Share {
    type Target = SharePolicy;

    fn deref(&self) -> &Self::Target {
        &self.policy
    }
}

impl std::ops::DerefMut for Share {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.policy
    }
}

fn share_bound_apps(share: &Share) -> BTreeSet<AppKind> {
    if share.bindings.is_empty() {
        return BTreeSet::from([share.app]);
    }
    share.bindings.iter().map(|binding| binding.app).collect()
}

pub fn share_enabled_apps(share: &Share) -> BTreeSet<AppKind> {
    let bound = share_bound_apps(share);
    match share.enabled_apps.as_ref() {
        Some(enabled) => enabled.intersection(&bound).copied().collect(),
        None => bound,
    }
}

pub fn share_app_api_enabled(share: &Share, app: AppKind) -> bool {
    share_enabled_apps(share).contains(&app)
}

pub fn support_from_enabled_apps(
    enabled: &BTreeSet<AppKind>,
) -> crate::domain::sharing::router_contract::ShareSupport {
    crate::domain::sharing::router_contract::ShareSupport {
        claude: enabled.contains(&AppKind::Claude),
        codex: enabled.contains(&AppKind::Codex),
        gemini: enabled.contains(&AppKind::Gemini),
    }
}

fn enabled_apps_from_support(
    support: &crate::domain::sharing::router_contract::ShareSupport,
    bound: &BTreeSet<AppKind>,
) -> Result<BTreeSet<AppKind>, SharePatchError> {
    let mut enabled = BTreeSet::new();
    if support.claude {
        enabled.insert(AppKind::Claude);
    }
    if support.codex {
        enabled.insert(AppKind::Codex);
    }
    if support.gemini {
        enabled.insert(AppKind::Gemini);
    }
    if let Some(app) = enabled.iter().find(|app| !bound.contains(app)) {
        return Err(SharePatchError::Invalid(format!(
            "support contains unbound app {}",
            app.as_str()
        )));
    }
    if enabled.is_empty() {
        return Err(SharePatchError::Invalid(
            "at least one bound app API must stay enabled".to_string(),
        ));
    }
    Ok(enabled)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBinding {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBindingHistory {
    pub app: AppKind,
    pub previous_provider_id: Option<String>,
    pub next_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_subscription_identity_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_subscription_identity_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_kind: Option<String>,
    pub changed_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpsertShareInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub owner_email: Option<String>,
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub free_access: Option<bool>,
    #[serde(default)]
    pub allow_personal_credits: Option<bool>,
    #[serde(default)]
    pub auto_consume_banked_reset: Option<bool>,
    #[serde(default)]
    pub banked_reset_expiry_lead_minutes: Option<u32>,
    #[serde(default)]
    pub previous_response_cache_enabled: Option<bool>,
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_apps: Option<BTreeSet<AppKind>>,
    #[serde(default)]
    pub bindings: Vec<ShareBinding>,
    #[serde(default)]
    pub runtime_snapshot: Option<Value>,
    #[serde(default)]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

impl ShareStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = shares_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            fs::read_to_string(&path).with_context(|| format!("read shares {}", path.display()))?;
        let mut value: Value = serde_json::from_str(&content)
            .with_context(|| format!("parse shares {}", path.display()))?;
        super::legacy_token_market_migration::migrate_legacy_share_contract(&mut value)
            .with_context(|| format!("migrate legacy shares {}", path.display()))?;
        serde_json::from_value(value).with_context(|| format!("decode shares {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = shares_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write shares {}", path.display()))
    }

    pub fn upsert(&mut self, input: UpsertShareInput) -> Result<Share, SharePatchError> {
        self.upsert_with_capacity(input, None)
    }

    pub fn upsert_with_capacity(
        &mut self,
        mut input: UpsertShareInput,
        capacity_pool_id: Option<String>,
    ) -> Result<Share, SharePatchError> {
        let _binding =
            crate::domain::sharing::invariants::validate_and_normalize_upsert_input(&mut input)?;
        let provider_id = input.provider_id.clone();
        let provider_type = input.provider_type;
        let app = input.app;
        let existing_id = input.id.clone().or_else(|| {
            self.shares
                .iter()
                .find(|item| item.app == app && item.provider_id == provider_id)
                .map(|item| item.id.clone())
        });
        if let Some(existing) = existing_id
            .as_deref()
            .and_then(|id| self.shares.iter().find(|share| share.id == id))
        {
            let binding_changed = existing.app != input.app
                || existing.provider_id != input.provider_id
                || existing.provider_type != input.provider_type
                || existing.bindings != input.bindings;
            if binding_changed {
                return Err(SharePatchError::BindingImmutable);
            }
        }
        let owner_email = input.owner_email.clone();
        let existing_policy = existing_id
            .as_deref()
            .and_then(|id| self.shares.iter().find(|share| share.id == id))
            .map(|share| share.policy.clone())
            .unwrap_or_default();
        let tunnel_subdomain = input.tunnel_subdomain.clone().or_else(|| {
            existing_id
                .as_deref()
                .and_then(|id| self.shares.iter().find(|share| share.id == id))
                .and_then(|share| share.tunnel_subdomain.clone())
                .or_else(|| Some(generate_unique_share_slug(&self.shares)))
        });

        if let Some(conflict) = self.shares.iter().find(|item| {
            existing_id.as_deref() != Some(item.id.as_str())
                && item.status != "deleted"
                && input.bindings.iter().any(|binding| {
                    item.bindings.iter().any(|existing| {
                        existing.app == binding.app && existing.provider_id == binding.provider_id
                    })
                })
        }) {
            return Err(SharePatchError::Invalid(format!(
                "provider already has share {}",
                conflict.id
            )));
        }

        if let Some(subdomain) = tunnel_subdomain.as_deref() {
            if let Some(conflict) = self.shares.iter().find(|item| {
                item.tunnel_subdomain.as_deref() == Some(subdomain)
                    && existing_id.as_deref() != Some(item.id.as_str())
                    && item.status != "deleted"
            }) {
                return Err(SharePatchError::Invalid(format!(
                    "share subdomain is already used by {}",
                    conflict.id
                )));
            }
        }

        let preserved = existing_id
            .as_deref()
            .and_then(|id| self.shares.iter().find(|item| item.id == id))
            .map(|existing| {
                (
                    existing.tokens_used,
                    existing.requests_count,
                    existing.binding_history.clone(),
                    existing.created_at_ms,
                    existing.router_last_synced_at_ms,
                    existing.router_last_sync_error.clone(),
                    existing.router_url.clone(),
                    existing.last_error.clone(),
                    existing.config_revision,
                    existing.router_synced_revision,
                    existing.descriptor_generation,
                    existing.descriptor_fingerprint.clone(),
                    existing.router_synced_descriptor_generation,
                    existing.router_synced_descriptor_fingerprint.clone(),
                    existing.user_grants.clone(),
                    existing.enabled_apps.clone(),
                )
            });

        let share_id = existing_id.unwrap_or_else(generate_share_id);
        let (
            tokens_used,
            requests_count,
            binding_history,
            created_at_ms,
            router_last_synced_at_ms,
            router_last_sync_error,
            router_url,
            last_error,
            config_revision,
            router_synced_revision,
            descriptor_generation,
            descriptor_fingerprint,
            router_synced_descriptor_generation,
            router_synced_descriptor_fingerprint,
            preserved_user_grants,
            preserved_enabled_apps,
        ) = preserved.unwrap_or((
            0,
            0,
            Vec::new(),
            0,
            None,
            None,
            None,
            None,
            0,
            0,
            0,
            None,
            0,
            None,
            BTreeMap::new(),
            None,
        ));
        let created_at_ms = if created_at_ms > 0 {
            created_at_ms
        } else {
            crate::infra::time::now_ms()
        };
        let explicit_user_grants = (!input.user_grants.is_empty()).then_some(input.user_grants);
        let capacity_pool_id = capacity_pool_id
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.shares
                    .iter()
                    .find(|share| share.id == share_id)
                    .map(|share| share.capacity_pool_id.clone())
            })
            .unwrap_or_default();

        let share = Share {
            id: share_id,
            capacity_pool_id,
            owner_email,
            app,
            provider_id,
            provider_type,
            display_name: input.display_name,
            enabled: input.enabled.unwrap_or(true),
            status: input.status.unwrap_or_else(default_share_status),
            subscription_level: input.subscription_level,
            account_email: input.account_email,
            quota_percent: input.quota_percent,
            tunnel_subdomain,
            policy: SharePolicy {
                token_limit: input.token_limit,
                parallel_limit: input.parallel_limit,
                expires_at: input.expires_at,
                free_access: input.free_access.unwrap_or(false),
                allow_personal_credits: input
                    .allow_personal_credits
                    .unwrap_or(existing_policy.allow_personal_credits),
                auto_consume_banked_reset: input
                    .auto_consume_banked_reset
                    .unwrap_or(existing_policy.auto_consume_banked_reset),
                banked_reset_expiry_lead_minutes: input
                    .banked_reset_expiry_lead_minutes
                    .unwrap_or(existing_policy.banked_reset_expiry_lead_minutes),
                previous_response_cache_enabled: input
                    .previous_response_cache_enabled
                    .unwrap_or(existing_policy.previous_response_cache_enabled),
            },
            tokens_used,
            requests_count,
            created_at_ms,
            auto_start: input.auto_start.unwrap_or(true),
            description: input.description,
            enabled_apps: input.enabled_apps.or(preserved_enabled_apps),
            bindings: input.bindings,
            binding_history,
            runtime_snapshot: input.runtime_snapshot,
            last_error,
            integrity_error: None,
            router_last_synced_at_ms,
            router_last_sync_error,
            router_url,
            config_revision: config_revision.saturating_add(1).max(1),
            router_synced_revision,
            descriptor_generation,
            descriptor_fingerprint,
            router_synced_descriptor_generation,
            router_synced_descriptor_fingerprint,
            user_grants: preserved_user_grants.clone(),
        };

        let mut share = share;
        if let Some(user_grants) = explicit_user_grants.as_ref() {
            share.user_grants = normalize_user_grants(
                user_grants,
                &preserved_user_grants,
                share.owner_email.as_deref(),
            )?;
        }
        reconcile_user_grants(&mut share);
        share.router_last_sync_error = None;

        self.cancel_pending_router_delete(&share.id);
        if let Some(existing) = self.shares.iter_mut().find(|item| item.id == share.id) {
            *existing = share.clone();
        } else {
            self.shares.push(share.clone());
        }

        Ok(share)
    }

    pub fn get(&self, share_id: &str) -> Option<&Share> {
        self.shares.iter().find(|item| item.id == share_id)
    }

    pub fn share_ids_for_provider(&self, app: AppKind, provider_id: &str) -> Vec<String> {
        self.shares
            .iter()
            .filter(|item| {
                item.bindings
                    .iter()
                    .any(|binding| binding.app == app && binding.provider_id == provider_id)
                    || (item.bindings.is_empty()
                        && item.app == app
                        && item.provider_id == provider_id)
            })
            .map(|item| item.id.clone())
            .collect()
    }

    pub fn refresh_capacity_pool_ids(
        &mut self,
        providers: &ProviderStore,
        accounts: &AccountStore,
        root_key: &[u8; 32],
    ) -> Result<Vec<String>, crate::domain::sharing::credential_source::ShareCredentialSourceError>
    {
        let mut updated_ids = Vec::new();
        for share in self
            .shares
            .iter_mut()
            .filter(|share| share.status != "deleted")
        {
            let capacity_pool_id =
                crate::domain::sharing::credential_source::capacity_pool_id_for_share(
                    providers, accounts, share, root_key,
                )?;
            if share.capacity_pool_id == capacity_pool_id {
                continue;
            }
            share.capacity_pool_id = capacity_pool_id;
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        Ok(updated_ids)
    }

    pub fn refresh_capacity_pool_ids_for_shares(
        &mut self,
        share_ids: &BTreeSet<String>,
        providers: &ProviderStore,
        accounts: &AccountStore,
        root_key: &[u8; 32],
    ) -> Result<Vec<String>, crate::domain::sharing::credential_source::ShareCredentialSourceError>
    {
        let mut updated_ids = Vec::new();
        for share in self
            .shares
            .iter_mut()
            .filter(|share| share.status != "deleted" && share_ids.contains(&share.id))
        {
            let capacity_pool_id =
                crate::domain::sharing::credential_source::capacity_pool_id_for_share(
                    providers, accounts, share, root_key,
                )?;
            if share.capacity_pool_id == capacity_pool_id {
                continue;
            }
            share.capacity_pool_id = capacity_pool_id;
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        Ok(updated_ids)
    }

    pub fn repair_integrity(
        &mut self,
        providers: &ProviderStore,
        accounts: &AccountStore,
        root_key: &[u8; 32],
    ) -> Vec<ShareIntegrityOutcome> {
        let mut outcomes = Vec::new();
        for share in self
            .shares
            .iter_mut()
            .filter(|share| share.status != "deleted")
        {
            let mut repaired = false;
            if share.bindings.is_empty() {
                let primary_exists = providers.providers.iter().any(|stored| {
                    stored.app == share.app
                        && stored.provider.id == share.provider_id
                        && stored.provider_type == share.provider_type
                });
                if primary_exists {
                    share.bindings.push(ShareBinding {
                        app: share.app,
                        provider_id: share.provider_id.clone(),
                        provider_type: share.provider_type,
                    });
                    repaired = true;
                } else {
                    outcomes.push(disable_share_for_integrity(
                        share,
                        "cc_switch_share_binding_integrity_failed",
                        "Share has no bindings and its primary Provider does not exist",
                    ));
                    continue;
                }
            }

            if let Err(message) = validate_share_provider_bindings(share, providers) {
                outcomes.push(disable_share_for_integrity(
                    share,
                    "cc_switch_share_provider_reference_invalid",
                    &message,
                ));
                continue;
            }

            match crate::domain::sharing::credential_source::capacity_pool_id_for_share(
                providers, accounts, share, root_key,
            ) {
                Ok(capacity_pool_id) => {
                    if share.capacity_pool_id != capacity_pool_id {
                        share.capacity_pool_id = capacity_pool_id;
                        repaired = true;
                    }
                    if share.integrity_error.take().is_some() {
                        repaired = true;
                    }
                    if repaired {
                        mark_share_config_pending(share);
                    }
                    outcomes.push(ShareIntegrityOutcome {
                        share_id: share.id.clone(),
                        status: if repaired {
                            ShareIntegrityStatus::Repaired
                        } else {
                            ShareIntegrityStatus::Healthy
                        },
                        changed: repaired,
                        error: None,
                    });
                }
                Err(error) => outcomes.push(disable_share_for_integrity(
                    share,
                    error.code(),
                    &error.to_string(),
                )),
            }
        }
        outcomes
    }

    pub fn apply_capacity_pool_ids(
        &mut self,
        capacity_pool_ids: &BTreeMap<String, String>,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in self
            .shares
            .iter_mut()
            .filter(|share| share.status != "deleted")
        {
            let Some(capacity_pool_id) = capacity_pool_ids.get(&share.id) else {
                continue;
            };
            if capacity_pool_id.is_empty() || share.capacity_pool_id == *capacity_pool_id {
                continue;
            }
            share.capacity_pool_id.clone_from(capacity_pool_id);
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn delete(&mut self, share_id: &str) -> Option<ShareDeleteTombstone> {
        self.delete_with_router_target(share_id, None)
    }

    pub fn delete_for_router_target(
        &mut self,
        share_id: &str,
        router_api_base: &str,
        installation_id: &str,
    ) -> Option<ShareDeleteTombstone> {
        self.delete_with_router_target(share_id, Some((router_api_base, installation_id)))
    }

    fn delete_with_router_target(
        &mut self,
        share_id: &str,
        router_target: Option<(&str, &str)>,
    ) -> Option<ShareDeleteTombstone> {
        let before = self.shares.len();
        self.shares.retain(|item| item.id != share_id);
        if self.shares.len() == before {
            return None;
        }
        self.pending_router_deletes
            .retain(|pending| pending.share_id != share_id);
        let tombstone = ShareDeleteTombstone {
            share_id: share_id.to_string(),
            operation_id: generate_share_delete_operation_id(),
            created_at_ms: now_ms(),
            router_api_base: router_target
                .map(|(router_api_base, _)| normalize_router_api_base(router_api_base)),
            installation_id: router_target
                .map(|(_, installation_id)| installation_id.trim().to_string()),
            last_attempt_at_ms: None,
            last_error: None,
        };
        self.pending_router_deletes.push(tombstone.clone());
        Some(tombstone)
    }

    pub fn bind_legacy_router_delete_target(
        &mut self,
        operation_id: &str,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        let Some(tombstone) = self
            .pending_router_deletes
            .iter_mut()
            .find(|pending| pending.operation_id == operation_id)
        else {
            return false;
        };
        if tombstone.router_api_base.is_some() || tombstone.installation_id.is_some() {
            return false;
        }
        tombstone.router_api_base = Some(normalize_router_api_base(router_api_base));
        tombstone.installation_id = Some(installation_id.trim().to_string());
        true
    }

    pub fn pending_router_delete(
        &self,
        share_id: &str,
        operation_id: &str,
    ) -> Option<&ShareDeleteTombstone> {
        self.pending_router_deletes
            .iter()
            .find(|pending| pending.share_id == share_id && pending.operation_id == operation_id)
    }

    pub fn has_pending_router_delete_for_share(&self, share_id: &str) -> bool {
        self.pending_router_deletes
            .iter()
            .any(|pending| pending.share_id == share_id)
    }

    pub fn router_share_prune_applied_for(
        &self,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        self.router_share_prune_marker
            .as_ref()
            .is_some_and(|marker| {
                normalize_router_api_base(&marker.router_api_base)
                    == normalize_router_api_base(router_api_base)
                    && marker.installation_id.trim() == installation_id.trim()
            })
    }

    pub fn mark_router_share_prune_applied(
        &mut self,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        if self.router_share_prune_applied_for(router_api_base, installation_id) {
            return false;
        }
        self.router_share_prune_marker = Some(RouterSharePruneMarker {
            router_api_base: normalize_router_api_base(router_api_base),
            installation_id: installation_id.trim().to_string(),
        });
        true
    }

    pub fn complete_pending_router_delete(&mut self, operation_id: &str) -> bool {
        let before = self.pending_router_deletes.len();
        self.pending_router_deletes
            .retain(|pending| pending.operation_id != operation_id);
        self.pending_router_deletes.len() != before
    }

    pub fn mark_pending_router_delete_failure(
        &mut self,
        operation_id: &str,
        error: String,
    ) -> bool {
        let Some(pending) = self
            .pending_router_deletes
            .iter_mut()
            .find(|pending| pending.operation_id == operation_id)
        else {
            return false;
        };
        pending.last_attempt_at_ms = Some(now_ms());
        pending.last_error = Some(error);
        true
    }

    fn cancel_pending_router_delete(&mut self, share_id: &str) -> bool {
        let before = self.pending_router_deletes.len();
        self.pending_router_deletes
            .retain(|pending| pending.share_id != share_id);
        self.pending_router_deletes.len() != before
    }

    pub fn pause(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = false;
        share.status = "paused".to_string();
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn resume(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = true;
        share.status = "active".to_string();
        share.last_error = None;
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn reset_usage(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.tokens_used = 0;
        share.requests_count = 0;
        let reset_at = now_ms();
        for grant in share.user_grants.values_mut() {
            grant.usage = ShareUserUsage::default();
            // The durable operator baseline must go with the snapshot.  A
            // surviving rebase would be re-applied by the next history
            // rebuild and silently restore the very usage this reset cleared.
            grant.usage_rebase = None;
            grant.updated_at_ms = reset_at;
        }
        if share.status == "exhausted" {
            share.status = "paused".to_string();
            share.enabled = false;
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.remove("usage");
                object.remove("lastRequest");
                object.insert("tokensUsed".to_string(), json!(share.tokens_used));
                object.insert("requestsCount".to_string(), json!(share.requests_count));
            }
        }
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn validate_for_invocation(
        &mut self,
        share_id: &str,
        app: AppKind,
        user_email: Option<&str>,
        now_ms: i64,
    ) -> Result<ShareInvocation, ShareInvocationRejection> {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::NotFound,
                message: "Share not found on this cc-switch.".to_string(),
                status_changed: false,
                concurrency: None,
            });
        };

        if !share.bindings.iter().any(|binding| binding.app == app) {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::UnsupportedApp,
                message: format!("Share does not provide the {} API format.", app.as_str()),
                status_changed: false,
                concurrency: None,
            });
        }
        if !share_app_api_enabled(share, app) {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::UnsupportedApp,
                message: format!("Share has disabled the {} API.", app.as_str()),
                status_changed: false,
                concurrency: None,
            });
        }

        if !share.enabled || share.status != "active" {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Inactive,
                message: format!(
                    "Share is not active (current status: {}). Start the share first.",
                    share.status
                ),
                status_changed: false,
                concurrency: None,
            });
        }

        if share
            .expires_at
            .is_some_and(|expires_at| share_expired(expires_at, now_ms))
        {
            share.status = "expired".to_string();
            share.enabled = false;
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Expired,
                message: "Share has expired. Extend the share expiration or create a new share."
                    .to_string(),
                status_changed: true,
                concurrency: None,
            });
        }

        if share
            .token_limit
            .is_some_and(|token_limit| share.tokens_used >= token_limit)
        {
            share.status = "exhausted".to_string();
            share.enabled = false;
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Exhausted,
                message:
                    "Share token quota has been exhausted. Reset usage or increase the token limit."
                        .to_string(),
                status_changed: true,
                concurrency: None,
            });
        }

        let normalized_user_email = user_email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let user_grant = normalized_user_email
            .as_deref()
            .and_then(|email| share.user_grants.get(email))
            .filter(|grant| grant.active);
        if normalized_user_email.is_none() {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::UserIdentityRequired,
                message: "An authenticated user identity is required to invoke this Share."
                    .to_string(),
                status_changed: false,
                concurrency: None,
            });
        }
        if !share.free_access && normalized_user_email.is_some() && user_grant.is_none() {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Unauthorized,
                message: "This user is not authorized to invoke the Share.".to_string(),
                status_changed: false,
                concurrency: None,
            });
        }
        if let Some(grant) = user_grant {
            if !grant.policy.allowed_apps.is_empty() && !grant.policy.allowed_apps.contains(&app) {
                return Err(ShareInvocationRejection {
                    reason: ShareRejectReason::AppNotAllowed,
                    message: format!(
                        "This market entitlement does not include the {} API.",
                        app.as_str()
                    ),
                    status_changed: false,
                    concurrency: None,
                });
            }
            if grant
                .policy
                .expires_at
                .is_some_and(|expires_at| share_expired(expires_at, now_ms))
            {
                return Err(ShareInvocationRejection {
                    reason: ShareRejectReason::UserExpired,
                    message: "This user's Share access has expired.".to_string(),
                    status_changed: false,
                    concurrency: None,
                });
            }
            if grant
                .policy
                .token_limit
                .is_some_and(|limit| effective_grant_tokens_for_policy(grant, now_ms) >= limit)
            {
                return Err(ShareInvocationRejection {
                    reason: ShareRejectReason::UserExhausted,
                    message: "This user's Share token quota has been exhausted.".to_string(),
                    status_changed: false,
                    concurrency: None,
                });
            }
        }

        Ok(ShareInvocation {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            parallel_limit: share.parallel_limit,
            user_email: normalized_user_email,
            user_parallel_limit: user_grant.and_then(|grant| grant.policy.parallel_limit),
        })
    }

    pub fn record_invocation_result(&mut self, share_id: &str, tokens: u64) -> Option<Share> {
        self.record_user_invocation_result(share_id, None, tokens, now_ms() as i64)
    }

    pub fn record_user_invocation_result(
        &mut self,
        share_id: &str,
        user_email: Option<&str>,
        tokens: u64,
        recorded_at_ms: i64,
    ) -> Option<Share> {
        self.record_user_usage(share_id, user_email, tokens, recorded_at_ms, true)
    }

    pub fn record_user_supplemental_usage(
        &mut self,
        share_id: &str,
        user_email: Option<&str>,
        tokens: u64,
        recorded_at_ms: i64,
    ) -> Option<Share> {
        self.record_user_usage(share_id, user_email, tokens, recorded_at_ms, false)
    }

    fn record_user_usage(
        &mut self,
        share_id: &str,
        user_email: Option<&str>,
        tokens: u64,
        recorded_at_ms: i64,
        count_request: bool,
    ) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        if count_request {
            share.requests_count = share.requests_count.saturating_add(1);
        }
        share.tokens_used = share.tokens_used.saturating_add(tokens);
        if share
            .token_limit
            .is_some_and(|token_limit| share.tokens_used >= token_limit)
        {
            share.status = "exhausted".to_string();
            share.enabled = false;
        }
        if let Some(email) = user_email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
        {
            if let Some(grant) = share.user_grants.get_mut(&email) {
                let policy = grant.policy.clone();
                let baseline = effective_grant_tokens_for_policy(grant, recorded_at_ms);
                let current = grant.usage.tokens_for_policy(&policy, recorded_at_ms);
                if baseline > current {
                    let _ = grant.usage.rebuild_current_policy_bucket(
                        &policy,
                        recorded_at_ms,
                        baseline,
                        grant
                            .usage_quota
                            .map(|quota| quota.observed_requests_count)
                            .unwrap_or(0),
                    );
                }
                if count_request {
                    grant
                        .usage
                        .record_for_policy(&policy, tokens, recorded_at_ms);
                } else {
                    grant
                        .usage
                        .record_supplemental_for_policy(&policy, tokens, recorded_at_ms);
                }
                let used = grant.usage.tokens_for_policy(&policy, recorded_at_ms);
                if let Some(quota) = grant.usage_quota.as_mut() {
                    quota.effective_tokens_used = used;
                    quota.observed_tokens_used = quota.observed_tokens_used.saturating_add(tokens);
                    if count_request {
                        quota.observed_requests_count =
                            quota.observed_requests_count.saturating_add(1);
                    }
                }
                grant.updated_at_ms = now_ms();
            }
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("tokensUsed".to_string(), json!(share.tokens_used));
                object.insert("requestsCount".to_string(), json!(share.requests_count));
                object.insert("shareStatus".to_string(), json!(share.status));
            }
        }
        Some(share.clone())
    }

    pub fn update_binding(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
    ) -> Result<Share, ShareUpdateError> {
        self.update_binding_inner(share_id, binding, None)
    }

    pub fn update_binding_with_capacity(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
        capacity_pool_id: String,
    ) -> Result<Share, ShareUpdateError> {
        self.update_binding_inner(share_id, binding, Some(capacity_pool_id))
    }

    fn update_binding_inner(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
        capacity_pool_id: Option<String>,
    ) -> Result<Share, ShareUpdateError> {
        if self.shares.iter().any(|item| {
            item.id != share_id
                && item.status != "deleted"
                && item.bindings.iter().any(|existing| {
                    existing.app == binding.app && existing.provider_id == binding.provider_id
                })
        }) {
            return Err(ShareUpdateError::ProviderAlreadyShared);
        }
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(ShareUpdateError::NotFound)?;
        if share.status != "paused" {
            return Err(ShareUpdateError::MustBePaused);
        }

        if binding.app != share.app {
            return Err(ShareUpdateError::InvalidApp);
        }

        let Some(existing_binding) = share
            .bindings
            .iter_mut()
            .find(|existing| existing.app == binding.app)
        else {
            return Err(ShareUpdateError::InvalidApp);
        };
        let previous_provider_id = Some(existing_binding.provider_id.clone());
        *existing_binding = binding.clone();

        share.provider_id = binding.provider_id.clone();
        share.provider_type = binding.provider_type;
        if let Some(capacity_pool_id) = capacity_pool_id {
            share.capacity_pool_id = capacity_pool_id;
        }

        share.binding_history.push(ShareBindingHistory {
            app: binding.app,
            previous_provider_id,
            next_provider_id: Some(binding.provider_id),
            previous_subscription_identity_fingerprint: None,
            next_subscription_identity_fingerprint: None,
            change_kind: Some("provider_binding".to_string()),
            changed_at_ms: now_ms(),
        });

        mark_share_config_pending(share);

        Some(share.clone()).ok_or(ShareUpdateError::NotFound)
    }

    pub fn add_binding(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
    ) -> Result<Share, SharePatchError> {
        self.add_binding_inner(share_id, binding, None)
    }

    pub fn add_binding_with_capacity(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
        capacity_pool_id: String,
    ) -> Result<Share, SharePatchError> {
        self.add_binding_inner(share_id, binding, Some(capacity_pool_id))
    }

    fn add_binding_inner(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
        capacity_pool_id: Option<String>,
    ) -> Result<Share, SharePatchError> {
        if self.shares.iter().any(|share| {
            share.id != share_id
                && share.status != "deleted"
                && share.bindings.iter().any(|existing| {
                    existing.app == binding.app && existing.provider_id == binding.provider_id
                })
        }) {
            return Err(SharePatchError::Invalid(
                "provider already has an active share".to_string(),
            ));
        }
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        if share
            .bindings
            .iter()
            .any(|existing| existing.app == binding.app)
        {
            return Err(SharePatchError::Invalid(format!(
                "share already has a {} binding",
                binding.app.as_str()
            )));
        }
        if share.bindings.len() >= 3 {
            return Err(SharePatchError::Invalid(
                "share already has the maximum number of bindings".to_string(),
            ));
        }
        share.bindings.push(binding.clone());
        share.bindings.sort_by_key(|item| item.app);
        if let Some(capacity_pool_id) = capacity_pool_id {
            share.capacity_pool_id = capacity_pool_id;
        }
        share.binding_history.push(ShareBindingHistory {
            app: binding.app,
            previous_provider_id: None,
            next_provider_id: Some(binding.provider_id),
            previous_subscription_identity_fingerprint: None,
            next_subscription_identity_fingerprint: None,
            change_kind: Some("binding_added".to_string()),
            changed_at_ms: now_ms(),
        });
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    pub fn remove_binding(
        &mut self,
        share_id: &str,
        app: AppKind,
        provider_id: &str,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        if share.bindings.len() <= 1 {
            return Err(SharePatchError::Invalid(
                "removing the final binding must delete the share".to_string(),
            ));
        }
        let Some(index) = share
            .bindings
            .iter()
            .position(|binding| binding.app == app && binding.provider_id == provider_id)
        else {
            return Err(SharePatchError::Invalid(
                "provider is not bound to this share".to_string(),
            ));
        };
        let removed = share.bindings.remove(index);
        if share.app == removed.app {
            let primary = share
                .bindings
                .first()
                .expect("a multi-binding share retains at least one binding");
            share.app = primary.app;
            share.provider_id = primary.provider_id.clone();
            share.provider_type = primary.provider_type;
        }
        share.binding_history.push(ShareBindingHistory {
            app: removed.app,
            previous_provider_id: Some(removed.provider_id),
            next_provider_id: None,
            previous_subscription_identity_fingerprint: None,
            next_subscription_identity_fingerprint: None,
            change_kind: Some("binding_removed".to_string()),
            changed_at_ms: now_ms(),
        });
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn replace_bundle_configuration_with_usage(
        &mut self,
        share_id: &str,
        mut bindings: Vec<ShareBinding>,
        capacity_pool_id: String,
        tunnel_subdomain: Option<String>,
        settings: ShareSettingsPatch,
        usage_edits: Option<&BTreeMap<String, ShareUserUsageEdit>>,
        enabled: bool,
        usage: &UsageStore,
        applied_at_ms: i64,
        operator: Option<&str>,
    ) -> Result<Share, SharePatchError> {
        if !(1..=3).contains(&bindings.len()) {
            return Err(SharePatchError::Invalid(
                "share must have between one and three bindings".to_string(),
            ));
        }
        bindings.sort_by_key(|binding| binding.app);
        if bindings.windows(2).any(|pair| pair[0].app == pair[1].app) {
            return Err(SharePatchError::Invalid(
                "share must have at most one binding per app".to_string(),
            ));
        }
        if bindings
            .iter()
            .any(|binding| binding.provider_id.trim().is_empty())
        {
            return Err(SharePatchError::Invalid(
                "share binding provider_id is required".to_string(),
            ));
        }

        let mut candidate = self.clone();
        let index = candidate
            .shares
            .iter()
            .position(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        if let Some(conflict) = candidate.shares.iter().find(|share| {
            share.id != share_id
                && share.status != "deleted"
                && share.bindings.iter().any(|existing| {
                    bindings.iter().any(|binding| {
                        existing.app == binding.app && existing.provider_id == binding.provider_id
                    })
                })
        }) {
            return Err(SharePatchError::Invalid(format!(
                "provider already has share {}",
                conflict.id
            )));
        }

        let tunnel_subdomain = tunnel_subdomain
            .map(|subdomain| {
                normalize_share_subdomain(&subdomain)
                    .map_err(|message| SharePatchError::Invalid(message.to_string()))
            })
            .transpose()?;
        if let Some(subdomain) = tunnel_subdomain.as_deref() {
            if let Some(conflict) = candidate.shares.iter().find(|share| {
                share.id != share_id
                    && share.status != "deleted"
                    && share.tunnel_subdomain.as_deref() == Some(subdomain)
            }) {
                return Err(SharePatchError::Invalid(format!(
                    "share subdomain is already used by {}",
                    conflict.id
                )));
            }
        }

        let mut share = candidate.shares[index].clone();
        let previous_bindings = share
            .bindings
            .iter()
            .map(|binding| (binding.app, binding.clone()))
            .collect::<BTreeMap<_, _>>();
        let next_bindings = bindings
            .iter()
            .map(|binding| (binding.app, binding.clone()))
            .collect::<BTreeMap<_, _>>();
        let changed_at_ms = now_ms();
        for app in previous_bindings
            .keys()
            .chain(next_bindings.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            let previous = previous_bindings.get(&app);
            let next = next_bindings.get(&app);
            if previous == next {
                continue;
            }
            share.binding_history.push(ShareBindingHistory {
                app,
                previous_provider_id: previous.map(|binding| binding.provider_id.clone()),
                next_provider_id: next.map(|binding| binding.provider_id.clone()),
                previous_subscription_identity_fingerprint: None,
                next_subscription_identity_fingerprint: None,
                change_kind: Some(
                    match (previous, next) {
                        (None, Some(_)) => "binding_added",
                        (Some(_), None) => "binding_removed",
                        (Some(_), Some(_)) => "provider_binding",
                        (None, None) => unreachable!("binding union contains the app"),
                    }
                    .to_string(),
                ),
                changed_at_ms,
            });
        }

        let primary = bindings
            .iter()
            .find(|binding| binding.app == share.app)
            .unwrap_or_else(|| bindings.first().expect("bindings are non-empty"));
        share.app = primary.app;
        share.provider_id = primary.provider_id.clone();
        share.provider_type = primary.provider_type;
        share.bindings = bindings;
        share.capacity_pool_id = capacity_pool_id;
        if let Some(subdomain) = tunnel_subdomain {
            share.tunnel_subdomain = Some(subdomain.clone());
            if let Some(snapshot) = share.runtime_snapshot.as_mut() {
                if let Some(object) = snapshot.as_object_mut() {
                    object.insert("subdomain".to_string(), json!(subdomain));
                }
            }
        }
        share.enabled = enabled;
        share.status = if enabled { "active" } else { "paused" }.to_string();
        if enabled {
            share.last_error = None;
        }
        crate::domain::sharing::invariants::validate_share_import(&share)?;
        candidate.shares[index] = share;
        candidate.apply_settings_patch_with_usage_edits(
            share_id,
            settings,
            usage_edits,
            usage,
            applied_at_ms,
            operator,
        )?;
        let saved = candidate
            .get(share_id)
            .cloned()
            .ok_or(SharePatchError::NotFound)?;
        *self = candidate;
        Ok(saved)
    }

    pub fn record_subscription_identity_rebind(
        &mut self,
        share_id: &str,
        app: AppKind,
        provider_id: &str,
        previous_fingerprint: String,
        next_fingerprint: String,
    ) -> Result<Share, ShareUpdateError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(ShareUpdateError::NotFound)?;
        if share.status != "paused" {
            return Err(ShareUpdateError::MustBePaused);
        }
        let binding = share
            .bindings
            .iter()
            .find(|binding| binding.app == app && binding.provider_id == provider_id)
            .ok_or(ShareUpdateError::InvalidApp)?;
        share.binding_history.push(ShareBindingHistory {
            app: binding.app,
            previous_provider_id: Some(binding.provider_id.clone()),
            next_provider_id: Some(binding.provider_id.clone()),
            previous_subscription_identity_fingerprint: Some(previous_fingerprint),
            next_subscription_identity_fingerprint: Some(next_fingerprint),
            change_kind: Some("subscription_identity".to_string()),
            changed_at_ms: now_ms(),
        });
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    pub fn update_subdomain(
        &mut self,
        share_id: &str,
        subdomain: String,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let subdomain = normalize_share_subdomain(&subdomain)
            .map_err(|message| SharePatchError::Invalid(message.to_string()))?;
        share.tunnel_subdomain = Some(subdomain.clone());
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("subdomain".to_string(), json!(subdomain));
            }
        }
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    pub fn rebase_full_tunnel_subdomains(
        &mut self,
        from_client: &ClientSubdomain,
        to_client: &ClientSubdomain,
    ) -> usize {
        self.rebase_full_tunnel_subdomains_matching(to_client, |suffix| {
            suffix == from_client.as_str()
        })
    }

    pub fn normalize_full_tunnel_subdomains_to_client(
        &mut self,
        client: &ClientSubdomain,
    ) -> usize {
        self.rebase_full_tunnel_subdomains_matching(client, |suffix| suffix != client.as_str())
    }

    fn rebase_full_tunnel_subdomains_matching(
        &mut self,
        to_client: &ClientSubdomain,
        matches_suffix: impl Fn(&str) -> bool,
    ) -> usize {
        let mut updated = 0;
        for share in &mut self.shares {
            let Some(configured) = share.tunnel_subdomain.as_deref() else {
                continue;
            };
            let Some((slug, suffix)) = configured.split_once("--") else {
                continue;
            };
            if !matches_suffix(suffix) || ShareSlug::parse(slug).is_err() {
                continue;
            }

            let subdomain = format!("{}--{}", slug, to_client.as_str());
            share.tunnel_subdomain = Some(subdomain.clone());
            if let Some(snapshot) = share.runtime_snapshot.as_mut() {
                if let Some(object) = snapshot.as_object_mut() {
                    object.insert("subdomain".to_string(), json!(subdomain));
                }
            }
            mark_share_config_pending(share);
            updated += 1;
        }
        updated
    }

    pub fn bind_all_to_client_owner(
        &mut self,
        owner_email: &str,
    ) -> Result<Vec<Share>, SharePatchError> {
        let owner_email = normalize_verified_email(owner_email)?;
        let mut updated = Vec::new();
        for share in &mut self.shares {
            let previous_grants = share.user_grants.clone();
            let owner_changed = bind_share_to_client_owner(share, &owner_email);
            reconcile_user_grants(share);
            if owner_changed || share.user_grants != previous_grants {
                mark_share_config_pending(share);
                updated.push(share.clone());
            }
        }
        Ok(updated)
    }

    pub fn normalize_all_user_grants(&mut self) -> Vec<Share> {
        let mut updated = Vec::new();
        for share in &mut self.shares {
            let previous_grants = share.user_grants.clone();
            reconcile_user_grants(share);
            if share.user_grants != previous_grants {
                mark_share_config_pending(share);
                updated.push(share.clone());
            }
        }
        updated
    }

    fn sync_owner_email_snapshot(share: &mut Share, owner_email: &str) {
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("ownerEmail".to_string(), json!(owner_email));
            }
        }
    }

    pub fn apply_settings_patch(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
    ) -> Result<Share, SharePatchError> {
        let index = self
            .shares
            .iter()
            .position(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let mut share = self.shares[index].clone();
        let managed_grant = patch.managed_grant.clone();
        let mut managed_grant_fingerprint = None;

        if let Some(operation) = managed_grant.as_ref() {
            validate_managed_grant_operation(operation)?;
            let fingerprint = serde_json::to_string(&(share_id, operation)).map_err(|error| {
                SharePatchError::Invalid(format!("managed grant operation is invalid: {error}"))
            })?;
            if let Some(applied) = self
                .applied_router_control_operations
                .get(&operation.operation_id)
            {
                if applied.fingerprint != fingerprint {
                    return Err(SharePatchError::Invalid(
                        "managed grant operationId was reused with a different payload".to_string(),
                    ));
                }
                return Ok(share);
            }
            managed_grant_fingerprint = Some(fingerprint);
            if operation.expected_config_revision != share.config_revision {
                return Err(SharePatchError::RevisionConflict {
                    expected: operation.expected_config_revision,
                    current: share.config_revision,
                });
            }
        }

        if let Some(owner_email) = patch.owner_email {
            let owner_email = normalize_optional_email(Some(owner_email))
                .ok_or_else(|| SharePatchError::Invalid("ownerEmail is empty".to_string()))?;
            if !share
                .owner_email
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(&owner_email))
            {
                return Err(SharePatchError::Invalid(
                    "share owner is managed by the client owner".to_string(),
                ));
            }
        }

        if let Some(description) = patch.description {
            share.description = description
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
        if let Some(free_access) = patch.free_access {
            share.free_access = free_access;
        }
        if let Some(token_limit) = patch.token_limit {
            share.token_limit = (token_limit >= 0).then_some(token_limit as u64);
        }
        if let Some(parallel_limit) = patch.parallel_limit {
            share.parallel_limit = (parallel_limit >= 0).then_some(parallel_limit as u32);
        }
        if let Some(expires_at) = patch.expires_at {
            share.expires_at = parse_share_expiration(&expires_at)?;
        }
        if let Some(allow_personal_credits) = patch.allow_personal_credits {
            share.allow_personal_credits = allow_personal_credits;
        }
        if let Some(auto_consume_banked_reset) = patch.auto_consume_banked_reset {
            share.auto_consume_banked_reset = auto_consume_banked_reset;
        }
        if let Some(lead_minutes) = patch.banked_reset_expiry_lead_minutes {
            if !(MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES..=MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES)
                .contains(&lead_minutes)
            {
                return Err(SharePatchError::Invalid(format!(
                    "bankedResetExpiryLeadMinutes must be between {} and {}",
                    MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES, MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES
                )));
            }
            share.banked_reset_expiry_lead_minutes = lead_minutes;
        }
        if let Some(enabled) = patch.previous_response_cache_enabled {
            share.previous_response_cache_enabled = enabled;
        }
        let explicit_user_grants = patch.user_grants;
        if let Some(user_grants) = explicit_user_grants.as_ref() {
            share.user_grants = normalize_user_grants(
                user_grants,
                &share.user_grants,
                share.owner_email.as_deref(),
            )?;
        }
        if let Some(auto_start) = patch.auto_start {
            share.auto_start = auto_start;
        }
        if let Some(support) = patch.support {
            let bound = share_bound_apps(&share);
            let enabled = enabled_apps_from_support(&support, &bound)?;
            share.enabled_apps = if enabled == bound {
                None
            } else {
                Some(enabled)
            };
        }

        crate::domain::sharing::invariants::validate_share_import(&share)?;

        reconcile_user_grants(&mut share);
        if let Some(operation) = managed_grant.as_ref() {
            apply_managed_grant_operation(&mut share, operation)?;
        }
        let managed_grant_result = managed_grant
            .as_ref()
            .and_then(|operation| {
                (operation.action == ShareManagedGrantAction::Upsert).then(|| {
                    share
                        .user_grants
                        .values()
                        .find(|grant| {
                            grant.active
                                && grant.entitlement_id.as_deref()
                                    == Some(operation.entitlement_id.as_str())
                        })
                        .map(|grant| {
                            (
                                i64::try_from(grant.updated_at_ms).unwrap_or(i64::MAX),
                                grant.policy.clone(),
                            )
                        })
                })
            })
            .flatten();
        crate::domain::sharing::invariants::validate_share_import(&share)?;
        mark_share_config_pending(&mut share);
        self.shares[index] = share.clone();
        if let Some(operation) = managed_grant {
            // Router serializes managed operations per Share. Seeing a newer
            // shareSequence proves every older operation for this Share was
            // resolved, so retain only the replay window that can still be
            // requested. Legacy records without share metadata stay intact.
            self.applied_router_control_operations.retain(|_, applied| {
                applied.share_id.is_empty()
                    || applied.share_id != share_id
                    || applied.share_sequence >= operation.share_sequence
            });
            self.applied_router_control_operations.insert(
                operation.operation_id,
                AppliedRouterControlOperation {
                    applied_at_ms: now_ms(),
                    fingerprint: managed_grant_fingerprint
                        .expect("managed operation fingerprint is populated after validation"),
                    share_id: share_id.to_string(),
                    share_sequence: operation.share_sequence,
                    result_applied_at_ms: managed_grant_result
                        .as_ref()
                        .map(|(applied_at_ms, _)| *applied_at_ms),
                    result_effective_policy: managed_grant_result
                        .map(|(_, effective_policy)| effective_policy),
                },
            );
        }

        Ok(share)
    }

    pub(crate) fn router_control_upsert_result(
        &self,
        operation_id: &str,
    ) -> Option<(i64, ShareUserPolicy)> {
        let operation = self.applied_router_control_operations.get(operation_id)?;
        Some((
            operation.result_applied_at_ms?,
            operation.result_effective_policy.clone()?,
        ))
    }

    pub(crate) fn forget_router_control_operation(&mut self, operation_id: &str) -> bool {
        self.applied_router_control_operations
            .remove(operation_id)
            .is_some()
    }

    pub fn apply_settings_patch_with_usage(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
        usage: &UsageStore,
        applied_at_ms: i64,
    ) -> Result<Share, SharePatchError> {
        self.apply_settings_patch_with_usage_edits(
            share_id,
            patch,
            None,
            usage,
            applied_at_ms,
            None,
        )
    }

    pub fn apply_settings_patch_with_usage_edits(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
        usage_edits: Option<&BTreeMap<String, ShareUserUsageEdit>>,
        usage: &UsageStore,
        applied_at_ms: i64,
        operator: Option<&str>,
    ) -> Result<Share, SharePatchError> {
        let original = self
            .get(share_id)
            .cloned()
            .ok_or(SharePatchError::NotFound)?;
        let mut candidate = self.clone();
        let merged_usage_edits = match (usage_edits, patch.user_usage_edits.as_ref()) {
            (Some(explicit), _) => Some(explicit.clone()),
            (None, Some(from_patch)) => Some(from_patch.clone()),
            (None, None) => None,
        };
        candidate.apply_settings_patch(share_id, patch)?;
        if let Some(edits) = merged_usage_edits.as_ref() {
            candidate.apply_user_usage_edits(
                share_id,
                &original,
                edits,
                usage,
                applied_at_ms,
                operator,
            )?;
        }
        candidate.rebuild_user_usage_from_history(share_id, usage, applied_at_ms)?;
        let share = candidate
            .get(share_id)
            .cloned()
            .ok_or(SharePatchError::NotFound)?;
        *self = candidate;
        Ok(share)
    }

    pub(crate) fn apply_user_usage_edits(
        &mut self,
        share_id: &str,
        original: &Share,
        edits: &BTreeMap<String, ShareUserUsageEdit>,
        usage: &UsageStore,
        applied_at_ms: i64,
        operator: Option<&str>,
    ) -> Result<(), SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;

        for (key, edit) in edits {
            let email = normalize_verified_email(key)?;
            let previous = original.user_grants.get(&email);
            if let Some(expected) = edit.expected_grant_revision {
                let current = previous.map(|grant| grant.revision).unwrap_or_default();
                if current != expected {
                    return Err(SharePatchError::GrantRevisionConflict {
                        email,
                        expected,
                        current,
                    });
                }
            }
            let grant = share
                .user_grants
                .get_mut(&email)
                .ok_or_else(|| SharePatchError::Invalid(format!("user grant {email} not found")))?;
            if !grant.active {
                return Err(SharePatchError::Invalid(format!(
                    "user grant {email} is inactive"
                )));
            }
            if grant.manager == ShareGrantManager::RouterShareMarket {
                return Err(SharePatchError::ManagedGrantReadOnly(email));
            }
            if let Some(period) = edit.period {
                if period != grant.policy.token_period {
                    return Err(SharePatchError::Invalid(format!(
                        "usage edit period does not match grant policy for {email}"
                    )));
                }
            }
            if let Some(anchor) = edit.anchor_at_ms {
                if grant.policy.token_period_anchor_at_ms != Some(anchor) {
                    return Err(SharePatchError::Invalid(format!(
                        "usage edit anchor does not match grant policy for {email}"
                    )));
                }
            }

            match edit.action {
                ShareUserUsageEditAction::Clear => {
                    grant.usage_rebase = None;
                    grant.updated_at_ms = applied_at_ms.max(0) as u128;
                    grant.revision = grant.revision.saturating_add(1).max(1);
                }
                ShareUserUsageEditAction::Set => {
                    let target_tokens = edit.target_tokens.ok_or_else(|| {
                        SharePatchError::Invalid(format!(
                            "usage edit targetTokens is required for {email}"
                        ))
                    })?;
                    let snapshot =
                        observe_user_quota(usage, share_id, &email, grant, applied_at_ms)
                            .map_err(SharePatchError::Invalid)?;
                    if target_tokens < snapshot.observed_tokens {
                        return Err(SharePatchError::UsageTargetBelowObserved {
                            email,
                            target: target_tokens,
                            observed: snapshot.observed_tokens,
                        });
                    }
                    grant.usage_rebase = Some(
                        crate::domain::sharing::router_contract::ShareUserUsageRebase {
                            period: grant.policy.token_period,
                            anchor_at_ms: grant.policy.token_period_anchor_at_ms,
                            window_starts_at_ms: snapshot.window.starts_at_ms,
                            window_ends_at_ms: snapshot.window.ends_at_ms,
                            target_tokens,
                            observed_tokens_at_rebase: snapshot.observed_tokens,
                            observed_requests_at_rebase: snapshot.observed_requests,
                            usage_watermark: usage.journal_watermark(),
                            applied_at_ms,
                            applied_by: operator.map(str::to_string),
                            source: edit.source,
                        },
                    );
                    grant.updated_at_ms = applied_at_ms.max(0) as u128;
                    grant.revision = grant.revision.saturating_add(1).max(1);
                }
            }
        }
        Ok(())
    }

    /// Applies an explicit Share-total consumed-token correction.
    ///
    /// Unlike the per-user quota, the Share total counter is never rebuilt
    /// from the Usage history, so the operator value is written directly and
    /// stays authoritative until the next invocation adds to it.  Dropping
    /// below the total limit clears the `exhausted` state but deliberately
    /// leaves the Share paused: re-enabling traffic stays an explicit,
    /// separate operator decision, exactly as it is after `reset_usage`.
    pub(crate) fn apply_share_total_usage_edit(
        &mut self,
        share_id: &str,
        edit: &ShareTotalUsageEdit,
        applied_at_ms: i64,
    ) -> Result<(), SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let tokens_used = match edit.action {
            ShareUserUsageEditAction::Clear => 0,
            ShareUserUsageEditAction::Set => edit.tokens_used.ok_or_else(|| {
                SharePatchError::Invalid(
                    "share usage edit tokensUsed is required for action set".to_string(),
                )
            })?,
        };
        if share.tokens_used == tokens_used {
            return Ok(());
        }
        share.tokens_used = tokens_used;
        let exhausted = share
            .token_limit
            .is_some_and(|token_limit| share.tokens_used >= token_limit);
        if exhausted {
            share.status = "exhausted".to_string();
            share.enabled = false;
        } else if share.status == "exhausted" {
            share.status = "paused".to_string();
            share.enabled = false;
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("tokensUsed".to_string(), json!(share.tokens_used));
                object.insert("shareStatus".to_string(), json!(share.status));
            }
        }
        let _ = applied_at_ms;
        mark_share_config_pending(share);
        Ok(())
    }

    /// Rebuilds the active policy bucket exactly from the immutable Usage
    /// history plus any durable operator baseline.  Settings saves use this
    /// exact path so policy/anchor changes and explicit baseline clears can
    /// legitimately reduce a previously derived snapshot.
    pub fn rebuild_user_usage_from_history(
        &mut self,
        share_id: &str,
        usage: &UsageStore,
        now_ms: i64,
    ) -> Result<(), SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        for grant in share.user_grants.values_mut() {
            if !grant.active {
                if grant.policy.token_period.requires_anchor() {
                    grant
                        .usage
                        .rebuild_anchored(&grant.policy, now_ms, 0, 0)
                        .map_err(SharePatchError::Invalid)?;
                }
                grant.usage_quota = None;
                continue;
            }
            let normalized_email = grant.email.trim().to_ascii_lowercase();
            let snapshot = observe_user_quota(usage, share_id, &normalized_email, grant, now_ms)
                .map_err(SharePatchError::Invalid)?;
            let rebase_matches = grant
                .usage_rebase
                .as_ref()
                .is_none_or(|rebase| rebase_matches_window(rebase, &snapshot.window));
            if !rebase_matches {
                // A policy/window change invalidates the old baseline.  Do
                // not let it unexpectedly apply to a later period.
                grant.usage_rebase = None;
            }
            // Compute this after invalidating a stale baseline.  Otherwise a
            // policy/anchor edit could briefly publish the old baseline's
            // effective value into the new window until the next rebuild.
            let effective_tokens = if rebase_matches {
                effective_user_tokens(grant, &snapshot)
            } else {
                snapshot.observed_tokens
            };
            grant
                .usage
                .rebuild_current_policy_bucket(
                    &grant.policy,
                    now_ms,
                    effective_tokens,
                    snapshot.observed_requests,
                )
                .map_err(SharePatchError::Invalid)?;
            grant.usage_quota = Some(quota_view(
                grant,
                &snapshot,
                effective_tokens,
                rebase_matches,
            ));
        }
        Ok(())
    }

    /// Refreshes the fixed-period bucket after an invocation was already
    /// added to the in-memory Share counters. Usage persistence may lag that
    /// direct update by a few instructions, so this path never moves the
    /// effective bucket backwards. Non-anchored periods need no refresh: the
    /// direct record operation already advances their current bucket.
    pub fn rebuild_user_anchored_usage(
        &mut self,
        share_id: &str,
        usage: &UsageStore,
        now_ms: i64,
    ) -> Result<(), SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|share| share.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        for grant in share.user_grants.values_mut() {
            if !grant.active || !grant.policy.token_period.requires_anchor() {
                grant.usage.anchored = None;
                continue;
            }
            let normalized_email = grant.email.trim().to_ascii_lowercase();
            let snapshot = observe_user_quota(usage, share_id, &normalized_email, grant, now_ms)
                .map_err(SharePatchError::Invalid)?;
            let rebase_matches = grant
                .usage_rebase
                .as_ref()
                .is_none_or(|rebase| rebase_matches_window(rebase, &snapshot.window));
            if !rebase_matches {
                grant.usage_rebase = None;
            }
            let history_tokens = if rebase_matches {
                effective_user_tokens(grant, &snapshot)
            } else {
                snapshot.observed_tokens
            };
            let direct_tokens = grant.usage.tokens_for_policy(&grant.policy, now_ms);
            let direct_requests = grant
                .usage
                .anchored
                .as_ref()
                .map(|bucket| bucket.requests_count)
                .unwrap_or_default();
            let effective_tokens = history_tokens.max(direct_tokens);
            let observed_requests = snapshot.observed_requests.max(direct_requests);
            grant
                .usage
                .rebuild_current_policy_bucket(
                    &grant.policy,
                    now_ms,
                    effective_tokens,
                    observed_requests,
                )
                .map_err(SharePatchError::Invalid)?;
            let mut view = quota_view(grant, &snapshot, effective_tokens, rebase_matches);
            view.observed_requests_count = observed_requests;
            grant.usage_quota = Some(view);
        }
        Ok(())
    }

    pub fn import_shares(&mut self, shares: Vec<Share>) -> Result<usize, SharePatchError> {
        let mut candidate = self.clone();
        let mut imported = 0;
        for mut share in shares {
            crate::domain::sharing::invariants::validate_share_import(&share)?;
            if let Some(existing) = candidate.shares.iter().find(|item| item.id == share.id) {
                if existing.app != share.app
                    || existing.provider_id != share.provider_id
                    || existing.provider_type != share.provider_type
                    || existing.bindings != share.bindings
                {
                    return Err(SharePatchError::BindingImmutable);
                }
            }
            if candidate.cancel_pending_router_delete(&share.id) {
                mark_share_config_pending(&mut share);
            }
            if let Some(existing) = candidate.shares.iter_mut().find(|item| item.id == share.id) {
                *existing = share;
            } else {
                candidate.shares.push(share);
            }
            imported += 1;
        }
        for (index, share) in candidate.shares.iter().enumerate() {
            if share.status == "deleted" {
                continue;
            }
            if candidate
                .shares
                .iter()
                .enumerate()
                .any(|(other_index, other)| {
                    other_index != index
                        && other.status != "deleted"
                        && other.bindings.iter().any(|other_binding| {
                            share.bindings.iter().any(|binding| {
                                other_binding.app == binding.app
                                    && other_binding.provider_id == binding.provider_id
                            })
                        })
                })
            {
                return Err(SharePatchError::Invalid(format!(
                    "provider already has share {}",
                    share.provider_id
                )));
            }
        }
        *self = candidate;
        Ok(imported)
    }

    pub fn replace_configured_share(&mut self, candidate: Share) -> Result<Share, SharePatchError> {
        crate::domain::sharing::invariants::validate_share_import(&candidate)?;
        let index = self
            .shares
            .iter()
            .position(|share| share.id == candidate.id)
            .ok_or(SharePatchError::NotFound)?;
        let current = &self.shares[index];
        if current.app != candidate.app
            || current.provider_id != candidate.provider_id
            || current.provider_type != candidate.provider_type
            || current.bindings != candidate.bindings
        {
            return Err(SharePatchError::BindingImmutable);
        }
        if self.shares.iter().enumerate().any(|(other_index, share)| {
            other_index != index
                && share.status != "deleted"
                && share.bindings.iter().any(|other_binding| {
                    candidate.bindings.iter().any(|binding| {
                        other_binding.app == binding.app
                            && other_binding.provider_id == binding.provider_id
                    })
                })
        }) {
            return Err(SharePatchError::Invalid(
                "provider already has an active share".to_string(),
            ));
        }
        if let Some(subdomain) = candidate.tunnel_subdomain.as_deref() {
            if self.shares.iter().enumerate().any(|(other_index, share)| {
                other_index != index
                    && share.status != "deleted"
                    && share.tunnel_subdomain.as_deref() == Some(subdomain)
            }) {
                return Err(SharePatchError::Invalid(
                    "share subdomain is already in use".to_string(),
                ));
            }
        }
        self.cancel_pending_router_delete(&candidate.id);
        self.shares[index] = candidate.clone();
        Ok(candidate)
    }

    pub fn set_share_tunnel_status(
        &mut self,
        share_id: &str,
        status: &str,
        error: Option<String>,
    ) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        let enabled = status == "active";
        let changed =
            share.status != status || share.enabled != enabled || share.last_error != error;
        share.status = status.to_string();
        share.enabled = enabled;
        share.last_error = error;
        if changed {
            mark_share_config_pending(share);
        }
        Some(share.clone())
    }

    pub fn restore_auto_start(&mut self) -> Vec<Share> {
        for share in self.shares.iter_mut().filter(|item| item.auto_start) {
            let changed = share.status != "active" || !share.enabled || share.last_error.is_some();
            share.status = "active".to_string();
            share.enabled = true;
            share.last_error = None;
            if changed {
                mark_share_config_pending(share);
            }
        }
        self.shares.clone()
    }

    pub fn refresh_runtime_snapshots(
        &mut self,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<Share> {
        for share in &mut self.shares {
            share.runtime_snapshot = Some(runtime_snapshot_for_share(
                share, providers, accounts, usage,
            ));
        }
        self.shares.clone()
    }

    pub fn refresh_runtime_snapshots_for_providers(
        &mut self,
        provider_keys: &BTreeSet<(AppKind, String)>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in &mut self.shares {
            let uses_provider = if share.bindings.is_empty() {
                provider_keys.contains(&(share.app, share.provider_id.clone()))
            } else {
                share.bindings.iter().any(|binding| {
                    provider_keys.contains(&(binding.app, binding.provider_id.clone()))
                })
            };
            if !uses_provider {
                continue;
            }

            share.runtime_snapshot = Some(runtime_snapshot_for_share(
                share, providers, accounts, usage,
            ));
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn reconcile_runtime_snapshots_for_providers(
        &mut self,
        provider_keys: &BTreeSet<(AppKind, String)>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in &mut self.shares {
            let uses_provider = if share.bindings.is_empty() {
                provider_keys.contains(&(share.app, share.provider_id.clone()))
            } else {
                share.bindings.iter().any(|binding| {
                    provider_keys.contains(&(binding.app, binding.provider_id.clone()))
                })
            };
            if !uses_provider {
                continue;
            }

            let fingerprint = runtime_metadata_fingerprint_for_share(share, providers);
            let current_fingerprint = share
                .runtime_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.get("runtimeMetadataFingerprint"))
                .and_then(Value::as_str);
            if current_fingerprint == Some(fingerprint.as_str()) {
                continue;
            }

            share.runtime_snapshot = Some(runtime_snapshot_for_share(
                share, providers, accounts, usage,
            ));
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn refresh_subscription_expiry_snapshots_for_providers(
        &mut self,
        provider_keys: &BTreeSet<(AppKind, String)>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in &mut self.shares {
            let uses_provider = if share.bindings.is_empty() {
                provider_keys.contains(&(share.app, share.provider_id.clone()))
            } else {
                share.bindings.iter().any(|binding| {
                    provider_keys.contains(&(binding.app, binding.provider_id.clone()))
                })
            };
            if !uses_provider {
                continue;
            }

            let next_snapshot = runtime_snapshot_for_share(share, providers, accounts, usage);
            if subscription_expiry_fingerprint(share.runtime_snapshot.as_ref())
                == subscription_expiry_fingerprint(Some(&next_snapshot))
            {
                continue;
            }
            share.runtime_snapshot = Some(next_snapshot);
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn mark_router_sync(
        &mut self,
        share_id: &str,
        revision: u64,
        router_url: Option<String>,
        result: Result<u128, String>,
    ) {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return;
        };
        match result {
            Ok(now) => {
                share.router_synced_revision = share.router_synced_revision.max(revision);
                share.router_last_synced_at_ms = Some(now);
                if share.router_synced_revision >= share.config_revision {
                    share.router_last_sync_error = None;
                }
                share.router_url = router_url;
            }
            Err(error) => {
                if revision >= share.config_revision && share.router_synced_revision < revision {
                    share.router_last_sync_error = Some(error);
                }
            }
        }
    }

    pub fn prepare_descriptor_projection(
        &mut self,
        share_id: &str,
        fingerprint: String,
    ) -> Option<(u64, String)> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        let changed = share.descriptor_fingerprint.as_deref() != Some(fingerprint.as_str());
        // Grant/quota edits bump config_revision even when the static
        // fingerprint is unchanged (`usageQuota` is stripped).  If this
        // generation was already ACKed, mint a new one so Router's upsert
        // actually writes the payload.  Failed or in-flight pushes already
        // sit ahead of the ACK and must retry the same generation.
        let revision_pending = share.router_synced_revision < share.config_revision
            && share.descriptor_generation == share.router_synced_descriptor_generation;
        if changed || share.descriptor_generation == 0 || revision_pending {
            share.descriptor_generation = share
                .descriptor_generation
                .max(share.router_synced_descriptor_generation)
                .saturating_add(1)
                .max(1);
            share.descriptor_fingerprint = Some(fingerprint);
            share.router_last_sync_error = None;
        }
        Some((
            share.descriptor_generation,
            share.descriptor_fingerprint.clone().unwrap_or_default(),
        ))
    }

    pub fn mark_router_descriptor_sync(
        &mut self,
        share_id: &str,
        generation: u64,
        fingerprint: &str,
        config_revision: u64,
        router_url: Option<String>,
        result: Result<u128, String>,
    ) -> bool {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return false;
        };
        let is_current = share.descriptor_generation == generation
            && share.descriptor_fingerprint.as_deref() == Some(fingerprint);
        match result {
            Ok(now) if is_current => {
                share.router_synced_descriptor_generation = generation;
                share.router_synced_descriptor_fingerprint = Some(fingerprint.to_string());
                share.router_synced_revision = share.router_synced_revision.max(config_revision);
                share.router_last_synced_at_ms = Some(now);
                share.router_last_sync_error = None;
                share.router_url = router_url;
                true
            }
            Ok(_) => false,
            Err(error) if is_current => {
                share.router_last_sync_error = Some(error);
                false
            }
            Err(_) => false,
        }
    }

    pub fn descriptor_projection_pending(&self, share: &Share) -> bool {
        share.descriptor_generation == 0
            || share.descriptor_fingerprint.is_none()
            || share.router_synced_descriptor_generation != share.descriptor_generation
            || share.router_synced_descriptor_fingerprint != share.descriptor_fingerprint
            // Grant/quota edits always bump config_revision.  Some of those
            // fields are stripped from the static fingerprint (usageQuota) or
            // can otherwise leave the fingerprint unchanged, so revision
            // lag must still force a Router upsert.
            || share.router_synced_revision < share.config_revision
    }

    pub fn record_router_descriptor_sync_mode(
        &mut self,
        mode: RouterDescriptorSyncMode,
        diagnostic: Option<String>,
    ) {
        if self.router_descriptor_sync_mode != RouterDescriptorSyncMode::Strict
            || mode == RouterDescriptorSyncMode::Strict
        {
            self.router_descriptor_sync_mode = mode;
        }
        self.router_descriptor_sync_diagnostic = diagnostic;
    }
}

fn bind_share_to_client_owner(share: &mut Share, owner_email: &str) -> bool {
    if share
        .owner_email
        .as_deref()
        .is_some_and(|current| current == owner_email)
    {
        return false;
    }
    let previous_owner = share
        .owner_email
        .as_deref()
        .and_then(|email| normalize_verified_email(email).ok())
        .filter(|email| !email.eq_ignore_ascii_case(owner_email));
    if let Some(previous_owner) = previous_owner {
        let mut grant = share
            .user_grants
            .remove(&previous_owner)
            .unwrap_or_else(|| new_user_grant(share, previous_owner.clone(), "shareto"));
        grant.email = previous_owner.clone();
        grant.role = "shareto".to_string();
        grant.active = true;
        grant.updated_at_ms = now_ms();
        grant.revoked_at_ms = None;
        grant.revision = grant.revision.saturating_add(1).max(1);
        grant.manager = ShareGrantManager::Manual;
        grant.entitlement_id = None;
        share.user_grants.insert(previous_owner, grant);
    }
    share.owner_email = Some(owner_email.to_string());
    ShareStore::sync_owner_email_snapshot(share, owner_email);
    true
}

fn runtime_snapshot_for_share(
    share: &Share,
    providers: &ProviderStore,
    accounts: Option<&AccountStore>,
    usage: &UsageStore,
) -> Value {
    let descriptor =
        descriptor_for_share_with_accounts_and_usage(share, providers, accounts, Some(usage));
    let provider = providers
        .providers
        .iter()
        .find(|item| item.app == share.app && item.provider.id == share.provider_id);
    let health = provider.map(|item| {
        let runtime_plan = providers.runtime_plan(item.app, &item.provider.id);
        crate::domain::health::provider_health_for_plan(item, usage, runtime_plan.as_deref())
    });
    let last_request = usage
        .logs
        .iter()
        .filter(|log| {
            !log.is_health_check && log.provider_id == share.provider_id && log.app == share.app
        })
        .max_by_key(|log| log.created_at_ms);

    json!({
        "shareId": share.id,
        "app": share.app,
        "providerId": share.provider_id,
        "providerType": share.provider_type,
        "providerName": provider.map(|item| item.provider.name.clone()),
        "accountEmail": descriptor.upstream_provider.as_ref().and_then(|item| item.account_email.clone()).or_else(|| share.account_email.clone()),
        "subscriptionLevel": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_level.clone()).or_else(|| share.subscription_level.clone()),
        "subscriptionExpiresAt": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_expires_at.clone()),
        "subscriptionRemainingMs": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_remaining_ms),
        "quotaPercent": descriptor.upstream_provider.as_ref().and_then(|item| item.quota_percent).or(share.quota_percent),
        "tokensUsed": share.tokens_used,
        "requestsCount": share.requests_count,
        "health": health,
        "lastRequest": last_request,
        "upstreamProvider": descriptor.upstream_provider,
        "appRuntimes": descriptor.app_runtimes,
        "appProviders": descriptor.app_providers,
        "appAvailability": descriptor.app_availability,
        "modelHealth": descriptor.model_health,
        "runtimeMetadataFingerprint": runtime_metadata_fingerprint_for_share(share, providers),
        "updatedAtMs": now_ms(),
    })
}

fn runtime_metadata_fingerprint_for_share(share: &Share, providers: &ProviderStore) -> String {
    let mut bindings = if share.bindings.is_empty() {
        vec![(share.app, share.provider_id.clone())]
    } else {
        share
            .bindings
            .iter()
            .map(|binding| (binding.app, binding.provider_id.clone()))
            .collect::<Vec<_>>()
    };
    bindings.sort();
    let signatures = bindings
        .into_iter()
        .map(|(app, provider_id)| {
            let plan = providers.runtime_plan(app, &provider_id);
            let provider_revision = plan
                .as_ref()
                .map(|plan| plan.provider_revision)
                .or_else(|| {
                    providers
                        .providers
                        .iter()
                        .find(|provider| {
                            provider.app == app && provider.provider.id == provider_id
                        })
                        .map(|provider| provider.resource.revision)
                });
            json!({
                "app": app,
                "providerId": provider_id,
                "providerRevision": provider_revision,
                "runtimeFingerprint": plan.as_ref().map(|plan| plan.runtime_fingerprint.as_str()).unwrap_or("missing-runtime-plan"),
                "healthFingerprint": plan.as_ref().map(|plan| plan.health_fingerprint()).unwrap_or_else(|| "missing-runtime-plan".to_string()),
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&signatures)
        .expect("Share runtime metadata fingerprint input is serializable");
    hex::encode(Sha256::digest(bytes))
}

fn subscription_expiry_fingerprint(snapshot: Option<&Value>) -> Vec<String> {
    fn collect(value: &Value, path: &str, entries: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let child_path = format!("{path}/{key}");
                    if matches!(
                        key.as_str(),
                        "subscriptionExpiresAt" | "subscriptionPeriodEnd"
                    ) {
                        entries.push(format!("{child_path}={value}"));
                    } else {
                        collect(value, &child_path, entries);
                    }
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    collect(value, &format!("{path}/{index}"), entries);
                }
            }
            _ => {}
        }
    }

    let mut entries = Vec::new();
    if let Some(snapshot) = snapshot {
        collect(snapshot, "", &mut entries);
    }
    entries.sort();
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareUpdateError {
    NotFound,
    MustBePaused,
    InvalidApp,
    ProviderAlreadyShared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInvocation {
    pub share_id: String,
    pub share_name: String,
    pub parallel_limit: Option<u32>,
    pub user_email: Option<String>,
    pub user_parallel_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInvocationRejection {
    pub reason: ShareRejectReason,
    pub message: String,
    pub status_changed: bool,
    pub concurrency: Option<ShareConcurrencyLimit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareConcurrencyLimit {
    pub current: u32,
    pub limit: u32,
}

impl ShareInvocationRejection {
    pub fn formatted_message(&self) -> String {
        format!("{} [{}]", self.message, self.reason.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareRejectReason {
    NotFound,
    UnsupportedApp,
    UserIdentityRequired,
    Unauthorized,
    AppNotAllowed,
    Inactive,
    Expired,
    Exhausted,
    ParallelLimit,
    UserExpired,
    UserExhausted,
    UserParallelLimit,
}

impl ShareRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::UnsupportedApp => "UnsupportedApp",
            Self::UserIdentityRequired => "UserIdentityRequired",
            Self::Unauthorized => "Unauthorized",
            Self::AppNotAllowed => "AppNotAllowed",
            Self::Inactive => "Inactive",
            Self::Expired => "Expired",
            Self::Exhausted => "Exhausted",
            Self::ParallelLimit => "ParallelLimit",
            Self::UserExpired => "UserExpired",
            Self::UserExhausted => "UserExhausted",
            Self::UserParallelLimit => "UserParallelLimit",
        }
    }
}

impl std::fmt::Display for ShareUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("share not found"),
            Self::MustBePaused => {
                formatter.write_str("share must be paused before updating binding")
            }
            Self::InvalidApp => formatter.write_str("share binding app/provider is invalid"),
            Self::ProviderAlreadyShared => {
                formatter.write_str("provider already has an active share")
            }
        }
    }
}

impl std::error::Error for ShareUpdateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharePatchError {
    NotFound,
    BindingImmutable,
    PolicyDivergent(String),
    RevisionConflict {
        expected: u64,
        current: u64,
    },
    GrantRevisionConflict {
        email: String,
        expected: u64,
        current: u64,
    },
    UsageTargetBelowObserved {
        email: String,
        target: u64,
        observed: u64,
    },
    ManagedGrantReadOnly(String),
    Invalid(String),
}

impl SharePatchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "cc_switch_share_not_found",
            Self::BindingImmutable => "cc_switch_share_binding_immutable",
            Self::PolicyDivergent(_) => "cc_switch_share_policy_divergent",
            Self::RevisionConflict { .. } => "cc_switch_share_revision_conflict",
            Self::GrantRevisionConflict { .. } => "cc_switch_share_user_grant_revision_conflict",
            Self::UsageTargetBelowObserved { .. } => "cc_switch_share_usage_target_below_observed",
            Self::ManagedGrantReadOnly(_) => "cc_switch_share_market_grant_read_only",
            Self::Invalid(_) => "cc_switch_share_invalid_patch",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::RevisionConflict { .. })
    }
}

impl std::fmt::Display for SharePatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("share not found"),
            Self::BindingImmutable => formatter.write_str(
                "share binding is immutable in ordinary upsert/import; pause the Share and use the binding endpoint",
            ),
            Self::PolicyDivergent(message) => formatter.write_str(message),
            Self::RevisionConflict { expected, current } => write!(
                formatter,
                "managed grant expected config revision {expected}, current revision is {current}"
            ),
            Self::GrantRevisionConflict {
                email,
                expected,
                current,
            } => write!(
                formatter,
                "user grant {email} expected revision {expected}, current revision is {current}"
            ),
            Self::UsageTargetBelowObserved {
                email,
                target,
                observed,
            } => write!(
                formatter,
                "usage target for {email} ({target}) cannot be below observed usage ({observed})"
            ),
            Self::ManagedGrantReadOnly(email) => {
                write!(formatter, "Share Market managed user {email} is read-only")
            }
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SharePatchError {}

pub fn shares_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(SHARES_FILE_NAME)
}

fn generate_share_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("share-{suffix}")
}

fn generate_share_delete_operation_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_router_api_base(router_api_base: &str) -> String {
    router_api_base.trim().trim_end_matches('/').to_string()
}

fn generate_unique_share_slug(shares: &[Share]) -> String {
    for attempt in 0..crate::domain::subdomain_suggest::SUGGEST_MAX_ATTEMPTS {
        let candidate =
            crate::domain::subdomain_suggest::generate_candidate(&mut rand::thread_rng(), attempt);
        if !shares.iter().any(|share| {
            share.status != "deleted"
                && share.tunnel_subdomain.as_deref() == Some(candidate.as_str())
        }) {
            return candidate;
        }
    }
    crate::domain::subdomain_suggest::generate_share_slug(&mut rand::thread_rng())
}

fn default_share_status() -> String {
    "active".to_string()
}

fn validate_share_provider_bindings(
    share: &Share,
    providers: &ProviderStore,
) -> Result<(), String> {
    if !(1..=3).contains(&share.bindings.len()) {
        return Err(format!(
            "Share must have between one and three bindings (found {})",
            share.bindings.len()
        ));
    }
    let mut apps = BTreeSet::new();
    for binding in &share.bindings {
        if !apps.insert(binding.app) {
            return Err(format!(
                "Share repeats the {} binding",
                binding.app.as_str()
            ));
        }
        let Some(stored) = providers
            .providers
            .iter()
            .find(|stored| stored.app == binding.app && stored.provider.id == binding.provider_id)
        else {
            return Err(format!(
                "Share binding {}/{} references a missing Provider",
                binding.app.as_str(),
                binding.provider_id
            ));
        };
        if stored.provider_type != binding.provider_type {
            return Err(format!(
                "Share binding {}/{} has providerType {}, expected {}",
                binding.app.as_str(),
                binding.provider_id,
                binding.provider_type.as_str(),
                stored.provider_type.as_str()
            ));
        }
    }
    if !share.bindings.iter().any(|binding| {
        binding.app == share.app
            && binding.provider_id == share.provider_id
            && binding.provider_type == share.provider_type
    }) {
        return Err("Share primary Provider fields do not match a binding".to_string());
    }
    Ok(())
}

fn disable_share_for_integrity(
    share: &mut Share,
    code: &str,
    message: &str,
) -> ShareIntegrityOutcome {
    let unchanged = !share.enabled
        && share.status == "paused"
        && share.capacity_pool_id.is_empty()
        && share.last_error.as_deref() == Some(message)
        && share
            .integrity_error
            .as_ref()
            .is_some_and(|error| error.code == code && error.message == message);
    let error = if unchanged {
        share
            .integrity_error
            .clone()
            .expect("unchanged integrity failure has a structured error")
    } else {
        ShareIntegrityError {
            code: code.to_string(),
            message: message.to_string(),
            checked_at_ms: now_ms(),
        }
    };
    if !unchanged {
        share.enabled = false;
        share.status = "paused".to_string();
        share.capacity_pool_id.clear();
        share.last_error = Some(message.to_string());
        share.integrity_error = Some(error.clone());
        mark_share_config_pending(share);
    }
    ShareIntegrityOutcome {
        share_id: share.id.clone(),
        status: ShareIntegrityStatus::Disabled,
        changed: !unchanged,
        error: Some(error),
    }
}

fn mark_share_config_pending(share: &mut Share) {
    share.config_revision = share.config_revision.saturating_add(1).max(1);
    share.router_last_sync_error = None;
}

fn share_expired(expires_at: i64, now_ms: i64) -> bool {
    let expires_at_ms = if expires_at > 0 && expires_at < 10_000_000_000 {
        expires_at.saturating_mul(1000)
    } else {
        expires_at
    };
    now_ms > expires_at_ms
}

fn parse_share_expiration(value: &str) -> Result<Option<i64>, SharePatchError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return Ok(Some(timestamp));
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| Some(value.timestamp_millis()))
        .map_err(|_| SharePatchError::Invalid("expiresAt must be a timestamp or RFC3339".into()))
}

fn normalize_optional_email(value: Option<String>) -> Option<String> {
    value
        .map(|email| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty())
}

fn normalize_verified_email(email: &str) -> Result<String, SharePatchError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty()
        || email.len() > 254
        || email.chars().any(char::is_whitespace)
        || email.matches('@').count() != 1
    {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    }
    Ok(email)
}

fn default_user_policy(share: &Share) -> ShareUserPolicy {
    ShareUserPolicy {
        parallel_limit: share.parallel_limit,
        token_limit: share.token_limit,
        token_period: ShareTokenPeriod::Lifetime,
        token_period_anchor_at_ms: None,
        expires_at: share.expires_at,
        allowed_apps: Vec::new(),
    }
}

fn new_user_grant(share: &Share, email: String, role: &str) -> ShareUserGrant {
    let now = now_ms();
    ShareUserGrant {
        email,
        role: role.to_string(),
        active: true,
        policy: default_user_policy(share),
        usage: Default::default(),
        usage_rebase: None,
        usage_quota: None,
        created_at_ms: now,
        updated_at_ms: now,
        revoked_at_ms: None,
        revision: 1,
        manager: if role == "owner" {
            ShareGrantManager::Owner
        } else {
            ShareGrantManager::Manual
        },
        entitlement_id: None,
    }
}

fn validate_managed_grant_operation(
    operation: &ShareManagedGrantOperation,
) -> Result<(), SharePatchError> {
    if operation.operation_id.trim().is_empty() || operation.operation_id.len() > 128 {
        return Err(SharePatchError::Invalid(
            "managed grant operationId must contain 1-128 characters".to_string(),
        ));
    }
    if operation.entitlement_id.trim().is_empty() || operation.entitlement_id.len() > 128 {
        return Err(SharePatchError::Invalid(
            "managed grant entitlementId must contain 1-128 characters".to_string(),
        ));
    }
    if operation.share_sequence < 1 {
        return Err(SharePatchError::Invalid(
            "managed grant shareSequence must be positive".to_string(),
        ));
    }
    match operation.action {
        ShareManagedGrantAction::Upsert => {
            let policy = operation.policy.as_ref().ok_or_else(|| {
                SharePatchError::Invalid("managed grant upsert requires policy".to_string())
            })?;
            if let Some(duration_seconds) = operation.duration_seconds {
                if !(1..=MAX_MANAGED_GRANT_DURATION_SECONDS).contains(&duration_seconds) {
                    return Err(SharePatchError::Invalid(format!(
                        "managed grant durationSeconds must be between 1 and {MAX_MANAGED_GRANT_DURATION_SECONDS}"
                    )));
                }
                if policy.expires_at.is_some() || policy.token_period_anchor_at_ms.is_some() {
                    return Err(SharePatchError::Invalid(
                        "relative managed grants must not provide an absolute expiry or token-period anchor"
                            .to_string(),
                    ));
                }
            }
            Ok(())
        }
        ShareManagedGrantAction::Revoke => {
            if operation.policy.is_some() || operation.duration_seconds.is_some() {
                return Err(SharePatchError::Invalid(
                    "managed grant revoke must not include policy or durationSeconds".to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn apply_managed_grant_operation(
    share: &mut Share,
    operation: &ShareManagedGrantOperation,
) -> Result<(), SharePatchError> {
    let email = normalize_verified_email(&operation.email)?;
    if share
        .owner_email
        .as_deref()
        .is_some_and(|owner| owner.eq_ignore_ascii_case(&email))
    {
        return Err(SharePatchError::Invalid(
            "share owner cannot receive a market entitlement".to_string(),
        ));
    }

    match operation.action {
        ShareManagedGrantAction::Upsert => {
            let mut policy = operation.policy.clone().ok_or_else(|| {
                SharePatchError::Invalid("managed grant upsert requires policy".to_string())
            })?;
            if policy.parallel_limit == Some(0) || policy.token_limit == Some(0) {
                return Err(SharePatchError::Invalid(
                    "user limits must be positive or unlimited".to_string(),
                ));
            }
            let applied_at_ms = now_ms();
            let now = i64::try_from(applied_at_ms).unwrap_or(i64::MAX);
            if let Some(duration_seconds) = operation.duration_seconds {
                let duration_ms = i64::try_from(duration_seconds)
                    .ok()
                    .and_then(|seconds| seconds.checked_mul(1_000))
                    .ok_or_else(|| {
                        SharePatchError::Invalid(
                            "managed grant durationSeconds is too large".to_string(),
                        )
                    })?;
                policy.expires_at = now
                    .checked_add(duration_ms)
                    .ok_or_else(|| {
                        SharePatchError::Invalid(
                            "managed grant absolute expiry is outside the supported range"
                                .to_string(),
                        )
                    })?
                    .into();
            }
            if policy.token_period.requires_anchor() && policy.token_period_anchor_at_ms.is_none() {
                policy.token_period_anchor_at_ms = Some(now.div_euclid(60_000) * 60_000);
            }
            validate_user_policy(&policy, now).map_err(SharePatchError::Invalid)?;
            if share.user_grants.values().any(|grant| {
                grant.active
                    && grant.entitlement_id.as_deref() == Some(operation.entitlement_id.as_str())
                    && !grant.email.eq_ignore_ascii_case(&email)
            }) {
                return Err(SharePatchError::Invalid(
                    "market entitlement is already assigned to another user".to_string(),
                ));
            }
            if let Some(existing) = share.user_grants.get(&email).filter(|grant| grant.active) {
                let same_entitlement = existing.manager == ShareGrantManager::RouterShareMarket
                    && existing.entitlement_id.as_deref()
                        == Some(operation.entitlement_id.as_str());
                if !same_entitlement {
                    return Err(SharePatchError::Invalid(
                        "user already has Share access outside this market entitlement".to_string(),
                    ));
                }
            }

            let previous = share.user_grants.get(&email).cloned();
            share.user_grants.insert(
                email.clone(),
                ShareUserGrant {
                    email: email.clone(),
                    role: "shareto".to_string(),
                    active: true,
                    policy,
                    usage: previous
                        .as_ref()
                        .map(|grant| grant.usage.clone())
                        .unwrap_or_default(),
                    usage_rebase: previous
                        .as_ref()
                        .and_then(|grant| grant.usage_rebase.clone()),
                    usage_quota: None,
                    created_at_ms: previous
                        .as_ref()
                        .map(|grant| grant.created_at_ms)
                        .filter(|created_at| *created_at > 0)
                        .unwrap_or(applied_at_ms),
                    updated_at_ms: applied_at_ms,
                    revoked_at_ms: None,
                    revision: previous
                        .as_ref()
                        .map(|grant| grant.revision.saturating_add(1))
                        .unwrap_or(1)
                        .max(1),
                    manager: ShareGrantManager::RouterShareMarket,
                    entitlement_id: Some(operation.entitlement_id.clone()),
                },
            );
        }
        ShareManagedGrantAction::Revoke => {
            let Some((grant_email, grant)) = share.user_grants.iter().find(|(_, grant)| {
                grant.manager == ShareGrantManager::RouterShareMarket
                    && grant.entitlement_id.as_deref() == Some(operation.entitlement_id.as_str())
            }) else {
                return Ok(());
            };
            if !grant.email.eq_ignore_ascii_case(&email) {
                return Err(SharePatchError::Invalid(
                    "managed grant revoke email does not match the entitlement owner".to_string(),
                ));
            }
            let grant_email = grant_email.clone();
            let now = now_ms();
            if let Some(grant) = share.user_grants.get_mut(&grant_email) {
                grant.active = false;
                grant.updated_at_ms = now;
                grant.revoked_at_ms = Some(now);
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
    }
    Ok(())
}

fn normalize_user_grants(
    incoming: &BTreeMap<String, ShareUserGrant>,
    existing: &BTreeMap<String, ShareUserGrant>,
    owner_email: Option<&str>,
) -> Result<BTreeMap<String, ShareUserGrant>, SharePatchError> {
    let now = now_ms();
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    let mut normalized = existing.clone();
    for grant in normalized.values_mut() {
        if grant.role != "owner"
            && grant.manager != ShareGrantManager::RouterShareMarket
            && grant.active
        {
            grant.active = false;
            grant.revoked_at_ms = Some(now);
            grant.updated_at_ms = now;
            grant.revision = grant.revision.saturating_add(1).max(1);
        }
    }

    for (key, incoming_grant) in incoming {
        let email = normalize_verified_email(if incoming_grant.email.trim().is_empty() {
            key
        } else {
            &incoming_grant.email
        })?;
        if owner.as_deref() == Some(email.as_str()) && incoming_grant.role != "owner" {
            return Err(SharePatchError::Invalid(
                "share owner cannot also be a ShareTo user".to_string(),
            ));
        }
        if incoming_grant.policy.parallel_limit == Some(0)
            || incoming_grant.policy.token_limit == Some(0)
        {
            return Err(SharePatchError::Invalid(
                "user limits must be positive or unlimited".to_string(),
            ));
        }
        validate_user_policy(&incoming_grant.policy, now as i64)
            .map_err(SharePatchError::Invalid)?;
        let previous = existing.get(&email);
        if let Some(previous) = previous {
            if previous.manager == ShareGrantManager::RouterShareMarket {
                if incoming_grant.active != previous.active
                    || !email.eq_ignore_ascii_case(&previous.email)
                    || incoming_grant.role != previous.role
                    || incoming_grant.policy != previous.policy
                    || incoming_grant.manager != ShareGrantManager::RouterShareMarket
                    || incoming_grant.entitlement_id != previous.entitlement_id
                {
                    return Err(SharePatchError::Invalid(
                        "Share Market managed users are read-only".to_string(),
                    ));
                }
                normalized.insert(email, previous.clone());
                continue;
            }
        }
        let mut grant = incoming_grant.clone();
        grant.email = email.clone();
        grant.role = if owner.as_deref() == Some(email.as_str()) {
            "owner".to_string()
        } else {
            "shareto".to_string()
        };
        grant.active = true;
        grant.usage = previous.map(|item| item.usage.clone()).unwrap_or_default();
        // Usage snapshots and rebases are Server-owned.  Never trust either
        // value from a browser/Router patch; preserve the current durable
        // rebase and rebuild the derived snapshot below.
        grant.usage_rebase = previous.and_then(|item| item.usage_rebase.clone());
        grant.usage_quota = previous.and_then(|item| item.usage_quota);
        grant.created_at_ms = previous
            .map(|item| item.created_at_ms)
            .filter(|value| *value > 0)
            .unwrap_or(now);
        grant.updated_at_ms = now;
        grant.revoked_at_ms = None;
        grant.revision = previous
            .map(|item| item.revision.saturating_add(1))
            .unwrap_or(1)
            .max(1);
        grant.manager = if grant.role == "owner" {
            ShareGrantManager::Owner
        } else {
            ShareGrantManager::Manual
        };
        grant.entitlement_id = None;
        normalized.insert(email, grant);
    }
    Ok(normalized)
}

fn reconcile_user_grants(share: &mut Share) {
    let policy_template = default_user_policy(share);
    let owner = share
        .owner_email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if let Some(owner) = owner.as_ref() {
        let now = now_ms();
        let grant = share
            .user_grants
            .entry(owner.clone())
            .or_insert_with(|| ShareUserGrant {
                email: owner.clone(),
                role: "owner".to_string(),
                active: true,
                policy: policy_template.clone(),
                usage: Default::default(),
                usage_rebase: None,
                usage_quota: None,
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                revoked_at_ms: None,
                revision: 1,
                manager: ShareGrantManager::Owner,
                entitlement_id: None,
            });
        if grant.email != *owner
            || grant.role != "owner"
            || !grant.active
            || grant.revoked_at_ms.is_some()
        {
            grant.updated_at_ms = now;
            grant.revision = grant.revision.saturating_add(1).max(1);
        }
        grant.email = owner.clone();
        grant.role = "owner".to_string();
        grant.active = true;
        grant.revoked_at_ms = None;
        grant.manager = ShareGrantManager::Owner;
        grant.entitlement_id = None;

        for (email, grant) in &mut share.user_grants {
            if email != owner && grant.role == "owner" {
                grant.role = "shareto".to_string();
                grant.manager = ShareGrantManager::Manual;
                grant.entitlement_id = None;
                grant.updated_at_ms = now;
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
    }
}

pub fn normalize_share_subdomain(subdomain: &str) -> Result<String, &'static str> {
    let value = subdomain.trim().to_ascii_lowercase();
    crate::domain::router::ShareSlug::parse(&value)
        .map_err(|_| "share slug must be 6-30 lowercase DNS characters without '--'")?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed_operation(
        operation_id: &str,
        entitlement_id: &str,
        expected_config_revision: u64,
        action: ShareManagedGrantAction,
        email: &str,
    ) -> ShareManagedGrantOperation {
        ShareManagedGrantOperation {
            operation_id: operation_id.to_string(),
            entitlement_id: entitlement_id.to_string(),
            share_sequence: 1,
            expected_config_revision,
            action,
            email: email.to_string(),
            policy: (action == ShareManagedGrantAction::Upsert).then_some(ShareUserPolicy {
                parallel_limit: Some(2),
                token_limit: Some(10_000),
                token_period: ShareTokenPeriod::Day,
                token_period_anchor_at_ms: None,
                expires_at: None,
                allowed_apps: Vec::new(),
            }),
            duration_seconds: None,
        }
    }

    #[test]
    fn legacy_share_store_defaults_router_delete_outbox() {
        let store: ShareStore = serde_json::from_str(r#"{"shares":[]}"#).unwrap();
        assert!(store.pending_router_deletes.is_empty());
        assert!(store.router_share_prune_marker.is_none());

        let tombstone: ShareDeleteTombstone = serde_json::from_str(
            r#"{"shareId":"legacy","operationId":"legacy-op","createdAtMs":1}"#,
        )
        .unwrap();
        assert!(tombstone.has_legacy_router_target());
    }

    #[test]
    fn managed_grant_is_idempotent_payload_bound_and_protected_from_ordinary_edits() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("managed-grant")).unwrap();
        let operation = managed_operation(
            "operation-upsert",
            "entitlement-1",
            original.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter@example.com",
        );
        let updated = store
            .apply_settings_patch(
                "managed-grant",
                ShareSettingsPatch {
                    managed_grant: Some(operation.clone()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let grant = updated.user_grants.get("renter@example.com").unwrap();
        assert!(grant.active);
        assert_eq!(grant.manager, ShareGrantManager::RouterShareMarket);
        assert_eq!(grant.entitlement_id.as_deref(), Some("entitlement-1"));
        assert_eq!(grant.policy.token_limit, Some(10_000));

        let persisted = serde_json::to_string(&store).unwrap();
        let mut store: ShareStore = serde_json::from_str(&persisted).unwrap();
        let repeated = store
            .apply_settings_patch(
                "managed-grant",
                ShareSettingsPatch {
                    managed_grant: Some(operation.clone()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert_eq!(repeated.config_revision, updated.config_revision);

        let mut reused = operation;
        reused.action = ShareManagedGrantAction::Revoke;
        reused.policy = None;
        reused.expected_config_revision = repeated.config_revision;
        let error = store
            .apply_settings_patch(
                "managed-grant",
                ShareSettingsPatch {
                    managed_grant: Some(reused),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("reused with a different payload"));

        let mut ordinary_grants = repeated.user_grants.clone();
        ordinary_grants
            .get_mut("renter@example.com")
            .unwrap()
            .policy
            .token_limit = Some(1);
        let error = store
            .apply_settings_patch(
                "managed-grant",
                ShareSettingsPatch {
                    user_grants: Some(ordinary_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("managed users are read-only"));
        assert_eq!(
            store
                .get("managed-grant")
                .unwrap()
                .user_grants
                .get("renter@example.com")
                .unwrap()
                .policy
                .token_limit,
            Some(10_000)
        );
    }

    #[test]
    fn managed_grant_allowed_apps_is_enforced_without_breaking_legacy_empty_scope() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("app-scoped-grant")).unwrap();
        let shared = store
            .add_binding(
                "app-scoped-grant",
                ShareBinding {
                    app: AppKind::Claude,
                    provider_id: "claude-provider".into(),
                    provider_type: ProviderType::Claude,
                },
            )
            .unwrap();
        let mut operation = managed_operation(
            "operation-app-scope",
            "entitlement-app-scope",
            shared.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter@example.com",
        );
        operation
            .policy
            .as_mut()
            .expect("upsert policy")
            .allowed_apps = vec![AppKind::Codex];
        store
            .apply_settings_patch(
                &original.id,
                ShareSettingsPatch {
                    managed_grant: Some(operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply App-scoped managed grant");
        let invocation_now = i64::try_from(now_ms()).expect("current timestamp fits i64");

        assert!(store
            .validate_for_invocation(
                "app-scoped-grant",
                AppKind::Codex,
                Some("renter@example.com"),
                invocation_now,
            )
            .is_ok());
        let rejection = store
            .validate_for_invocation(
                "app-scoped-grant",
                AppKind::Claude,
                Some("renter@example.com"),
                invocation_now,
            )
            .expect_err("Codex rental must not authorize Claude");
        assert_eq!(rejection.reason, ShareRejectReason::AppNotAllowed);

        store
            .shares
            .iter_mut()
            .find(|share| share.id == "app-scoped-grant")
            .expect("App-scoped Share")
            .user_grants
            .get_mut("renter@example.com")
            .expect("managed renter grant")
            .policy
            .allowed_apps
            .clear();
        assert!(store
            .validate_for_invocation(
                "app-scoped-grant",
                AppKind::Claude,
                Some("renter@example.com"),
                invocation_now,
            )
            .is_ok());
    }

    #[test]
    fn managed_relative_term_starts_when_applied_and_replay_keeps_effective_policy() {
        let mut store = ShareStore::default();
        let original = store
            .upsert(codex_share_input("managed-relative-term"))
            .unwrap();
        let mut operation = managed_operation(
            "operation-relative-term",
            "entitlement-relative-term",
            original.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter@example.com",
        );
        let policy = operation.policy.as_mut().unwrap();
        policy.token_period = ShareTokenPeriod::SevenDays;
        operation.duration_seconds = Some(86_400);

        let updated = store
            .apply_settings_patch(
                "managed-relative-term",
                ShareSettingsPatch {
                    managed_grant: Some(operation.clone()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let applied = updated.user_grants.get("renter@example.com").unwrap();
        let applied_at_ms = i64::try_from(applied.updated_at_ms).unwrap();
        assert_eq!(applied.policy.expires_at, Some(applied_at_ms + 86_400_000));
        assert_eq!(
            applied.policy.token_period_anchor_at_ms,
            Some(applied_at_ms.div_euclid(60_000) * 60_000)
        );
        let effective_policy = applied.policy.clone();
        assert_eq!(
            store.router_control_upsert_result("operation-relative-term"),
            Some((applied_at_ms, effective_policy.clone()))
        );

        let persisted = serde_json::to_string(&store).unwrap();
        let mut store: ShareStore = serde_json::from_str(&persisted).unwrap();
        let replayed = store
            .apply_settings_patch(
                "managed-relative-term",
                ShareSettingsPatch {
                    managed_grant: Some(operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let replayed = replayed.user_grants.get("renter@example.com").unwrap();
        assert_eq!(
            replayed.updated_at_ms,
            u128::try_from(applied_at_ms).unwrap()
        );
        assert_eq!(replayed.policy, effective_policy);
        assert_eq!(
            store.router_control_upsert_result("operation-relative-term"),
            Some((applied_at_ms, effective_policy))
        );
        assert!(store.forget_router_control_operation("operation-relative-term"));
        assert!(store
            .router_control_upsert_result("operation-relative-term")
            .is_none());
    }

    #[test]
    fn newer_share_sequence_retires_only_that_shares_replay_record() {
        let mut store = ShareStore::default();
        let first = store
            .upsert(codex_share_input("managed-sequence-a"))
            .unwrap();
        let mut second_input = codex_share_input("managed-sequence-b");
        second_input.provider_id = "managed-sequence-b-provider".to_string();
        let second = store.upsert(second_input).unwrap();
        let first_operation = managed_operation(
            "operation-sequence-a-1",
            "entitlement-sequence-a",
            first.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter-a@example.com",
        );
        let second_operation = managed_operation(
            "operation-sequence-b-1",
            "entitlement-sequence-b",
            second.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter-b@example.com",
        );
        let first = store
            .apply_settings_patch(
                "managed-sequence-a",
                ShareSettingsPatch {
                    managed_grant: Some(first_operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        store
            .apply_settings_patch(
                "managed-sequence-b",
                ShareSettingsPatch {
                    managed_grant: Some(second_operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        let mut next_operation = managed_operation(
            "operation-sequence-a-2",
            "entitlement-sequence-a",
            first.config_revision,
            ShareManagedGrantAction::Revoke,
            "renter-a@example.com",
        );
        next_operation.share_sequence = 2;
        store
            .apply_settings_patch(
                "managed-sequence-a",
                ShareSettingsPatch {
                    managed_grant: Some(next_operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(!store
            .applied_router_control_operations
            .contains_key("operation-sequence-a-1"));
        assert!(store
            .applied_router_control_operations
            .contains_key("operation-sequence-a-2"));
        assert!(store
            .applied_router_control_operations
            .contains_key("operation-sequence-b-1"));
        assert_eq!(store.applied_router_control_operations.len(), 2);
    }

    #[test]
    fn managed_permanent_grant_materializes_rolling_token_period_anchor() {
        let mut store = ShareStore::default();
        let original = store
            .upsert(codex_share_input("managed-permanent-term"))
            .unwrap();
        let mut operation = managed_operation(
            "operation-permanent-term",
            "entitlement-permanent-term",
            original.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter@example.com",
        );
        operation.policy.as_mut().unwrap().token_period = ShareTokenPeriod::ThirtyDays;

        let updated = store
            .apply_settings_patch(
                "managed-permanent-term",
                ShareSettingsPatch {
                    managed_grant: Some(operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let grant = updated.user_grants.get("renter@example.com").unwrap();
        let applied_at_ms = i64::try_from(grant.updated_at_ms).unwrap();
        assert_eq!(grant.policy.expires_at, None);
        assert_eq!(
            grant.policy.token_period_anchor_at_ms,
            Some(applied_at_ms.div_euclid(60_000) * 60_000)
        );
    }

    #[test]
    fn managed_grant_operation_id_cannot_replay_against_another_share() {
        let mut store = ShareStore::default();
        let first = store.upsert(codex_share_input("managed-first")).unwrap();
        let mut second_input = codex_share_input("managed-second");
        second_input.provider_id = "p2".to_string();
        let second = store.upsert(second_input).unwrap();
        assert_eq!(first.config_revision, second.config_revision);
        let operation = managed_operation(
            "operation-cross-share",
            "entitlement-cross-share",
            first.config_revision,
            ShareManagedGrantAction::Upsert,
            "renter@example.com",
        );
        store
            .apply_settings_patch(
                "managed-first",
                ShareSettingsPatch {
                    managed_grant: Some(operation.clone()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        let error = store
            .apply_settings_patch(
                "managed-second",
                ShareSettingsPatch {
                    managed_grant: Some(operation),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("reused with a different payload"));
        let second = store.get("managed-second").unwrap();
        assert_eq!(second.config_revision, 1);
        assert!(!second.user_grants.contains_key("renter@example.com"));
    }

    #[test]
    fn managed_revoke_requires_matching_email_and_does_not_reactivate_later() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("managed-revoke")).unwrap();
        let granted = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    managed_grant: Some(managed_operation(
                        "operation-upsert",
                        "entitlement-1",
                        original.config_revision,
                        ShareManagedGrantAction::Upsert,
                        "renter@example.com",
                    )),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        let mismatch = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    managed_grant: Some(managed_operation(
                        "operation-wrong-email",
                        "entitlement-1",
                        granted.config_revision,
                        ShareManagedGrantAction::Revoke,
                        "other@example.com",
                    )),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();
        assert!(mismatch.to_string().contains("does not match"));
        assert!(
            store
                .get("managed-revoke")
                .unwrap()
                .user_grants
                .get("renter@example.com")
                .unwrap()
                .active
        );

        let revoked = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    managed_grant: Some(managed_operation(
                        "operation-revoke",
                        "entitlement-1",
                        granted.config_revision,
                        ShareManagedGrantAction::Revoke,
                        "renter@example.com",
                    )),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let grant = revoked.user_grants.get("renter@example.com").unwrap();
        assert!(!grant.active);

        let ordinary = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    description: Some(Some("ordinary edit".to_string())),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(
            !ordinary
                .user_grants
                .get("renter@example.com")
                .unwrap()
                .active
        );

        let mut reactivated = revoked.user_grants.clone();
        reactivated.get_mut("renter@example.com").unwrap().active = true;
        let error = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    user_grants: Some(reactivated),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("managed users are read-only"));
    }

    #[test]
    fn managed_grant_revision_conflict_has_no_partial_effect() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("managed-conflict")).unwrap();
        let error = store
            .apply_settings_patch(
                "managed-conflict",
                ShareSettingsPatch {
                    managed_grant: Some(managed_operation(
                        "operation-conflict",
                        "entitlement-conflict",
                        original.config_revision + 1,
                        ShareManagedGrantAction::Upsert,
                        "renter@example.com",
                    )),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("expected config revision"));
        let unchanged = store.get("managed-conflict").unwrap();
        assert_eq!(unchanged.config_revision, original.config_revision);
        assert!(!unchanged.user_grants.contains_key("renter@example.com"));
        assert!(store.applied_router_control_operations.is_empty());
    }

    #[test]
    fn account_metadata_refresh_follows_effective_share_bindings() {
        let mut input = codex_share_input("share-multi-app");
        input.provider_id = "codex-managed".to_string();
        input.provider_type = ProviderType::CodexOAuth;
        input.bindings = vec![ShareBinding {
            app: AppKind::Codex,
            provider_id: "codex-managed".to_string(),
            provider_type: ProviderType::CodexOAuth,
        }];
        let mut store = ShareStore::default();
        let original = store.upsert(input).unwrap();
        let provider_keys = BTreeSet::from([(AppKind::Codex, "codex-managed".to_string())]);

        let updated = store.refresh_runtime_snapshots_for_providers(
            &provider_keys,
            &ProviderStore::default(),
            None,
            &UsageStore::default(),
        );

        assert_eq!(updated, vec!["share-multi-app"]);
        assert!(store.get("share-multi-app").unwrap().config_revision > original.config_revision);
    }

    #[test]
    fn provider_runtime_snapshot_reconciliation_is_idempotent_and_tracks_defaults() {
        let mut providers = ProviderStore::default();
        providers.upsert_with_resource(
            AppKind::Codex,
            crate::domain::providers::model::Provider {
                id: "runtime-defaults-provider".to_string(),
                name: "Runtime defaults Provider".to_string(),
                settings_config: json!({
                    "apiKey": "test-key",
                    "modelMapping": {"mode": "passthrough"}
                }),
                category: None,
                meta: None,
                extra: BTreeMap::new(),
            },
            crate::domain::providers::store::ProviderResourceMetadata {
                profile_id: Some(
                    crate::domain::providers::registry::ProfileId::parse("codex.openai_api_key")
                        .unwrap(),
                ),
                profile_schema_revision: Some(1),
                revision: 1,
                credential_generation: 1,
                ..Default::default()
            },
        );
        let accounts = AccountStore::default();
        providers.rebuild_runtime_index(&accounts).unwrap();
        let mut input = codex_share_input("runtime-defaults-share");
        input.provider_id = "runtime-defaults-provider".to_string();
        let mut store = ShareStore::default();
        let original = store.upsert(input).unwrap();
        let provider_keys =
            BTreeSet::from([(AppKind::Codex, "runtime-defaults-provider".to_string())]);
        let usage = UsageStore::default();

        let first = store.reconcile_runtime_snapshots_for_providers(
            &provider_keys,
            &providers,
            Some(&accounts),
            &usage,
        );
        assert_eq!(first, vec!["runtime-defaults-share"]);
        let first_share = store.get("runtime-defaults-share").unwrap().clone();
        let first_fingerprint = first_share
            .runtime_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot["runtimeMetadataFingerprint"].as_str())
            .unwrap()
            .to_string();
        assert_eq!(first_share.config_revision, original.config_revision);

        let repeated = store.reconcile_runtime_snapshots_for_providers(
            &provider_keys,
            &providers,
            Some(&accounts),
            &usage,
        );
        assert!(repeated.is_empty());
        assert_eq!(
            store.get("runtime-defaults-share").unwrap().config_revision,
            first_share.config_revision
        );

        let mut defaults = providers.runtime_defaults().clone();
        defaults.transport.timeout_ms = 310_000;
        defaults.test_models.codex = "gpt-runtime-defaults-test".to_string();
        providers.set_runtime_defaults(defaults);
        providers.rebuild_runtime_index(&accounts).unwrap();
        let changed = store.reconcile_runtime_snapshots_for_providers(
            &provider_keys,
            &providers,
            Some(&accounts),
            &usage,
        );
        assert_eq!(changed, vec!["runtime-defaults-share"]);
        let changed_share = store.get("runtime-defaults-share").unwrap();
        assert_ne!(
            changed_share
                .runtime_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot["runtimeMetadataFingerprint"].as_str()),
            Some(first_fingerprint.as_str())
        );
        assert_eq!(changed_share.config_revision, first_share.config_revision);
        let stored = &providers.providers[0].provider;
        assert!(stored.settings_config.get("transport").is_none());
        assert!(stored.settings_config.get("testModel").is_none());
        assert!(!stored.extra.contains_key("testModel"));
    }

    #[test]
    fn subscription_expiry_fingerprint_ignores_volatile_remaining_time() {
        let first = json!({
            "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
            "subscriptionRemainingMs": 1000,
            "updatedAtMs": 10,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-08-10T23:59:59Z"}
            }
        });
        let same_expiry = json!({
            "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
            "subscriptionRemainingMs": 1,
            "updatedAtMs": 20,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-08-10T23:59:59Z"}
            }
        });
        let next_period = json!({
            "subscriptionExpiresAt": "2026-09-10T23:59:59Z",
            "subscriptionRemainingMs": 1000,
            "updatedAtMs": 30,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-09-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-09-10T23:59:59Z"}
            }
        });

        assert_eq!(
            subscription_expiry_fingerprint(Some(&first)),
            subscription_expiry_fingerprint(Some(&same_expiry))
        );
        assert_ne!(
            subscription_expiry_fingerprint(Some(&first)),
            subscription_expiry_fingerprint(Some(&next_period))
        );
    }

    #[test]
    fn router_delete_tombstone_normalizes_bound_target() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("share-targeted")).unwrap();

        let tombstone = store
            .delete_for_router_target(
                "share-targeted",
                " https://router.example.test/api/// ",
                " installation-a ",
            )
            .unwrap();

        assert_eq!(
            tombstone.router_api_base.as_deref(),
            Some("https://router.example.test/api")
        );
        assert_eq!(tombstone.installation_id.as_deref(), Some("installation-a"));
        assert!(
            tombstone.router_target_matches("https://router.example.test/api/", "installation-a")
        );
    }

    #[test]
    fn router_share_prune_marker_normalizes_target_and_changes_with_installation() {
        let mut store = ShareStore::default();
        assert!(store.mark_router_share_prune_applied(
            " https://router.example.test/api/// ",
            " installation-a "
        ));
        assert!(store
            .router_share_prune_applied_for("https://router.example.test/api", "installation-a"));
        assert!(!store
            .mark_router_share_prune_applied("https://router.example.test/api/", "installation-a"));

        assert!(!store
            .router_share_prune_applied_for("https://router.example.test/api", "installation-b"));
        assert!(store
            .mark_router_share_prune_applied("https://router.example.test/api", "installation-b"));
    }

    #[test]
    fn recreating_same_share_id_cancels_pending_router_delete() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("share-recreated")).unwrap();
        let tombstone = store.delete("share-recreated").unwrap();
        assert!(store
            .pending_router_delete("share-recreated", &tombstone.operation_id)
            .is_some());

        let recreated = store.upsert(codex_share_input("share-recreated")).unwrap();

        assert!(store.pending_router_deletes.is_empty());
        assert!(recreated.config_revision > recreated.router_synced_revision);
    }

    fn codex_share_input(id: &str) -> UpsertShareInput {
        UpsertShareInput {
            id: Some(id.to_string()),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_type: ProviderType::Codex,
            display_name: None,
            enabled: None,
            status: None,
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            free_access: None,
            allow_personal_credits: None,
            auto_consume_banked_reset: None,
            banked_reset_expiry_lead_minutes: None,
            previous_response_cache_enabled: None,
            auto_start: None,
            description: None,
            enabled_apps: None,
            bindings: Vec::new(),
            runtime_snapshot: None,
            user_grants: BTreeMap::new(),
        }
    }

    fn add_manual_shareto(input: &mut UpsertShareInput, email: &str) {
        let email = email.trim().to_ascii_lowercase();
        input.user_grants.insert(
            email.clone(),
            ShareUserGrant {
                email,
                role: "shareto".to_string(),
                active: true,
                policy: ShareUserPolicy {
                    parallel_limit: input.parallel_limit,
                    token_limit: input.token_limit,
                    token_period: ShareTokenPeriod::Lifetime,
                    token_period_anchor_at_ms: None,
                    expires_at: input.expires_at,
                    allowed_apps: Vec::new(),
                },
                ..ShareUserGrant::default()
            },
        );
    }

    fn codex_provider_store(provider_ids: &[&str]) -> ProviderStore {
        let mut providers = ProviderStore::default();
        for provider_id in provider_ids {
            let stored = providers.upsert(
                AppKind::Codex,
                crate::domain::providers::model::Provider {
                    id: (*provider_id).to_string(),
                    name: (*provider_id).to_string(),
                    settings_config: json!({}),
                    category: None,
                    meta: None,
                    extra: BTreeMap::new(),
                },
            );
            assert_eq!(stored.provider_type, ProviderType::Codex);
        }
        providers
    }

    #[test]
    fn integrity_repair_restores_empty_binding_without_disturbing_healthy_share() {
        let providers = codex_provider_store(&["p1", "p2"]);
        let accounts = AccountStore::default();
        let mut store = ShareStore::default();
        let mut repaired_input = codex_share_input("needs-repair");
        repaired_input.provider_id = "p1".to_string();
        store.upsert(repaired_input).unwrap();
        let mut healthy_input = codex_share_input("healthy");
        healthy_input.provider_id = "p2".to_string();
        store.upsert(healthy_input).unwrap();
        store
            .shares
            .iter_mut()
            .find(|share| share.id == "needs-repair")
            .unwrap()
            .bindings
            .clear();

        let outcomes = store.repair_integrity(&providers, &accounts, &[7; 32]);

        let repaired = outcomes
            .iter()
            .find(|outcome| outcome.share_id == "needs-repair")
            .unwrap();
        assert_eq!(repaired.status, ShareIntegrityStatus::Repaired);
        assert!(repaired.changed);
        assert_eq!(store.get("needs-repair").unwrap().bindings.len(), 1);
        let healthy = store.get("healthy").unwrap();
        assert!(healthy.enabled);
        assert!(healthy.integrity_error.is_none());
    }

    #[test]
    fn integrity_repair_disables_only_invalid_share_and_persists_the_diagnosis() {
        let providers = codex_provider_store(&["p1"]);
        let accounts = AccountStore::default();
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("healthy")).unwrap();
        let mut invalid_input = codex_share_input("invalid");
        invalid_input.provider_id = "missing".to_string();
        store.upsert(invalid_input).unwrap();

        let outcomes = store.repair_integrity(&providers, &accounts, &[9; 32]);

        assert!(store.get("healthy").unwrap().enabled);
        let invalid = store.get("invalid").unwrap();
        assert!(!invalid.enabled);
        assert_eq!(invalid.status, "paused");
        assert!(invalid.capacity_pool_id.is_empty());
        assert_eq!(
            invalid
                .integrity_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("cc_switch_share_provider_reference_invalid")
        );
        let disabled = outcomes
            .iter()
            .find(|outcome| outcome.share_id == "invalid")
            .unwrap();
        assert_eq!(disabled.status, ShareIntegrityStatus::Disabled);
        assert!(disabled.changed);

        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-share-integrity-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        store.save(&config_dir).unwrap();
        let loaded = ShareStore::load_or_default(&config_dir).unwrap();
        let persisted = loaded.get("invalid").unwrap();
        assert!(!persisted.enabled);
        assert!(persisted.integrity_error.is_some());
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    fn test_timestamp_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC test timestamp")
            .timestamp_millis()
    }

    #[test]
    fn new_owner_and_explicit_shareto_grants_use_canonical_policies() {
        let expires_at = test_timestamp_ms(2030, 1, 1, 0, 0);
        let mut input = codex_share_input("grant-defaults");
        input.token_limit = Some(50_000);
        input.parallel_limit = Some(7);
        input.expires_at = Some(expires_at);
        let expected = ShareUserPolicy {
            parallel_limit: Some(7),
            token_limit: Some(50_000),
            token_period: ShareTokenPeriod::Lifetime,
            token_period_anchor_at_ms: None,
            expires_at: Some(expires_at),
            allowed_apps: Vec::new(),
        };
        input.user_grants.insert(
            "User@Example.com".to_string(),
            ShareUserGrant {
                email: "User@Example.com".to_string(),
                role: "shareto".to_string(),
                active: true,
                policy: expected.clone(),
                ..ShareUserGrant::default()
            },
        );

        let share = ShareStore::default().upsert(input).unwrap();

        assert_eq!(share.user_grants["owner@example.com"].policy, expected);
        assert_eq!(share.user_grants["user@example.com"].policy, expected);
        assert_eq!(share.user_grants["owner@example.com"].role, "owner");
        assert_eq!(share.user_grants["user@example.com"].role, "shareto");
    }

    #[test]
    fn user_usage_uses_utc_day_week_and_calendar_month_boundaries() {
        let sunday = test_timestamp_ms(2026, 7, 19, 23, 59);
        let monday = test_timestamp_ms(2026, 7, 20, 0, 0);
        let january_end = test_timestamp_ms(2027, 1, 31, 23, 59);
        let february = test_timestamp_ms(2027, 2, 1, 0, 0);
        let mut usage = ShareUserUsage::default();

        usage.record(11, sunday);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Lifetime, monday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Day, sunday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Day, monday), 0);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Week, sunday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Week, monday), 0);

        usage.record(13, january_end);
        assert_eq!(
            usage.tokens_for(ShareTokenPeriod::CalendarMonth, january_end),
            13
        );
        assert_eq!(
            usage.tokens_for(ShareTokenPeriod::CalendarMonth, february),
            0
        );
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Lifetime, february), 24);
    }

    #[test]
    fn anchored_usage_is_scoped_by_period_anchor_and_current_window() {
        let anchor = test_timestamp_ms(2026, 7, 1, 12, 15);
        let policy = ShareUserPolicy {
            token_limit: Some(100),
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(anchor),
            ..ShareUserPolicy::default()
        };
        let mut usage = ShareUserUsage::default();
        usage.record_for_policy(&policy, 17, test_timestamp_ms(2026, 7, 14, 8, 0));
        assert_eq!(
            usage.tokens_for_policy(&policy, test_timestamp_ms(2026, 7, 15, 12, 14)),
            17
        );
        assert_eq!(
            usage.tokens_for_policy(&policy, test_timestamp_ms(2026, 7, 15, 12, 15)),
            0
        );

        let shifted = ShareUserPolicy {
            token_period_anchor_at_ms: Some(test_timestamp_ms(2026, 7, 2, 12, 15)),
            ..policy
        };
        assert_eq!(
            usage.tokens_for_policy(&shifted, test_timestamp_ms(2026, 7, 15, 12, 14)),
            0
        );
    }

    #[test]
    fn changing_fixed_period_rebuilds_usage_from_persistent_history() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("anchored-history");
        add_manual_shareto(&mut input, "user@example.com");
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();

        let mut log = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(17),
                ..Default::default()
            },
        );
        log.share_id = Some("anchored-history".to_string());
        log.user_email = Some("USER@example.com".to_string());
        log.created_at_ms = test_timestamp_ms(2026, 7, 24, 8, 0) as u128;
        let mut usage = UsageStore::default();
        usage.push(log);

        let mut grants = store.get("anchored-history").unwrap().user_grants.clone();
        grants.get_mut("user@example.com").unwrap().policy = ShareUserPolicy {
            token_limit: Some(100),
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(test_timestamp_ms(2026, 7, 1, 12, 0)),
            ..ShareUserPolicy::default()
        };
        let updated = store
            .apply_settings_patch_with_usage(
                "anchored-history",
                ShareSettingsPatch {
                    user_grants: Some(grants.clone()),
                    ..ShareSettingsPatch::default()
                },
                &usage,
                now,
            )
            .unwrap();
        assert_eq!(
            updated.user_grants["user@example.com"]
                .usage
                .tokens_for_policy(&updated.user_grants["user@example.com"].policy, now),
            17
        );

        grants
            .get_mut("user@example.com")
            .unwrap()
            .policy
            .token_period_anchor_at_ms = Some(test_timestamp_ms(2026, 7, 25, 12, 0));
        let shifted = store
            .apply_settings_patch_with_usage(
                "anchored-history",
                ShareSettingsPatch {
                    user_grants: Some(grants),
                    ..ShareSettingsPatch::default()
                },
                &usage,
                now,
            )
            .unwrap();
        assert_eq!(
            shifted.user_grants["user@example.com"]
                .usage
                .tokens_for_policy(&shifted.user_grants["user@example.com"].policy, now),
            0
        );
    }

    #[test]
    fn anchored_usage_rebuild_counts_summary_tokens_without_a_second_request() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("anchored-summary");
        add_manual_shareto(&mut input, "user@example.com");
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();

        let mut invocation = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(7),
                ..Default::default()
            },
        );
        invocation.request_id = "anchored-invocation".to_string();
        invocation.share_id = Some("anchored-summary".to_string());
        invocation.user_email = Some("USER@example.com".to_string());
        invocation.created_at_ms = test_timestamp_ms(2026, 7, 24, 8, 0) as u128;

        let mut summary = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(10),
                ..Default::default()
            },
        );
        summary.request_id = "anchored-summary".to_string();
        summary.record_kind = crate::domain::usage::store::UsageRecordKind::InternalSupplemental;
        summary.share_id = Some("anchored-summary".to_string());
        summary.user_email = Some("user@example.com".to_string());
        summary.data_source = Some(
            crate::domain::usage::store::CODEX_OVERFLOW_COMPACT_SUMMARY_DATA_SOURCE.to_string(),
        );
        summary.created_at_ms = test_timestamp_ms(2026, 7, 24, 8, 1) as u128;

        let mut usage = UsageStore::default();
        usage.push(invocation);
        usage.push(summary);

        let mut grants = store.get("anchored-summary").unwrap().user_grants.clone();
        grants.get_mut("user@example.com").unwrap().policy = ShareUserPolicy {
            token_limit: Some(100),
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(test_timestamp_ms(2026, 7, 1, 12, 0)),
            ..ShareUserPolicy::default()
        };
        let updated = store
            .apply_settings_patch_with_usage(
                "anchored-summary",
                ShareSettingsPatch {
                    user_grants: Some(grants),
                    ..ShareSettingsPatch::default()
                },
                &usage,
                now,
            )
            .unwrap();
        let anchored = updated.user_grants["user@example.com"]
            .usage
            .anchored
            .as_ref()
            .unwrap();
        assert_eq!(anchored.tokens_used, 17);
        assert_eq!(anchored.requests_count, 1);
    }

    #[test]
    fn usage_rebase_sets_target_and_accumulates_only_new_history() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("usage-rebase");
        add_manual_shareto(&mut input, "user@example.com");
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["user@example.com"].revision;

        let mut first = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(100),
                ..Default::default()
            },
        );
        first.request_id = "usage-rebase-1".to_string();
        first.share_id = Some("usage-rebase".to_string());
        first.user_email = Some("user@example.com".to_string());
        first.created_at_ms = (now - 60_000) as u128;
        let mut usage = UsageStore::default();
        usage.push(first);

        let edit = BTreeMap::from([(
            "USER@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(150),
                expected_grant_revision: Some(revision),
                period: Some(ShareTokenPeriod::Lifetime),
                anchor_at_ms: None,
                source:
                    crate::domain::sharing::router_contract::ShareUsageRebaseSource::ProviderReset,
            },
        )]);
        let rebased = store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase",
                ShareSettingsPatch::default(),
                Some(&edit),
                &usage,
                now,
                Some("admin@example.com"),
            )
            .unwrap();
        let grant = &rebased.user_grants["user@example.com"];
        assert_eq!(
            grant.usage_rebase.as_ref().unwrap().applied_by.as_deref(),
            Some("admin@example.com"),
            "a manual correction is indistinguishable from traffic unless the operator is recorded"
        );
        assert_eq!(grant.usage.lifetime.tokens_used, 150);
        assert_eq!(grant.usage_rebase.as_ref().unwrap().target_tokens, 150);
        assert_eq!(
            grant
                .usage_rebase
                .as_ref()
                .unwrap()
                .observed_tokens_at_rebase,
            100
        );

        let mut second = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(20),
                ..Default::default()
            },
        );
        second.request_id = "usage-rebase-2".to_string();
        second.share_id = Some("usage-rebase".to_string());
        second.user_email = Some("user@example.com".to_string());
        second.created_at_ms = now as u128;
        usage.push(second);
        store
            .rebuild_user_usage_from_history("usage-rebase", &usage, now)
            .unwrap();
        assert_eq!(
            store.get("usage-rebase").unwrap().user_grants["user@example.com"]
                .usage
                .lifetime
                .tokens_used,
            170
        );
        // The derived view is what a client reads instead of re-deriving the
        // effective/observed split from the rebase record itself.
        let quota = store.get("usage-rebase").unwrap().user_grants["user@example.com"]
            .usage_quota
            .expect("the Server publishes a derived quota view");
        assert_eq!(quota.effective_tokens_used, 170);
        assert_eq!(quota.observed_tokens_used, 120);
        assert_eq!(quota.manual_offset_tokens, 50);
        assert_eq!(quota.observed_requests_count, 2);
        assert!(quota.rebase_applies);

        store
            .record_user_invocation_result("usage-rebase", Some("user@example.com"), 500, now)
            .unwrap();
        assert_eq!(
            store.get("usage-rebase").unwrap().user_grants["user@example.com"]
                .usage
                .lifetime
                .tokens_used,
            670,
            "a later request must accumulate on the saved consumed-token baseline"
        );

        let clear = BTreeMap::from([(
            "user@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Clear,
                target_tokens: None,
                expected_grant_revision: Some(
                    store.get("usage-rebase").unwrap().user_grants["user@example.com"].revision,
                ),
                period: Some(ShareTokenPeriod::Lifetime),
                anchor_at_ms: None,
                source: crate::domain::sharing::router_contract::ShareUsageRebaseSource::Manual,
            },
        )]);
        let cleared = store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase",
                ShareSettingsPatch::default(),
                Some(&clear),
                &usage,
                now,
                None,
            )
            .unwrap();
        assert!(cleared.user_grants["user@example.com"]
            .usage_rebase
            .is_none());
        assert_eq!(
            cleared.user_grants["user@example.com"]
                .usage
                .lifetime
                .tokens_used,
            120
        );
        let cleared_quota = cleared.user_grants["user@example.com"]
            .usage_quota
            .expect("the derived view survives a baseline clear");
        assert_eq!(cleared_quota.effective_tokens_used, 120);
        assert_eq!(cleared_quota.observed_tokens_used, 120);
        assert_eq!(cleared_quota.manual_offset_tokens, 0);
        assert!(
            !cleared_quota.rebase_applies,
            "no baseline is standing once it is cleared"
        );
    }

    #[test]
    fn saved_consumed_tokens_become_the_baseline_for_the_next_request() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("usage-baseline");
        add_manual_shareto(&mut input, "user@example.com");
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["user@example.com"].revision;
        let usage = UsageStore::default();
        let edit = BTreeMap::from([(
            "user@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(10_000),
                expected_grant_revision: Some(revision),
                period: Some(ShareTokenPeriod::Lifetime),
                anchor_at_ms: None,
                source: crate::domain::sharing::router_contract::ShareUsageRebaseSource::Manual,
            },
        )]);
        let rebased = store
            .apply_settings_patch_with_usage_edits(
                "usage-baseline",
                ShareSettingsPatch::default(),
                Some(&edit),
                &usage,
                now,
                None,
            )
            .unwrap();
        assert_eq!(
            rebased.user_grants["user@example.com"]
                .usage
                .lifetime
                .tokens_used,
            10_000
        );

        store
            .record_user_invocation_result("usage-baseline", Some("user@example.com"), 500, now)
            .unwrap();
        let grant = &store.get("usage-baseline").unwrap().user_grants["user@example.com"];
        assert_eq!(grant.usage.lifetime.tokens_used, 10_500);
        assert_eq!(grant.usage_quota.unwrap().effective_tokens_used, 10_500);
    }

    #[test]
    fn usage_rebase_supports_a_past_fixed_period_anchor() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let anchor = test_timestamp_ms(2026, 7, 24, 12, 0);
        let mut input = codex_share_input("usage-rebase-fixed");
        add_manual_shareto(&mut input, "user@example.com");
        let mut grants = input.user_grants.clone();
        grants.get_mut("user@example.com").unwrap().policy = ShareUserPolicy {
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(anchor),
            token_limit: Some(500),
            ..ShareUserPolicy::default()
        };
        input.user_grants = grants;
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["user@example.com"].revision;
        let mut log = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(40),
                ..Default::default()
            },
        );
        log.request_id = "usage-rebase-fixed-1".to_string();
        log.share_id = Some("usage-rebase-fixed".to_string());
        log.user_email = Some("user@example.com".to_string());
        log.created_at_ms = (now - 60_000) as u128;
        let mut usage = UsageStore::default();
        usage.push(log);
        let edit = BTreeMap::from([(
            "user@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(60),
                expected_grant_revision: Some(revision),
                period: Some(ShareTokenPeriod::SevenDays),
                anchor_at_ms: Some(anchor),
                source:
                    crate::domain::sharing::router_contract::ShareUsageRebaseSource::ProviderReset,
            },
        )]);
        let saved = store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase-fixed",
                ShareSettingsPatch::default(),
                Some(&edit),
                &usage,
                now,
                None,
            )
            .unwrap();
        let grant = &saved.user_grants["user@example.com"];
        assert_eq!(grant.usage.anchored.as_ref().unwrap().tokens_used, 60);
        assert_eq!(
            grant.usage_rebase.as_ref().unwrap().window_starts_at_ms,
            Some(anchor)
        );
    }

    #[test]
    fn usage_rebase_is_rejected_for_share_market_managed_grants() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("usage-rebase-managed");
        add_manual_shareto(&mut input, "renter@example.com");
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["renter@example.com"].revision;
        store
            .shares
            .first_mut()
            .unwrap()
            .user_grants
            .get_mut("renter@example.com")
            .unwrap()
            .manager = ShareGrantManager::RouterShareMarket;

        let edit = BTreeMap::from([(
            "renter@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(1_000),
                expected_grant_revision: Some(revision),
                period: None,
                anchor_at_ms: None,
                source: crate::domain::sharing::router_contract::ShareUsageRebaseSource::Manual,
            },
        )]);
        let error = store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase-managed",
                ShareSettingsPatch::default(),
                Some(&edit),
                &UsageStore::default(),
                now,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            SharePatchError::ManagedGrantReadOnly(ref email) if email == "renter@example.com"
        ));
        // The rejection must not leave a partially applied baseline behind.
        assert!(
            store.get("usage-rebase-managed").unwrap().user_grants["renter@example.com"]
                .usage_rebase
                .is_none()
        );
    }

    #[test]
    fn usage_rebase_is_discarded_once_the_fixed_period_rolls_over() {
        let anchor = test_timestamp_ms(2026, 7, 24, 12, 0);
        let inside_window = test_timestamp_ms(2026, 7, 28, 12, 0);
        // One full seven-day period later: the baseline belongs to the
        // previous window and must not carry into the new one.
        let next_window = test_timestamp_ms(2026, 8, 1, 12, 0);
        let mut input = codex_share_input("usage-rebase-rollover");
        add_manual_shareto(&mut input, "user@example.com");
        let mut grants = input.user_grants.clone();
        grants.get_mut("user@example.com").unwrap().policy = ShareUserPolicy {
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(anchor),
            token_limit: Some(50_000),
            ..ShareUserPolicy::default()
        };
        input.user_grants = grants;
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["user@example.com"].revision;

        let mut log = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider-codex".to_string(),
            "Provider".to_string(),
            ProviderType::Codex,
            200,
            10,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(40),
                ..Default::default()
            },
        );
        log.request_id = "usage-rebase-rollover-1".to_string();
        log.share_id = Some("usage-rebase-rollover".to_string());
        log.user_email = Some("user@example.com".to_string());
        log.created_at_ms = (inside_window - 60_000) as u128;
        let mut usage = UsageStore::default();
        usage.push(log);

        let edit = BTreeMap::from([(
            "user@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(12_000),
                expected_grant_revision: Some(revision),
                period: Some(ShareTokenPeriod::SevenDays),
                anchor_at_ms: Some(anchor),
                source: crate::domain::sharing::router_contract::ShareUsageRebaseSource::Manual,
            },
        )]);
        let saved = store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase-rollover",
                ShareSettingsPatch::default(),
                Some(&edit),
                &usage,
                inside_window,
                None,
            )
            .unwrap();
        assert_eq!(
            saved.user_grants["user@example.com"]
                .usage
                .anchored
                .as_ref()
                .unwrap()
                .tokens_used,
            12_000
        );

        // Rebuilding inside the *next* window drops the baseline entirely: the
        // new window starts from the raw observation, which is zero here.
        store
            .rebuild_user_usage_from_history("usage-rebase-rollover", &usage, next_window)
            .unwrap();
        let grant = &store.get("usage-rebase-rollover").unwrap().user_grants["user@example.com"];
        assert!(
            grant.usage_rebase.is_none(),
            "a stale baseline must not survive a period rollover"
        );
        assert_eq!(grant.usage.anchored.as_ref().unwrap().tokens_used, 0);
        assert_eq!(
            grant.usage.anchored.as_ref().unwrap().started_at_ms,
            anchor + 7 * 24 * 60 * 60 * 1000
        );
    }

    #[test]
    fn share_total_usage_edit_sets_the_counter_and_reconciles_the_exhausted_state() {
        let now = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("share-total-usage");
        input.token_limit = Some(10_000);
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();

        // Drive the Share into `exhausted` the way ordinary traffic would.
        store.record_user_invocation_result("share-total-usage", None, 10_000, now);
        let share = store.get("share-total-usage").unwrap();
        assert_eq!(share.status, "exhausted");
        assert!(!share.enabled);

        // The operator reconciles against an upstream quota reset.  The Share
        // leaves `exhausted` but stays paused: resuming is a separate call.
        store
            .apply_share_total_usage_edit(
                "share-total-usage",
                &ShareTotalUsageEdit {
                    action: ShareUserUsageEditAction::Set,
                    tokens_used: Some(2_500),
                },
                now,
            )
            .unwrap();
        let share = store.get("share-total-usage").unwrap();
        assert_eq!(share.tokens_used, 2_500);
        assert_eq!(share.status, "paused");
        assert!(!share.enabled);

        // Later traffic accumulates on top of the operator value.
        store.record_user_invocation_result("share-total-usage", None, 500, now);
        assert_eq!(store.get("share-total-usage").unwrap().tokens_used, 3_000);

        // Setting at or above the limit re-exhausts the Share.
        store
            .apply_share_total_usage_edit(
                "share-total-usage",
                &ShareTotalUsageEdit {
                    action: ShareUserUsageEditAction::Set,
                    tokens_used: Some(10_000),
                },
                now,
            )
            .unwrap();
        assert_eq!(store.get("share-total-usage").unwrap().status, "exhausted");

        // `clear` is the zero shorthand and needs no `tokensUsed`.
        store
            .apply_share_total_usage_edit(
                "share-total-usage",
                &ShareTotalUsageEdit {
                    action: ShareUserUsageEditAction::Clear,
                    tokens_used: None,
                },
                now,
            )
            .unwrap();
        let share = store.get("share-total-usage").unwrap();
        assert_eq!(share.tokens_used, 0);
        assert_eq!(share.status, "paused");

        // `set` without a value is rejected rather than silently zeroing.
        let error = store
            .apply_share_total_usage_edit(
                "share-total-usage",
                &ShareTotalUsageEdit {
                    action: ShareUserUsageEditAction::Set,
                    tokens_used: None,
                },
                now,
            )
            .unwrap_err();
        assert!(matches!(error, SharePatchError::Invalid(_)));
    }

    #[test]
    fn reset_usage_clears_the_operator_baseline_so_the_reset_is_not_undone() {
        let anchor = test_timestamp_ms(2026, 7, 24, 12, 0);
        let inside_window = test_timestamp_ms(2026, 7, 28, 12, 0);
        let mut input = codex_share_input("usage-rebase-reset");
        add_manual_shareto(&mut input, "user@example.com");
        let mut grants = input.user_grants.clone();
        grants.get_mut("user@example.com").unwrap().policy = ShareUserPolicy {
            token_period: ShareTokenPeriod::SevenDays,
            token_period_anchor_at_ms: Some(anchor),
            token_limit: Some(50_000),
            ..ShareUserPolicy::default()
        };
        input.user_grants = grants;
        let mut store = ShareStore::default();
        let created = store.upsert(input).unwrap();
        let revision = created.user_grants["user@example.com"].revision;

        let usage = UsageStore::default();
        let edit = BTreeMap::from([(
            "user@example.com".to_string(),
            ShareUserUsageEdit {
                action: ShareUserUsageEditAction::Set,
                target_tokens: Some(12_000),
                expected_grant_revision: Some(revision),
                period: Some(ShareTokenPeriod::SevenDays),
                anchor_at_ms: Some(anchor),
                source: crate::domain::sharing::router_contract::ShareUsageRebaseSource::Manual,
            },
        )]);
        store
            .apply_settings_patch_with_usage_edits(
                "usage-rebase-reset",
                ShareSettingsPatch::default(),
                Some(&edit),
                &usage,
                inside_window,
                None,
            )
            .unwrap();

        let reset = store.reset_usage("usage-rebase-reset").unwrap();
        let grant = &reset.user_grants["user@example.com"];
        assert!(
            grant.usage_rebase.is_none(),
            "resetting usage must drop the operator baseline as well"
        );
        assert!(grant.usage.anchored.is_none());

        // The decisive part: a later rebuild inside the same window must not
        // resurrect the baseline the operator just cleared.
        store
            .rebuild_user_usage_from_history("usage-rebase-reset", &usage, inside_window)
            .unwrap();
        let grant = &store.get("usage-rebase-reset").unwrap().user_grants["user@example.com"];
        assert!(grant.usage_rebase.is_none());
        assert_eq!(grant.usage.anchored.as_ref().unwrap().tokens_used, 0);
    }

    #[test]
    fn user_quota_isolated_from_other_users_and_total_quota_remains_authoritative() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("user-quota");
        input.token_limit = Some(100);
        add_manual_shareto(&mut input, "alice@example.com");
        add_manual_shareto(&mut input, "bob@example.com");
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        store
            .shares
            .first_mut()
            .unwrap()
            .user_grants
            .get_mut("alice@example.com")
            .unwrap()
            .policy = ShareUserPolicy {
            token_limit: Some(5),
            token_period: ShareTokenPeriod::Day,
            ..ShareUserPolicy::default()
        };

        store.record_user_invocation_result("user-quota", Some("alice@example.com"), 5, now);
        assert_eq!(
            store
                .validate_for_invocation(
                    "user-quota",
                    AppKind::Codex,
                    Some("alice@example.com"),
                    now,
                )
                .unwrap_err()
                .reason,
            ShareRejectReason::UserExhausted
        );
        assert!(store
            .validate_for_invocation("user-quota", AppKind::Codex, Some("bob@example.com"), now,)
            .is_ok());
        assert_eq!(store.get("user-quota").unwrap().status, "active");

        let reset = store.reset_usage("user-quota").unwrap();
        assert_eq!(
            reset.user_grants["alice@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            0
        );
        assert!(store
            .validate_for_invocation("user-quota", AppKind::Codex, Some("alice@example.com"), now,)
            .is_ok());

        store.record_user_invocation_result("user-quota", Some("bob@example.com"), 95, now);
        store.record_user_invocation_result("user-quota", Some("alice@example.com"), 5, now);
        let rejection = store
            .validate_for_invocation("user-quota", AppKind::Codex, Some("bob@example.com"), now)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Inactive);
        assert_eq!(store.get("user-quota").unwrap().status, "exhausted");
    }

    #[test]
    fn invocation_acl_rejects_unknown_or_missing_user_identity() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut private_input = codex_share_input("private-acl");
        add_manual_shareto(&mut private_input, "allowed@example.com");
        let mut store = ShareStore::default();
        store.upsert(private_input).unwrap();

        let rejection = store
            .validate_for_invocation(
                "private-acl",
                AppKind::Codex,
                Some("unknown@example.com"),
                now,
            )
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Unauthorized);
        assert!(store
            .validate_for_invocation(
                "private-acl",
                AppKind::Codex,
                Some("ALLOWED@EXAMPLE.COM"),
                now,
            )
            .is_ok());
        let missing_identity = store
            .validate_for_invocation("private-acl", AppKind::Codex, None, now)
            .unwrap_err();
        assert_eq!(
            missing_identity.reason,
            ShareRejectReason::UserIdentityRequired
        );

        let mut free_input = codex_share_input("free-acl");
        free_input.provider_id = "p-free-acl".to_string();
        free_input.free_access = Some(true);
        store.upsert(free_input).unwrap();
        assert!(store
            .validate_for_invocation("free-acl", AppKind::Codex, Some("anyone@example.com"), now,)
            .is_ok());
        let missing_free_identity = store
            .validate_for_invocation("free-acl", AppKind::Codex, None, now)
            .unwrap_err();
        assert_eq!(
            missing_free_identity.reason,
            ShareRejectReason::UserIdentityRequired
        );

        let free_share = store.get("free-acl").expect("free share").clone();
        let mut limited_grant =
            new_user_grant(&free_share, "limited@example.com".to_string(), "shareto");
        limited_grant.policy.token_limit = Some(1);
        store
            .shares
            .iter_mut()
            .find(|share| share.id == "free-acl")
            .expect("mutable free share")
            .user_grants
            .insert("limited@example.com".to_string(), limited_grant);
        store.record_user_invocation_result("free-acl", Some("limited@example.com"), 1, now);
        assert_eq!(
            store
                .validate_for_invocation(
                    "free-acl",
                    AppKind::Codex,
                    Some("limited@example.com"),
                    now,
                )
                .unwrap_err()
                .reason,
            ShareRejectReason::UserExhausted
        );
        assert!(store
            .validate_for_invocation("free-acl", AppKind::Codex, Some("other@example.com"), now,)
            .is_ok());
    }

    #[test]
    fn supplemental_usage_counts_tokens_without_a_second_request() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("supplemental-usage");
        input.token_limit = Some(10);
        add_manual_shareto(&mut input, "alice@example.com");
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();

        store.record_user_supplemental_usage(
            "supplemental-usage",
            Some("alice@example.com"),
            4,
            now,
        );
        store.record_user_invocation_result(
            "supplemental-usage",
            Some("alice@example.com"),
            6,
            now,
        );

        let share = store.get("supplemental-usage").unwrap();
        assert_eq!(share.requests_count, 1);
        assert_eq!(share.tokens_used, 10);
        assert_eq!(share.status, "exhausted");
        let usage = &share.user_grants["alice@example.com"].usage;
        assert_eq!(usage.lifetime.requests_count, 1);
        assert_eq!(usage.day.requests_count, 1);
        assert_eq!(usage.week.requests_count, 1);
        assert_eq!(usage.calendar_month.requests_count, 1);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Lifetime, now), 10);
    }

    #[test]
    fn canonical_grants_revoke_and_restore_policy_history() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("market-user")).unwrap();
        let mut added_grants = original.user_grants.clone();
        added_grants.insert(
            "buyer@example.com".to_string(),
            new_user_grant(&original, "buyer@example.com".to_string(), "shareto"),
        );

        let added = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    user_grants: Some(added_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(added.user_grants["buyer@example.com"].active);
        store.record_user_invocation_result("market-user", Some("buyer@example.com"), 17, now);

        let mut revoked_grants = added.user_grants.clone();
        revoked_grants.remove("buyer@example.com");
        let revoked = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    user_grants: Some(revoked_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(!revoked.user_grants["buyer@example.com"].active);
        assert_eq!(
            revoked.user_grants["buyer@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            17
        );

        let restored_grants = revoked.user_grants.clone();
        let restored = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    user_grants: Some(restored_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(restored.user_grants["buyer@example.com"].active);
        assert_eq!(
            restored.user_grants["buyer@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            17
        );
    }

    #[test]
    fn invocation_completion_records_usage_after_grant_is_revoked() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("revoked-inflight-user");
        add_manual_shareto(&mut input, "user@example.com");
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        let mut user_grants = store
            .get("revoked-inflight-user")
            .unwrap()
            .user_grants
            .clone();
        user_grants.remove("user@example.com");
        store
            .apply_settings_patch(
                "revoked-inflight-user",
                ShareSettingsPatch {
                    user_grants: Some(user_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        store.record_user_invocation_result(
            "revoked-inflight-user",
            Some("user@example.com"),
            23,
            now,
        );

        let grant = &store.get("revoked-inflight-user").unwrap().user_grants["user@example.com"];
        assert!(!grant.active);
        assert_eq!(grant.usage.tokens_for(ShareTokenPeriod::Lifetime, now), 23);
    }

    #[test]
    fn explicit_user_grants_are_the_only_editable_shareto_source() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("explicit-market-user"))
            .unwrap();
        let owner_grant = share.user_grants["owner@example.com"].clone();
        let buyer_grant = new_user_grant(&share, "buyer@example.com".to_string(), "shareto");

        let updated = store
            .apply_settings_patch(
                "explicit-market-user",
                ShareSettingsPatch {
                    user_grants: Some(BTreeMap::from([
                        ("owner@example.com".to_string(), owner_grant),
                        ("buyer@example.com".to_string(), buyer_grant),
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(updated.user_grants["buyer@example.com"].active);
        assert!(!updated.user_grants.contains_key("stale@example.com"));
    }

    #[test]
    fn apply_settings_patch_persists_free_access() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("share-free"))
            .expect("upsert");
        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    free_access: Some(true),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply free");
        assert!(updated.free_access);
    }

    #[test]
    fn upsert_share_defaults_binding_and_status() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: Some("share".to_string()),
                enabled: None,
                status: None,
                subscription_level: Some("pro".to_string()),
                account_email: Some("owner@example.com".to_string()),
                quota_percent: None,
                tunnel_subdomain: None,
                token_limit: Some(1000),
                parallel_limit: Some(2),
                expires_at: None,
                free_access: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: Some(true),
                description: Some("test".to_string()),
                enabled_apps: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        assert_eq!(share.status, "active");
        assert!(share.enabled);
        assert_eq!(share.bindings.len(), 1);
        assert_eq!(share.bindings[0].provider_id, "p1");
        assert_eq!(share.token_limit, Some(1000));
        assert!(!share.free_access);
    }

    #[test]
    fn settings_patch_can_disable_one_app_api_but_not_all() {
        use crate::domain::sharing::router_contract::ShareSupport;

        let mut store = ShareStore::default();
        let mut input = codex_share_input("multi-app");
        input.bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
            },
            ShareBinding {
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
            },
        ];
        store.upsert(input).unwrap();

        let updated = store
            .apply_settings_patch(
                "multi-app",
                ShareSettingsPatch {
                    support: Some(ShareSupport {
                        claude: true,
                        codex: false,
                        gemini: false,
                    }),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            updated.enabled_apps.as_ref(),
            Some(&BTreeSet::from([AppKind::Claude]))
        );
        assert!(share_app_api_enabled(&updated, AppKind::Claude));
        assert!(!share_app_api_enabled(&updated, AppKind::Codex));

        let preserved = store
            .upsert({
                let mut input = codex_share_input("multi-app");
                input.bindings = vec![
                    ShareBinding {
                        app: AppKind::Claude,
                        provider_id: "p1".to_string(),
                        provider_type: ProviderType::Claude,
                    },
                    ShareBinding {
                        app: AppKind::Codex,
                        provider_id: "p1".to_string(),
                        provider_type: ProviderType::Codex,
                    },
                ];
                input
            })
            .unwrap();
        assert_eq!(
            preserved.enabled_apps.as_ref(),
            Some(&BTreeSet::from([AppKind::Claude]))
        );

        let rejected = store.apply_settings_patch(
            "multi-app",
            ShareSettingsPatch {
                support: Some(ShareSupport {
                    claude: false,
                    codex: false,
                    gemini: false,
                }),
                ..ShareSettingsPatch::default()
            },
        );
        assert!(rejected.is_err());
    }

    #[test]
    fn rejects_second_share_for_same_provider_instance() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("s1")).unwrap();
        let error = store.upsert(codex_share_input("s2")).unwrap_err();
        assert!(error.to_string().contains("provider already has share"));
    }

    #[test]
    fn allows_multiple_instances_of_same_provider_type() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("s1")).unwrap();
        let mut second = codex_share_input("s2");
        second.provider_id = "p2".to_string();
        let share = store.upsert(second).unwrap();
        assert_eq!(share.provider_type, ProviderType::Codex);
        assert_eq!(store.shares.len(), 2);
    }

    #[test]
    fn ordinary_upsert_refreshes_derived_capacity_pool() {
        let mut store = ShareStore::default();
        let original = store
            .upsert_with_capacity(
                codex_share_input("capacity-refresh"),
                Some("cp_old".to_string()),
            )
            .unwrap();

        let refreshed = store
            .upsert_with_capacity(
                codex_share_input("capacity-refresh"),
                Some("cp_new".to_string()),
            )
            .unwrap();

        assert_eq!(refreshed.capacity_pool_id, "cp_new");
        assert!(refreshed.config_revision > original.config_revision);
    }

    #[test]
    fn multi_app_binding_shares_configuration_and_removes_one_app() {
        let mut store = ShareStore::default();
        let original = store
            .upsert_with_capacity(
                codex_share_input("multi-app"),
                Some("cp_shared".to_string()),
            )
            .unwrap();
        let shared = store
            .add_binding(
                "multi-app",
                ShareBinding {
                    app: AppKind::Claude,
                    provider_id: "claude-provider".to_string(),
                    provider_type: ProviderType::Claude,
                },
            )
            .unwrap();

        assert_eq!(shared.capacity_pool_id, "cp_shared");
        assert_eq!(shared.bindings.len(), 2);
        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            &shared,
            &ProviderStore::default(),
        );
        assert_eq!(descriptor.bindings.len(), 2);

        let mut user_grants = shared.user_grants.clone();
        user_grants.insert(
            "user@example.com".to_string(),
            new_user_grant(&shared, "user@example.com".to_string(), "shareto"),
        );
        let updated = store
            .apply_settings_patch(
                "multi-app",
                ShareSettingsPatch {
                    user_grants: Some(user_grants),
                    token_limit: Some(42),
                    parallel_limit: Some(3),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(updated.user_grants["user@example.com"].active);
        assert_eq!(updated.token_limit, Some(42));
        assert_eq!(updated.parallel_limit, Some(3));
        assert!(store
            .validate_for_invocation(
                "multi-app",
                AppKind::Claude,
                Some("user@example.com"),
                1_000,
            )
            .is_ok());
        let rejection = store
            .validate_for_invocation("multi-app", AppKind::Gemini, None, 1_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::UnsupportedApp);

        let retained = store
            .remove_binding("multi-app", AppKind::Codex, &original.provider_id)
            .unwrap();
        assert_eq!(retained.app, AppKind::Claude);
        assert_eq!(retained.provider_id, "claude-provider");
        assert_eq!(retained.bindings.len(), 1);
        assert_eq!(store.shares.len(), 1);
        assert!(!retained
            .bindings
            .iter()
            .any(|binding| binding.app == AppKind::Codex));
    }

    #[test]
    fn validates_share_invocation_lifecycle_limits_and_counters() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("expired")).unwrap();
        store.shares[0].expires_at = Some(999);
        let rejection = store
            .validate_for_invocation("expired", AppKind::Codex, None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Expired);
        assert_eq!(
            rejection.formatted_message(),
            "Share has expired. Extend the share expiration or create a new share. [Expired]"
        );
        assert_eq!(store.shares[0].status, "expired");
        assert!(!store.shares[0].enabled);

        let mut paused = codex_share_input("paused");
        paused.provider_id = "p-paused".to_string();
        let _ = store.upsert(paused).unwrap();
        store.pause("paused").unwrap();
        let rejection = store
            .validate_for_invocation("paused", AppKind::Codex, None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Inactive);
        assert!(rejection.formatted_message().contains("[Inactive]"));

        let mut limited_input = codex_share_input("limited");
        limited_input.provider_id = "p-limited".to_string();
        let _ = store.upsert(limited_input).unwrap();
        {
            let limited = store
                .shares
                .iter_mut()
                .find(|share| share.id == "limited")
                .unwrap();
            limited.token_limit = Some(10);
            limited.tokens_used = 10;
        }
        let rejection = store
            .validate_for_invocation("limited", AppKind::Codex, None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Exhausted);
        assert_eq!(
            store
                .shares
                .iter()
                .find(|share| share.id == "limited")
                .unwrap()
                .status,
            "exhausted"
        );

        let mut record = codex_share_input("record");
        record.provider_id = "p-record".to_string();
        let _ = store.upsert(record).unwrap();
        store
            .shares
            .iter_mut()
            .find(|share| share.id == "record")
            .unwrap()
            .token_limit = Some(10);
        store.record_invocation_result("record", 4).unwrap();
        let recorded = store.record_invocation_result("record", 6).unwrap();
        assert_eq!(recorded.tokens_used, 10);
        assert_eq!(recorded.requests_count, 2);
        assert_eq!(recorded.status, "exhausted");

        let reset = store.reset_usage("record").unwrap();
        assert_eq!(reset.tokens_used, 0);
        assert_eq!(reset.requests_count, 0);
        assert_eq!(reset.status, "paused");
    }

    #[test]
    fn upsert_share_generates_default_slug_from_shared_generator() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("abc.def@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                free_access: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                enabled_apps: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let subdomain = share.tunnel_subdomain.unwrap();
        assert!(crate::domain::router::ShareSlug::parse(&subdomain).is_ok());
    }

    #[test]
    fn updates_binding_only_when_paused() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                free_access: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                enabled_apps: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let error = store
            .update_binding(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "p2".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
            )
            .unwrap_err();
        assert_eq!(error, ShareUpdateError::MustBePaused);

        store.pause("s1").unwrap();
        let share = store
            .update_binding_with_capacity(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "p2".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
                "cp-rebound".to_string(),
            )
            .unwrap();

        assert_eq!(share.provider_id, "p2");
        assert_eq!(share.provider_type, ProviderType::OpenRouter);
        assert_eq!(share.capacity_pool_id, "cp-rebound");
        assert_eq!(share.binding_history.len(), 1);
    }

    #[test]
    fn updating_binding_rejects_provider_used_as_another_shares_secondary_app() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("s1")).unwrap();
        store.pause("s1").unwrap();

        let mut multi = codex_share_input("s2");
        multi.app = AppKind::Claude;
        multi.provider_id = "claude-p1".to_string();
        multi.provider_type = ProviderType::Claude;
        multi.bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "claude-p1".to_string(),
                provider_type: ProviderType::Claude,
            },
            ShareBinding {
                app: AppKind::Codex,
                provider_id: "codex-secondary".to_string(),
                provider_type: ProviderType::OpenRouter,
            },
        ];
        store.upsert(multi).unwrap();

        let error = store
            .update_binding(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "codex-secondary".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
            )
            .unwrap_err();

        assert_eq!(error, ShareUpdateError::ProviderAlreadyShared);
        assert_eq!(store.get("s1").unwrap().provider_id, "p1");
    }

    #[test]
    fn ordinary_upsert_and_import_cannot_change_binding() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("s1")).unwrap();

        let mut rebound = codex_share_input("s1");
        rebound.provider_id = "p2".to_string();
        let error = store.upsert(rebound).unwrap_err();
        assert_eq!(error, SharePatchError::BindingImmutable);
        assert_eq!(store.get("s1").unwrap().provider_id, "p1");

        let mut metadata_update = original.clone();
        metadata_update.description = Some("must remain atomic".to_string());
        let mut imported_rebind = original;
        imported_rebind.provider_id = "p2".to_string();
        imported_rebind.bindings[0].provider_id = "p2".to_string();
        let error = store
            .import_shares(vec![metadata_update, imported_rebind])
            .unwrap_err();
        assert_eq!(error, SharePatchError::BindingImmutable);
        assert_eq!(store.get("s1").unwrap().provider_id, "p1");
        assert!(store.get("s1").unwrap().description.is_none());
    }

    #[test]
    fn stale_full_share_replacement_cannot_restore_an_old_binding() {
        let mut store = ShareStore::default();
        let mut stale = store.upsert(codex_share_input("s1")).unwrap();
        stale.description = Some("stale settings update".to_string());

        store.pause("s1").unwrap();
        store
            .update_binding(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "p2".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
            )
            .unwrap();

        let error = store.replace_configured_share(stale).unwrap_err();
        assert_eq!(error, SharePatchError::BindingImmutable);
        let current = store.get("s1").unwrap();
        assert_eq!(current.provider_id, "p2");
        assert_eq!(current.provider_type, ProviderType::OpenRouter);
        assert!(current.description.is_none());
    }

    #[test]
    fn imports_canonical_user_grants() {
        let mut store = ShareStore::default();
        let share = Share {
            id: "s1".to_string(),
            capacity_pool_id: "cp-s1".to_string(),
            owner_email: None,
            app: AppKind::Claude,
            provider_id: "p1".to_string(),
            provider_type: ProviderType::Claude,
            display_name: None,
            enabled: true,
            status: "active".to_string(),
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
            policy: SharePolicy::default(),
            tokens_used: 0,
            requests_count: 0,
            created_at_ms: 0,
            auto_start: false,
            description: None,
            enabled_apps: None,
            bindings: vec![ShareBinding {
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
            }],
            binding_history: Vec::new(),
            runtime_snapshot: None,
            last_error: None,
            integrity_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
            config_revision: 0,
            router_synced_revision: 0,
            descriptor_generation: 0,
            descriptor_fingerprint: None,
            router_synced_descriptor_generation: 0,
            router_synced_descriptor_fingerprint: None,
            user_grants: BTreeMap::from([(
                "user@example.com".to_string(),
                ShareUserGrant {
                    email: "user@example.com".to_string(),
                    role: "shareto".to_string(),
                    active: true,
                    ..ShareUserGrant::default()
                },
            )]),
        };
        assert_eq!(store.import_shares(vec![share]).unwrap(), 1);
        assert!(store.get("s1").unwrap().user_grants["user@example.com"].active);
    }

    #[test]
    fn mark_router_sync_records_success_and_failure_details() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                free_access: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                enabled_apps: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let revision = store.get("s1").unwrap().config_revision;
        store.mark_router_sync(
            "s1",
            revision,
            Some("https://router.example".to_string()),
            Ok(123),
        );
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_url.as_deref(), Some("https://router.example"));
        assert_eq!(share.router_last_sync_error, None);

        store.mark_router_sync(
            "s1",
            revision,
            Some("https://router.example".to_string()),
            Err("failed".to_string()),
        );
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_last_sync_error, None);

        let newer = store.pause("s1").unwrap();
        store.mark_router_sync(
            "s1",
            newer.config_revision,
            Some("https://router.example".to_string()),
            Err("failed".to_string()),
        );
        assert_eq!(
            store.get("s1").unwrap().router_last_sync_error.as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn descriptor_projection_generation_and_ack_are_monotonic() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("projection-order")).unwrap();

        let (first_generation, first_fingerprint) = store
            .prepare_descriptor_projection("projection-order", "a".repeat(64))
            .unwrap();
        assert_eq!(first_generation, 1);
        assert!(store.mark_router_descriptor_sync(
            "projection-order",
            first_generation,
            &first_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(100),
        ));

        let (second_generation, second_fingerprint) = store
            .prepare_descriptor_projection("projection-order", "b".repeat(64))
            .unwrap();
        assert_eq!(second_generation, 2);
        assert!(!store.mark_router_descriptor_sync(
            "projection-order",
            first_generation,
            &first_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(200),
        ));
        let share = store.get("projection-order").unwrap();
        assert_eq!(share.router_synced_descriptor_generation, first_generation);
        assert!(store.descriptor_projection_pending(share));

        assert!(store.mark_router_descriptor_sync(
            "projection-order",
            second_generation,
            &second_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(300),
        ));
        assert!(!store.descriptor_projection_pending(store.get("projection-order").unwrap()));

        store
            .shares
            .iter_mut()
            .find(|share| share.id == "projection-order")
            .expect("projection share")
            .config_revision = 2;
        assert!(
            store.descriptor_projection_pending(store.get("projection-order").unwrap()),
            "a later config revision must still push to Router even when the static fingerprint is unchanged"
        );
        let (third_generation, third_fingerprint) = store
            .prepare_descriptor_projection("projection-order", second_fingerprint.clone())
            .unwrap();
        assert_eq!(third_generation, 3);
        assert_eq!(third_fingerprint, second_fingerprint);
    }

    #[test]
    fn runtime_snapshot_includes_model_health_summary() {
        let mut store = ShareStore::default();
        let share = store.upsert(codex_share_input("s1")).unwrap();
        let providers = ProviderStore::default();
        let usage = UsageStore::default();

        let snapshot = runtime_snapshot_for_share(&share, &providers, None, &usage);

        assert!(snapshot.pointer("/modelHealth/codex").is_some());
        assert_eq!(
            snapshot
                .pointer("/modelHealth/codex")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn canonical_user_grants_add_and_revoke_target_user() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("s1")).unwrap();
        let mut added_grants = original.user_grants.clone();
        added_grants.insert(
            "buyer@example.com".to_string(),
            new_user_grant(&original, "buyer@example.com".to_string(), "shareto"),
        );

        let added = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    user_grants: Some(added_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(added.user_grants["buyer@example.com"].active);

        let mut revoked_grants = added.user_grants.clone();
        revoked_grants.remove("buyer@example.com");
        let revoked = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    user_grants: Some(revoked_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(!revoked.user_grants["buyer@example.com"].active);
    }

    #[test]
    fn sequential_canonical_grant_patches_are_deterministic() {
        let mut store = ShareStore::default();
        let original = store.upsert(codex_share_input("s1")).unwrap();
        let mut first_grants = original.user_grants.clone();
        first_grants.insert(
            "buyer-a@example.com".to_string(),
            new_user_grant(&original, "buyer-a@example.com".to_string(), "shareto"),
        );

        let first = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    user_grants: Some(first_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let mut final_grants = first.user_grants.clone();
        final_grants.remove("buyer-a@example.com");
        final_grants.insert(
            "buyer-b@example.com".to_string(),
            new_user_grant(&first, "buyer-b@example.com".to_string(), "shareto"),
        );
        let final_share = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    user_grants: Some(final_grants),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(!final_share.user_grants["buyer-a@example.com"].active);
        assert!(final_share.user_grants["buyer-b@example.com"].active);
    }

    #[test]
    fn bind_owner_reassigns_canonical_owner_grant() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("s1");
        add_manual_shareto(&mut input, "buyer@example.com");
        store.upsert(input).unwrap();

        let updated = store
            .bind_all_to_client_owner("new-owner@example.com")
            .unwrap();
        let updated = &updated[0];
        assert_eq!(
            updated.owner_email.as_deref(),
            Some("new-owner@example.com")
        );
        assert!(updated.user_grants["buyer@example.com"].active);
        assert_eq!(updated.user_grants["new-owner@example.com"].role, "owner");
        assert_eq!(updated.user_grants["owner@example.com"].role, "shareto");
    }

    #[test]
    fn bind_all_to_client_owner_preserves_previous_owner_access_and_is_idempotent() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("owner-bind");
        input.owner_email = Some("previous@example.com".to_string());
        add_manual_shareto(&mut input, "buyer@example.com");
        store.upsert(input).unwrap();

        let updated = store
            .bind_all_to_client_owner("client@example.com")
            .unwrap();
        assert_eq!(updated.len(), 1);
        let share = store.get("owner-bind").unwrap();
        assert_eq!(share.owner_email.as_deref(), Some("client@example.com"));
        assert!(share.user_grants["previous@example.com"].active);
        assert!(share.user_grants["buyer@example.com"].active);
        assert_eq!(
            share
                .user_grants
                .values()
                .filter(|grant| grant.role == "owner")
                .count(),
            1
        );
        assert_eq!(share.user_grants["client@example.com"].role, "owner");
        let previous_owner = &share.user_grants["previous@example.com"];
        assert_eq!(previous_owner.role, "shareto");
        assert!(previous_owner.active);
        let revision = share.config_revision;
        assert!(store
            .bind_all_to_client_owner("client@example.com")
            .unwrap()
            .is_empty());
        assert!(store.normalize_all_user_grants().is_empty());
        assert_eq!(store.get("owner-bind").unwrap().config_revision, revision);
    }

    #[test]
    fn canonical_normalization_repairs_stale_owner_role_without_rebinding_owner() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("stale-owner-role");
        input.owner_email = Some("previous@example.com".to_string());
        store.upsert(input).unwrap();

        let share = store.shares.first_mut().unwrap();
        share.owner_email = Some("client@example.com".to_string());
        let previous_revision = share.config_revision;

        let migrated = store.normalize_all_user_grants();

        assert_eq!(migrated.len(), 1);
        let share = store.get("stale-owner-role").unwrap();
        assert_eq!(share.config_revision, previous_revision + 1);
        assert_eq!(
            share
                .user_grants
                .values()
                .filter(|grant| grant.role == "owner")
                .count(),
            1
        );
        assert_eq!(share.user_grants["client@example.com"].role, "owner");
        assert_eq!(share.user_grants["previous@example.com"].role, "shareto");
        assert!(share.user_grants["previous@example.com"].active);
        assert!(store.normalize_all_user_grants().is_empty());
    }

    #[test]
    fn bind_all_to_client_owner_discards_invalid_previous_owner() {
        let mut store = ShareStore::default();
        store
            .upsert(codex_share_input("invalid-owner-bind"))
            .unwrap();
        store.shares[0].owner_email = Some("invalid-owner".to_string());
        store
            .bind_all_to_client_owner("client@example.com")
            .unwrap();
        let share = store.get("invalid-owner-bind").unwrap();
        assert_eq!(share.owner_email.as_deref(), Some("client@example.com"));
        assert!(!share.user_grants.contains_key("invalid-owner"));
    }

    #[test]
    fn bind_owner_updates_all_shares() {
        let mut store = ShareStore::default();
        let mut first = codex_share_input("s1");
        add_manual_shareto(&mut first, "new-owner@example.com");
        add_manual_shareto(&mut first, "buyer@example.com");
        let _ = store.upsert(first).unwrap();
        let mut second = codex_share_input("s2");
        second.provider_id = "p2".to_string();
        let _ = store.upsert(second).unwrap();
        let mut other = codex_share_input("s3");
        other.provider_id = "p3".to_string();
        other.owner_email = Some("other@example.com".to_string());
        let _ = store.upsert(other).unwrap();

        let updated = store
            .bind_all_to_client_owner("New-Owner@Example.com")
            .unwrap();

        assert_eq!(updated.len(), 3);
        assert_eq!(
            store
                .shares
                .iter()
                .filter(|share| share.owner_email.as_deref() == Some("new-owner@example.com"))
                .count(),
            3
        );
        assert_eq!(
            store
                .shares
                .iter()
                .find(|share| share.id == "s3")
                .and_then(|share| share.owner_email.as_deref()),
            Some("new-owner@example.com")
        );
        let first = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(first.user_grants["new-owner@example.com"].role, "owner");
        assert_eq!(first.user_grants["owner@example.com"].role, "shareto");
        assert!(first.user_grants["buyer@example.com"].active);
    }

    #[test]
    fn binding_owner_demotes_previous_owner() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("s1");
        add_manual_shareto(&mut input, "buyer@example.com");
        store.upsert(input).unwrap();

        let updated = store.bind_all_to_client_owner("buyer@example.com").unwrap();
        let updated = &updated[0];
        assert_eq!(updated.owner_email.as_deref(), Some("buyer@example.com"));
        assert_eq!(updated.user_grants["buyer@example.com"].role, "owner");
        assert_eq!(updated.user_grants["owner@example.com"].role, "shareto");
    }
}
