use crate::domain::failover::{FailoverAppConfig, FailoverSnapshot};
use crate::domain::health::ProviderHealth;
use crate::domain::providers::model::{AppKind, Provider, ProviderType};
use crate::domain::providers::store::StoredProvider;
use crate::domain::providers::universal::{
    UniversalProvider, UniversalProviderPreset, UniversalProviderSyncResult,
};
use crate::proxy::adapters::AdapterSupport;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListProvidersQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<StoredProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderHealthResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<ProviderHealth>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FailoverResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) failover: FailoverSnapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateFailoverAppResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) app: AppKind,
    pub(in crate::api) config: FailoverAppConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FailoverProviderResetQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ResetFailoverProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) breaker: crate::domain::failover::ProviderBreaker,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateProviderRequest {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider: Provider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) stored: StoredProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ExportProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<StoredProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportProviderItem {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider: Provider,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportProvidersRequest {
    pub(in crate::api) providers: Vec<ImportProviderItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) imported: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FetchProviderModelsRequest {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) merge: Option<bool>,
    #[serde(default)]
    pub(in crate::api) timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FetchedProviderModel {
    pub(in crate::api) id: String,
    pub(in crate::api) upstream_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) display_name: Option<String>,
    pub(in crate::api) raw: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FetchProviderModelsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) url: String,
    pub(in crate::api) merged: bool,
    pub(in crate::api) merged_count: usize,
    pub(in crate::api) models: Vec<FetchedProviderModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) provider: Option<StoredProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListUniversalProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: BTreeMap<String, UniversalProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UniversalProviderPresetsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) presets: Vec<UniversalProviderPreset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ExportUniversalProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<UniversalProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportUniversalProvidersRequest {
    pub(in crate::api) providers: Vec<UniversalProvider>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportUniversalProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) imported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct GetUniversalProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) provider: Option<UniversalProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertUniversalProviderRequest {
    pub(in crate::api) provider: UniversalProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertUniversalProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) provider: UniversalProvider,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SyncUniversalProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) result: UniversalProviderSyncResult,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct TestProviderQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) network: Option<bool>,
    #[serde(default)]
    pub(in crate::api) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::api) model: Option<String>,
    #[serde(default)]
    pub(in crate::api) stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct TestProvidersRequest {
    #[serde(default)]
    pub(in crate::api) provider_ids: Option<Vec<String>>,
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) network: Option<bool>,
    #[serde(default)]
    pub(in crate::api) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(in crate::api) model: Option<String>,
    #[serde(default)]
    pub(in crate::api) stream: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct TestProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) results: Vec<TestProviderResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct TestProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    pub(in crate::api) adapter: &'static str,
    pub(in crate::api) support: AdapterSupport,
    pub(in crate::api) endpoint: String,
    pub(in crate::api) model: String,
    pub(in crate::api) stream: bool,
    pub(in crate::api) header_names: Vec<String>,
    pub(in crate::api) network_checked: bool,
    pub(in crate::api) network_status_code: Option<u16>,
    pub(in crate::api) network_latency_ms: Option<u128>,
    pub(in crate::api) network_stream_completed: Option<bool>,
    pub(in crate::api) network_error: Option<String>,
    pub(in crate::api) message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateProviderFromPresetRequest {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderPresetsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderPresetsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) presets: Vec<crate::api::web::coverage::PresetSummary>,
}
