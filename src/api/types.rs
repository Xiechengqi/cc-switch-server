use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::build_info::BuildInfo;
use crate::clients::router::client::RouterRegisterResult;
use crate::domain::accounts::login::{OAuthLoginFinish, OAuthLoginStart};
use crate::domain::accounts::oauth::{OAuthHttpRequest, OAuthQuotaStrategy, OAuthSupportStage};
use crate::domain::accounts::store::{Account, AccountQuota};
use crate::domain::failover::{FailoverAppConfig, FailoverSnapshot};
use crate::domain::health::ProviderHealth;
use crate::domain::providers::model::{AppKind, Provider, ProviderType};
use crate::domain::providers::store::StoredProvider;
use crate::domain::providers::universal::{
    UniversalProvider, UniversalProviderPreset, UniversalProviderSyncResult,
};
use crate::domain::settings::config::{mask_proxy_url, RouterConfig, ServerConfig};
use crate::domain::sharing::shares::{Share, ShareAcl, ShareBinding, ShareMarketGrantStatus};
use crate::domain::usage::pricing::ModelPricingEntry;
use crate::domain::usage::store::{
    ModelUsageStats, ProviderUsageStats, UsageLog, UsageLogFilter, UsageRollup, UsageStatsFilter,
    UsageTrendPoint,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct HealthResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) config_dir: String,
    pub(in crate::api) web_dist_dir: Option<String>,
    pub(in crate::api) embedded_web_assets: usize,
    pub(in crate::api) unix_ms: u128,
}

pub(in crate::api) type VersionResponse = BuildInfo;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupStatusResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) needs_setup: bool,
    pub(in crate::api) owner_email: Option<String>,
    pub(in crate::api) router_url: Option<String>,
    pub(in crate::api) client_tunnel_subdomain: Option<String>,
}

impl SetupStatusResponse {
    pub(in crate::api) fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            needs_setup: !config.is_setup_complete(),
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) owner_email: Option<String>,
    pub(in crate::api) router_url: Option<String>,
    pub(in crate::api) client_tunnel_subdomain: Option<String>,
    pub(in crate::api) message: &'static str,
}

