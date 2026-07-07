use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::usage::pricing::ModelPricingEntry;
use crate::domain::usage::store::{
    ModelUsageStats, ProviderUsageStats, UsageLog, UsageLogFilter, UsageRollup, UsageStatsFilter,
    UsageTrendPoint,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageLogsQuery {
    #[serde(default)]
    pub(in crate::api) limit: Option<usize>,
    #[serde(default)]
    pub(in crate::api) from_ms: Option<u128>,
    #[serde(default)]
    pub(in crate::api) to_ms: Option<u128>,
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) provider_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) share_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) user_email: Option<String>,
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) data_source: Option<String>,
    #[serde(default)]
    pub(in crate::api) is_health_check: Option<bool>,
    #[serde(default)]
    pub(in crate::api) stream_status: Option<String>,
}

impl From<UsageLogsQuery> for UsageLogFilter {
    fn from(query: UsageLogsQuery) -> Self {
        Self {
            limit: query.limit,
            from_ms: query.from_ms,
            to_ms: query.to_ms,
            app: query.app,
            provider_id: query.provider_id,
            share_id: query.share_id,
            user_email: query.user_email,
            session_id: query.session_id,
            data_source: query.data_source,
            is_health_check: query.is_health_check,
            stream_status: query.stream_status,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageStatsQuery {
    #[serde(default)]
    pub(in crate::api) limit: Option<usize>,
    #[serde(default)]
    pub(in crate::api) from_ms: Option<u128>,
    #[serde(default)]
    pub(in crate::api) to_ms: Option<u128>,
    #[serde(default)]
    pub(in crate::api) window_ms: Option<u128>,
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) provider_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) share_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) user_email: Option<String>,
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) data_source: Option<String>,
    #[serde(default)]
    pub(in crate::api) is_health_check: Option<bool>,
    #[serde(default)]
    pub(in crate::api) stream_status: Option<String>,
}

impl From<UsageStatsQuery> for UsageStatsFilter {
    fn from(query: UsageStatsQuery) -> Self {
        Self {
            limit: query.limit,
            from_ms: query.from_ms,
            to_ms: query.to_ms,
            window_ms: query.window_ms,
            app: query.app,
            provider_id: query.provider_id,
            share_id: query.share_id,
            user_email: query.user_email,
            session_id: query.session_id,
            data_source: query.data_source,
            is_health_check: query.is_health_check,
            stream_status: query.stream_status,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageLogsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) logs: Vec<UsageLog>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageLogDetailResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) log: UsageLog,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageSummaryResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) summary: UsageRollup,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageTrendsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) trends: Vec<UsageTrendPoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageProviderStatsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<ProviderUsageStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageModelStatsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) models: Vec<ModelUsageStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageBackfillResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) updated: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageRouterSyncRetryResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) attempted: usize,
    pub(in crate::api) synced: usize,
    pub(in crate::api) failed: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelPricingListResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) models: Vec<ModelPricingEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelPricingUpdateResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) model: ModelPricingEntry,
    pub(in crate::api) backfilled: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelPricingDeleteResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderLimitsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderLimitsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) limits: Vec<ProviderLimitStatusView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderLimitResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) limit: ProviderLimitStatusView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderLimitStatusView {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) provider_name: String,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) daily_usage_usd: f64,
    pub(in crate::api) daily_limit_usd: Option<f64>,
    pub(in crate::api) daily_exceeded: bool,
    pub(in crate::api) monthly_usage_usd: f64,
    pub(in crate::api) monthly_limit_usd: Option<f64>,
    pub(in crate::api) monthly_exceeded: bool,
    pub(in crate::api) account_id: Option<String>,
    pub(in crate::api) account_email: Option<String>,
    pub(in crate::api) account_quota_percent: Option<f64>,
    pub(in crate::api) account_quota_refreshed_at: Option<i64>,
    pub(in crate::api) account_last_refresh_error: Option<String>,
    pub(in crate::api) quota_dispatch_limit_percent: Option<f64>,
    pub(in crate::api) quota_dispatch_exceeded: bool,
    pub(in crate::api) shares: Vec<ShareLimitStatusView>,
    pub(in crate::api) warnings: Vec<String>,
    pub(in crate::api) blocked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareLimitStatusView {
    pub(in crate::api) share_id: String,
    pub(in crate::api) share_name: String,
    pub(in crate::api) status: String,
    pub(in crate::api) enabled: bool,
    pub(in crate::api) token_limit: Option<u64>,
    pub(in crate::api) tokens_used: u64,
    pub(in crate::api) parallel_limit: Option<u32>,
    pub(in crate::api) expires_at: Option<i64>,
    pub(in crate::api) token_exceeded: bool,
    pub(in crate::api) expired: bool,
    pub(in crate::api) blocked: bool,
    pub(in crate::api) warnings: Vec<String>,
}
