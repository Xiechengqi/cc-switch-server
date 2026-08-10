use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::usage::query::{
    ModelUsage, ProviderBundleUsage, ShareUsage, UsageFacets, UsageOverview, UsageTrendPoint,
};
use crate::domain::usage::store::{UsageLog, UsageOutcome, UsageState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageQueryParams {
    #[serde(default)]
    pub(in crate::api) from_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::api) to_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) bundle_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) share_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) user_email: Option<String>,
    #[serde(default)]
    pub(in crate::api) actual_model: Option<String>,
    #[serde(default)]
    pub(in crate::api) outcome: Option<UsageOutcome>,
    #[serde(default)]
    pub(in crate::api) usage_state: Option<UsageState>,
    #[serde(default)]
    pub(in crate::api) window_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::api) cursor: Option<String>,
    #[serde(default)]
    pub(in crate::api) limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageResponseMeta {
    pub(in crate::api) from_ms: u128,
    pub(in crate::api) to_ms: u128,
    pub(in crate::api) generated_at_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageDataResponse<T> {
    pub(in crate::api) data: T,
    pub(in crate::api) meta: UsageResponseMeta,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageRequestPageMeta {
    pub(in crate::api) from_ms: u128,
    pub(in crate::api) to_ms: u128,
    pub(in crate::api) generated_at_ms: u128,
    pub(in crate::api) total: usize,
    pub(in crate::api) next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageRequestPageResponse {
    pub(in crate::api) data: Vec<UsageLog>,
    pub(in crate::api) meta: UsageRequestPageMeta,
}

pub(in crate::api) type UsageOverviewResponse = UsageDataResponse<UsageOverview>;
pub(in crate::api) type UsageTrendsResponse = UsageDataResponse<Vec<UsageTrendPoint>>;
pub(in crate::api) type UsageFacetsResponse = UsageDataResponse<UsageFacets>;
pub(in crate::api) type UsageProviderBundlesResponse = UsageDataResponse<Vec<ProviderBundleUsage>>;
pub(in crate::api) type UsageModelsResponse = UsageDataResponse<Vec<ModelUsage>>;
pub(in crate::api) type UsageSharesResponse = UsageDataResponse<Vec<ShareUsage>>;
pub(in crate::api) type UsageRequestDetailResponse = UsageDataResponse<UsageLog>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UsageRouterSyncRetryResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) attempted: usize,
    pub(in crate::api) synced: usize,
    pub(in crate::api) failed: usize,
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
    pub(in crate::api) account_id: Option<String>,
    pub(in crate::api) account_email: Option<String>,
    pub(in crate::api) account_quota_percent: Option<f64>,
    pub(in crate::api) account_quota_refreshed_at: Option<i64>,
    pub(in crate::api) account_last_refresh_error: Option<String>,
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