impl SetupResponse {
    pub(in crate::api) fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            message: "setup complete; use password login to enter cc-switch-server",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct LoginRequest {
    #[serde(default = "default_password_method")]
    pub(in crate::api) method: String,
    #[serde(default)]
    pub(in crate::api) password: String,
    #[serde(default)]
    pub(in crate::api) api_token: Option<String>,
    #[serde(default)]
    pub(in crate::api) email: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ChangePasswordRequest {
    pub(in crate::api) new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ChangePasswordResponse {
    pub(in crate::api) ok: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EmailLoginCodeRequest {
    pub(in crate::api) email: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EmailLoginVerifyCodeRequest {
    pub(in crate::api) email: String,
    pub(in crate::api) code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebPasswordRequest {
    pub(in crate::api) password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebSessionRefreshRequest {
    pub(in crate::api) refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebPasswordChangeRequest {
    pub(in crate::api) current_password: String,
    pub(in crate::api) new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct LoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) token: String,
    pub(in crate::api) token_type: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ApiTokenResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) api_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AuthMeResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) owner_email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EventQuery {
    #[serde(default)]
    pub(in crate::api) token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct CreateBackupRequest {
    #[serde(default)]
    pub(in crate::api) reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupListResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) backups: Vec<crate::infra::backup::BackupManifest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupCreateResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) backup: crate::infra::backup::BackupManifest,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct BackupRestoreResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) result: crate::infra::backup::BackupRestoreResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ConfigSnapshotResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) owner_email: Option<String>,
    pub(in crate::api) router_url: Option<String>,
    pub(in crate::api) client_tunnel_subdomain: Option<String>,
    pub(in crate::api) upstream_proxy: UpstreamProxyView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpstreamProxyResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) upstream_proxy: UpstreamProxyView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpstreamProxyView {
    pub(in crate::api) enabled: bool,
    pub(in crate::api) url: Option<String>,
    pub(in crate::api) masked_url: Option<String>,
    pub(in crate::api) follow_system_proxy: bool,
}

impl UpstreamProxyView {
    pub(in crate::api) fn from_config(config: &ServerConfig) -> Self {
        let url = config.upstream_proxy.url.clone();
        Self {
            enabled: url.as_deref().is_some_and(|value| !value.trim().is_empty()),
            masked_url: url.as_deref().map(mask_proxy_url),
            url,
            follow_system_proxy: config.upstream_proxy.follow_system_proxy,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterConfigResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) router: RouterConfigView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterConfigView {
    pub(in crate::api) url: Option<String>,
    pub(in crate::api) api_base: Option<String>,
    pub(in crate::api) domain: Option<String>,
    pub(in crate::api) region: Option<String>,
    pub(in crate::api) ssh_host: Option<String>,
    pub(in crate::api) ssh_user: Option<String>,
    pub(in crate::api) custom: bool,
    pub(in crate::api) installation_id: Option<String>,
    public_key: Option<String>,
    pub(in crate::api) control_secret_present: bool,
    pub(in crate::api) last_register_error: Option<String>,
    pub(in crate::api) last_registered_at_ms: Option<i64>,
}

impl RouterConfigView {
    pub(in crate::api) fn from_config(config: &RouterConfig) -> Self {
        Self {
            url: config.url.clone(),
            api_base: config.api_base.clone(),
            domain: config.domain.clone(),
            region: config.region.clone(),
            ssh_host: config.ssh_host.clone(),
            ssh_user: config.ssh_user.clone(),
            custom: config.custom,
            installation_id: config
                .identity
                .as_ref()
                .map(|identity| identity.installation_id.clone()),
            public_key: config
                .identity
                .as_ref()
                .map(|identity| identity.public_key.clone()),
            control_secret_present: config
                .identity
                .as_ref()
                .and_then(|identity| identity.control_secret.as_ref())
                .is_some(),
            last_register_error: config.last_register_error.clone(),
            last_registered_at_ms: config.last_registered_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterRegisterResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) registration: RouterRegisterResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ClientTunnelResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) tunnel_subdomain: Option<String>,
    pub(in crate::api) tunnel_status: Option<String>,
    pub(in crate::api) last_heartbeat_ms: Option<u128>,
    pub(in crate::api) runtime_status: Option<crate::clients::router::tunnel::TunnelRuntimeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) remote_tunnel: Option<crate::clients::router::client::ClientTunnelView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) remote_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ClientTunnelClaimResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) status: String,
    pub(in crate::api) error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ClientTunnelLeaseResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) status: Option<crate::clients::router::tunnel::TunnelRuntimeStatus>,
    pub(in crate::api) message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterTunnelsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) tunnels: Vec<crate::clients::router::tunnel::TunnelRuntimeStatus>,
}

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
    pub(in crate::api) support: proxy::adapters::AdapterSupport,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListAccountsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) accounts: Vec<Account>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertAccountResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account: Account,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountCapabilitiesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) capabilities:
        Vec<crate::domain::accounts::managers::AccountManagerCapability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountImportTemplatesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) templates: Vec<crate::domain::accounts::managers::AccountImportTemplate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartAccountLoginRequest {
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    #[serde(default)]
    pub(in crate::api) redirect_uri: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartAccountLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) login: OAuthLoginStart,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCopilotDeviceLoginRequest {
    #[serde(default)]
    pub(in crate::api) github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartCopilotDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::copilot_device::GitHubDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCopilotDeviceLoginRequest {
    pub(in crate::api) device_code: String,
    #[serde(default)]
    pub(in crate::api) github_domain: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollCopilotDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartKiroDeviceLoginRequest {
    #[serde(default)]
    pub(in crate::api) region: Option<String>,
    #[serde(default)]
    pub(in crate::api) start_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct StartKiroDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) device: crate::clients::oauth::kiro_device::KiroDeviceCodeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollKiroDeviceLoginRequest {
    pub(in crate::api) device_code: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PollKiroDeviceLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) pending: bool,
    pub(in crate::api) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountLoginCallbackQuery {
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) state: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
    #[serde(default)]
    pub(in crate::api) error: Option<String>,
    #[serde(default, alias = "error_description")]
    pub(in crate::api) error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FinishAccountLoginRequest {
    #[serde(default)]
    pub(in crate::api) session_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) state: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
    #[serde(default)]
    pub(in crate::api) execute_token_exchange: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct FinishAccountLoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) login: OAuthLoginFinish,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<AccountLoginAccountSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountLoginAccountSummary {
    pub(in crate::api) id: String,
    pub(in crate::api) provider_type: ProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) expires_at: Option<i64>,
    pub(in crate::api) has_access_token: bool,
    pub(in crate::api) has_refresh_token: bool,
    pub(in crate::api) scopes: Vec<String>,
}

