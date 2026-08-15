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
    descriptor_for_share_with_accounts_and_usage, ShareAnchoredUsageBucket, ShareAppAccess,
    ShareAppSettings, ShareGrantManager, ShareManagedGrantAction, ShareManagedGrantOperation,
    ShareSettingsPatch, ShareTokenPeriod, ShareUserGrant, ShareUserPolicy, ShareUserUsage,
    ShareUserUsageBucket,
};
use crate::domain::sharing::token_period::{token_period_window, validate_user_policy};
use crate::domain::usage::store::UsageStore;
use crate::infra::time::now_ms;

const SHARES_FILE_NAME: &str = "shares.json";
pub const DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 60;
pub const MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 10;
pub const MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 7 * 24 * 60;

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
    pub acl: ShareAcl,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub for_sale: bool,
    #[serde(default)]
    pub free_access: bool,
    #[serde(default)]
    pub official_price_percent: Option<u16>,
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
            acl: ShareAcl::default(),
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            for_sale: false,
            free_access: false,
            official_price_percent: None,
            allow_personal_credits: false,
            auto_consume_banked_reset: false,
            banked_reset_expiry_lead_minutes: DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES,
            previous_response_cache_enabled: false,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAcl {
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default)]
    pub public_market_email: Option<String>,
    #[serde(default)]
    pub market_access_mode: Option<String>,
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
#[serde(rename_all = "camelCase")]
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
    pub acl: Option<ShareAcl>,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub for_sale: Option<bool>,
    #[serde(default)]
    pub free_access: Option<bool>,
    #[serde(default)]
    pub access_by_app: BTreeMap<AppKind, ShareAppAccess>,
    #[serde(default)]
    pub app_settings: BTreeMap<AppKind, ShareAppSettings>,
    #[serde(default)]
    pub for_sale_official_price_percent_by_app: BTreeMap<AppKind, u16>,
    #[serde(default)]
    pub official_price_percent: Option<u16>,
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
        serde_json::from_str(&content).with_context(|| format!("parse shares {}", path.display()))
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
                acl: input.acl.unwrap_or_default(),
                token_limit: input.token_limit,
                parallel_limit: input.parallel_limit,
                expires_at: input.expires_at,
                for_sale: input.for_sale.unwrap_or(false),
                free_access: input.free_access.unwrap_or(false),
                official_price_percent: input.official_price_percent.or_else(|| {
                    input
                        .for_sale_official_price_percent_by_app
                        .values()
                        .next()
                        .copied()
                }),
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
        reconcile_user_grants(&mut share, explicit_user_grants.as_ref());
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
        if !share.free_access && normalized_user_email.is_none() {
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
                .is_some_and(|limit| grant.usage.tokens_for_policy(&grant.policy, now_ms) >= limit)
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
                if count_request {
                    grant
                        .usage
                        .record_for_policy(&policy, tokens, recorded_at_ms);
                } else {
                    grant
                        .usage
                        .record_supplemental_for_policy(&policy, tokens, recorded_at_ms);
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
        enabled: bool,
        usage: &UsageStore,
        applied_at_ms: i64,
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
        candidate.apply_settings_patch_with_usage(share_id, settings, usage, applied_at_ms)?;
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

    pub fn replace_acl(&mut self, share_id: &str, acl: ShareAcl) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.acl = acl;
        mark_share_config_pending(share);
        Some(share.clone())
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
            if bind_share_to_client_owner(share, &owner_email) {
                reconcile_user_grants(share, None);
                mark_share_config_pending(share);
                updated.push(share.clone());
            }
        }
        Ok(updated)
    }

    pub fn migrate_user_grants_from_acl(&mut self) -> Vec<Share> {
        let mut updated = Vec::new();
        for share in &mut self.shares {
            let previous = share.user_grants.clone();
            reconcile_user_grants(share, None);
            if share.user_grants != previous {
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
        mut patch: ShareSettingsPatch,
    ) -> Result<Share, SharePatchError> {
        let index = self
            .shares
            .iter()
            .position(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let mut share = self.shares[index].clone();
        let bound_apps = share
            .bindings
            .iter()
            .map(|binding| binding.app)
            .collect::<BTreeSet<_>>();
        normalize_projected_share_policy_patch(
            &mut patch,
            self.shares[index].owner_email.as_deref(),
            &bound_apps,
        )?;
        let pricing_was_explicit = patch.official_price_percent.is_some();
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
        if let Some(for_sale) = patch.for_sale {
            apply_router_for_sale_patch(&mut share, &for_sale);
        }
        if let Some(market_access_mode) = patch.market_access_mode {
            share.acl.market_access_mode =
                Some(normalize_non_empty(market_access_mode, "selected"));
        }
        if let Some(shared_with_emails) = patch.shared_with_emails {
            share.acl.shared_with_emails =
                normalize_email_list(&shared_with_emails, share.owner_email.as_deref());
        }
        if let Some(official_price_percent) = patch.official_price_percent {
            share.official_price_percent = official_price_percent;
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

        let pricing_eligible = share.for_sale && !share.free_access;
        if !pricing_eligible {
            if pricing_was_explicit && share.official_price_percent.is_some() {
                return Err(SharePatchError::Invalid(
                    "share official price percent requires forSale=Yes".to_string(),
                ));
            }
            share.official_price_percent = None;
        }
        crate::domain::sharing::invariants::validate_share_import(&share)?;

        reconcile_user_grants(&mut share, explicit_user_grants.as_ref());
        if let Some(operation) = managed_grant.as_ref() {
            apply_managed_grant_operation(&mut share, operation)?;
        }
        crate::domain::sharing::invariants::validate_share_import(&share)?;
        mark_share_config_pending(&mut share);
        self.shares[index] = share.clone();
        if let Some(operation) = managed_grant {
            self.applied_router_control_operations.insert(
                operation.operation_id,
                AppliedRouterControlOperation {
                    applied_at_ms: now_ms(),
                    fingerprint: managed_grant_fingerprint
                        .expect("managed operation fingerprint is populated after validation"),
                },
            );
            while self.applied_router_control_operations.len() > 2_048 {
                let Some(oldest) = self
                    .applied_router_control_operations
                    .iter()
                    .min_by_key(|(_, operation)| operation.applied_at_ms)
                    .map(|(operation_id, _)| operation_id.clone())
                else {
                    break;
                };
                self.applied_router_control_operations.remove(&oldest);
            }
        }

        Ok(share)
    }

    pub fn apply_settings_patch_with_usage(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
        usage: &UsageStore,
        applied_at_ms: i64,
    ) -> Result<Share, SharePatchError> {
        let mut candidate = self.clone();
        candidate.apply_settings_patch(share_id, patch)?;
        candidate.rebuild_user_anchored_usage(share_id, usage, applied_at_ms)?;
        let share = candidate
            .get(share_id)
            .cloned()
            .ok_or(SharePatchError::NotFound)?;
        *self = candidate;
        Ok(share)
    }

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
                grant
                    .usage
                    .rebuild_anchored(&grant.policy, now_ms, 0, 0)
                    .map_err(SharePatchError::Invalid)?;
                continue;
            }
            let window =
                token_period_window(&grant.policy, now_ms).map_err(SharePatchError::Invalid)?;
            let start = window
                .starts_at_ms
                .ok_or_else(|| SharePatchError::Invalid("fixed period has no start".into()))?;
            let end = window
                .ends_at_ms
                .ok_or_else(|| SharePatchError::Invalid("fixed period has no end".into()))?;
            let normalized_email = grant.email.trim().to_ascii_lowercase();
            let (tokens_used, requests_count) =
                usage.share_user_quota_usage(share_id, &normalized_email, start, end);
            grant
                .usage
                .rebuild_anchored(&grant.policy, now_ms, tokens_used, requests_count)
                .map_err(SharePatchError::Invalid)?;
        }
        Ok(())
    }

    pub fn canonicalize_primary_app_settings(
        &mut self,
        share_id: &str,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        Ok(share.clone())
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
        if changed || share.descriptor_generation == 0 {
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
        insert_email(&mut share.acl.shared_with_emails, previous_owner.clone());
    }
    share.owner_email = Some(owner_email.to_string());
    share.acl.shared_with_emails =
        normalize_email_list(&share.acl.shared_with_emails, Some(owner_email));
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
    RevisionConflict { expected: u64, current: u64 },
    Invalid(String),
}

impl SharePatchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "cc_switch_share_not_found",
            Self::BindingImmutable => "cc_switch_share_binding_immutable",
            Self::PolicyDivergent(_) => "cc_switch_share_policy_divergent",
            Self::RevisionConflict { .. } => "cc_switch_share_revision_conflict",
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

pub(crate) fn share_router_for_sale_label(share: &Share) -> String {
    if share.free_access {
        "Free".to_string()
    } else if share.for_sale {
        "Yes".to_string()
    } else {
        "No".to_string()
    }
}

pub(crate) fn apply_router_for_sale_patch(share: &mut Share, value: &str) {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => {
            share.free_access = true;
            share.for_sale = false;
        }
        "yes" | "true" | "1" | "share" => {
            share.free_access = false;
            share.for_sale = true;
        }
        _ => {
            share.free_access = false;
            share.for_sale = false;
        }
    }
}

