use std::collections::BTreeMap;

use crate::domain::health::ProviderHealth;
use crate::domain::providers::credentials::ProviderImportPreview;
use crate::domain::providers::credentials::{
    CredentialPatch, ProviderAccountBindingMigrationPreview, ProviderIdentityChangePreview,
    ProviderView,
};
use crate::domain::providers::model::{AppKind, Provider, ProviderType};
use crate::domain::providers::registry::{
    CustomBindingInput, ProfileId, ProviderKey, ProviderRegistry,
};
use crate::proxy::adapters::AdapterSupport;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub(in crate::api) providers: Vec<ProviderView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderHealthResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<ProviderHealth>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct CreateProviderRequest {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider: Provider,
    #[serde(default)]
    pub(in crate::api) profile_id: Option<ProfileId>,
    #[serde(default)]
    pub(in crate::api) custom_binding: Option<CustomBindingInput>,
    #[serde(default)]
    pub(in crate::api) client_request_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) credential_patches: BTreeMap<String, CredentialPatch>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) stored: ProviderView,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct ProviderResourceQuery {
    pub(in crate::api) app: AppKind,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct DeleteProviderQuery {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct UpdateProviderRequest {
    pub(in crate::api) provider: Provider,
    #[serde(default)]
    pub(in crate::api) profile_id: Option<ProfileId>,
    #[serde(default)]
    pub(in crate::api) custom_binding: Option<CustomBindingInput>,
    pub(in crate::api) expected_revision: u64,
    #[serde(default)]
    pub(in crate::api) credential_patches: BTreeMap<String, CredentialPatch>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct DeleteProviderResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderDeletePreviewResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) preview: crate::domain::providers::credentials::ProviderReferencePreview,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ExportProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) providers: Vec<ProviderView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct ImportProviderItem {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider: Provider,
    #[serde(default)]
    pub(in crate::api) profile_id: Option<ProfileId>,
    #[serde(default)]
    pub(in crate::api) custom_binding: Option<CustomBindingInput>,
    #[serde(default)]
    pub(in crate::api) expected_revision: Option<u64>,
    #[serde(default)]
    pub(in crate::api) client_request_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) credential_patches: BTreeMap<String, CredentialPatch>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::api) enum ProviderImportMode {
    Preview,
    Apply,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::api) enum ProviderActionMode {
    Preview,
    Apply,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct AdoptProviderProfileRequest {
    pub(in crate::api) mode: ProviderActionMode,
    pub(in crate::api) expected_revision: u64,
    pub(in crate::api) profile_id: ProfileId,
    #[serde(default)]
    pub(in crate::api) account_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) preview_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct RebindCustomProviderRequest {
    pub(in crate::api) mode: ProviderActionMode,
    pub(in crate::api) expected_revision: u64,
    pub(in crate::api) custom_binding: CustomBindingInput,
    #[serde(default)]
    pub(in crate::api) credential_patches: BTreeMap<String, CredentialPatch>,
    #[serde(default)]
    pub(in crate::api) preview_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct CloneProviderAsCustomRequest {
    pub(in crate::api) mode: ProviderActionMode,
    pub(in crate::api) expected_revision: u64,
    pub(in crate::api) target_provider_id: String,
    pub(in crate::api) target_name: String,
    pub(in crate::api) custom_binding: CustomBindingInput,
    pub(in crate::api) client_request_id: String,
    #[serde(default)]
    pub(in crate::api) preview_token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderIdentityActionResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) mode: ProviderActionMode,
    pub(in crate::api) preview: ProviderIdentityChangePreview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) stored: Option<ProviderView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct ApplyProviderAccountBindingMigrationRequest {
    pub(in crate::api) preview_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderAccountBindingMigrationResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) preview: ProviderAccountBindingMigrationPreview,
    pub(in crate::api) applied: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct ImportProvidersRequest {
    pub(in crate::api) mode: ProviderImportMode,
    #[serde(default)]
    pub(in crate::api) preview_token: Option<String>,
    pub(in crate::api) providers: Vec<ImportProviderItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportProvidersResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) mode: ProviderImportMode,
    pub(in crate::api) preview: ProviderImportPreview,
    pub(in crate::api) imported: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FetchProviderModelsRequest {
    pub(in crate::api) app: AppKind,
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
    pub(in crate::api) outcome: ProviderOperationOutcome,
    pub(in crate::api) driver_id: String,
    pub(in crate::api) runtime_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) message: Option<String>,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) provider_revision: u64,
    pub(in crate::api) url: String,
    pub(in crate::api) merged: bool,
    pub(in crate::api) merged_count: usize,
    pub(in crate::api) models: Vec<FetchedProviderModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) provider: Option<ProviderView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct TestProviderQuery {
    pub(in crate::api) app: AppKind,
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
    pub(in crate::api) provider_keys: Option<Vec<ProviderKey>>,
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
    pub(in crate::api) outcome: ProviderOperationOutcome,
    pub(in crate::api) driver_id: String,
    pub(in crate::api) runtime_fingerprint: String,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    pub(in crate::api) provider_revision: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::api) enum ProviderOperationOutcome {
    Success,
    Unsupported,
    InvalidConfig,
    MissingCredential,
    Auth,
    RateLimit,
    Quota,
    Timeout,
    Network,
    Upstream,
    Protocol,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateProviderFromPresetRequest {
    pub(in crate::api) app: AppKind,
    #[serde(default)]
    pub(in crate::api) profile_id: Option<ProfileId>,
    #[serde(default)]
    pub(in crate::api) name: Option<String>,
    #[serde(default)]
    pub(in crate::api) client_request_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) account_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) custom_binding: Option<CustomBindingInput>,
    #[serde(default)]
    pub(in crate::api) credential_patches: BTreeMap<String, CredentialPatch>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProviderRegistryResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) registry: ProviderRegistry,
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