impl AccountLoginAccountSummary {
    pub(in crate::api) fn from_account(account: &Account) -> Self {
        Self {
            id: account.id.clone(),
            provider_type: account.provider_type,
            email: account.email.clone(),
            subscription_level: account.subscription_level.clone(),
            expires_at: account.expires_at,
            has_access_token: account
                .access_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            has_refresh_token: account
                .refresh_token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            scopes: account.scopes.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountQuotaResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) quota: Option<AccountQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) account: Option<Account>,
    pub(in crate::api) refreshed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) next_refresh_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountQuotaQuery {
    #[serde(default)]
    pub(in crate::api) refresh: Option<bool>,
    #[serde(default)]
    pub(in crate::api) force: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AccountRefreshPlanResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) account_id: String,
    pub(in crate::api) provider_type: crate::domain::providers::model::ProviderType,
    pub(in crate::api) refresh_required: bool,
    pub(in crate::api) server_native_stage: Option<OAuthSupportStage>,
    pub(in crate::api) quota_strategy: Option<OAuthQuotaStrategy>,
    pub(in crate::api) refresh_request: Option<OAuthHttpRequest>,
    pub(in crate::api) profile_request: Option<OAuthHttpRequest>,
    pub(in crate::api) message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct DeleteResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted: bool,
}

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportSharesRequest {
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) imported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertShareResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share: Share,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareConnectInfoResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share_id: String,
    pub(in crate::api) direct_url: String,
    pub(in crate::api) subdomain: String,
    pub(in crate::api) router_domain: String,
    pub(in crate::api) snippets: Vec<ShareConnectSnippet>,
    pub(in crate::api) note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareConnectSnippet {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) title: String,
    pub(in crate::api) env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareSubdomainRequest {
    pub(in crate::api) subdomain: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareSubdomainResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) remote_claimed: bool,
    pub(in crate::api) share: Share,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ReplaceShareAclRequest {
    pub(in crate::api) acl: ShareAcl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareBindingRequest {
    pub(in crate::api) binding: ShareBinding,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareMarketGrantRequest {
    pub(in crate::api) market_grant: Option<ShareMarketGrantStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PublicShareMarket {
    pub(in crate::api) id: String,
    pub(in crate::api) display_name: String,
    pub(in crate::api) email: String,
    pub(in crate::api) subdomain: String,
    public_base_url: Option<String>,
    pub(in crate::api) market_kind: String,
    pub(in crate::api) status: String,
    #[serde(default)]
    pub(in crate::api) scopes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListShareMarketsResponse {
    #[serde(default)]
    pub(in crate::api) ok: bool,
    pub(in crate::api) markets: Vec<PublicShareMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AuthorizeShareMarketRequest {
    pub(in crate::api) market_email: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterStatusResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) registered: bool,
    pub(in crate::api) last_error: Option<String>,
    pub(in crate::api) last_heartbeat_ms: Option<u128>,
    pub(in crate::api) pending_request_log_sync: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterDiagnosticsResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) router: RouterConfigView,
    pub(in crate::api) registered: bool,
    pub(in crate::api) last_error: Option<String>,
    pub(in crate::api) last_heartbeat_ms: Option<u128>,
    pub(in crate::api) pending_request_log_sync: usize,
    pub(in crate::api) tunnels: Vec<crate::clients::router::tunnel::TunnelRuntimeStatus>,
    pub(in crate::api) share_sync: Vec<ShareSyncDiagnostic>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareSyncDiagnostic {
    pub(in crate::api) share_id: String,
    pub(in crate::api) share_name: String,
    pub(in crate::api) status: String,
    pub(in crate::api) enabled: bool,
    pub(in crate::api) router_last_synced_at_ms: Option<u128>,
    pub(in crate::api) router_last_sync_error: Option<String>,
    pub(in crate::api) router_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterBatchSyncResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) synced: usize,
    pub(in crate::api) remote_synced: bool,
    pub(in crate::api) message: String,
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterShareEditPullResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) summary: crate::state::ShareEditSyncSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RouterDeleteAllSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ProxyCapabilitiesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) capabilities: Vec<proxy::adapters::AdapterCapability>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModelsResponse {
    pub(in crate::api) object: &'static str,
    pub(in crate::api) data: Vec<OpenAiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModel {
    pub(in crate::api) id: String,
    pub(in crate::api) object: &'static str,
    pub(in crate::api) owned_by: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct GeminiModelsResponse {
    pub(in crate::api) models: Vec<GeminiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct GeminiModel {
    pub(in crate::api) name: String,
    pub(in crate::api) version: String,
    pub(in crate::api) display_name: String,
    pub(in crate::api) description: String,
    pub(in crate::api) input_token_limit: u32,
    pub(in crate::api) output_token_limit: u32,
    pub(in crate::api) supported_generation_methods: Vec<String>,
}

impl ConfigSnapshotResponse {
    pub(in crate::api) fn from_config(config: &ServerConfig) -> Self {
        Self {
            ok: true,
            owner_email: config.owner.email.clone(),
            router_url: config.router.url.clone(),
            client_tunnel_subdomain: config.client.tunnel_subdomain.clone(),
            upstream_proxy: UpstreamProxyView::from_config(config),
        }
    }
}

fn default_password_method() -> String {
    "password".to_string()
}