pub(crate) fn normalize_router_for_sale_setting(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => "Free".to_string(),
        "yes" | "true" | "1" | "share" => "Yes".to_string(),
        _ => "No".to_string(),
    }
}

fn normalize_non_empty(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_ascii_lowercase()
    }
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
        ShareManagedGrantAction::Upsert if operation.policy.is_none() => Err(
            SharePatchError::Invalid("managed grant upsert requires policy".to_string()),
        ),
        ShareManagedGrantAction::Revoke if operation.policy.is_some() => Err(
            SharePatchError::Invalid("managed grant revoke must not include policy".to_string()),
        ),
        _ => Ok(()),
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
            let policy = operation.policy.clone().ok_or_else(|| {
                SharePatchError::Invalid("managed grant upsert requires policy".to_string())
            })?;
            if policy.parallel_limit == Some(0) || policy.token_limit == Some(0) {
                return Err(SharePatchError::Invalid(
                    "user limits must be positive or unlimited".to_string(),
                ));
            }
            validate_user_policy(&policy, now_ms() as i64).map_err(SharePatchError::Invalid)?;
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

            let now = now_ms();
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
                    created_at_ms: previous
                        .as_ref()
                        .map(|grant| grant.created_at_ms)
                        .filter(|created_at| *created_at > 0)
                        .unwrap_or(now),
                    updated_at_ms: now,
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
            add_grant_email_to_acl(share, &email);
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
            remove_grant_email_from_acl(share, &grant_email);
        }
    }
    Ok(())
}

