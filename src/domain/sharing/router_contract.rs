use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::accounts::grok_subscription::canonical_grok_subscription_level;
use crate::domain::accounts::store::{
    active_account_usage_block_for_share, Account, AccountQuotaTier, AccountStore,
    AccountUsageBlock,
};
use crate::domain::accounts::subscription_expiry::resolved_subscription_expiry;
use crate::domain::health;
use crate::domain::providers::bundle::{
    bundle_model_policy_scope, bundle_model_policy_source, surface_enabled, ModelPolicyScope,
    ModelPolicySource,
};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::model_routing::{policy_from_settings, ModelRoutingMode};
use crate::domain::providers::registry::profile_by_id;
use crate::domain::providers::runtime::{
    authoritative_managed_account, build_provider_model_probe, ProviderModelProbe,
    ProviderRuntimePlan, RuntimeModelPolicy, PROVIDER_MODEL_PROBE_PROMPT,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::model_health::ShareModelHealthSummary;
use crate::domain::sharing::shares::Share;

pub const SHARE_CONTRACT_VERSION: u16 = 6;
use crate::domain::usage::store::UsageStore;

/// Distinguishes a missing JSON field (`None`) from an explicit `null`
/// (`Some(None)`). `Option<Option<T>>` with only `default` treats both as
/// absent, so `"description": null` would never clear the stored value.
fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_nullable_string"
    )]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_access: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_personal_credits: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_consume_banked_reset: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banked_reset_expiry_lead_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_cache_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grok_media_policy: Option<GrokMediaPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<ShareSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_grants: Option<BTreeMap<String, ShareUserGrant>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_usage_edits: Option<BTreeMap<String, ShareUserUsageEdit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_grant: Option<ShareManagedGrantOperation>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrokMediaPolicy {
    #[serde(default)]
    pub image_generation_enabled: bool,
    #[serde(default)]
    pub image_edit_enabled: bool,
    #[serde(default)]
    pub video_generation_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareManagedGrantAction {
    Upsert,
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareManagedGrantOperation {
    pub operation_id: String,
    pub entitlement_id: String,
    pub share_sequence: i64,
    pub expected_config_revision: u64,
    pub action: ShareManagedGrantAction,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ShareUserPolicy>,
    /// Relative service term. The Server turns this into an absolute expiry
    /// when it actually applies the grant, so queueing time is never consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareTokenPeriod {
    #[default]
    Lifetime,
    Day,
    Week,
    SevenDays,
    CalendarMonth,
    ThirtyDays,
}

pub fn supported_share_token_periods() -> Vec<ShareTokenPeriod> {
    vec![
        ShareTokenPeriod::Lifetime,
        ShareTokenPeriod::Day,
        ShareTokenPeriod::Week,
        ShareTokenPeriod::SevenDays,
        ShareTokenPeriod::CalendarMonth,
        ShareTokenPeriod::ThirtyDays,
    ]
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub token_period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_period_anchor_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Empty keeps legacy/manual ShareTo behavior. Router-managed market
    /// grants carry an explicit App set and may include core Apps that are not
    /// bound yet, so enabling them later does not require replacing the grant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_apps: Vec<AppKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsageBucket {
    #[serde(default)]
    pub started_at_ms: i64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsage {
    #[serde(default)]
    pub lifetime: ShareUserUsageBucket,
    #[serde(default)]
    pub day: ShareUserUsageBucket,
    #[serde(default)]
    pub week: ShareUserUsageBucket,
    #[serde(default)]
    pub calendar_month: ShareUserUsageBucket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchored: Option<ShareAnchoredUsageBucket>,
}

/// The persisted, Server-owned baseline used when an operator reconciles a
/// user's quota with an external Provider reset.  `ShareUserUsage` remains a
/// derived snapshot; this record is the durable input used to rebuild it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsageRebase {
    pub period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_starts_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ends_at_ms: Option<i64>,
    pub target_tokens: u64,
    #[serde(default)]
    pub observed_tokens_at_rebase: u64,
    #[serde(default)]
    pub observed_requests_at_rebase: u64,
    /// Usage journal waterline captured together with the observed snapshot.
    /// It is diagnostic/concurrency metadata; the raw Usage log is never
    /// rewritten by a rebase.
    #[serde(default)]
    pub usage_watermark: u64,
    pub applied_at_ms: i64,
    /// Verified admin identity that applied the baseline.  Accountability for
    /// a manual quota correction has to survive in the record itself: the
    /// derived snapshot it produces is indistinguishable from real traffic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_by: Option<String>,
    #[serde(default)]
    pub source: ShareUsageRebaseSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareUsageRebaseSource {
    #[default]
    Manual,
    ProviderReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareUserUsageEditAction {
    Set,
    Clear,
}

/// Explicit usage edit input.  It is intentionally separate from
/// `ShareUserGrant.usage`: client supplied usage snapshots are never trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareUserUsageEdit {
    pub action: ShareUserUsageEditAction,
    #[serde(default)]
    pub target_tokens: Option<u64>,
    #[serde(default)]
    pub expected_grant_revision: Option<u64>,
    #[serde(default)]
    pub period: Option<ShareTokenPeriod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    #[serde(default)]
    pub source: ShareUsageRebaseSource,
}

/// Explicit Share-total consumed-token edit.
///
/// The Share total counter has no window and no anchor: it is a plain
/// accumulator that only `reset_usage` ever moved backwards.  An operator
/// correction is therefore a direct set, and needs none of the rebase
/// bookkeeping the per-user quota requires.  It is kept out of
/// [`ShareSettingsPatch`] on purpose so no Router settings sync can rewrite
/// consumption counters as a side effect of pushing configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareTotalUsageEdit {
    pub action: ShareUserUsageEditAction,
    /// Required for `set`; ignored for `clear`, which means zero.
    #[serde(default)]
    pub tokens_used: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAnchoredUsageBucket {
    pub period: ShareTokenPeriod,
    pub anchor_at_ms: i64,
    pub started_at_ms: i64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserGrant {
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub policy: ShareUserPolicy,
    #[serde(default)]
    pub usage: ShareUserUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_rebase: Option<ShareUserUsageRebase>,
    /// Server-derived quota view for the grant's current window.
    ///
    /// Recomputed from the Usage history on every rebuild and never accepted
    /// from a browser or Router patch.  It exists so a client can show the
    /// effective/observed split and the window bounds without re-deriving
    /// them by inverting the rebase formula, which would silently disagree
    /// with the Server the moment the formula changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_quota: Option<ShareUserQuotaView>,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub updated_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u128>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub manager: ShareGrantManager,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entitlement_id: Option<String>,
}

/// Derived per-grant quota view.
///
/// Every field is computed by the Server from the append-only Usage history
/// plus the durable rebase record; nothing here is authoritative state.  It is
/// excluded from the descriptor fingerprint for the same reason `usage` is —
/// consumption moves constantly and must not force a Router descriptor resync.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareUserQuotaView {
    pub period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    /// Inclusive window start; absent for `lifetime`, which has no window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_starts_at_ms: Option<i64>,
    /// Exclusive window end; absent for `lifetime`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ends_at_ms: Option<i64>,
    /// What the limit is actually checked against.
    pub effective_tokens_used: u64,
    /// What the Usage history alone reports for this window.
    pub observed_tokens_used: u64,
    /// `effective - observed`; the standing operator correction, which is
    /// negative when the baseline was set below what was already observed.
    pub manual_offset_tokens: i64,
    pub observed_requests_count: u64,
    /// False when a rebase exists but belongs to a window that has since
    /// rolled over, so the client can explain why the offset disappeared.
    pub rebase_applies: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareGrantManager {
    Owner,
    #[default]
    Manual,
    RouterShareMarket,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareDescriptor {
    pub contract_version: u16,
    pub share_id: String,
    #[serde(default)]
    pub capacity_pool_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub free_access: bool,
    pub subdomain: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<AppKind, String>,
    #[serde(default)]
    pub token_limit: i64,
    #[serde(default = "default_parallel_limit")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub tokens_used: i64,
    #[serde(default)]
    pub requests_count: i64,
    #[serde(default)]
    pub share_status: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub app_providers: ShareAppProviders,
    #[serde(default)]
    pub app_availability: ShareAppAvailability,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_start: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_personal_credits: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_consume_banked_reset: bool,
    #[serde(
        default = "default_banked_reset_expiry_lead_minutes",
        skip_serializing_if = "is_default_banked_reset_expiry_lead_minutes"
    )]
    pub banked_reset_expiry_lead_minutes: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub previous_response_cache_enabled: bool,
    #[serde(default, skip_serializing_if = "is_default_grok_media_policy")]
    pub grok_media_policy: GrokMediaPolicy,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub descriptor_generation: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub descriptor_fingerprint: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
}

fn is_zero_revision(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_default_grok_media_policy(value: &GrokMediaPolicy) -> bool {
    *value == GrokMediaPolicy::default()
}

fn default_banked_reset_expiry_lead_minutes() -> u32 {
    crate::domain::sharing::shares::DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES
}

fn is_default_banked_reset_expiry_lead_minutes(value: &u32) -> bool {
    *value == default_banked_reset_expiry_lead_minutes()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSupport {
    #[serde(default)]
    pub claude: bool,
    #[serde(default)]
    pub codex: bool,
    #[serde(default)]
    pub gemini: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuotaTier {
    #[serde(alias = "name")]
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_weekly_capacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuota {
    pub status: String,
    #[serde(
        default,
        alias = "credentialMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_cost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_scope: Option<String>,
    #[serde(default)]
    pub tiers: Vec<ShareUpstreamQuotaTier>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamProvider {
    pub kind: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_scope: Option<ModelPolicyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_source: Option<ModelPolicySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<RuntimeModelPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_probe: Option<ProviderModelProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamModel {
    pub slot: String,
    pub actual_model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProvider {
    pub id: String,
    pub name: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_apps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_current: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub codex_image_generation_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_scope: Option<ModelPolicyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_source: Option<ModelPolicySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<RuntimeModelPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_probe: Option<ProviderModelProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProviders {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gemini: Vec<ShareAppProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppRuntimes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareUpstreamProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAvailability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareProviderAvailability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderAvailability {
    pub app: String,
    pub provider_id: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderHealth {
    pub healthy: bool,
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSyncOperation {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogEntry {
    #[serde(default)]
    pub export_sequence: u64,
    pub request_id: String,
    #[serde(default = "default_request_kind")]
    pub request_kind: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    pub share_id: String,
    pub share_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub request_model: String,
    pub request_agent: String,
    pub requested_model: String,
    pub actual_model: String,
    pub actual_model_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier_decision: Option<String>,
    #[serde(default)]
    pub usage_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_status: Option<String>,
    #[serde(default)]
    pub usage_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub status_code: u16,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    /// Whether the upstream protocol reported a cache-token breakdown. False means the
    /// numeric cache fields are compatibility placeholders and must not be presented as zero.
    #[serde(default)]
    pub cache_usage_observed: bool,
    /// Whether token counts were derived locally instead of reported by the upstream.
    #[serde(default)]
    pub usage_estimated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_tokens: Option<u32>,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_duration_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_aspect_ratio: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub is_health_check: bool,
}

fn default_request_kind() -> String {
    "text".to_string()
}

pub fn descriptor_for_share(share: &Share, providers: &ProviderStore) -> ShareDescriptor {
    descriptor_for_share_with_usage(share, providers, None)
}

pub fn descriptor_for_share_with_usage(
    share: &Share,
    providers: &ProviderStore,
    usage: Option<&UsageStore>,
) -> ShareDescriptor {
    descriptor_for_share_with_accounts_and_usage(share, providers, None, usage)
}

pub fn descriptor_for_share_with_accounts_and_usage(
    share: &Share,
    providers: &ProviderStore,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareDescriptor {
    let mut bindings = BTreeMap::new();
    if share.bindings.is_empty() {
        bindings.insert(share.app, share.provider_id.clone());
    } else {
        for binding in &share.bindings {
            bindings.insert(binding.app, binding.provider_id.clone());
        }
    }

    let mut enabled_apps = crate::domain::sharing::shares::share_enabled_apps(share);
    for (app, provider_id) in &bindings {
        let Some(provider) = providers
            .providers
            .iter()
            .find(|item| item.app == *app && item.provider.id == *provider_id)
        else {
            continue;
        };
        if !surface_enabled(&provider.provider) {
            enabled_apps.remove(app);
        }
    }
    let support = crate::domain::sharing::shares::support_from_enabled_apps(&enabled_apps);

    let mut app_runtimes = ShareAppRuntimes::default();
    let mut app_providers = ShareAppProviders::default();
    let mut app_availability = ShareAppAvailability::default();
    let mut primary_upstream = None;
    for (app, provider_id) in &bindings {
        if let Some(provider) = providers
            .providers
            .iter()
            .find(|item| item.app == *app && item.provider.id == *provider_id)
        {
            let runtime_plan = providers.runtime_plan(provider.app, &provider.provider.id);
            let upstream = upstream_provider(
                app.as_str(),
                provider,
                share,
                accounts,
                usage,
                runtime_plan.as_deref(),
            );
            let availability = provider_availability(
                app.as_str(),
                provider,
                share,
                accounts,
                usage,
                runtime_plan.as_deref(),
            );
            if *app == share.app {
                primary_upstream = Some(upstream.clone());
            }
            match app {
                AppKind::Claude => {
                    app_runtimes.claude = Some(upstream.clone());
                    app_providers.claude.push(app_provider(AppProviderInput {
                        app: app.as_str(),
                        provider,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: true,
                        enabled: surface_enabled(&provider.provider),
                    }));
                    app_availability.claude = Some(availability);
                }
                AppKind::Codex => {
                    app_runtimes.codex = Some(upstream.clone());
                    app_providers.codex.push(app_provider(AppProviderInput {
                        app: app.as_str(),
                        provider,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: true,
                        enabled: surface_enabled(&provider.provider),
                    }));
                    app_availability.codex = Some(availability);
                }
                AppKind::Gemini => {
                    app_runtimes.gemini = Some(upstream.clone());
                    app_providers.gemini.push(app_provider(AppProviderInput {
                        app: app.as_str(),
                        provider,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: true,
                        enabled: surface_enabled(&provider.provider),
                    }));
                    app_availability.gemini = Some(availability);
                }
            }
        }
    }
    if let Some(bundle_id) = bindings.values().find_map(|provider_id| {
        providers.providers.iter().find_map(|stored| {
            (stored.provider.id == *provider_id)
                .then(|| crate::domain::providers::bundle::bundle_id(&stored.provider))
                .flatten()
                .map(str::to_string)
        })
    }) {
        for stored in &providers.providers {
            if crate::domain::providers::bundle::bundle_id(&stored.provider)
                != Some(bundle_id.as_str())
                || bindings.contains_key(&stored.app)
            {
                continue;
            }
            let runtime_plan = providers.runtime_plan(stored.app, &stored.provider.id);
            let upstream = upstream_provider(
                stored.app.as_str(),
                stored,
                share,
                accounts,
                usage,
                runtime_plan.as_deref(),
            );
            let availability = provider_availability(
                stored.app.as_str(),
                stored,
                share,
                accounts,
                usage,
                runtime_plan.as_deref(),
            );
            match stored.app {
                AppKind::Claude if app_providers.claude.is_empty() => {
                    app_runtimes.claude = Some(upstream);
                    app_providers.claude.push(app_provider(AppProviderInput {
                        app: stored.app.as_str(),
                        provider: stored,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: false,
                        enabled: surface_enabled(&stored.provider),
                    }));
                    app_availability.claude = Some(availability);
                }
                AppKind::Codex if app_providers.codex.is_empty() => {
                    app_runtimes.codex = Some(upstream);
                    app_providers.codex.push(app_provider(AppProviderInput {
                        app: stored.app.as_str(),
                        provider: stored,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: false,
                        enabled: surface_enabled(&stored.provider),
                    }));
                    app_availability.codex = Some(availability);
                }
                AppKind::Gemini if app_providers.gemini.is_empty() => {
                    app_runtimes.gemini = Some(upstream);
                    app_providers.gemini.push(app_provider(AppProviderInput {
                        app: stored.app.as_str(),
                        provider: stored,
                        share,
                        accounts,
                        usage,
                        runtime_plan: runtime_plan.as_deref(),
                        is_current: false,
                        enabled: surface_enabled(&stored.provider),
                    }));
                    app_availability.gemini = Some(availability);
                }
                _ => {}
            }
        }
    }
    let model_health =
        crate::domain::sharing::model_health::summary_for_share(share, providers, accounts, usage);

    ShareDescriptor {
        contract_version: SHARE_CONTRACT_VERSION,
        share_id: share.id.clone(),
        capacity_pool_id: if share.capacity_pool_id.is_empty() {
            share.id.clone()
        } else {
            share.capacity_pool_id.clone()
        },
        share_name: share
            .display_name
            .clone()
            .unwrap_or_else(|| share.id.clone()),
        owner_email: share.owner_email.clone(),
        description: share.description.clone(),
        free_access: share.free_access,
        subdomain: share
            .tunnel_subdomain
            .clone()
            .unwrap_or_else(|| share.id.replace('_', "-")),
        app_type: app_key(share.app).to_string(),
        provider_id: Some(share.provider_id.clone()),
        bindings,
        token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
        parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(-1),
        tokens_used: share.tokens_used as i64,
        requests_count: share.requests_count as i64,
        share_status: share.status.clone(),
        created_at: share_created_at_rfc3339(share),
        expires_at: share_expires_at_rfc3339(share.expires_at),
        support,
        upstream_provider: primary_upstream,
        app_runtimes,
        app_providers,
        app_availability,
        model_health,
        auto_start: share.auto_start,
        allow_personal_credits: share.allow_personal_credits,
        auto_consume_banked_reset: share.auto_consume_banked_reset,
        banked_reset_expiry_lead_minutes: share.banked_reset_expiry_lead_minutes,
        previous_response_cache_enabled: share.previous_response_cache_enabled,
        grok_media_policy: share.grok_media_policy,
        config_revision: share.config_revision,
        descriptor_generation: share.descriptor_generation,
        descriptor_fingerprint: share.descriptor_fingerprint.clone().unwrap_or_default(),
        user_grants: share.user_grants.clone(),
        supported_user_token_periods: supported_share_token_periods(),
    }
}

/// Hashes only facts owned by the Server's static Share projection. Router-owned
/// request counters and health observations are deliberately excluded, as are
/// Provider display names and volatile subscription remaining-time values.
pub fn static_descriptor_fingerprint(
    descriptor: &ShareDescriptor,
    providers: &ProviderStore,
) -> anyhow::Result<String> {
    let projection = static_descriptor_projection(descriptor, providers)?;
    let bytes = serde_json::to_vec(&projection)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn static_descriptor_projection(
    descriptor: &ShareDescriptor,
    providers: &ProviderStore,
) -> anyhow::Result<Value> {
    let mut projection = serde_json::to_value(descriptor)?;
    let object = projection
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Share descriptor must serialize as an object"))?;
    for field in [
        "tokensUsed",
        "requestsCount",
        "appAvailability",
        "modelHealth",
        "createdAt",
        "configRevision",
        "descriptorGeneration",
        "descriptorFingerprint",
    ] {
        object.remove(field);
    }
    if let Some(provider) = object.get_mut("upstreamProvider") {
        strip_dynamic_provider_projection(provider, false);
    }
    if let Some(runtimes) = object.get_mut("appRuntimes").and_then(Value::as_object_mut) {
        for provider in runtimes.values_mut() {
            strip_dynamic_provider_projection(provider, false);
        }
    }
    if let Some(apps) = object
        .get_mut("appProviders")
        .and_then(Value::as_object_mut)
    {
        for providers in apps.values_mut().filter_map(Value::as_array_mut) {
            for provider in providers {
                strip_dynamic_provider_projection(provider, true);
            }
        }
    }
    if let Some(grants) = object.get_mut("userGrants").and_then(Value::as_object_mut) {
        for grant in grants.values_mut().filter_map(Value::as_object_mut) {
            for field in [
                "usage",
                "usageQuota",
                "createdAtMs",
                "updatedAtMs",
                "revokedAtMs",
                "revision",
            ] {
                grant.remove(field);
            }
        }
    }

    let runtime_fingerprints = descriptor
        .bindings
        .iter()
        .map(|(app, provider_id)| {
            let app_kind = match app.as_str() {
                "claude" => AppKind::Claude,
                "codex" => AppKind::Codex,
                "gemini" => AppKind::Gemini,
                other => anyhow::bail!("unsupported Share binding app {other}"),
            };
            let fingerprint = providers
                .runtime_plan(app_kind, provider_id)
                .map(|plan| plan.runtime_fingerprint.clone())
                .unwrap_or_else(|| "missing-runtime-plan".to_string());
            Ok((*app, fingerprint))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
    object.insert(
        "runtimeFingerprints".to_string(),
        serde_json::to_value(runtime_fingerprints)?,
    );
    object.insert("fingerprintSchema".to_string(), Value::from(1));

    Ok(projection)
}

fn strip_dynamic_provider_projection(value: &mut Value, app_provider: bool) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for field in [
        "providerName",
        "subscriptionRemainingMs",
        "quotaPercent",
        "quotaBlocked",
        "quota",
        "health",
        "available",
    ] {
        object.remove(field);
    }
    if app_provider {
        object.remove("name");
    }
}

fn app_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "claude",
        AppKind::Codex => "codex",
        AppKind::Gemini => "gemini",
    }
}

fn upstream_provider(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> ShareUpstreamProvider {
    let health = usage.map(|usage| provider_health(provider, usage, runtime_plan));
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let account_context = account_context_for_share(provider, share, accounts);
    let usage_block = account.and_then(|account| current_account_usage_block(account, share));
    let quota_blocked = account.map(|_| usage_block.is_some());
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    let resolved_provider_type = provider.provider_type;
    let provider_type_id = resolved_provider_type.as_str().to_string();
    let (model_policy_scope, model_policy_source, model_policy) =
        provider_model_policy_metadata(provider, runtime_plan);
    let model_probe = provider_model_probe(provider, runtime_plan);
    ShareUpstreamProvider {
        kind: provider_type_id.clone(),
        app: app.to_string(),
        provider_name: Some(provider.provider.name.clone()),
        provider_type: Some(provider_type_id.clone()),
        account_label: account_context.account_label,
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        quota: account.and_then(|account| upstream_quota_from_account(account, share)),
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        model_policy_scope,
        model_policy_source,
        model_policy,
        model_probe,
        health,
        available,
    }
}

/// Inputs for [`app_provider`].
///
/// Grouped into a struct so the descriptor builder keeps readable call sites
/// instead of a long positional argument list.
struct AppProviderInput<'a> {
    app: &'a str,
    provider: &'a StoredProvider,
    share: &'a Share,
    accounts: Option<&'a AccountStore>,
    usage: Option<&'a UsageStore>,
    runtime_plan: Option<&'a ProviderRuntimePlan>,
    is_current: bool,
    enabled: bool,
}

fn app_provider(input: AppProviderInput<'_>) -> ShareAppProvider {
    let AppProviderInput {
        app,
        provider,
        share,
        accounts,
        usage,
        runtime_plan,
        is_current,
        enabled,
    } = input;
    let health = usage.map(|usage| provider_health(provider, usage, runtime_plan));
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let account_context = account_context_for_share(provider, share, accounts);
    let usage_block = account.and_then(|account| current_account_usage_block(account, share));
    let quota_blocked = account.map(|_| usage_block.is_some());
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    let resolved_provider_type = provider.provider_type;
    let provider_type_id = resolved_provider_type.as_str().to_string();
    let (model_policy_scope, model_policy_source, model_policy) =
        provider_model_policy_metadata(provider, runtime_plan);
    let model_probe = provider_model_probe(provider, runtime_plan);
    ShareAppProvider {
        id: provider.provider.id.clone(),
        name: provider.provider.name.clone(),
        app: app.to_string(),
        bundle_id: crate::domain::providers::bundle::bundle_id(&provider.provider)
            .map(str::to_string),
        supported_apps: crate::domain::providers::bundle::bundle_supported_apps(&provider.provider)
            .unwrap_or_else(|| vec![provider.app])
            .into_iter()
            .map(|app| app.as_str().to_string())
            .collect(),
        kind: Some(provider_type_id.clone()),
        provider_type: Some(provider_type_id),
        is_current,
        enabled,
        codex_image_generation_enabled: provider
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_image_generation_enabled)
            .unwrap_or(false),
        account_label: account_context.account_label,
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        quota: account.and_then(|account| upstream_quota_from_account(account, share)),
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        model_policy_scope,
        model_policy_source,
        model_policy,
        model_probe,
        health,
        available,
    }
}

fn provider_model_policy_metadata(
    provider: &StoredProvider,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> (
    Option<ModelPolicyScope>,
    Option<ModelPolicySource>,
    Option<RuntimeModelPolicy>,
) {
    let profile = provider
        .resource
        .profile_id
        .as_ref()
        .and_then(|profile_id| profile_by_id(profile_id.as_str()));
    let model_policy_scope = match bundle_model_policy_scope(&provider.provider) {
        Ok(Some(scope)) => Some(scope),
        Ok(None) => Some(ModelPolicyScope::PerApp),
        Err(_) => None,
    };
    let model_policy_source = bundle_model_policy_source(&provider.provider, profile).ok();
    let model_policy = runtime_plan.map(|plan| plan.model_policy.clone());
    (model_policy_scope, model_policy_source, model_policy)
}

fn provider_model_probe(
    provider: &StoredProvider,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> Option<ProviderModelProbe> {
    let plan = runtime_plan?;
    if health::provider_probe_support(plan) != health::ProviderProbeSupport::Supported {
        return None;
    }
    let test_model = plan.test_model.as_deref()?.trim();
    if test_model.is_empty() {
        return None;
    }
    Some(build_provider_model_probe(
        provider.app,
        provider.provider_type,
        test_model,
        PROVIDER_MODEL_PROBE_PROMPT,
        true,
        plan.health_fingerprint(),
    ))
}

fn provider_availability(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> ShareProviderAvailability {
    let health = usage.map(|usage| health::provider_health_for_plan(provider, usage, runtime_plan));
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let usage_block = account.and_then(|account| current_account_usage_block(account, share));
    let quota_blocked = account.map(|_| usage_block.is_some());
    let available = health
        .as_ref()
        .map(|health| health.available)
        .unwrap_or(true)
        && quota_blocked != Some(true);
    let reason = usage_block
        .map(|block| block.reason.to_string())
        .or_else(|| health.as_ref().and_then(|health| health.reason.clone()));
    ShareProviderAvailability {
        app: app.to_string(),
        provider_id: provider.provider.id.clone(),
        available,
        reason,
        quota_blocked,
        last_status_code: health.as_ref().and_then(|health| health.last_status_code),
        success_rate: health.as_ref().and_then(|health| health.success_rate),
        avg_latency_ms: health.as_ref().and_then(|health| health.avg_latency_ms),
    }
}

fn provider_health(
    provider: &StoredProvider,
    usage: &UsageStore,
    runtime_plan: Option<&ProviderRuntimePlan>,
) -> ShareProviderHealth {
    let health = health::provider_health_for_plan(provider, usage, runtime_plan);
    ShareProviderHealth {
        healthy: health.available,
        requests: health.requests,
        successes: health.successes,
        failures: health.failures,
        success_rate: health.success_rate,
        avg_latency_ms: health.avg_latency_ms,
        last_status_code: health.last_status_code,
        last_request_at_ms: health.last_request_at_ms,
        reason: health.reason,
    }
}

fn current_account_usage_block(account: &Account, share: &Share) -> Option<AccountUsageBlock> {
    active_account_usage_block_for_share(
        account,
        crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
        share.allow_personal_credits,
    )
}

#[derive(Debug, Clone, Default)]
struct ShareAccountContext {
    account_label: Option<String>,
    account_email: Option<String>,
    subscription_level: Option<String>,
    subscription_expires_at: Option<String>,
    subscription_remaining_ms: Option<i64>,
    quota_percent: Option<f64>,
}

fn account_context_for_share(
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
) -> ShareAccountContext {
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let cursor_identity = provider.resource.cursor_verified_identity.as_ref();
    ShareAccountContext {
        account_label: account.and_then(account_display_label).or_else(|| {
            cursor_identity.and_then(|identity| {
                identity
                    .email
                    .clone()
                    .or_else(|| identity.display_name.clone())
                    .or_else(|| identity.credential_name.clone())
            })
        }),
        account_email: account
            .and_then(|account| account.email.clone())
            .or_else(|| cursor_identity.and_then(|identity| identity.email.clone()))
            .or_else(|| share.account_email.clone()),
        subscription_level: account
            .and_then(|account| {
                canonical_provider_subscription_level(
                    account.provider_type,
                    account.subscription_level.as_deref(),
                )
            })
            .or_else(|| cursor_identity.and_then(|identity| identity.subscription_level.clone()))
            .or_else(|| {
                canonical_provider_subscription_level(
                    provider.provider_type,
                    share.subscription_level.as_deref(),
                )
            }),
        subscription_expires_at: account.and_then(account_subscription_expires_at),
        subscription_remaining_ms: account.and_then(account_subscription_remaining_ms),
        quota_percent: account
            .and_then(|account| account.quota_percent)
            .or(share.quota_percent),
    }
}

fn account_display_label(account: &Account) -> Option<String> {
    account.email.clone().or_else(|| {
        let profile = account.profile.as_ref()?;
        [
            "/displayName",
            "/name",
            "/userEmail",
            "/profileRaw/displayName",
            "/profileRaw/name",
            "/profileRaw/userEmail",
            "/profileRaw/user/name",
        ]
        .iter()
        .find_map(|pointer| {
            profile
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    })
}

fn canonical_provider_subscription_level(
    provider_type: ProviderType,
    value: Option<&str>,
) -> Option<String> {
    let value = value?;
    if provider_type == ProviderType::GrokOAuth {
        return canonical_grok_subscription_level(value);
    }
    Some(value.to_string())
}

fn account_for_provider<'a>(
    accounts: &'a AccountStore,
    provider: &StoredProvider,
) -> Option<&'a Account> {
    authoritative_managed_account(provider, accounts)
}

fn account_subscription_expires_at(account: &Account) -> Option<String> {
    let expires_at_ms = resolved_subscription_expiry(account).expires_at_ms?;
    Utc.timestamp_millis_opt(expires_at_ms)
        .single()
        .map(|value| value.to_rfc3339())
}

fn account_subscription_remaining_ms(account: &Account) -> Option<i64> {
    resolved_subscription_expiry(account)
        .expires_at_ms
        .map(|expires_at_ms| {
            expires_at_ms
                .saturating_sub(crate::infra::time::now_ms().min(i64::MAX as u128) as i64)
                .max(0)
        })
}

fn upstream_quota_from_account(account: &Account, share: &Share) -> Option<ShareUpstreamQuota> {
    let subscription_period_end = account_subscription_expires_at(account);
    let block = current_account_usage_block(account, share);
    let availability = block
        .as_ref()
        .map(|block| block.kind.availability())
        .unwrap_or("available")
        .to_string();
    let blocked_until = block
        .as_ref()
        .and_then(|block| unix_ms_to_rfc3339(block.until_ms));
    let blocked_reason = block.as_ref().map(|block| block.reason.to_string());
    let blocked_scope = block.as_ref().map(|block| block.scope.to_string());
    let Some(quota) = account.quota.as_ref() else {
        if subscription_period_end.is_none() && block.is_none() {
            return None;
        }
        return Some(ShareUpstreamQuota {
            status: "ok".to_string(),
            plan: canonical_provider_subscription_level(
                account.provider_type,
                account.subscription_level.as_deref(),
            ),
            activity_cost: None,
            queried_at: None,
            subscription_period_end,
            availability: Some(availability),
            blocked_until,
            blocked_reason,
            blocked_scope,
            tiers: Vec::new(),
        });
    };
    if quota.tiers.is_empty()
        && !quota.success
        && subscription_period_end.is_none()
        && block.is_none()
    {
        return None;
    }
    let plan = quota
        .credential_message
        .as_deref()
        .and_then(|value| canonical_provider_subscription_level(account.provider_type, Some(value)))
        .or_else(|| {
            canonical_provider_subscription_level(
                account.provider_type,
                account.subscription_level.as_deref(),
            )
        });
    Some(ShareUpstreamQuota {
        status: if quota.success {
            "ok".to_string()
        } else {
            "failed".to_string()
        },
        plan,
        activity_cost: None,
        queried_at: account.quota_refreshed_at,
        subscription_period_end,
        availability: Some(availability),
        blocked_until,
        blocked_reason,
        blocked_scope,
        tiers: quota
            .tiers
            .iter()
            .map(share_upstream_quota_tier_from_account)
            .collect(),
    })
}

fn share_upstream_quota_tier_from_account(tier: &AccountQuotaTier) -> ShareUpstreamQuotaTier {
    ShareUpstreamQuotaTier {
        label: share_quota_tier_label(&tier.name),
        utilization: utilization_percent_for_router_share(tier.utilization),
        resets_at: tier.resets_at.and_then(unix_ms_to_rfc3339),
        used: tier.used,
        limit: tier.limit,
        unit: tier.unit.clone(),
        scope: Some(tier.scope.clone().unwrap_or_else(|| "account".to_string())),
        capacity_pool: tier.capacity_pool.clone(),
        model_family: tier.model_family.clone(),
        relative_weekly_capacity: tier.relative_weekly_capacity,
        source: tier.source.clone(),
    }
}

fn share_quota_tier_label(name: &str) -> String {
    match name {
        "five_hour" => "5h".to_string(),
        "seven_day" => "1w".to_string(),
        "30_day" => "30d".to_string(),
        "seven_day_opus" => "7d Opus".to_string(),
        "seven_day_omelette" => "7d Opus".to_string(),
        "seven_day_sonnet" => "7d Sonnet".to_string(),
        "seven_day_fable" => "Fable 7d".to_string(),
        "premium" => "premium".to_string(),
        "kiro_agentic_requests" => "Kiro".to_string(),
        other => other.replace('_', " "),
    }
}

fn utilization_percent_for_router_share(value: Option<f64>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if !value.is_finite() {
        return 0.0;
    }
    if value <= 1.0 {
        (value * 100.0).clamp(0.0, 100.0)
    } else {
        value.clamp(0.0, 100.0)
    }
}

fn unix_ms_to_rfc3339(ms: i64) -> Option<String> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|value| value.to_rfc3339())
}

const UNLIMITED_SHARE_EXPIRES_AT: &str = "2099-12-31T23:59:59Z";

fn share_timestamp_to_rfc3339(value: i64) -> Option<String> {
    if value <= 0 {
        return None;
    }
    let ms = if value < 10_000_000_000 {
        value.saturating_mul(1000)
    } else {
        value
    };
    unix_ms_to_rfc3339(ms)
}

pub(crate) fn share_expires_at_rfc3339(expires_at: Option<i64>) -> String {
    expires_at
        .and_then(share_timestamp_to_rfc3339)
        .unwrap_or_else(|| UNLIMITED_SHARE_EXPIRES_AT.to_string())
}

fn share_created_at_rfc3339(share: &Share) -> String {
    if share.created_at_ms > 0 {
        return unix_ms_to_rfc3339(share.created_at_ms as i64)
            .unwrap_or_else(|| Utc::now().to_rfc3339());
    }
    share
        .binding_history
        .iter()
        .map(|entry| entry.changed_at_ms)
        .min()
        .and_then(|value| unix_ms_to_rfc3339(value as i64))
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

fn provider_models(provider: &StoredProvider) -> Vec<ShareUpstreamModel> {
    let settings = &provider.provider.settings_config;
    let passthrough = policy_from_settings(settings)
        .is_some_and(|policy| policy.mode == ModelRoutingMode::Passthrough);
    if let Some(model) = single_upstream_model_from_settings(settings) {
        return vec![ShareUpstreamModel {
            slot: "model".to_string(),
            actual_model: model,
        }];
    }

    if provider.app == AppKind::Codex && !passthrough {
        if let Some(model) = codex_model_from_settings(settings) {
            return vec![ShareUpstreamModel {
                slot: "model".to_string(),
                actual_model: model,
            }];
        }
    }

    let mut models = Vec::new();
    if let Some(mapping) = settings.get("modelMapping").and_then(Value::as_object) {
        for (slot, value) in mapping {
            if is_model_mapping_metadata_key(slot) {
                continue;
            }
            if let Some(actual_model) = value.as_str().filter(|model| !model.trim().is_empty()) {
                models.push(ShareUpstreamModel {
                    slot: slot.clone(),
                    actual_model: actual_model.to_string(),
                });
            }
        }
    }
    if let Some(values) = settings.get("models").and_then(Value::as_array) {
        for value in values {
            let model = value.as_str().or_else(|| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("name").and_then(Value::as_str))
            });
            if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
                models.push(ShareUpstreamModel {
                    slot: "available".to_string(),
                    actual_model: model.to_string(),
                });
            }
        }
    }
    models
}

fn is_model_mapping_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "mode" | "type" | "upstreamModel" | "upstream_model" | "model"
    )
}

fn single_upstream_model_from_settings(settings: &Value) -> Option<String> {
    let mapping = settings.get("modelMapping")?;
    let mode = mapping
        .get("mode")
        .or_else(|| mapping.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode != "single" {
        return None;
    }
    mapping
        .get("upstreamModel")
        .or_else(|| mapping.get("upstream_model"))
        .or_else(|| mapping.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn codex_model_from_settings(settings: &Value) -> Option<String> {
    settings
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| settings.pointer("/config/model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| {
            settings
                .get("config")
                .and_then(Value::as_str)
                .and_then(extract_codex_toml_model)
                .map(str::to_string)
        })
}

fn extract_codex_toml_model(config: &str) -> Option<&str> {
    for line in config.lines() {
        let trimmed = line.split('#').next().unwrap_or(line).trim();
        for marker in ["model = \"", "model = '"] {
            let Some(rest) = trimmed.strip_prefix(marker) else {
                continue;
            };
            let quote = marker.chars().last()?;
            let Some(end) = rest.find(quote) else {
                continue;
            };
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn extract_codex_toml_base_url(config: &str) -> Option<&str> {
    for marker in ["base_url = \"", "base_url = '"] {
        let Some(start) = config.find(marker) else {
            continue;
        };
        let quote = marker.chars().last()?;
        let rest = &config[start + marker.len()..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        let value = rest[..end].trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn normalize_api_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn provider_api_url(provider: &StoredProvider) -> Option<String> {
    let settings = &provider.provider.settings_config;
    let env = settings.get("env");
    let app_env_keys: &[&str] = match provider.app {
        AppKind::Claude => &["ANTHROPIC_BASE_URL", "BASE_URL"],
        AppKind::Codex => &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        AppKind::Gemini => &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
    };
    for key in app_env_keys {
        if let Some(url) = settings
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
            .or_else(|| settings.get(*key).and_then(Value::as_str))
        {
            if let Some(url) = normalize_api_url(url) {
                return Some(url);
            }
        }
    }
    if let Some(url) = [
        "/env/ANTHROPIC_BASE_URL",
        "/env/OPENAI_BASE_URL",
        "/env/GOOGLE_GEMINI_BASE_URL",
        "/env/GEMINI_BASE_URL",
    ]
    .into_iter()
    .find_map(|pointer| settings.pointer(pointer).and_then(Value::as_str))
    .and_then(normalize_api_url)
    {
        return Some(url);
    }
    if provider.app == AppKind::Codex {
        if let Some(url) = settings
            .get("config")
            .and_then(Value::as_str)
            .and_then(extract_codex_toml_base_url)
            .and_then(normalize_api_url)
        {
            return Some(url);
        }
    }
    env.and_then(|value| value.get("BASE_URL"))
        .and_then(Value::as_str)
        .and_then(normalize_api_url)
        .or_else(|| provider_type_default_api_url(provider.provider_type))
}

fn provider_type_default_api_url(provider_type: ProviderType) -> Option<String> {
    let url = match provider_type {
        ProviderType::Nvidia => "https://integrate.api.nvidia.com/v1",
        ProviderType::DeepSeekApi => "https://api.deepseek.com",
        ProviderType::OpenRouter => "https://openrouter.ai/api",
        ProviderType::OllamaCloud => "https://ollama.com",
        _ => return None,
    };
    Some(url.to_string())
}

fn default_parallel_limit() -> i64 {
    -1
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{AccountQuota, AccountQuotaTier, AccountStore};
    use crate::domain::health::{ProviderHealthObservation, ProviderHealthStatus};
    use crate::domain::providers::model::{
        AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
    };
    use crate::domain::sharing::shares::{ShareBinding, SharePolicy};
    use crate::domain::usage::store::{UsageLog, UsageLogContext, UsageModelMetadata};

    #[test]
    fn explicit_null_description_clears_instead_of_being_absent() {
        let patch: ShareSettingsPatch =
            serde_json::from_str(r#"{"description":null}"#).expect("parse null description");
        assert_eq!(patch.description, Some(None));

        let omitted: ShareSettingsPatch =
            serde_json::from_str(r#"{}"#).expect("parse omitted description");
        assert_eq!(omitted.description, None);
    }

    #[test]
    fn v3_all_app_scope_matches_router_wire_format_and_keeps_legacy_omission() {
        let policy = ShareUserPolicy {
            allowed_apps: vec![AppKind::Claude, AppKind::Codex, AppKind::Gemini],
            ..ShareUserPolicy::default()
        };
        let wire = serde_json::to_value(&policy).expect("serialize App-scoped policy");

        assert_eq!(
            wire,
            json!({
                "tokenPeriod": "lifetime",
                "allowedApps": ["claude", "codex", "gemini"]
            })
        );
        assert_eq!(
            serde_json::from_value::<ShareUserPolicy>(wire)
                .expect("deserialize Router App scope")
                .allowed_apps,
            [AppKind::Claude, AppKind::Codex, AppKind::Gemini]
        );
        assert!(serde_json::from_value::<ShareUserPolicy>(json!({}))
            .expect("deserialize legacy policy")
            .allowed_apps
            .is_empty());
    }

    #[test]
    fn descriptor_exposes_provider_bundle_identity_and_supported_apps() {
        let provider_type = ProviderType::GrokOAuth;
        let mut provider = test_provider(provider_type);
        provider
            .provider
            .extra
            .insert("bundleId".to_string(), json!("p1"));
        provider
            .provider
            .extra
            .insert("familyId".to_string(), json!("family.grok_oauth"));
        provider
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(true));
        provider
            .provider
            .extra
            .insert("modelPolicyScope".to_string(), json!("global"));
        provider
            .provider
            .extra
            .insert("testApp".to_string(), json!("codex"));
        provider.resource.profile_id =
            Some(crate::domain::providers::registry::ProfileId::parse("codex.grok_oauth").unwrap());
        let mut providers = ProviderStore {
            providers: vec![provider],
            ..ProviderStore::default()
        };
        providers
            .rebuild_runtime_index(&AccountStore::default())
            .expect("compile Provider runtime fixture");
        let share = test_share(provider_type, None);

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(descriptor.contract_version, 6);
        assert_eq!(provider.bundle_id.as_deref(), Some("p1"));
        assert_eq!(provider.supported_apps, ["claude", "codex", "gemini"]);
        assert_eq!(provider.model_policy_scope, Some(ModelPolicyScope::Global));
        assert_eq!(
            provider.model_policy_source,
            Some(ModelPolicySource::BundleGlobal)
        );
        let serialized = serde_json::to_value(provider).unwrap();
        assert_eq!(serialized["modelPolicyScope"], json!("global"));
        assert_eq!(serialized["modelPolicySource"], json!("bundle_global"));
        let probe = provider.model_probe.as_ref().expect("v4 model probe");
        assert_eq!(probe.api_type, "openai");
        assert_eq!(probe.method, "POST");
        assert_eq!(probe.path, "/v1/responses");
        assert_eq!(probe.body["model"], probe.wire_model);
        assert_eq!(
            descriptor
                .app_runtimes
                .codex
                .as_ref()
                .and_then(|runtime| runtime.model_probe.as_ref()),
            Some(probe)
        );
        assert!(provider.enabled);
    }

    #[test]
    fn cursor_api_key_descriptor_exposes_safe_verified_account_presentation() {
        let provider_type = ProviderType::CursorApiKey;
        let mut provider = test_provider(provider_type);
        provider.resource.cursor_verified_identity =
            Some(crate::domain::providers::store::CursorVerifiedIdentity {
                schema_version: 2,
                account_id: "cursor_apikey_fixture".to_string(),
                principal_source: "user_id".to_string(),
                verified_at_ms: 1_000,
                email: Some("owner@example.com".to_string()),
                display_name: Some("Ada Lovelace".to_string()),
                credential_name: Some("Production key".to_string()),
                subscription_level: Some("Cursor Pro+".to_string()),
            });
        let providers = ProviderStore {
            providers: vec![provider],
            ..ProviderStore::default()
        };
        let share = test_share(provider_type, None);

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let app_provider = descriptor.app_providers.codex.first().unwrap();
        let upstream_provider = descriptor.upstream_provider.as_ref().unwrap();

        assert_eq!(
            app_provider.account_label.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(
            app_provider.account_email.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(
            app_provider.subscription_level.as_deref(),
            Some("Cursor Pro+")
        );
        assert_eq!(
            upstream_provider.account_label.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(
            upstream_provider.account_email.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(
            upstream_provider.subscription_level.as_deref(),
            Some("Cursor Pro+")
        );
    }

    #[test]
    fn descriptor_omits_model_probe_when_the_runtime_driver_cannot_test() {
        let provider_type = ProviderType::GrokOAuth;
        let mut provider = test_provider(provider_type);
        provider.resource.profile_id =
            Some(crate::domain::providers::registry::ProfileId::parse("codex.grok_oauth").unwrap());
        let mut providers = ProviderStore {
            providers: vec![provider],
            ..ProviderStore::default()
        };
        providers
            .rebuild_runtime_index(&AccountStore::default())
            .expect("compile Provider runtime fixture");
        let stored = providers.providers.first().unwrap();
        let mut plan = providers
            .runtime_plan(stored.app, &stored.provider.id)
            .unwrap()
            .as_ref()
            .clone();
        plan.driver_id =
            crate::domain::providers::registry::DriverId::parse("special.antigravity").unwrap();

        assert_eq!(
            health::provider_probe_support(&plan),
            health::ProviderProbeSupport::Unsupported
        );
        assert!(provider_model_probe(stored, Some(&plan)).is_none());
    }

    #[test]
    fn descriptor_projects_disabled_bundle_surfaces_without_unbinding() {
        let mut codex = test_provider(ProviderType::GrokOAuth);
        codex
            .provider
            .extra
            .insert("bundleId".to_string(), json!("p1"));
        codex
            .provider
            .extra
            .insert("familyId".to_string(), json!("family.grok_oauth"));
        codex
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(true));
        codex
            .provider
            .extra
            .insert("modelPolicyScope".to_string(), json!("global"));
        codex
            .provider
            .extra
            .insert("testApp".to_string(), json!("codex"));
        codex.resource.profile_id =
            Some(crate::domain::providers::registry::ProfileId::parse("codex.grok_oauth").unwrap());

        let mut claude = test_provider(ProviderType::GrokOAuth);
        claude.app = AppKind::Claude;
        claude.provider.extra = codex.provider.extra.clone();
        claude
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(false));
        claude.resource.profile_id = Some(
            crate::domain::providers::registry::ProfileId::parse("claude.grok_oauth").unwrap(),
        );

        let providers = ProviderStore {
            providers: vec![codex, claude],
            ..ProviderStore::default()
        };
        let share = test_share(ProviderType::GrokOAuth, None);
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);

        assert!(descriptor.support.codex);
        assert!(!descriptor.support.claude);
        assert!(descriptor.app_providers.codex.first().unwrap().enabled);
        let claude_provider = descriptor.app_providers.claude.first().unwrap();
        assert!(!claude_provider.enabled);
        assert_eq!(
            claude_provider.supported_apps,
            ["claude", "codex", "gemini"]
        );
        assert!(!claude_provider.is_current);
    }

    #[test]
    fn static_descriptor_fingerprint_tracks_execution_but_not_display_or_usage() {
        let provider_type = ProviderType::Codex;
        let base_provider = test_provider(provider_type);
        let providers = ProviderStore {
            providers: vec![base_provider.clone()],
            ..ProviderStore::default()
        };
        let share = test_share(provider_type, Some(25.0));
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let baseline = static_descriptor_fingerprint(&descriptor, &providers).unwrap();

        let mut display_provider = base_provider.clone();
        display_provider.provider.name = "renamed for display only".to_string();
        let display_providers = ProviderStore {
            providers: vec![display_provider],
            ..ProviderStore::default()
        };
        let mut usage_only_share = share.clone();
        usage_only_share.tokens_used = 987_654;
        usage_only_share.requests_count = 321;
        usage_only_share.quota_percent = Some(99.0);
        let display_descriptor =
            descriptor_for_share_with_usage(&usage_only_share, &display_providers, None);
        assert_eq!(
            static_descriptor_projection(&descriptor, &providers).unwrap(),
            static_descriptor_projection(&display_descriptor, &display_providers).unwrap()
        );
        assert_eq!(
            baseline,
            static_descriptor_fingerprint(&display_descriptor, &display_providers).unwrap()
        );

        // Every proxied request rewrites the per-grant usage bucket.  If that
        // bucket reached the projection, ordinary traffic would bump the
        // descriptor generation and produce a Router sync storm.
        let mut grant_usage_share = share.clone();
        grant_usage_share.user_grants.insert(
            "owner@example.com".to_string(),
            ShareUserGrant {
                email: "owner@example.com".to_string(),
                role: "owner".to_string(),
                active: true,
                usage: ShareUserUsage {
                    lifetime: ShareUserUsageBucket {
                        started_at_ms: 1,
                        tokens_used: 123_456,
                        requests_count: 789,
                    },
                    ..ShareUserUsage::default()
                },
                created_at_ms: 11,
                updated_at_ms: 22,
                revision: 9,
                ..ShareUserGrant::default()
            },
        );
        let mut grant_baseline_share = share.clone();
        grant_baseline_share.user_grants.insert(
            "owner@example.com".to_string(),
            ShareUserGrant {
                email: "owner@example.com".to_string(),
                role: "owner".to_string(),
                active: true,
                ..ShareUserGrant::default()
            },
        );
        let grant_usage_descriptor =
            descriptor_for_share_with_usage(&grant_usage_share, &providers, None);
        let grant_baseline_descriptor =
            descriptor_for_share_with_usage(&grant_baseline_share, &providers, None);
        assert_eq!(
            static_descriptor_fingerprint(&grant_baseline_descriptor, &providers).unwrap(),
            static_descriptor_fingerprint(&grant_usage_descriptor, &providers).unwrap(),
            "per-grant usage counters must stay out of the static projection"
        );

        let mut rebase_share = share.clone();
        rebase_share.user_grants.insert(
            "owner@example.com".to_string(),
            ShareUserGrant {
                email: "owner@example.com".to_string(),
                role: "owner".to_string(),
                active: true,
                policy: ShareUserPolicy::default(),
                usage: ShareUserUsage::default(),
                usage_rebase: Some(ShareUserUsageRebase {
                    period: ShareTokenPeriod::Lifetime,
                    anchor_at_ms: None,
                    window_starts_at_ms: None,
                    window_ends_at_ms: None,
                    target_tokens: 42,
                    observed_tokens_at_rebase: 10,
                    observed_requests_at_rebase: 1,
                    usage_watermark: 7,
                    applied_at_ms: 1,
                    applied_by: Some("admin@example.com".to_string()),
                    source: ShareUsageRebaseSource::ProviderReset,
                }),
                ..ShareUserGrant::default()
            },
        );
        let rebase_descriptor = descriptor_for_share_with_usage(&rebase_share, &providers, None);
        assert_ne!(
            baseline,
            static_descriptor_fingerprint(&rebase_descriptor, &providers).unwrap()
        );

        let mut scope_descriptor = descriptor.clone();
        let upstream = scope_descriptor
            .upstream_provider
            .as_mut()
            .expect("descriptor upstream provider");
        upstream.model_policy_scope = Some(ModelPolicyScope::Global);
        upstream.model_policy_source = Some(ModelPolicySource::BundleGlobal);
        assert_ne!(
            baseline,
            static_descriptor_fingerprint(&scope_descriptor, &providers).unwrap()
        );

        let mut endpoint_provider = base_provider;
        endpoint_provider.provider.settings_config["env"]["OPENAI_BASE_URL"] =
            Value::String("https://other-upstream.example/v1".to_string());
        let endpoint_providers = ProviderStore {
            providers: vec![endpoint_provider],
            ..ProviderStore::default()
        };
        let endpoint_descriptor =
            descriptor_for_share_with_usage(&share, &endpoint_providers, None);
        assert_ne!(
            baseline,
            static_descriptor_fingerprint(&endpoint_descriptor, &endpoint_providers).unwrap()
        );
    }

    #[test]
    fn descriptor_maps_unlimited_expiry_to_shared_permanent_constant() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
            ..Default::default()
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_maps_unlimited_parallel_limit_to_negative_one() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
            ..Default::default()
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);

        assert_eq!(descriptor.parallel_limit, -1);
    }

    #[test]
    fn descriptor_omits_quota_percent_when_share_has_no_percent() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
            ..Default::default()
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let value = serde_json::to_value(&descriptor).unwrap();
        let provider = &value["appProviders"]["codex"][0];

        assert_eq!(provider["accountEmail"], "owner@example.com");
        assert_eq!(provider["subscriptionLevel"], "pro");
        assert!(provider.get("quotaPercent").is_none());
        assert!(provider.get("quotaBlocked").is_none());
    }

    #[test]
    fn descriptor_uses_account_quota_over_manual_share_fields() {
        let mut share = test_share(ProviderType::CodexOAuth, Some(5.0));
        share.account_email = Some("share-owner@example.com".to_string());
        share.subscription_level = Some("manual".to_string());
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::CodexOAuth)],
            ..Default::default()
        };
        let accounts = AccountStore {
            accounts: vec![test_account(ProviderType::CodexOAuth)],
            ..Default::default()
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.account_email.as_deref(),
            Some("account@example.com")
        );
        assert_eq!(
            provider.subscription_level.as_deref(),
            Some("ChatGPT Pro 20x")
        );
        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some("2026-07-25T04:49:24+00:00")
        );
        assert_eq!(provider.quota_percent, Some(42.0));
        assert_eq!(provider.quota_blocked, Some(false));
    }

    #[test]
    fn descriptor_canonicalizes_cached_grokpro_for_router_nodes() {
        let share = test_share(ProviderType::GrokOAuth, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::GrokOAuth)],
            ..Default::default()
        };
        let mut account = test_account(ProviderType::GrokOAuth);
        account.subscription_level = Some("GrokPro".to_string());
        account.quota.as_mut().unwrap().credential_message = Some("GrokPro".to_string());
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let upstream = descriptor.upstream_provider.as_ref().unwrap();
        let app_provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(upstream.subscription_level.as_deref(), Some("SuperGrok"));
        assert_eq!(
            app_provider.subscription_level.as_deref(),
            Some("SuperGrok")
        );
        assert_eq!(
            upstream.quota.as_ref().unwrap().plan.as_deref(),
            Some("SuperGrok")
        );
        assert_eq!(
            accounts.accounts[0].subscription_level.as_deref(),
            Some("GrokPro")
        );
    }

    #[test]
    fn descriptor_uses_manual_account_billing_expiry_without_changing_share_expiry() {
        let share = test_share(ProviderType::ClaudeOAuth, Some(5.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::ClaudeOAuth)],
            ..Default::default()
        };
        let mut account = test_account(ProviderType::ClaudeOAuth);
        account.quota = None;
        account.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        account.manual_subscription_expiry_updated_at_ms = Some(1_784_000_000_000);
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some("2026-08-17T00:00:00+00:00")
        );
        assert!(provider.subscription_remaining_ms.is_some());
        assert_eq!(provider.quota.as_ref().unwrap().status, "ok");
        assert_eq!(
            provider
                .quota
                .as_ref()
                .and_then(|quota| quota.subscription_period_end.as_deref()),
            Some("2026-08-17T00:00:00+00:00")
        );
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_derives_recurring_account_expiry_for_router_metadata() {
        use crate::domain::accounts::subscription_expiry::{
            resolved_subscription_expiry, SubscriptionExpiryCadence, SubscriptionExpiryRuleDraft,
        };

        let share = test_share(ProviderType::ClaudeOAuth, Some(5.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::ClaudeOAuth)],
            ..Default::default()
        };
        let mut account = test_account(ProviderType::ClaudeOAuth);
        account.quota = None;
        account.manual_subscription_expiry_rule = Some(
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Monthly,
                month: None,
                day: 10,
                time_zone: "Asia/Shanghai".to_string(),
            }
            .into_rule(1_784_000_000_000)
            .unwrap(),
        );
        let expected = Utc
            .timestamp_millis_opt(
                resolved_subscription_expiry(&account)
                    .expires_at_ms
                    .unwrap(),
            )
            .single()
            .unwrap()
            .to_rfc3339();
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            provider
                .quota
                .as_ref()
                .and_then(|quota| quota.subscription_period_end.as_deref()),
            Some(expected.as_str())
        );
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_maps_codex_quota_tiers_for_router_share_card() {
        let share = test_share(ProviderType::CodexOAuth, Some(1.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::CodexOAuth)],
            ..Default::default()
        };
        let mut account = test_account(ProviderType::CodexOAuth);
        account.quota = Some(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Plus".to_string()),
            tiers: vec![AccountQuotaTier {
                name: "five_hour".to_string(),
                utilization: Some(0.01),
                resets_at: Some(1_700_000_000_000),
                ..Default::default()
            }],
            extra_usage: None,
        });
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let runtime = descriptor.app_runtimes.codex.expect("codex runtime");
        let quota = runtime.quota.expect("quota payload");

        assert_eq!(quota.tiers[0].label, "5h");
        assert_eq!(quota.tiers[0].utilization, 1.0);
    }

    #[test]
    fn descriptor_maps_confirmed_provider_failure_to_availability() {
        let share = test_share(ProviderType::Codex, Some(42.0));
        let provider = test_provider(ProviderType::Codex);
        let mut log = UsageLog::new(
            AppKind::Codex,
            provider.provider.id.clone(),
            provider.provider.name.clone(),
            ProviderType::Codex,
            500,
            250,
            UsageModelMetadata::default(),
            Default::default(),
        );
        log.created_at_ms = crate::infra::time::now_ms();
        let mut usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        record_health_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            false,
            Some(500),
            Some("upstream rejected the request"),
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(!availability.available);
        assert_eq!(availability.last_status_code, Some(500));
        assert_eq!(provider.quota_percent, Some(42.0));
        assert_eq!(provider.health.as_ref().unwrap().failures, 1);
    }

    #[test]
    fn descriptor_includes_share_model_health_from_health_check_usage() {
        let share = test_share(ProviderType::Codex, Some(42.0));
        let provider = test_provider(ProviderType::Codex);
        let mut log = UsageLog::new(
            AppKind::Codex,
            provider.provider.id.clone(),
            provider.provider.name.clone(),
            ProviderType::Codex,
            200,
            250,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: None,
                actual_model_source: None,
            },
            Default::default(),
        );
        log.apply_context(UsageLogContext {
            share_id: Some(share.id.clone()),
            share_name: share.display_name.clone(),
            is_health_check: true,
            is_streaming: true,
            stream_status: Some("completed".to_string()),
            ..UsageLogContext::default()
        });
        let mut usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        record_health_snapshot(
            &mut usage,
            &provider,
            ProviderHealthStatus::Healthy,
            false,
            Some(200),
            None,
        );
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let result = descriptor.model_health.codex.first().unwrap();

        assert_eq!(result.requested_model, "gpt-5.5");
        assert_eq!(result.actual_model, "glm-5.2");
        assert_eq!(result.status, "success");
        assert_eq!(result.source, "cc-switch-health-check");
    }

    #[test]
    fn descriptor_waits_for_confirmation_before_disabling_transient_failure() {
        let share = test_share(ProviderType::Codex, None);
        let provider = test_provider(ProviderType::Codex);
        let providers = ProviderStore {
            providers: vec![provider.clone()],
            ..Default::default()
        };
        let mut usage = UsageStore::default();
        let confirmed_at = crate::infra::time::now_ms();
        record_health_snapshot_at(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            true,
            None,
            Some("network unavailable"),
            confirmed_at
                .saturating_sub(crate::domain::health::PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS),
        );

        let first = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        assert!(first.app_availability.codex.unwrap().available);
        assert!(first.upstream_provider.as_ref().unwrap().available.unwrap());
        assert!(first.app_providers.codex[0].available.unwrap());
        assert!(
            first.app_providers.codex[0]
                .health
                .as_ref()
                .unwrap()
                .healthy
        );
        assert_eq!(first.model_health.codex[0].status, "success");

        record_health_snapshot_at(
            &mut usage,
            &provider,
            ProviderHealthStatus::Unhealthy,
            true,
            None,
            Some("network unavailable"),
            confirmed_at,
        );
        let confirmed = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        assert!(!confirmed.app_availability.codex.unwrap().available);
        assert!(!confirmed
            .upstream_provider
            .as_ref()
            .unwrap()
            .available
            .unwrap());
        assert!(!confirmed.app_providers.codex[0].available.unwrap());
        assert!(
            !confirmed.app_providers.codex[0]
                .health
                .as_ref()
                .unwrap()
                .healthy
        );
        assert_eq!(confirmed.model_health.codex[0].status, "failed");
    }

    #[test]
    fn descriptor_marks_fresh_explicit_account_limit_as_blocked() {
        let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
        let share = test_share(ProviderType::CodexOAuth, Some(100.0));
        let provider = test_provider(ProviderType::CodexOAuth);
        let providers = ProviderStore {
            providers: vec![provider],
            ..Default::default()
        };
        let mut account = test_account(ProviderType::CodexOAuth);
        account.quota_percent = Some(100.0);
        account.quota_refreshed_at = Some(now);
        account.quota_next_refresh_at = Some(now + 30 * 60 * 1000);
        account.quota.as_mut().unwrap().extra_usage = Some(json!({
            "subscriptionEvidence": {
                "usageAllowed": false,
                "usageLimitReached": true
            }
        }));
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };
        let usage = UsageStore::default();

        let descriptor = descriptor_for_share_with_accounts_and_usage(
            &share,
            &providers,
            Some(&accounts),
            Some(&usage),
        );
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(!availability.available);
        assert_eq!(availability.quota_blocked, Some(true));
        assert_eq!(provider.quota_percent, Some(100.0));
        assert_eq!(provider.quota_blocked, Some(true));
        assert_eq!(
            provider
                .quota
                .as_ref()
                .and_then(|quota| quota.availability.as_deref()),
            Some("quota_exhausted")
        );
        assert!(provider
            .quota
            .as_ref()
            .and_then(|quota| quota.blocked_until.as_deref())
            .is_some());
    }

    #[test]
    fn descriptor_keeps_share_percent_as_display_only_data() {
        let share = test_share(ProviderType::Codex, Some(100.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::Codex)],
            ..Default::default()
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(availability.available);
        assert_eq!(availability.quota_blocked, None);
        assert_eq!(provider.quota_percent, Some(100.0));
        assert_eq!(provider.quota_blocked, None);
    }

    #[test]
    fn descriptor_maps_nvidia_codex_api_url_and_single_model() {
        let share = test_share(ProviderType::Nvidia, None);
        let providers = ProviderStore {
            providers: vec![StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "p1".to_string(),
                    name: "Nvidia".to_string(),
                    settings_config: json!({
                        "config": "model_provider = \"custom\"\nmodel = \"moonshotai/kimi-k2.5\"\n\n[model_providers.custom]\nname = \"nvidia\"\nbase_url = \"https://integrate.api.nvidia.com/v1\"\n",
                        "modelMapping": {
                            "mode": "single",
                            "upstreamModel": "moonshotai/kimi-k2.5"
                        }
                    }),
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                provider_type: ProviderType::Nvidia,
                provider_type_id: ProviderType::Nvidia.as_str().to_string(),
                resource: Default::default(),
            }],
            ..Default::default()
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let provider = descriptor.app_providers.codex.first().unwrap();
        let runtime = descriptor.app_runtimes.codex.as_ref().unwrap();

        assert_eq!(
            provider.api_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert_eq!(
            runtime.api_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].actual_model, "moonshotai/kimi-k2.5");
        assert_eq!(runtime.models.len(), 1);
        assert_eq!(runtime.models[0].actual_model, "moonshotai/kimi-k2.5");
    }

    #[test]
    fn descriptor_does_not_project_codex_config_model_as_fixed_in_passthrough_mode() {
        let share = test_share(ProviderType::OpenRouter, None);
        let providers = ProviderStore {
            providers: vec![StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "p1".to_string(),
                    name: "OpenRouter".to_string(),
                    settings_config: json!({
                        "config": "model_provider = \"custom\"\nmodel = \"stale-fixed-model\"\n\n[model_providers.custom]\nbase_url = \"https://openrouter.ai/api/v1\"\n",
                        "modelMapping": {"mode": "passthrough"},
                        "models": ["gpt-5.4", {"id": "gpt-5.5"}]
                    }),
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                provider_type: ProviderType::OpenRouter,
                provider_type_id: ProviderType::OpenRouter.as_str().to_string(),
                resource: Default::default(),
            }],
            ..Default::default()
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let provider = descriptor.app_providers.codex.first().unwrap();
        let runtime = descriptor.app_runtimes.codex.as_ref().unwrap();

        assert_eq!(provider.models.len(), 2);
        assert!(provider
            .models
            .iter()
            .all(|model| model.slot == "available"));
        assert!(provider
            .models
            .iter()
            .all(|model| model.actual_model != "stale-fixed-model"));
        let runtime_models = runtime
            .models
            .iter()
            .map(|model| (&model.slot, &model.actual_model))
            .collect::<Vec<_>>();
        let provider_models = provider
            .models
            .iter()
            .map(|model| (&model.slot, &model.actual_model))
            .collect::<Vec<_>>();
        assert_eq!(runtime_models, provider_models);
    }

    fn test_provider(provider_type: ProviderType) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "provider 1".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": "https://upstream.example/v1"
                    },
                    "modelMapping": {
                        "upstreamModel": "glm-5.2",
                        "gpt-5.5": "glm-5.2"
                    },
                    "models": ["glm-5.2"],
                    "testModel": "glm-5.2"
                }),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("managed_account".to_string()),
                        auth_provider: Some(provider_type.as_str().to_string()),
                        account_id: Some("acct-1".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    fn test_share(provider_type: ProviderType, quota_percent: Option<f64>) -> Share {
        Share {
            id: "share-1".to_string(),
            capacity_pool_id: "cp-share-1".to_string(),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_type,
            display_name: Some("codex share".to_string()),
            enabled: true,
            status: "active".to_string(),
            subscription_level: Some("pro".to_string()),
            account_email: Some("owner@example.com".to_string()),
            quota_percent,
            tunnel_subdomain: Some("codex-share".to_string()),
            policy: SharePolicy::default(),
            tokens_used: 0,
            requests_count: 0,
            created_at_ms: 0,
            auto_start: false,
            description: None,
            enabled_apps: None,
            bindings: vec![ShareBinding {
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type,
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
        }
    }

    fn record_health_snapshot(
        usage: &mut UsageStore,
        provider: &StoredProvider,
        status: ProviderHealthStatus,
        transient_failure: bool,
        status_code: Option<u16>,
        error_message: Option<&str>,
    ) {
        record_health_snapshot_at(
            usage,
            provider,
            status,
            transient_failure,
            status_code,
            error_message,
            crate::infra::time::now_ms(),
        );
    }

    fn record_health_snapshot_at(
        usage: &mut UsageStore,
        provider: &StoredProvider,
        status: ProviderHealthStatus,
        transient_failure: bool,
        status_code: Option<u16>,
        error_message: Option<&str>,
        checked_at_ms: u128,
    ) {
        usage.provider_health.record(ProviderHealthObservation {
            app: provider.app,
            provider_id: provider.provider.id.clone(),
            provider_revision: provider.resource.revision,
            runtime_fingerprint: "runtime-test".to_string(),
            status,
            checked_at_ms,
            source: "cc-switch-health-check".to_string(),
            status_code,
            latency_ms: Some(250),
            model: Some("gpt-5.5".to_string()),
            error_category: (!status.is_success()).then(|| "network".to_string()),
            error_message: error_message.map(str::to_string),
            transient_failure,
        });
    }

    fn test_account(provider_type: ProviderType) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("account@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: None,
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: Some("ChatGPT Pro 20x".to_string()),
            entitlement_status: None,
            quota_percent: Some(42.0),
            quota: Some(AccountQuota {
                success: true,
                credential_message: Some("ChatGPT Pro 20x".to_string()),
                tiers: Vec::new(),
                extra_usage: Some(json!({
                    "subscription": {
                        "expiresAt": "2026-07-25T04:49:24+00:00"
                    }
                })),
            }),
            quota_refreshed_at: Some(1_000),
            quota_next_refresh_at: Some(2_000),
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capacity_pool_limits: Default::default(),
            capability_observations: Default::default(),
        }
    }
}
