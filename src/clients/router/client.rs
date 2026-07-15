#![allow(dead_code)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::domain::settings::config::{
    PayoutProfileState, RouterIdentity, ServerConfig, UpgradePolicyConfig,
};
use crate::domain::sharing::router_contract::*;
use crate::self_update::version::LatestReleaseMeta;

const ROUTER_LEASE_RENEW_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationRequest {
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelView {
    pub owner_email: String,
    pub subdomain: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelResponse {
    #[serde(default)]
    pub tunnel: Option<ClientTunnelView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallationOwnerEmailResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientTunnelUpdateRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationPayoutProfileUpdateRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    update: PayoutProfileState,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationPayoutProfileUpdateResponse {
    pub ok: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRuntimeRefreshRequest {
    installation_id: String,
    timestamp_ms: i64,
    nonce: String,
    signature: String,
    refresh: ShareRuntimeRefreshPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRuntimeRefreshPayload {
    share_id: String,
    subdomain: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewLeasePayload {
    pub lease_id: String,
    pub connection_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenewLeaseResponse {
    expires_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RenewLeaseError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Terminal(String),
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

    let request = build_register_installation_request(
        &identity,
        std::env::consts::OS,
        crate::build_info::router_registration_version(),
        nonce(),
        now_ms(),
    )?;
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubdomainAvailability {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub async fn check_client_tunnel_subdomain_available(
    http: &reqwest::Client,
    router_api_base: &str,
    subdomain: &str,
    installation_id: Option<&str>,
) -> anyhow::Result<SubdomainAvailability> {
    let api_base = router_api_base.trim_end_matches('/');
    let mut url = reqwest::Url::parse(&format!(
        "{api_base}/v1/client-tunnel/subdomain-availability"
    ))
    .context("parse router subdomain availability url")?;
    url.query_pairs_mut()
        .append_pair("subdomain", subdomain.trim());
    if let Some(installation_id) = installation_id.filter(|value| !value.trim().is_empty()) {
        url.query_pairs_mut()
            .append_pair("installationId", installation_id.trim());
    }
    let response = http
        .get(url)
        .send()
        .await
        .context("send router subdomain availability check")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("router subdomain availability check failed: {status}: {body}");
    }
    response
        .json::<SubdomainAvailability>()
        .await
        .context("decode router subdomain availability response")
}

pub async fn get_client_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
) -> anyhow::Result<Option<ClientTunnelView>> {
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
    let empty = serde_json::json!({});
    let signature = sign_payload(identity, "client_tunnel_get", &empty, timestamp_ms, &nonce)?;
    let timestamp_ms = timestamp_ms.to_string();
    let response = http
        .get(format!("{api_base}/v1/installations/client-tunnel"))
        .query(&[
            ("installationId", identity.installation_id.as_str()),
            ("timestampMs", timestamp_ms.as_str()),
            ("nonce", nonce.as_str()),
            ("signature", signature.as_str()),
        ])
        .send()
        .await
        .context("send router client tunnel get")?;
    if response.status().is_success() {
        return Ok(response
            .json::<ClientTunnelResponse>()
            .await
            .context("parse router client tunnel get response")?
            .tunnel);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router client tunnel get failed: {status}: {body}");
}

pub async fn get_installation_owner_email(
    http: &reqwest::Client,
    config: &ServerConfig,
) -> anyhow::Result<Option<String>> {
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
    let empty = serde_json::json!({});
    let signature = sign_payload(
        identity,
        "get_installation_owner_email",
        &empty,
        timestamp_ms,
        &nonce,
    )?;
    let timestamp_ms = timestamp_ms.to_string();
    let response = http
        .get(format!("{api_base}/v1/installations/owner-email"))
        .query(&[
            ("installationId", identity.installation_id.as_str()),
            ("timestampMs", timestamp_ms.as_str()),
            ("nonce", nonce.as_str()),
            ("signature", signature.as_str()),
        ])
        .send()
        .await
        .context("send router installation owner email status")?;
    if response.status().is_success() {
        let body = response
            .json::<InstallationOwnerEmailResponse>()
            .await
            .context("parse router installation owner email response")?;
        return Ok(body.owner_email);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router installation owner email status failed: {status}: {body}");
}

pub async fn update_client_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
    tunnel: ClientTunnelConfig,
) -> anyhow::Result<Option<ClientTunnelView>> {
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
        "client_tunnel_update",
        &tunnel,
        timestamp_ms,
        &nonce,
    )?;
    let request = ClientTunnelUpdateRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        tunnel,
    };
    let response = http
        .patch(format!("{api_base}/v1/installations/client-tunnel"))
        .json(&request)
        .send()
        .await
        .context("send router client tunnel update")?;
    if response.status().is_success() {
        return Ok(response
            .json::<ClientTunnelResponse>()
            .await
            .context("parse router client tunnel update response")?
            .tunnel);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router client tunnel update failed: {status}: {body}");
}

pub async fn release_client_tunnel(
    http: &reqwest::Client,
    config: &ServerConfig,
) -> anyhow::Result<Option<ClientTunnelView>> {
    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| anyhow::anyhow!("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| anyhow::anyhow!("client tunnel subdomain is not configured"))?;
    update_client_tunnel(
        http,
        config,
        ClientTunnelConfig {
            owner_email,
            subdomain,
            enabled: false,
        },
    )
    .await
}

pub async fn push_payout_profile(
    http: &reqwest::Client,
    config: &ServerConfig,
    update: PayoutProfileState,
) -> anyhow::Result<InstallationPayoutProfileUpdateResponse> {
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
        "update_installation_payout_profile",
        &update,
        timestamp_ms,
        &nonce,
    )?;
    let revision = update.revision;
    let request = InstallationPayoutProfileUpdateRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        update,
    };
    let response = http
        .put(format!("{api_base}/v1/installations/payout-profile"))
        .json(&request)
        .send()
        .await
        .context("send router payout profile update")?;
    if response.status().is_success() {
        let response = response
            .json::<InstallationPayoutProfileUpdateResponse>()
            .await
            .context("parse router payout profile update response")?;
        if !response.ok || response.revision != revision {
            bail!("router payout profile update acknowledgement mismatch");
        }
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router payout profile update failed: {status}: {body}");
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

pub async fn renew_tunnel_lease(
    http: &reqwest::Client,
    config: &ServerConfig,
    lease_id: String,
    connection_id: String,
) -> Result<String, RenewLeaseError> {
    renew_tunnel_lease_with_timeout(
        http,
        config,
        lease_id,
        connection_id,
        ROUTER_LEASE_RENEW_TIMEOUT,
    )
    .await
}

async fn renew_tunnel_lease_with_timeout(
    http: &reqwest::Client,
    config: &ServerConfig,
    lease_id: String,
    connection_id: String,
    timeout: Duration,
) -> Result<String, RenewLeaseError> {
    let api_base = config
        .router_api_base()
        .ok_or_else(|| RenewLeaseError::Terminal("router api base is not configured".into()))?
        .trim_end_matches('/')
        .to_string();
    let identity =
        config.router.identity.as_ref().ok_or_else(|| {
            RenewLeaseError::Terminal("router installation is not registered".into())
        })?;
    let request = signed_request(
        identity,
        "renew_lease",
        RenewLeasePayload {
            lease_id,
            connection_id,
        },
    )
    .map_err(|error| RenewLeaseError::Terminal(error.to_string()))?;
    let response = http
        .post(format!("{api_base}/v1/tunnels/lease/renew"))
        .timeout(timeout)
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            RenewLeaseError::Retryable(format!("send router tunnel lease renewal: {error}"))
        })?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<RenewLeaseResponse>()
            .await
            .map(|response| response.expires_at)
            .map_err(|error| {
                RenewLeaseError::Retryable(format!(
                    "parse router tunnel lease renewal response: {error}"
                ))
            });
    }
    let body = response.text().await.unwrap_or_default();
    let message = format!("router tunnel lease renewal failed: {status}: {body}");
    if renew_status_is_retryable(status) {
        Err(RenewLeaseError::Retryable(message))
    } else {
        Err(RenewLeaseError::Terminal(message))
    }
}

fn renew_status_is_retryable(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
                | reqwest::StatusCode::BAD_GATEWAY
                | reqwest::StatusCode::SERVICE_UNAVAILABLE
                | reqwest::StatusCode::GATEWAY_TIMEOUT
        )
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

pub async fn push_share_ops(
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
    let response = send_share_ops_request(http, &api_base, identity, ops.clone()).await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if should_retry_legacy_share_sync(status, &body, &ops) {
        tracing::warn!(
            "router rejected versioned share sync signature; retrying legacy signed payload"
        );
        let response =
            send_share_ops_request(http, &api_base, identity, legacy_share_sync_ops(ops)).await?;
        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("router legacy share batch sync failed: {status}: {body}");
    }
    bail!("router share batch sync failed: {status}: {body}");
}

async fn send_share_ops_request(
    http: &reqwest::Client,
    api_base: &str,
    identity: &RouterIdentity,
    ops: Vec<ShareSyncOperation>,
) -> anyhow::Result<reqwest::Response> {
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
    http.post(format!("{api_base}/v1/shares/batch-sync"))
        .json(&request)
        .send()
        .await
        .context("send router share batch sync")
}

fn should_retry_legacy_share_sync(
    status: reqwest::StatusCode,
    body: &str,
    ops: &[ShareSyncOperation],
) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED
        && body.contains("signature verification failed")
        && ops.iter().any(|op| {
            op.share
                .as_ref()
                .is_some_and(|share| share.config_revision > 0)
        })
}

fn legacy_share_sync_ops(mut ops: Vec<ShareSyncOperation>) -> Vec<ShareSyncOperation> {
    for op in &mut ops {
        if let Some(share) = op.share.as_mut() {
            share.auto_start = false;
            share.config_revision = 0;
        }
    }
    ops
}

pub async fn notify_runtime_refresh(
    http: &reqwest::Client,
    config: &ServerConfig,
    share_id: String,
    subdomain: String,
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
    let refresh = ShareRuntimeRefreshPayload {
        share_id,
        subdomain,
    };
    let timestamp_ms = now_ms();
    let nonce = nonce();
    let signature = sign_payload(
        identity,
        "share_runtime_refresh",
        &refresh,
        timestamp_ms,
        &nonce,
    )?;
    let request = ShareRuntimeRefreshRequest {
        installation_id: identity.installation_id.clone(),
        timestamp_ms,
        nonce,
        signature,
        refresh,
    };
    let response = http
        .post(format!("{api_base}/v1/shares/runtime-refresh"))
        .json(&request)
        .send()
        .await
        .context("send router share runtime refresh")?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router share runtime refresh failed: {status}: {body}");
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

fn build_register_installation_request(
    identity: &RouterIdentity,
    platform: &str,
    app_version: &str,
    instance_nonce: String,
    timestamp_ms: i64,
) -> anyhow::Result<RegisterInstallationRequest> {
    let (timestamp_ms, signature) = if identity.installation_id.trim().is_empty() {
        (None, None)
    } else {
        let signature = sign_registration_recovery(
            identity,
            platform,
            app_version,
            &instance_nonce,
            timestamp_ms,
        )?;
        (Some(timestamp_ms), Some(signature))
    };
    Ok(RegisterInstallationRequest {
        public_key: identity.public_key.clone(),
        platform: platform.to_string(),
        app_version: app_version.to_string(),
        instance_nonce,
        timestamp_ms,
        signature,
    })
}

fn sign_registration_recovery(
    identity: &RouterIdentity,
    platform: &str,
    app_version: &str,
    instance_nonce: &str,
    timestamp_ms: i64,
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
        "{}\nregister_installation\n{}\n{}\n{}\n{}\n{}",
        identity.installation_id,
        identity.public_key.trim(),
        platform.trim(),
        app_version,
        instance_nonce,
        timestamp_ms
    );
    Ok(STANDARD.encode(signing_key.sign(canonical.as_bytes()).to_bytes()))
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportInstallationStatusPayload {
    delegate_upgrade_to_router_owner: bool,
    auto_upgrade_enabled: bool,
    app_commit_id: String,
    update_available: bool,
    upgrade_capable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportInstallationStatusResponse {
    ok: bool,
}

pub async fn report_installation_status(
    http: &reqwest::Client,
    config: &ServerConfig,
    policy: &UpgradePolicyConfig,
    latest: &LatestReleaseMeta,
    upgrade_capable: bool,
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
    let payload = ReportInstallationStatusPayload {
        delegate_upgrade_to_router_owner: policy.delegate_upgrade_to_router_owner,
        auto_upgrade_enabled: policy.auto_upgrade_enabled,
        app_commit_id: crate::build_info::build_info().commit_id.to_string(),
        update_available: latest.update_available,
        upgrade_capable,
    };
    let request = signed_request(identity, "report_installation_status", payload)?;
    let response = http
        .post(format!("{api_base}/v1/installations/report-status"))
        .json(&request)
        .send()
        .await
        .context("send router installation status report")?;
    if response.status().is_success() {
        let _ = response
            .json::<ReportInstallationStatusResponse>()
            .await
            .context("parse router installation status report response")?;
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("router installation status report failed: {status}: {body}");
}

fn default_tunnel_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

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
    fn first_registration_request_is_unsigned() {
        let identity = generate_identity_without_installation();

        let request = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "registration-nonce".into(),
            123,
        )
        .unwrap();

        assert!(request.timestamp_ms.is_none());
        assert!(request.signature.is_none());
        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("timestampMs").is_none());
        assert!(value.get("signature").is_none());
    }

    #[test]
    fn existing_registration_request_signs_router_recovery_payload() {
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-existing".into();

        let request = build_register_installation_request(
            &identity,
            "linux",
            "1.2.3",
            "registration-nonce".into(),
            123,
        )
        .unwrap();

        assert_eq!(request.timestamp_ms, Some(123));
        let public_key = STANDARD.decode(&identity.public_key).unwrap();
        let public_key: [u8; 32] = public_key.try_into().unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key).unwrap();
        let signature = STANDARD.decode(request.signature.unwrap()).unwrap();
        let signature: [u8; 64] = signature.try_into().unwrap();
        let signature = Signature::from_bytes(&signature);
        let canonical = format!(
            "inst-existing\nregister_installation\n{}\nlinux\n1.2.3\nregistration-nonce\n123",
            identity.public_key
        );

        verifying_key
            .verify(canonical.as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn legacy_share_sync_omits_versioned_fields_from_signed_payload() {
        let ops = vec![ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(ShareDescriptor {
                auto_start: true,
                config_revision: 7,
                ..ShareDescriptor::default()
            }),
        }];
        let serialized = serde_json::to_string(&ops).unwrap();
        assert!(serialized.contains("autoStart"));
        assert!(serialized.contains("configRevision"));

        let legacy = legacy_share_sync_ops(ops);
        let serialized = serde_json::to_string(&legacy).unwrap();
        assert!(!serialized.contains("autoStart"));
        assert!(!serialized.contains("configRevision"));
    }

    #[test]
    fn legacy_share_sync_retry_is_limited_to_versioned_signature_failures() {
        let ops = vec![ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(ShareDescriptor {
                config_revision: 1,
                ..ShareDescriptor::default()
            }),
        }];
        assert!(should_retry_legacy_share_sync(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"message":"signature verification failed"}"#,
            &ops,
        ));
        assert!(!should_retry_legacy_share_sync(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"message":"installation not found"}"#,
            &ops,
        ));
        assert!(!should_retry_legacy_share_sync(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"message":"signature verification failed"}"#,
            &ops,
        ));
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
    fn lease_renewal_status_classification_preserves_transient_connections() {
        assert!(renew_status_is_retryable(
            reqwest::StatusCode::REQUEST_TIMEOUT
        ));
        assert!(renew_status_is_retryable(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(renew_status_is_retryable(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!renew_status_is_retryable(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!renew_status_is_retryable(reqwest::StatusCode::NOT_FOUND));
        assert!(!renew_status_is_retryable(reqwest::StatusCode::CONFLICT));
    }

    #[tokio::test]
    async fn lease_renewal_timeout_is_retryable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let mut config = ServerConfig::empty();
        config.router.url = Some(format!("http://{addr}"));
        let mut identity = generate_identity_without_installation();
        identity.installation_id = "inst-timeout".into();
        config.router.identity = Some(identity);

        let error = renew_tunnel_lease_with_timeout(
            &reqwest::Client::new(),
            &config,
            "lease-timeout".into(),
            "connection-timeout".into(),
            Duration::from_millis(20),
        )
        .await
        .expect_err("a hung renewal response must time out");

        assert!(matches!(error, RenewLeaseError::Retryable(_)));
        server.abort();
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
}