fn add_grant_email_to_acl(share: &mut Share, email: &str) {
    insert_email(&mut share.acl.shared_with_emails, email.to_string());
}

fn remove_grant_email_from_acl(share: &mut Share, email: &str) {
    share
        .acl
        .shared_with_emails
        .retain(|value| !value.eq_ignore_ascii_case(email));
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

fn reconcile_user_grants(
    share: &mut Share,
    explicit_user_grants: Option<&BTreeMap<String, ShareUserGrant>>,
) {
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
                grant.updated_at_ms = now;
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
    }

    if explicit_user_grants.is_none() {
        let desired_emails = share_acl_emails(share);
        let now = now_ms();
        for email in &desired_emails {
            if let Some(grant) = share.user_grants.get_mut(email) {
                if grant.manager == ShareGrantManager::RouterShareMarket {
                    continue;
                }
                if grant.role == "shareto" && !grant.active {
                    grant.active = true;
                    grant.revoked_at_ms = None;
                    grant.updated_at_ms = now;
                    grant.revision = grant.revision.saturating_add(1).max(1);
                }
            } else {
                share.user_grants.insert(
                    email.clone(),
                    new_user_grant(share, email.clone(), "shareto"),
                );
            }
        }
        for grant in share.user_grants.values_mut().filter(|grant| {
            grant.role == "shareto"
                && grant.active
                && grant.manager != ShareGrantManager::RouterShareMarket
        }) {
            if !desired_emails.contains(&grant.email) {
                grant.active = false;
                grant.revoked_at_ms = Some(now);
                grant.updated_at_ms = now;
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
        let managed_grants = share
            .user_grants
            .values()
            .filter(|grant| grant.manager == ShareGrantManager::RouterShareMarket)
            .map(|grant| (grant.email.clone(), grant.active))
            .collect::<Vec<_>>();
        for (email, active) in managed_grants {
            if active {
                add_grant_email_to_acl(share, &email);
            } else {
                remove_grant_email_from_acl(share, &email);
            }
        }
    } else {
        let previous_direct = share
            .user_grants
            .values()
            .filter(|grant| grant.role == "shareto")
            .map(|grant| grant.email.clone())
            .collect::<BTreeSet<_>>();
        share
            .acl
            .shared_with_emails
            .retain(|email| !previous_direct.contains(&email.trim().to_ascii_lowercase()));
        let active_grant_emails = share
            .user_grants
            .values()
            .filter(|grant| grant.active && grant.role == "shareto")
            .map(|grant| grant.email.clone())
            .collect::<Vec<_>>();
        for email in active_grant_emails {
            insert_email(&mut share.acl.shared_with_emails, email);
        }
        let desired_emails = share_acl_emails(share);
        for email in desired_emails {
            if share
                .user_grants
                .get(&email)
                .is_some_and(|grant| grant.manager == ShareGrantManager::RouterShareMarket)
            {
                continue;
            }
            if share
                .user_grants
                .get(&email)
                .is_some_and(|grant| grant.active)
            {
                continue;
            }
            if let Some(grant) = share.user_grants.get_mut(&email) {
                grant.active = true;
                grant.revoked_at_ms = None;
                grant.updated_at_ms = now_ms();
                grant.revision = grant.revision.saturating_add(1).max(1);
            } else {
                share
                    .user_grants
                    .insert(email.clone(), new_user_grant(share, email, "shareto"));
            }
        }
    }
}

fn share_acl_emails(share: &Share) -> BTreeSet<String> {
    let owner = share
        .owner_email
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    share
        .acl
        .shared_with_emails
        .iter()
        .map(|email| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty() && owner.as_deref() != Some(email.as_str()))
        .collect()
}

pub fn normalize_share_subdomain(subdomain: &str) -> Result<String, &'static str> {
    let value = subdomain.trim().to_ascii_lowercase();
    crate::domain::router::ShareSlug::parse(&value)
        .map_err(|_| "share slug must be 6-30 lowercase DNS characters without '--'")?;
    Ok(value)
}

fn insert_email(emails: &mut Vec<String>, email: String) {
    let Some(email) = normalize_optional_email(Some(email)) else {
        return;
    };
    if !emails.iter().any(|item| item.eq_ignore_ascii_case(&email)) {
        emails.push(email);
    }
}

fn normalize_email_list(values: &[String], owner_email: Option<&str>) -> Vec<String> {
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    let mut seen = BTreeSet::new();
    let mut emails = Vec::new();
    for value in values {
        let email = value.trim().to_ascii_lowercase();
        if email.is_empty() || owner.as_deref() == Some(email.as_str()) {
            continue;
        }
        if seen.insert(email.clone()) {
            emails.push(email);
        }
    }
    emails
}

fn normalize_access_by_app(
    access_by_app: BTreeMap<AppKind, ShareAppAccess>,
    owner_email: Option<&str>,
) -> BTreeMap<AppKind, ShareAppAccess> {
    let mut normalized = BTreeMap::new();
    for (app, mut access) in access_by_app {
        access.market_access_mode = normalize_non_empty(access.market_access_mode, "selected");
        access.shared_with_emails = normalize_email_list(&access.shared_with_emails, owner_email);
        normalized.insert(app, access);
    }
    normalized
}

fn normalize_app_settings(
    app_settings: BTreeMap<AppKind, ShareAppSettings>,
    owner_email: Option<&str>,
) -> BTreeMap<AppKind, ShareAppSettings> {
    let mut normalized = BTreeMap::new();
    for (app, mut setting) in app_settings {
        setting.for_sale = normalize_router_for_sale_setting(&setting.for_sale);
        setting.market_access_mode = normalize_non_empty(setting.market_access_mode, "selected");
        setting.shared_with_emails = normalize_email_list(&setting.shared_with_emails, owner_email);
        normalized.insert(app, setting);
    }
    normalized
}

fn normalize_projected_share_policy_patch(
    patch: &mut ShareSettingsPatch,
    owner_email: Option<&str>,
    bound_apps: &BTreeSet<AppKind>,
) -> Result<(), SharePatchError> {
    if let Some(access_by_app) = patch.access_by_app.take() {
        ensure_projected_apps_are_bound(&access_by_app, bound_apps, "accessByApp")?;
        let normalized = normalize_access_by_app(access_by_app, owner_email);
        let access = one_projected_value(normalized.values(), "accessByApp")?;
        if let Some(access) = access {
            let emails = normalize_email_list(&access.shared_with_emails, owner_email);
            if patch
                .shared_with_emails
                .as_ref()
                .is_some_and(|current| normalize_email_list(current, owner_email) != emails)
                || patch.market_access_mode.as_ref().is_some_and(|current| {
                    normalize_non_empty(current.clone(), "selected") != access.market_access_mode
                })
            {
                return Err(SharePatchError::PolicyDivergent(
                    "accessByApp disagrees with the global Share ACL".to_string(),
                ));
            }
            patch.shared_with_emails.get_or_insert(emails);
            patch
                .market_access_mode
                .get_or_insert(access.market_access_mode.clone());
        }
    }

    if let Some(app_settings) = patch.app_settings.take() {
        ensure_projected_apps_are_bound(&app_settings, bound_apps, "appSettings")?;
        let normalized = normalize_app_settings(app_settings, owner_email);
        let settings = one_projected_value(normalized.values(), "appSettings")?;
        if let Some(settings) = settings {
            let emails = normalize_email_list(&settings.shared_with_emails, owner_email);
            let token_limit = settings.token_limit;
            let parallel_limit = settings.parallel_limit;
            let expires_at = parse_share_expiration(&settings.expires_at)?;
            if patch
                .shared_with_emails
                .as_ref()
                .is_some_and(|current| normalize_email_list(current, owner_email) != emails)
                || patch.market_access_mode.as_ref().is_some_and(|current| {
                    normalize_non_empty(current.clone(), "selected") != settings.market_access_mode
                })
                || patch.for_sale.as_ref().is_some_and(|current| {
                    normalize_router_for_sale_setting(current) != settings.for_sale
                })
                || patch
                    .token_limit
                    .is_some_and(|current| current != token_limit)
                || patch
                    .parallel_limit
                    .is_some_and(|current| current != parallel_limit)
                || patch
                    .expires_at
                    .as_ref()
                    .is_some_and(|current| parse_share_expiration(current).ok() != Some(expires_at))
            {
                return Err(SharePatchError::PolicyDivergent(
                    "appSettings disagrees with the global Share policy".to_string(),
                ));
            }
            patch.shared_with_emails.get_or_insert(emails);
            patch
                .market_access_mode
                .get_or_insert(settings.market_access_mode.clone());
            patch.for_sale.get_or_insert(settings.for_sale.clone());
            patch.token_limit.get_or_insert(token_limit);
            patch.parallel_limit.get_or_insert(parallel_limit);
            patch.expires_at.get_or_insert(settings.expires_at.clone());
        }
    }

    if let Some(pricing) = patch.for_sale_official_price_percent_by_app.take() {
        ensure_projected_apps_are_bound(&pricing, bound_apps, "forSaleOfficialPricePercentByApp")?;
        if pricing.values().any(|percent| !(1..=100).contains(percent)) {
            return Err(SharePatchError::Invalid(
                "share official price percent must be between 1 and 100".to_string(),
            ));
        }
        let price = one_projected_value(pricing.values(), "per-app pricing")?.copied();
        if patch
            .official_price_percent
            .is_some_and(|current| current != price)
        {
            return Err(SharePatchError::PolicyDivergent(
                "per-app pricing disagrees with the global Share price".to_string(),
            ));
        }
        patch.official_price_percent = Some(price);
    }
    Ok(())
}

fn ensure_projected_apps_are_bound<T>(
    values: &BTreeMap<AppKind, T>,
    bound_apps: &BTreeSet<AppKind>,
    field: &str,
) -> Result<(), SharePatchError> {
    if let Some(app) = values.keys().find(|app| !bound_apps.contains(app)) {
        return Err(SharePatchError::Invalid(format!(
            "{field} contains unbound app {}",
            app.as_str()
        )));
    }
    Ok(())
}

fn one_projected_value<'a, T: PartialEq>(
    mut values: impl Iterator<Item = &'a T>,
    field: &str,
) -> Result<Option<&'a T>, SharePatchError> {
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.any(|value| value != first) {
        return Err(SharePatchError::PolicyDivergent(format!(
            "Router {field} entries must describe one global Share policy"
        )));
    }
    Ok(Some(first))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sharing::router_contract::share_expires_at_rfc3339;

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
            }),
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
        assert!(updated
            .acl
            .shared_with_emails
            .contains(&"renter@example.com".to_string()));

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

        let acl_edit = store
            .apply_settings_patch(
                "managed-grant",
                ShareSettingsPatch {
                    shared_with_emails: Some(Vec::new()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(acl_edit.user_grants["renter@example.com"].active);
        assert!(acl_edit
            .acl
            .shared_with_emails
            .contains(&"renter@example.com".to_string()));

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
        assert!(!revoked
            .acl
            .shared_with_emails
            .contains(&"renter@example.com".to_string()));

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

        let acl_attempt = store
            .apply_settings_patch(
                "managed-revoke",
                ShareSettingsPatch {
                    shared_with_emails: Some(vec!["renter@example.com".to_string()]),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(!acl_attempt.user_grants["renter@example.com"].active);
        assert!(!acl_attempt
            .acl
            .shared_with_emails
            .contains(&"renter@example.com".to_string()));

        let mut reactivated = acl_attempt.user_grants.clone();
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
            acl: None,
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            for_sale: Some(true),
            free_access: None,
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            allow_personal_credits: None,
            auto_consume_banked_reset: None,
            banked_reset_expiry_lead_minutes: None,
            previous_response_cache_enabled: None,
            auto_start: None,
            description: None,
            bindings: Vec::new(),
            runtime_snapshot: None,
            user_grants: BTreeMap::new(),
        }
    }

    fn codex_app_settings(emails: Vec<&str>) -> BTreeMap<AppKind, ShareAppSettings> {
        let mut app_settings = BTreeMap::new();
        app_settings.insert(
            AppKind::Codex,
            ShareAppSettings {
                for_sale: "Yes".to_string(),
                market_access_mode: "selected".to_string(),
                shared_with_emails: emails.into_iter().map(str::to_string).collect(),
                token_limit: 5000,
                parallel_limit: 2,
                expires_at: "1893456000".to_string(),
            },
        );
        app_settings
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

    #[test]
    fn divergent_router_app_policies_are_rejected_atomically() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("divergent-policy")).unwrap();
        let original = store
            .add_binding(
                "divergent-policy",
                ShareBinding {
                    app: AppKind::Claude,
                    provider_id: "claude-provider".to_string(),
                    provider_type: ProviderType::Claude,
                },
            )
            .unwrap();
        let error = store
            .apply_settings_patch(
                &original.id,
                ShareSettingsPatch {
                    app_settings: Some(BTreeMap::from([
                        (
                            AppKind::Claude,
                            ShareAppSettings {
                                token_limit: 10,
                                ..codex_app_settings(Vec::new())[&AppKind::Codex].clone()
                            },
                        ),
                        (
                            AppKind::Codex,
                            ShareAppSettings {
                                token_limit: 20,
                                ..codex_app_settings(Vec::new())[&AppKind::Codex].clone()
                            },
                        ),
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap_err();

        assert!(matches!(error, SharePatchError::PolicyDivergent(_)));
        let unchanged = store.get(&original.id).unwrap();
        assert_eq!(unchanged.config_revision, original.config_revision);
        assert!(unchanged.token_limit.is_none());
    }

    #[test]
    fn upsert_canonicalizes_app_settings_to_the_share_expiration() {
        let expires_at = Utc
            .with_ymd_and_hms(2099, 12, 31, 23, 59, 59)
            .single()
            .unwrap()
            .timestamp_millis()
            .saturating_add(323);
        let mut input = codex_share_input("canonical-expiration");
        input.expires_at = Some(expires_at);
        input.app_settings = codex_app_settings(Vec::new());

        let share = ShareStore::default().upsert(input).unwrap();
        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            &share,
            &ProviderStore::default(),
        );
        let settings = &descriptor.app_settings[&AppKind::Codex];

        assert_eq!(share.expires_at, Some(expires_at));
        assert_eq!(
            settings.expires_at,
            share_expires_at_rfc3339(Some(expires_at))
        );
    }

    fn test_timestamp_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC test timestamp")
            .timestamp_millis()
    }

    #[test]
    fn new_owner_and_shareto_grants_snapshot_total_share_limits() {
        let expires_at = test_timestamp_ms(2030, 1, 1, 0, 0);
        let mut input = codex_share_input("grant-defaults");
        input.token_limit = Some(50_000);
        input.parallel_limit = Some(7);
        input.expires_at = Some(expires_at);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["User@Example.com".to_string()],
            ..ShareAcl::default()
        });

        let share = ShareStore::default().upsert(input).unwrap();
        let expected = ShareUserPolicy {
            parallel_limit: Some(7),
            token_limit: Some(50_000),
            token_period: ShareTokenPeriod::Lifetime,
            token_period_anchor_at_ms: None,
            expires_at: Some(expires_at),
        };

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
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
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
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
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
    fn user_quota_isolated_from_other_users_and_total_quota_remains_authoritative() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("user-quota");
        input.token_limit = Some(100);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec![
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
            ],
            ..ShareAcl::default()
        });
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
        private_input.acl = Some(ShareAcl {
            shared_with_emails: vec!["allowed@example.com".to_string()],
            ..ShareAcl::default()
        });
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
        free_input.for_sale = Some(false);
        free_input.free_access = Some(true);
        store.upsert(free_input).unwrap();
        assert!(store
            .validate_for_invocation("free-acl", AppKind::Codex, Some("anyone@example.com"), now,)
            .is_ok());
    }

    #[test]
    fn supplemental_usage_counts_tokens_without_a_second_request() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("supplemental-usage");
        input.token_limit = Some(10);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["alice@example.com".to_string()],
            ..ShareAcl::default()
        });
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
    fn app_scoped_acl_grants_revoke_and_restore_policy_history() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("market-user")).unwrap();

        let added = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(added.user_grants["buyer@example.com"].active);
        store.record_user_invocation_result("market-user", Some("buyer@example.com"), 17, now);

        let revoked = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(Vec::new())),
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

        let restored = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
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
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        store
            .apply_settings_patch(
                "revoked-inflight-user",
                ShareSettingsPatch {
                    shared_with_emails: Some(Vec::new()),
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
    fn explicit_user_policies_fill_new_acl_users_with_default_policy() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("explicit-market-user"))
            .unwrap();
        let owner_grant = share.user_grants["owner@example.com"].clone();

        let updated = store
            .apply_settings_patch(
                "explicit-market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    user_grants: Some(BTreeMap::from([(
                        "owner@example.com".to_string(),
                        owner_grant,
                    )])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(updated.user_grants["buyer@example.com"].active);
        assert_eq!(
            updated.user_grants["buyer@example.com"].policy,
            default_user_policy(&updated)
        );
    }

    #[test]
    fn apply_settings_patch_persists_free_for_sale_mode() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("share-free"))
            .expect("upsert");
        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    for_sale: Some("Free".to_string()),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply free");
        assert!(updated.free_access);
        assert!(!updated.for_sale);
        assert_eq!(share_router_for_sale_label(&updated), "Free");
    }

    #[test]
    fn apply_settings_patch_persists_valid_token_market_pricing() {
        let mut store = ShareStore::default();
        let input = codex_share_input("share-priced");
        let share = store.upsert(input).expect("upsert");

        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    for_sale_official_price_percent_by_app: Some(BTreeMap::from([(
                        AppKind::Codex,
                        80,
                    )])),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply pricing");

        assert_eq!(updated.official_price_percent, Some(80));
    }

    #[test]
    fn apply_settings_patch_rejects_invalid_pricing_without_partial_mutation() {
        let mut store = ShareStore::default();
        let input = codex_share_input("share-invalid-price");
        let share = store.upsert(input).expect("upsert");

        for pricing in [
            BTreeMap::from([(AppKind::Codex, 0)]),
            BTreeMap::from([(AppKind::Codex, 101)]),
            BTreeMap::from([(AppKind::Claude, 80)]),
        ] {
            let result = store.apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    description: Some(Some("must not persist".to_string())),
                    for_sale_official_price_percent_by_app: Some(pricing),
                    ..ShareSettingsPatch::default()
                },
            );
            assert!(matches!(result, Err(SharePatchError::Invalid(_))));
            let stored = store.get(&share.id).expect("stored share");
            assert_eq!(stored.description, None);
            assert!(stored.official_price_percent.is_none());
        }
    }

    #[test]
    fn sale_mode_transition_clears_pricing_and_rejects_contradictory_payload() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("share-price-transition");
        input.for_sale_official_price_percent_by_app = BTreeMap::from([(AppKind::Codex, 75)]);
        let share = store.upsert(input).expect("upsert");

        let rejected = store.apply_settings_patch(
            &share.id,
            ShareSettingsPatch {
                for_sale: Some("No".to_string()),
                for_sale_official_price_percent_by_app: Some(BTreeMap::from([(
                    AppKind::Codex,
                    75,
                )])),
                ..ShareSettingsPatch::default()
            },
        );
        assert!(matches!(rejected, Err(SharePatchError::Invalid(_))));
        assert_eq!(
            store
                .get(&share.id)
                .expect("stored share")
                .official_price_percent,
            Some(75)
        );

        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    for_sale: Some("No".to_string()),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("disable market sale");
        assert!(updated.official_price_percent.is_none());
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
                acl: None,
                token_limit: Some(1000),
                parallel_limit: Some(2),
                expires_at: None,
                for_sale: Some(true),
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: Some(80),
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: Some(true),
                description: Some("test".to_string()),
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
        assert!(share.for_sale);
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
        assert_eq!(
            descriptor.app_settings[&AppKind::Claude],
            descriptor.app_settings[&AppKind::Codex]
        );
        assert_eq!(
            descriptor.access_by_app[&AppKind::Claude],
            descriptor.access_by_app[&AppKind::Codex]
        );

        let updated = store
            .apply_settings_patch(
                "multi-app",
                ShareSettingsPatch {
                    shared_with_emails: Some(vec!["user@example.com".to_string()]),
                    market_access_mode: Some("selected".to_string()),
                    token_limit: Some(42),
                    parallel_limit: Some(3),
                    for_sale: Some("Yes".to_string()),
                    for_sale_official_price_percent_by_app: Some(BTreeMap::from([(
                        AppKind::Claude,
                        80,
                    )])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert_eq!(updated.acl.shared_with_emails, vec!["user@example.com"]);
        assert_eq!(updated.acl.market_access_mode.as_deref(), Some("selected"));
        assert_eq!(updated.token_limit, Some(42));
        assert_eq!(updated.parallel_limit, Some(3));
        assert_eq!(updated.official_price_percent, Some(80));
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
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
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
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
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
    fn imports_and_replaces_acl() {
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
            policy: SharePolicy {
                acl: ShareAcl::default(),
                ..SharePolicy::default()
            },
            tokens_used: 0,
            requests_count: 0,
            created_at_ms: 0,
            auto_start: false,
            description: None,
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
            user_grants: BTreeMap::new(),
        };
        assert_eq!(store.import_shares(vec![share]).unwrap(), 1);
        let updated = store
            .replace_acl(
                "s1",
                ShareAcl {
                    shared_with_emails: vec!["user@example.com".to_string()],
                    public_market_email: Some("market@example.com".to_string()),
                    market_access_mode: Some("selected".to_string()),
                },
            )
            .unwrap();

        assert_eq!(updated.acl.shared_with_emails, vec!["user@example.com"]);
        assert_eq!(
            updated.acl.public_market_email.as_deref(),
            Some("market@example.com")
        );
    }

    #[test]
    fn applies_app_settings_patch() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
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
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: Some(true),
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let mut app_settings = BTreeMap::new();
        app_settings.insert(
            AppKind::Codex,
            ShareAppSettings {
                for_sale: "Yes".to_string(),
                market_access_mode: "selected".to_string(),
                shared_with_emails: vec![
                    "buyer@example.com".to_string(),
                    "OWNER@example.com".to_string(),
                    "buyer@example.com".to_string(),
                ],
                token_limit: 5000,
                parallel_limit: 2,
                expires_at: "1893456000".to_string(),
            },
        );

        let share = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(app_settings),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert_eq!(share.acl.shared_with_emails, vec!["buyer@example.com"]);

        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            &share,
            &ProviderStore::default(),
        );
        assert_eq!(
            descriptor
                .app_settings
                .get(&AppKind::Codex)
                .unwrap()
                .shared_with_emails,
            vec!["buyer@example.com"]
        );
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
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
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
    fn app_settings_add_and_revoke_target_user() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();

        let added = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec![
                        "buyer@example.com",
                        "BUYER@example.com",
                        "owner@example.com",
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert_eq!(added.acl.shared_with_emails, vec!["buyer@example.com"]);

        let revoked = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(Vec::new())),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(revoked.acl.shared_with_emails.is_empty());
    }

    #[test]
    fn sequential_app_settings_patches_are_deterministic() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();

        store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer-a@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let final_share = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec![
                        "buyer-b@example.com",
                        "buyer-b@example.com",
                        "OWNER@example.com",
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert_eq!(
            final_share.acl.shared_with_emails,
            vec!["buyer-b@example.com".to_string()]
        );
    }

    #[test]
    fn bind_owner_renormalizes_acl() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: Some(true),
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: Some(ShareAcl {
                    shared_with_emails: vec![
                        "owner@example.com".to_string(),
                        "buyer@example.com".to_string(),
                    ],
                    public_market_email: None,
                    market_access_mode: Some("selected".to_string()),
                }),
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let updated = store
            .bind_all_to_client_owner("new-owner@example.com")
            .unwrap();
        let updated = &updated[0];
        assert_eq!(
            updated.owner_email.as_deref(),
            Some("new-owner@example.com")
        );
        assert_eq!(
            updated.acl.shared_with_emails,
            vec![
                "owner@example.com".to_string(),
                "buyer@example.com".to_string(),
            ]
        );
    }

    #[test]
    fn bind_all_to_client_owner_preserves_previous_owner_access_and_is_idempotent() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("owner-bind");
        input.owner_email = Some("previous@example.com".to_string());
        input.access_by_app.insert(
            AppKind::Codex,
            ShareAppAccess {
                shared_with_emails: vec!["buyer@example.com".to_string()],
                market_access_mode: "selected".to_string(),
            },
        );
        input.app_settings = codex_app_settings(vec!["buyer@example.com"]);
        store.upsert(input).unwrap();

        let updated = store
            .bind_all_to_client_owner("client@example.com")
            .unwrap();
        assert_eq!(updated.len(), 1);
        let share = store.get("owner-bind").unwrap();
        assert_eq!(share.owner_email.as_deref(), Some("client@example.com"));
        assert!(share
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            share,
            &ProviderStore::default(),
        );
        assert!(descriptor.access_by_app[&AppKind::Codex]
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
        assert!(descriptor.app_settings[&AppKind::Codex]
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
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
        assert!(store.migrate_user_grants_from_acl().is_empty());
        assert_eq!(store.get("owner-bind").unwrap().config_revision, revision);
    }

    #[test]
    fn old_share_json_migrates_user_grants_once_and_marks_router_sync_pending() {
        let expires_at = test_timestamp_ms(2030, 1, 1, 0, 0);
        let mut input = codex_share_input("legacy-user-grants");
        input.token_limit = Some(50_000);
        input.parallel_limit = Some(7);
        input.expires_at = Some(expires_at);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        let mut value = serde_json::to_value(store).unwrap();
        value["shares"][0]
            .as_object_mut()
            .unwrap()
            .remove("userGrants");
        let mut loaded: ShareStore = serde_json::from_value(value).unwrap();
        let previous_revision = loaded.shares[0].config_revision;

        let migrated = loaded.migrate_user_grants_from_acl();

        assert_eq!(migrated.len(), 1);
        let share = loaded.get("legacy-user-grants").unwrap();
        assert_eq!(share.config_revision, previous_revision + 1);
        assert_eq!(share.router_synced_revision, 0);
        let expected = ShareUserPolicy {
            parallel_limit: Some(7),
            token_limit: Some(50_000),
            token_period: ShareTokenPeriod::Lifetime,
            token_period_anchor_at_ms: None,
            expires_at: Some(expires_at),
        };
        assert_eq!(share.user_grants["owner@example.com"].policy, expected);
        assert_eq!(share.user_grants["user@example.com"].policy, expected);
        assert!(loaded.migrate_user_grants_from_acl().is_empty());
        assert_eq!(
            loaded.get("legacy-user-grants").unwrap().config_revision,
            previous_revision + 1
        );
    }

    #[test]
    fn grant_migration_repairs_stale_owner_role_without_rebinding_owner() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("stale-owner-role");
        input.owner_email = Some("previous@example.com".to_string());
        store.upsert(input).unwrap();

        let share = store.shares.first_mut().unwrap();
        share.owner_email = Some("client@example.com".to_string());
        insert_email(
            &mut share.acl.shared_with_emails,
            "previous@example.com".to_string(),
        );
        let previous_revision = share.config_revision;

        let migrated = store.migrate_user_grants_from_acl();

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
        assert!(store.migrate_user_grants_from_acl().is_empty());
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
        assert!(!share
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "invalid-owner"));
    }

    #[test]
    fn bind_owner_updates_all_shares() {
        let mut store = ShareStore::default();
        let mut first = codex_share_input("s1");
        first.acl = Some(ShareAcl {
            shared_with_emails: vec![
                "new-owner@example.com".to_string(),
                "buyer@example.com".to_string(),
            ],
            public_market_email: None,
            market_access_mode: Some("selected".to_string()),
        });
        first.app_settings = codex_app_settings(vec!["new-owner@example.com", "buyer@example.com"]);
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
        assert_eq!(
            first.acl.shared_with_emails,
            vec![
                "buyer@example.com".to_string(),
                "owner@example.com".to_string()
            ]
        );
        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            first,
            &ProviderStore::default(),
        );
        assert_eq!(
            descriptor.app_settings[&AppKind::Codex].shared_with_emails,
            vec![
                "buyer@example.com".to_string(),
                "owner@example.com".to_string()
            ]
        );
    }

    #[test]
    fn binding_owner_demotes_previous_owner() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: Some(true),
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: Some(ShareAcl {
                    shared_with_emails: vec!["buyer@example.com".to_string()],
                    public_market_email: None,
                    market_access_mode: Some("selected".to_string()),
                }),
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                allow_personal_credits: None,
                auto_consume_banked_reset: None,
                banked_reset_expiry_lead_minutes: None,
                previous_response_cache_enabled: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let updated = store.bind_all_to_client_owner("buyer@example.com").unwrap();
        let updated = &updated[0];
        assert_eq!(updated.owner_email.as_deref(), Some("buyer@example.com"));
        assert!(updated
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "owner@example.com"));
        assert!(!updated
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "buyer@example.com"));
    }
}
