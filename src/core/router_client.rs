#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::accounts::{Account, AccountStore};
use crate::core::config::{RouterIdentity, ServerConfig};
use crate::core::health;
use crate::core::model_health::ShareModelHealthSummary;
use crate::core::provider::AppKind;
use crate::core::providers::{ProviderStore, StoredProvider};
use crate::core::shares::{Share, ShareMarketGrantStatus};
use crate::core::usage::UsageStore;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationRequest {
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResponse {
    pub installation_id: String,
    #[serde(default)]
    pub control_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelConfig {
    pub owner_email: String,
    pub subdomain: String,
    #[serde(default = "default_tunnel_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedRequest<T> {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelPayload {
    pub tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelClaimRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeasePayload {
    pub requested_subdomain: String,
    pub tunnel_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueLeaseRequest {
    installation_id: String,
    requested_subdomain: String,
    tunnel_type: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareBatchSyncRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    ops: Vec<ShareSyncOperation>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRequestLogBatchSyncRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    logs: Vec<ShareRequestLogEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_market_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_access_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_with_emails: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_by_app: Option<BTreeMap<String, ShareAppAccess>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_settings: Option<BTreeMap<String, ShareAppSettings>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale_official_price_percent_by_app: Option<BTreeMap<String, u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditView {
    pub id: String,
    pub share_id: String,
    pub installation_id: String,
    pub revision: i64,
    pub status: String,
    pub patch: ShareSettingsPatch,
    pub created_by_email: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePendingEditsPayload {
    #[serde(default)]
    pub share_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePendingEditsResponse {
    #[serde(default)]
    pub edits: Vec<ShareEditView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAckPayload {
    pub edit_id: String,
    pub revision: i64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAckEnvelope {
    pub ack: ShareEditAckPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditEventSignaturePayload {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAvailableEvent {
    pub kind: String,
    pub installation_id: String,
    pub share_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareClaimSubdomainRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    claim: ShareClaimPayload,
    share: ShareDescriptor,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareClaimPayload {
    share_id: String,
    subdomain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseResponse {
    pub lease_id: String,
    pub connection_id: String,
    pub ssh_username: String,
    pub ssh_password: String,
    pub ssh_addr: String,
    pub expires_at: String,
    pub tunnel_url: String,
    pub subdomain: String,
    #[serde(default)]
    pub ssh_host_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptor {
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_grant: Option<ShareMarketGrantStatus>,
    #[serde(default)]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    pub subdomain: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAccess {
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppSettings {
    #[serde(default)]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default)]
    pub token_limit: i64,
    #[serde(default = "default_parallel_limit")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub expires_at: String,
}

impl Default for ShareAppSettings {
    fn default() -> Self {
        Self {
            for_sale: default_share_for_sale(),
            sale_market_kind: default_sale_market_kind(),
            market_access_mode: default_market_access_mode(),
            shared_with_emails: Vec::new(),
            token_limit: -1,
            parallel_limit: default_parallel_limit(),
            expires_at: String::new(),
        }
    }
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
pub struct ShareUpstreamProvider {
    pub kind: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
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
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
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
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub enabled: bool,
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
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
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
    pub request_id: String,
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
    pub status_code: u16,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub is_health_check: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterRegisterResult {
    pub installation_id: String,
    pub public_key: String,
    pub control_secret_present: bool,
    pub registered_at_ms: i64,
}

pub async fn register_installation(
    http: &reqwest::Client,
    config: &mut ServerConfig,
) -> anyhow::Result<RouterRegisterResult> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let mut identity = config
        .router
        .identity
        .clone()
        .unwrap_or_else(generate_identity_without_installation);

    let request = RegisterInstallationRequest {
        public_key: identity.public_key.clone(),
        platform: std::env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        instance_nonce: nonce(),
    };
    let response = http
        .post(format!("{api_base}/v1/installations/register"))
        .json(&request)
        .send()
        .await
        .context("send router installation register")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("router installation register failed: {status}: {body}");
    }

    let registered = response
        .json::<RegisterInstallationResponse>()
        .await
        .context("parse router installation register response")?;
    identity.installation_id = registered.installation_id;
    identity.control_secret = registered.control_secret;
    let registered_at_ms = now_ms();
    let result = RouterRegisterResult {
        installation_id: identity.installation_id.clone(),
        public_key: identity.public_key.clone(),
        control_secret_present: identity.control_secret.is_some(),
        registered_at_ms,
    };
    config.router.identity = Some(identity);
    config.router.last_register_error = None;
    config.router.last_registered_at_ms = Some(registered_at_ms);
    Ok(result)
}

pub async fn claim_client_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
    tunnel: ClientTunnelConfig,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(
        identity,
        "client_tunnel_claim",
        &tunnel,
        timestamp_ms,
        &nonce,
    )?;
    let request = ClientTunnelClaimRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        tunnel,
    };
    let response = http
        .post(format!("{api_base}/v1/installations/client-tunnel/claim"))
        .json(&request)
        .send()
        .await
        .context("send router client tunnel claim")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router client tunnel claim failed: {status}: {body}");
}

pub async fn issue_client_web_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    requested_subdomain: String,
) -> anyhow::Result<IssueLeaseResponse> {
    issue_lease(http, config, requested_subdomain, "client-web-http", None).await
}

pub async fn issue_share_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    requested_subdomain: String,
    share: ShareDescriptor,
) -> anyhow::Result<IssueLeaseResponse> {
    issue_lease(http, config, requested_subdomain, "http", Some(share)).await
}

pub async fn claim_share_subdomain(
    http: &reqwest::Client,
    config: &ServerConfig,
    share: ShareDescriptor,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let claim = ShareClaimPayload {
        share_id: share.share_id.clone(),
        subdomain: share.subdomain.clone(),
        owner_email: share.owner_email.clone(),
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(
        identity,
        "share_claim_subdomain",
        &claim,
        timestamp_ms,
        &nonce,
    )?;
    let request = ShareClaimSubdomainRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        claim,
        share,
    };
    let response = http
        .post(format!("{api_base}/v1/shares/claim-subdomain"))
        .json(&request)
        .send()
        .await
        .context("send router share subdomain claim")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share subdomain claim failed: {status}: {body}");
}

pub async fn batch_sync_shares(
    http: &reqwest::Client,
    config: &ServerConfig,
    ops: Vec<ShareSyncOperation>,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(identity, "share_batch_sync", &ops, timestamp_ms, &nonce)?;
    let request = ShareBatchSyncRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        ops,
    };
    let response = http
        .post(format!("{api_base}/v1/shares/batch-sync"))
        .json(&request)
        .send()
        .await
        .context("send router share batch sync")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share batch sync failed: {status}: {body}");
}

pub async fn delete_all_shares(
    http: &reqwest::Client,
    config: &ServerConfig,
) -> anyhow::Result<()> {
    batch_sync_shares(
        http,
        config,
        vec![ShareSyncOperation {
            kind: "delete_all".to_string(),
            share_id: None,
            share: None,
        }],
    )
    .await
}

pub async fn batch_sync_share_request_logs(
    http: &reqwest::Client,
    config: &ServerConfig,
    logs: Vec<ShareRequestLogEntry>,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(
        identity,
        "share_request_logs_batch_sync",
        &logs,
        timestamp_ms,
        &nonce,
    )?;
    let request = ShareRequestLogBatchSyncRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        logs,
    };
    let response = http
        .post(format!("{api_base}/v1/share-request-logs/batch-sync"))
        .json(&request)
        .send()
        .await
        .context("send router share request logs batch sync")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share request logs batch sync failed: {status}: {body}");
}

pub async fn pending_share_edits(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_ids: Vec<String>,
) -> anyhow::Result<Vec<ShareEditView>> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let payload = SharePendingEditsPayload { share_ids };
    let request = signed_request(identity, "share_pending_edits", payload)?;
    let response = http
        .post(format!("{api_base}/v1/shares/pending-edits"))
        .json(&request)
        .send()
        .await
        .context("send router pending share edits")?;
    if response.status().is_success() {
        let response = response
            .json::<SharePendingEditsResponse>()
            .await
            .context("parse router pending share edits response")?;
        return Ok(response.edits);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router pending share edits failed: {status}: {body}");
}

pub async fn ack_share_edit(
    http: &reqwest::Client,
    config: &ServerConfig,
    ack: ShareEditAckPayload,
) -> anyhow::Result<()> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let request = signed_request(identity, "share_edit_ack", ShareEditAckEnvelope { ack })?;
    let response = http
        .post(format!("{api_base}/v1/shares/edit-ack"))
        .json(&request)
        .send()
        .await
        .context("send router share edit ack")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share edit ack failed: {status}: {body}");
}

pub fn share_edit_events_url(config: &ServerConfig) -> anyhow::Result<String> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let payload = ShareEditEventSignaturePayload {
        installation_id: identity.installation_id.clone(),
    };
    let signature = sign_payload(
        identity,
        "share_edit_events",
        &payload,
        timestamp_ms,
        &nonce,
    )?;
    Ok(format!(
        "{api_base}/v1/shares/edit-events?installationId={}&timestampMs={timestamp_ms}&nonce={}&signature={}",
        url_encode(&identity.installation_id),
        url_encode(&nonce),
        url_encode(&signature),
    ))
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
        bindings.insert(app_key(share.app).to_string(), share.provider_id.clone());
    } else {
        for binding in &share.bindings {
            bindings.insert(
                app_key(binding.app).to_string(),
                binding.provider_id.clone(),
            );
        }
    }

    let mut support = ShareSupport::default();
    for app in bindings.keys() {
        match app.as_str() {
            "claude" => support.claude = true,
            "codex" => support.codex = true,
            "gemini" => support.gemini = true,
            _ => {}
        }
    }

    let shared_with_emails = share.acl.shared_with_emails.clone();
    let market_access_mode = share.acl.market_access_mode.clone().unwrap_or_else(|| {
        if share.acl.public_market_email.is_some() {
            "selected".to_string()
        } else if shared_with_emails.is_empty() {
            "all".to_string()
        } else {
            "selected".to_string()
        }
    });
    let mut access_by_app = BTreeMap::new();
    let mut app_settings = BTreeMap::new();
    for app in bindings.keys() {
        let app_access = share
            .access_by_app
            .get(app)
            .cloned()
            .unwrap_or_else(|| ShareAppAccess {
                shared_with_emails: shared_with_emails.clone(),
                market_access_mode: market_access_mode.clone(),
            });
        access_by_app.insert(app.clone(), app_access);

        let app_setting =
            share
                .app_settings
                .get(app)
                .cloned()
                .unwrap_or_else(|| ShareAppSettings {
                    for_sale: if share.for_sale { "Yes" } else { "No" }.to_string(),
                    sale_market_kind: share.sale_market_kind.clone(),
                    market_access_mode: market_access_mode.clone(),
                    shared_with_emails: shared_with_emails.clone(),
                    token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
                    parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(3),
                    expires_at: share
                        .expires_at
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                });
        app_settings.insert(app.clone(), app_setting);
    }

    let mut app_runtimes = ShareAppRuntimes::default();
    let mut app_providers = ShareAppProviders::default();
    let mut app_availability = ShareAppAvailability::default();
    let mut primary_upstream = None;
    for (app, provider_id) in &bindings {
        if let Some(provider) = providers
            .providers
            .iter()
            .find(|item| app_key(item.app) == app && item.provider.id == *provider_id)
        {
            let upstream = upstream_provider(app, provider, share, accounts, usage);
            let availability = provider_availability(app, provider, share, accounts, usage);
            if app.as_str() == app_key(share.app) {
                primary_upstream = Some(upstream.clone());
            }
            match app.as_str() {
                "claude" => {
                    app_runtimes.claude = Some(upstream.clone());
                    app_providers
                        .claude
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.claude = Some(availability);
                }
                "codex" => {
                    app_runtimes.codex = Some(upstream.clone());
                    app_providers
                        .codex
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.codex = Some(availability);
                }
                "gemini" => {
                    app_runtimes.gemini = Some(upstream.clone());
                    app_providers
                        .gemini
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.gemini = Some(availability);
                }
                _ => {}
            }
        }
    }
    let model_health =
        crate::core::model_health::summary_for_share(share, providers, accounts, usage);

    ShareDescriptor {
        share_id: share.id.clone(),
        share_name: share
            .display_name
            .clone()
            .unwrap_or_else(|| share.id.clone()),
        owner_email: share.owner_email.clone(),
        shared_with_emails,
        market_access_mode,
        access_by_app,
        app_settings,
        for_sale_official_price_percent_by_app: share
            .for_sale_official_price_percent_by_app
            .clone(),
        description: share.description.clone(),
        market_grant: share.market_grant.clone(),
        for_sale: if share.for_sale { "Yes" } else { "No" }.to_string(),
        sale_market_kind: share.sale_market_kind.clone(),
        subdomain: share
            .tunnel_subdomain
            .clone()
            .unwrap_or_else(|| share.id.replace('_', "-")),
        app_type: app_key(share.app).to_string(),
        provider_id: Some(share.provider_id.clone()),
        bindings,
        token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
        parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(3),
        tokens_used: share.tokens_used as i64,
        requests_count: share.requests_count as i64,
        share_status: share.status.clone(),
        created_at: now_ms().to_string(),
        expires_at: share
            .expires_at
            .map(|value| value.to_string())
            .unwrap_or_default(),
        support,
        upstream_provider: primary_upstream,
        app_runtimes,
        app_providers,
        app_availability,
        model_health,
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
) -> ShareUpstreamProvider {
    let health = usage.map(|usage| provider_health(provider, usage));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    ShareUpstreamProvider {
        kind: provider.provider_type_id.clone(),
        app: app.to_string(),
        provider_name: Some(provider.provider.name.clone()),
        provider_type: Some(provider.provider_type_id.clone()),
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        health,
        available,
    }
}

fn app_provider(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
    is_current: bool,
) -> ShareAppProvider {
    let health = usage.map(|usage| provider_health(provider, usage));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    ShareAppProvider {
        id: provider.provider.id.clone(),
        name: provider.provider.name.clone(),
        app: app.to_string(),
        kind: Some(provider.provider_type_id.clone()),
        provider_type: Some(provider.provider_type_id.clone()),
        is_current,
        enabled: true,
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        health,
        available,
    }
}

fn provider_availability(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareProviderAvailability {
    let health = usage.map(|usage| health::provider_health(provider, usage));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available =
        health.as_ref().map(|health| health.healthy).unwrap_or(true) && quota_blocked != Some(true);
    let reason = if quota_blocked == Some(true) {
        Some("quota blocked".to_string())
    } else {
        health.as_ref().and_then(|health| health.reason.clone())
    };
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

fn provider_health(provider: &StoredProvider, usage: &UsageStore) -> ShareProviderHealth {
    let health = health::provider_health(provider, usage);
    ShareProviderHealth {
        healthy: health.healthy,
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

fn quota_blocked_percent(quota_percent: Option<f64>) -> Option<bool> {
    quota_percent.map(|quota_percent| quota_percent >= 100.0)
}

#[derive(Debug, Clone, Default)]
struct ShareAccountContext {
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
    ShareAccountContext {
        account_email: account
            .and_then(|account| account.email.clone())
            .or_else(|| share.account_email.clone()),
        subscription_level: account
            .and_then(|account| account.subscription_level.clone())
            .or_else(|| share.subscription_level.clone()),
        subscription_expires_at: account.and_then(account_subscription_expires_at),
        subscription_remaining_ms: account.and_then(account_subscription_remaining_ms),
        quota_percent: account
            .and_then(|account| account.quota_percent)
            .or(share.quota_percent),
    }
}

fn account_for_provider<'a>(
    accounts: &'a AccountStore,
    provider: &StoredProvider,
) -> Option<&'a Account> {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    accounts.find_for_provider(provider.provider_type, account_id)
}

fn account_subscription_expires_at(account: &Account) -> Option<String> {
    account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(subscription_expires_at_from_extra)
}

fn account_subscription_remaining_ms(account: &Account) -> Option<i64> {
    account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(subscription_remaining_ms_from_extra)
}

fn subscription_expires_at_from_extra(value: &Value) -> Option<String> {
    [
        "/subscriptionPeriodEnd",
        "/subscription/expiresAt",
        "/subscription/expires_at",
        "/raw/SubscriptionPeriodEnd/Time",
        "/raw/subscriptionPeriodEnd/time",
    ]
    .iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn subscription_remaining_ms_from_extra(value: &Value) -> Option<i64> {
    value
        .pointer("/subscriptionRemainingMs")
        .and_then(Value::as_i64)
}

fn provider_models(provider: &StoredProvider) -> Vec<ShareUpstreamModel> {
    let mut models = Vec::new();
    if let Some(upstream_model) = provider
        .provider
        .settings_config
        .pointer("/modelMapping/upstreamModel")
        .and_then(serde_json::Value::as_str)
        .filter(|model| !model.trim().is_empty())
    {
        models.push(ShareUpstreamModel {
            slot: "default".to_string(),
            actual_model: upstream_model.to_string(),
        });
    }
    if let Some(mapping) = provider
        .provider
        .settings_config
        .get("modelMapping")
        .and_then(serde_json::Value::as_object)
    {
        for (slot, value) in mapping {
            if slot == "upstreamModel" {
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
    if let Some(values) = provider
        .provider
        .settings_config
        .get("models")
        .and_then(serde_json::Value::as_array)
    {
        for value in values {
            let model = value.as_str().or_else(|| {
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
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

fn provider_api_url(provider: &StoredProvider) -> Option<String> {
    let env = provider.provider.settings_config.get("env");
    [
        "/env/ANTHROPIC_BASE_URL",
        "/env/OPENAI_BASE_URL",
        "/env/GEMINI_BASE_URL",
        "/ANTHROPIC_BASE_URL",
        "/OPENAI_BASE_URL",
        "/GEMINI_BASE_URL",
    ]
    .into_iter()
    .find_map(|pointer| {
        provider
            .provider
            .settings_config
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    })
    .or_else(|| {
        env.and_then(|value| value.get("BASE_URL"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    })
}

async fn issue_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    requested_subdomain: String,
    tunnel_type: &str,
    share: Option<ShareDescriptor>,
) -> anyhow::Result<IssueLeaseResponse> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| anyhow::anyhow!("router api base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("router installation is not registered"))?;
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_lease_request(
        identity,
        &requested_subdomain,
        tunnel_type,
        timestamp_ms,
        &nonce,
    )?;
    let request = IssueLeaseRequest {
        installation_id: identity.installation_id.clone(),
        requested_subdomain,
        tunnel_type: tunnel_type.to_string(),
        timestamp_ms,
        nonce,
        signature,
        share,
    };
    let response = http
        .post(format!("{api_base}/v1/tunnels/lease"))
        .json(&request)
        .send()
        .await
        .context("send router tunnel lease")?;
    if response.status().is_success() {
        let lease = response
            .json::<IssueLeaseResponse>()
            .await
            .context("parse router tunnel lease response")?;
        return Ok(normalize_lease_url_scheme(config, lease));
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router tunnel lease failed: {status}: {body}");
}

fn normalize_lease_url_scheme(
    config: &ServerConfig,
    mut lease: IssueLeaseResponse,
) -> IssueLeaseResponse {
    let router_url = config
        .router
        .url
        .as_deref()
        .or(config.router.api_base.as_deref())
        .unwrap_or_default();
    if router_url.starts_with("https://") && lease.tunnel_url.starts_with("http://") {
        lease.tunnel_url = format!("https://{}", lease.tunnel_url.trim_start_matches("http://"));
    }
    lease
}

pub fn signed_request<T: Serialize>(
    identity: &RouterIdentity,
    action: &str,
    payload: T,
) -> anyhow::Result<SignedRequest<T>> {
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(identity, action, &payload, timestamp_ms, &nonce)?;
    Ok(SignedRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        payload,
    })
}

pub fn sign_payload<T: Serialize>(
    identity: &RouterIdentity,
    action: &str,
    payload: &T,
    timestamp_ms: i64,
    nonce: &str,
) -> anyhow::Result<String> {
    if identity.installation_id.trim().is_empty() {
        bail!("router installation id is missing");
    }
    let secret = STANDARD
        .decode(&identity.private_key)
        .context("decode router private key")?;
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid router private key length"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let payload_json = serde_json::to_string(payload).context("serialize signed payload")?;
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}",
        identity.installation_id, action, payload_json, timestamp_ms, nonce
    );
    Ok(STANDARD.encode(signing_key.sign(canonical.as_bytes()).to_bytes()))
}

pub fn sign_lease_request(
    identity: &RouterIdentity,
    requested_subdomain: &str,
    tunnel_type: &str,
    timestamp_ms: i64,
    nonce: &str,
) -> anyhow::Result<String> {
    if identity.installation_id.trim().is_empty() {
        bail!("router installation id is missing");
    }
    let secret = STANDARD
        .decode(&identity.private_key)
        .context("decode router private key")?;
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid router private key length"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}",
        identity.installation_id, requested_subdomain, tunnel_type, timestamp_ms, nonce
    );
    Ok(STANDARD.encode(signing_key.sign(canonical.as_bytes()).to_bytes()))
}

fn generate_identity_without_installation() -> RouterIdentity {
    let signing_key = SigningKey::generate(&mut OsRng);
    RouterIdentity {
        installation_id: String::new(),
        public_key: STANDARD.encode(signing_key.verifying_key().to_bytes()),
        private_key: STANDARD.encode(signing_key.to_bytes()),
        control_secret: None,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn default_tunnel_enabled() -> bool {
    true
}

fn default_market_access_mode() -> String {
    "selected".to_string()
}

fn default_sale_market_kind() -> String {
    "token".to_string()
}

fn default_parallel_limit() -> i64 {
    3
}

fn default_share_for_sale() -> String {
    "No".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use serde_json::json;

    use crate::core::provider::{Provider, ProviderType};
    use crate::core::shares::{Share, ShareAcl, ShareBinding, ShareMarketGrantStatus};
    use crate::core::usage::{UsageLog, UsageLogContext, UsageModelMetadata};

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestPayload {
        share_id: String,
    }

    #[test]
    fn signed_payload_matches_router_canonical_format() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let payload = TestPayload {
            share_id: "share-1".to_string(),
        };
        let signature = sign_payload(&identity, "share_delete", &payload, 123, "nonce-1").unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = "inst-1\nshare_delete\n{\"shareId\":\"share-1\"}\n123\nnonce-1";

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn signed_payload_changes_with_nonce() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let payload = TestPayload {
            share_id: "share-1".to_string(),
        };
        let first = sign_payload(&identity, "share_delete", &payload, 123, "nonce-1").unwrap();
        let second = sign_payload(&identity, "share_delete", &payload, 123, "nonce-2").unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn signed_payload_rejects_tampered_canonical_payload() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let payload = TestPayload {
            share_id: "share-1".to_string(),
        };
        let signature = sign_payload(&identity, "share_delete", &payload, 123, "nonce-1").unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let tampered = "inst-1\nshare_delete\n{\"shareId\":\"share-2\"}\n123\nnonce-1";

        assert!(verifying_key
            .verify(tampered.as_bytes(), &signature)
            .is_err());
    }

    #[test]
    fn lease_signature_matches_router_legacy_format() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let signature = sign_lease_request(&identity, "share-a", "http", 123, "nonce-1").unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = "inst-1\nshare-a\nhttp\n123\nnonce-1";

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn lease_url_scheme_follows_https_router_url() {
        let mut config = ServerConfig::empty();
        config.router.url = Some("https://jptokenswitch.cc".to_string());
        let lease = IssueLeaseResponse {
            lease_id: "lease-1".to_string(),
            connection_id: "conn-1".to_string(),
            ssh_username: "u".to_string(),
            ssh_password: "p".to_string(),
            ssh_addr: "127.0.0.1:2222".to_string(),
            expires_at: "2099-01-01T00:00:00Z".to_string(),
            tunnel_url: "http://share.jptokenswitch.cc".to_string(),
            subdomain: "share".to_string(),
            ssh_host_fingerprint: None,
        };

        let lease = normalize_lease_url_scheme(&config, lease);

        assert_eq!(lease.tunnel_url, "https://share.jptokenswitch.cc");
    }

    #[test]
    fn share_claim_signature_matches_router_canonical_format() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-1".to_string();
        let claim = ShareClaimPayload {
            share_id: "share-1".to_string(),
            subdomain: "share-sub".to_string(),
            owner_email: Some("owner@example.com".to_string()),
        };
        let signature =
            sign_payload(&identity, "share_claim_subdomain", &claim, 123, "nonce-1").unwrap();

        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(signature).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = "inst-1\nshare_claim_subdomain\n{\"shareId\":\"share-1\",\"subdomain\":\"share-sub\",\"ownerEmail\":\"owner@example.com\"}\n123\nnonce-1";

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn descriptor_omits_quota_percent_when_share_has_no_percent() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
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
        };
        let accounts = AccountStore {
            accounts: vec![test_account(ProviderType::CodexOAuth)],
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
    fn descriptor_includes_market_grant_when_present() {
        let mut share = test_share(ProviderType::Codex, Some(42.0));
        share.market_grant = Some(ShareMarketGrantStatus {
            status: "applied".to_string(),
            grant_id: Some("grant-1".to_string()),
            last_error: None,
            updated_at_ms: Some(123),
        });
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::Codex)],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let value = serde_json::to_value(&descriptor).unwrap();

        assert_eq!(value["marketGrant"]["status"], "applied");
        assert_eq!(value["marketGrant"]["grantId"], "grant-1");
    }

    #[test]
    fn descriptor_maps_recent_provider_failure_to_availability() {
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
        log.created_at_ms = crate::core::usage::now_ms();
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
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
                pricing_model: None,
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
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let result = descriptor.model_health.codex.first().unwrap();

        assert_eq!(result.requested_model, "gpt-5.5");
        assert_eq!(result.actual_model, "glm-5.2");
        assert_eq!(result.status, "success");
        assert_eq!(result.source, "cc-switch-health-check");
    }

    #[test]
    fn descriptor_marks_quota_blocked_without_confusing_missing_percent() {
        let share = test_share(ProviderType::Codex, Some(100.0));
        let provider = test_provider(ProviderType::Codex);
        let providers = ProviderStore {
            providers: vec![provider],
        };
        let usage = UsageStore::default();

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(!availability.available);
        assert_eq!(availability.quota_blocked, Some(true));
        assert_eq!(provider.quota_percent, Some(100.0));
        assert_eq!(provider.quota_blocked, Some(true));
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
                    "models": ["glm-5.2"]
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
        }
    }

    fn test_share(provider_type: ProviderType, quota_percent: Option<f64>) -> Share {
        Share {
            id: "share-1".to_string(),
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
            acl: ShareAcl::default(),
            token_limit: None,
            parallel_limit: None,
            tokens_used: 0,
            requests_count: 0,
            expires_at: None,
            for_sale: false,
            sale_market_kind: "token".to_string(),
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: false,
            description: None,
            bindings: vec![ShareBinding {
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type,
            }],
            binding_history: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            last_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
        }
    }

    fn test_account(provider_type: ProviderType) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type,
            email: Some("account@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: None,
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: Some("ChatGPT Pro 20x".to_string()),
            quota_percent: Some(42.0),
            quota: Some(crate::core::accounts::AccountQuota {
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
            last_refresh_error: None,
        }
    }
}
